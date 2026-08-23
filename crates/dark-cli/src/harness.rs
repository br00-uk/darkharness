//! Brings up a live session: the model, the engine, the tools, and the
//! context prefix.
//!
//! This is the composition root's centre. Every other command module here
//! either needs no model at all (`explore`, `map`, `doctor`) or reads
//! files a model produced (`replay`, `stats`). This module is the one
//! that turns files on disk into a running [`Engine`] the turn loop can
//! drive, and it is shared by `dark run` and the terminal application so
//! both bring a session up the same way.
//!
//! # Nothing here reaches the network
//!
//! The primary requirement is that a person disconnects the network after
//! `dark setup` and keeps working. This module therefore only ever reads
//! what is already under `$DARK_HOME/models`: [`installed`] scans that
//! directory for manifests, and a model that is not there is an error
//! with a remedy naming `dark setup`, never a download. `dark models
//! pull` and `dark setup` own downloading; nothing on a turn's path does.
//!
//! # The order the steps must happen in
//!
//! Rule 1 says to estimate before loading and never discover a limit by
//! allocation failure, so [`bring_up`] reads the model's shape from disk
//! and asks [`ResidentSet::begin_load`] whether it fits *before* it asks
//! mistral.rs for a single byte. The resident set's answer can be smaller
//! than the request — the degradation ladder may cut the context, or the
//! quantisation — and the load then honours what it granted rather than
//! what was asked for.
//!
//! Registration is deliberately two steps
//! ([`ResidentSet::finish_load`], then [`RealEngine::register_model`]):
//! see `RealEngine::register_model`'s own documentation for why the
//! memory accounting and a model's availability to answer are separate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result};
use dark_contract::{Caps, Engine, EventTx, RoleClass};
use dark_core::policy::{ActionKind, Policy, PolicyConfig, RunMode};
use dark_core::turn::ToolSet;
use dark_engine::load::Manifest;
use dark_engine::resident::ModelKey;
use dark_engine::{InstallSpec, ModelCapabilities, RealEngine};
use dark_tools::registry::{self, GatedTools};

use crate::scrape::ScrapingEngine;

/// The context length a session asks for when nothing configures one.
///
/// The resident set cuts this down when it does not fit, so asking for a
/// generous figure costs nothing on a small machine: the degradation
/// ladder reports what it granted, and [`Caps::granted_context`] is what
/// the budget then works against (Rule 4).
const DEFAULT_REQUESTED_CONTEXT: u64 = 32_768;

/// One model installed under `$DARK_HOME/models`.
#[derive(Debug, Clone)]
pub(crate) struct Installed {
    /// The directory holding the weights, the manifest, and `config.json`.
    pub(crate) dir: PathBuf,
    /// What `dark models pull` recorded about this model.
    pub(crate) manifest: Manifest,
}

/// Scans `$DARK_HOME/models` for installed models, newest name order last.
///
/// A directory with no readable `manifest.toml` is skipped rather than
/// reported: a partial download leaves one behind, and a half-pulled
/// model is not an installed model. The result is sorted by repository
/// name so two runs on the same machine pick the same model.
///
/// # Errors
///
/// Returns an error when `$DARK_HOME/models` exists but cannot be read.
/// A missing directory is not an error — it means no model is installed,
/// and the caller reports that with the remedy that fits what it was
/// doing.
pub(crate) fn installed(dark_home: &Path) -> Result<Vec<Installed>> {
    let models_dir = dark_home.join("models");
    if !models_dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&models_dir)
        .with_context(|| format!("could not read {}", models_dir.display()))?;

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        // A directory whose manifest does not read is a partial pull, not
        // an installed model. Skipping it keeps a broken download from
        // being chosen to serve a turn.
        if let Ok(manifest) = Manifest::read_from(&dir.join("manifest.toml")) {
            found.push(Installed { dir, manifest });
        }
    }

    found.sort_by(|left, right| left.manifest.repository.cmp(&right.manifest.repository));
    Ok(found)
}

/// Chooses which installed model serves this session.
///
/// `preferred` is the repository the configuration names, when it names
/// one. Without it, a single installed model is chosen on its own, and
/// several are an error that lists them: guessing between two models
/// would silently change which one answers, and the memory each needs is
/// different.
///
/// # Errors
///
/// Returns an error naming `dark setup` when nothing is installed, and an
/// error listing the candidates when several are installed and none is
/// configured.
fn choose(models: Vec<Installed>, preferred: Option<&str>) -> Result<Installed> {
    if models.is_empty() {
        anyhow::bail!(
            "no model is installed. Run dark setup to install one, or dark models pull \
             <repository> to add one."
        );
    }

    if let Some(wanted) = preferred {
        return models
            .into_iter()
            .find(|installed| installed.manifest.repository == wanted)
            .with_context(|| {
                format!(
                    "the configuration names the model {wanted}, which is not installed. Run \
                     dark models pull {wanted}, or change hardware.model."
                )
            });
    }

    if models.len() == 1 {
        // `models` is not empty and holds exactly one entry.
        return Ok(models.into_iter().next().expect("length checked above"));
    }

    let names: Vec<&str> = models
        .iter()
        .map(|installed| installed.manifest.repository.as_str())
        .collect();
    anyhow::bail!(
        "{} models are installed and the configuration names none: {}. Set hardware.model in \
         the configuration file to the one to use.",
        names.len(),
        names.join(", ")
    )
}

/// Maps a tool to the action the policy gates it on.
///
/// [`dark_contract::ToolSchema`] carries `mutating`, which separates a
/// read from everything else, but the policy has three values, not two:
/// `write` and `exec` are configured separately, and a repository may
/// allow one and deny the other. This is the composition root's table of
/// which is which, kept here because `dark-tools` depends on
/// `dark-contract` alone and so cannot name [`ActionKind`] itself.
///
/// An unfamiliar mutating tool is gated as an execution, the stricter
/// reading of the two: a tool this table does not know about could do
/// anything, and asking about it as a command is the safer error.
fn action_kind_of(name: &str, mutating: bool) -> ActionKind {
    if !mutating {
        return ActionKind::Read;
    }
    match name {
        "write_file" | "edit_file" | "apply_patch" => ActionKind::Write,
        _ => ActionKind::Exec,
    }
}

/// Turns the gated tool list into the [`ToolSet`] the turn loop takes.
fn tool_set(gated: GatedTools) -> ToolSet {
    let mut set = ToolSet::new();
    for tool in gated.tools {
        let schema = tool.schema();
        let kind = action_kind_of(&schema.name, schema.mutating);
        set = set.with(Arc::from(tool), kind);
    }
    set
}

/// A brought-up session: everything a turn needs that outlives it.
pub(crate) struct Harness {
    /// The engine, already wrapped for a model whose tool calls are
    /// scraped out of its text. See [`crate::scrape`].
    pub(crate) engine: Arc<dyn Engine>,
    /// The tools this model is gated to.
    pub(crate) tools: ToolSet,
    /// The permission policy.
    pub(crate) policy: Policy,
    /// What the model that serves this session can do.
    pub(crate) caps: Caps,
    /// The repository root a tool must not leave.
    ///
    /// Held here so a caller that brought the session up has the root the
    /// turn will be given, without threading it separately.
    #[allow(
        dead_code,
        reason = "the terminal application reads this; dark run passes its own"
    )]
    pub(crate) root: PathBuf,
    /// The model that answers, for the display and the transcript.
    pub(crate) model_id: String,
}

/// What [`bring_up`] needs from its caller.
pub(crate) struct BringUp<'a> {
    /// The repository root.
    pub(crate) root: PathBuf,
    /// `$DARK_HOME`.
    pub(crate) dark_home: PathBuf,
    /// The repository the configuration names, when it names one.
    pub(crate) preferred_model: Option<&'a str>,
    /// The permission configuration.
    pub(crate) policy: PolicyConfig,
    /// Whether a person can answer a confirmation now.
    pub(crate) mode: RunMode,
    /// Where the engine sends its events.
    pub(crate) events: EventTx,
    /// The tool tier the repository's `AGENTS.md` asks for, when it asks.
    pub(crate) tier_override: Option<u8>,
}

/// Loads a model and builds everything one session needs around it.
///
/// See the module documentation for the order the steps run in and why.
///
/// # Errors
///
/// Returns an error when no model is installed, when the chosen model's
/// files are incomplete, when the model does not fit in memory at any
/// size the degradation ladder offers, or when mistral.rs cannot load it.
/// Every one of those carries a remedy.
pub(crate) async fn bring_up(request: BringUp<'_>) -> Result<Harness> {
    let chosen = choose(installed(&request.dark_home)?, request.preferred_model)?;
    let repository = chosen.manifest.repository.clone();
    let quant = chosen.manifest.quantisation.clone();
    let key = ModelKey::new(repository, quant.clone());

    let memory = dark_engine::tune::memory::read();
    let engine = RealEngine::new(memory.budget_bytes(), request.events.clone());

    // `install` reads the model's shape, asks the resident set whether it
    // fits, and only then loads a weight (Rule 1). It lives in
    // `dark-engine` because every step of it names a mistral.rs type, and
    // Rule 12 keeps those behind that crate.
    let granted_context = engine
        .install(InstallSpec {
            key: key.clone(),
            dir: chosen.dir.clone(),
            format: chosen.manifest.format,
            quant,
            // One model serves every generating class. A separate,
            // smaller model for the scout class is a configuration this
            // composition root does not read yet — see the module
            // documentation of `crate::run`.
            classes: vec![RoleClass::Architect, RoleClass::Worker, RoleClass::Scout],
            capabilities: ModelCapabilities {
                // mistral.rs is never asked to parse tool calls:
                // `dark-qwen` scrapes them out of the text instead,
                // through `crate::scrape`. See that module for why.
                native_tools: false,
                thinking: true,
                grammar: false,
                vision: false,
                logprobs: false,
                // Filled in below from the caps the engine reports, which
                // it builds from the shape `install` read off disk.
                params_b: 0.0,
            },
            requested_context: DEFAULT_REQUESTED_CONTEXT,
            max_context: DEFAULT_REQUESTED_CONTEXT,
        })
        .await
        .map_err(crate::contract_error)?;

    let engine: Arc<dyn Engine> = Arc::new(ScrapingEngine::new(Arc::new(engine)));
    let caps = engine
        .caps(RoleClass::Worker)
        .await
        .map_err(crate::contract_error)?;

    let tools = tool_set(registry::resolve(&caps, request.tier_override));

    report_granted_context(granted_context);

    Ok(Harness {
        engine,
        tools,
        policy: Policy::new(request.policy, request.mode),
        caps,
        root: request.root,
        model_id: key.to_string(),
    })
}

/// Reports a granted context that came back smaller than asked for.
///
/// The degradation ladder cutting the context is not a failure, but it is
/// something a person should be told about rather than left to discover
/// when a long file will not fit.
fn report_granted_context(granted: u64) {
    if granted < DEFAULT_REQUESTED_CONTEXT {
        eprintln!(
            "note: this machine granted a {granted}-token context, not the \
             {DEFAULT_REQUESTED_CONTEXT} asked for."
        );
    }
}

/// Builds a [`Harness`] around an engine the caller supplies, for a test.
///
/// This is the seam that lets the composition be exercised against
/// `dark-engine-fake`: everything a turn touches — the real tool set
/// gated on the real caps, the real policy, the real prefix assembly —
/// is built here exactly as [`bring_up`] builds it, with only the engine
/// differing. Without it the whole composition would be reachable only
/// on a machine with model weights, which is to say never in this
/// workspace's tests.
#[cfg(test)]
pub(crate) async fn for_test(
    engine: Arc<dyn Engine>,
    root: PathBuf,
    policy: PolicyConfig,
    mode: RunMode,
) -> Result<Harness> {
    let caps = engine
        .caps(RoleClass::Worker)
        .await
        .map_err(crate::contract_error)?;
    let model_id = caps.model_id.clone();

    Ok(Harness {
        tools: tool_set(registry::resolve(&caps, None)),
        engine,
        policy: Policy::new(policy, mode),
        caps,
        root,
        model_id,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use dark_engine::load::ModelFormat;

    use super::*;

    /// Writes a model directory with a manifest under `models_root`.
    fn install(models_root: &Path, repository: &str, quant: &str) -> PathBuf {
        let dir = models_root.join(repository.replace('/', "__"));
        fs::create_dir_all(&dir).unwrap();
        Manifest {
            repository: repository.to_owned(),
            revision: "main".to_owned(),
            quantisation: quant.to_owned(),
            sha256: "0".repeat(64),
            measured_memory_bytes: 0,
            format: ModelFormat::Uqff,
        }
        .write_to(&dir.join("manifest.toml"))
        .unwrap();
        dir
    }

    #[test]
    fn no_models_directory_is_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        assert!(installed(home.path()).unwrap().is_empty());
    }

    #[test]
    fn installed_finds_every_model_with_a_manifest() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");
        install(&models, "Qwen/Qwen3-14B", "q4k");

        let found = installed(home.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].manifest.repository, "Qwen/Qwen3-14B",
            "the list is sorted, so a run picks the same model twice"
        );
    }

    #[test]
    fn a_directory_with_no_manifest_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");
        // A half-finished pull: the directory exists, the manifest does not.
        fs::create_dir_all(models.join("Qwen__Qwen3-32B")).unwrap();

        let found = installed(home.path()).unwrap();
        assert_eq!(found.len(), 1, "a partial pull is not an installed model");
        assert_eq!(found[0].manifest.repository, "Qwen/Qwen3-4B");
    }

    #[test]
    fn choosing_with_nothing_installed_names_dark_setup() {
        let err = choose(Vec::new(), None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("dark setup"), "message: {message}");
    }

    #[test]
    fn one_installed_model_is_chosen_without_configuration() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");

        let chosen = choose(installed(home.path()).unwrap(), None).unwrap();
        assert_eq!(chosen.manifest.repository, "Qwen/Qwen3-4B");
    }

    #[test]
    fn several_installed_models_and_no_configuration_lists_them() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");
        install(&models, "Qwen/Qwen3-14B", "q4k");

        let err = choose(installed(home.path()).unwrap(), None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Qwen/Qwen3-4B"), "message: {message}");
        assert!(message.contains("Qwen/Qwen3-14B"), "message: {message}");
        assert!(
            message.contains("hardware.model"),
            "the message says how to fix it: {message}"
        );
    }

    #[test]
    fn the_configured_model_wins_when_several_are_installed() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");
        install(&models, "Qwen/Qwen3-14B", "q4k");

        let chosen = choose(installed(home.path()).unwrap(), Some("Qwen/Qwen3-14B")).unwrap();
        assert_eq!(chosen.manifest.repository, "Qwen/Qwen3-14B");
    }

    #[test]
    fn a_configured_model_that_is_not_installed_says_so() {
        let home = tempfile::tempdir().unwrap();
        let models = home.path().join("models");
        install(&models, "Qwen/Qwen3-4B", "q4k");

        let err = choose(installed(home.path()).unwrap(), Some("Qwen/Qwen3-32B")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Qwen/Qwen3-32B"), "message: {message}");
        assert!(message.contains("dark models pull"), "message: {message}");
    }

    #[test]
    fn a_read_tool_is_gated_as_a_read() {
        assert_eq!(action_kind_of("read_file", false), ActionKind::Read);
        assert_eq!(action_kind_of("grep", false), ActionKind::Read);
        assert_eq!(action_kind_of("list_dir", false), ActionKind::Read);
    }

    #[test]
    fn a_file_writing_tool_is_gated_as_a_write() {
        assert_eq!(action_kind_of("write_file", true), ActionKind::Write);
        assert_eq!(action_kind_of("edit_file", true), ActionKind::Write);
        assert_eq!(action_kind_of("apply_patch", true), ActionKind::Write);
    }

    #[test]
    fn the_command_tool_is_gated_as_an_execution() {
        assert_eq!(action_kind_of("run_command", true), ActionKind::Exec);
    }

    #[test]
    fn an_unfamiliar_mutating_tool_is_gated_as_an_execution() {
        assert_eq!(
            action_kind_of("some_tool_added_later", true),
            ActionKind::Exec,
            "the stricter reading: an unknown mutating tool could do anything"
        );
    }

    #[test]
    fn every_real_tool_has_a_gate_that_matches_what_it_does() {
        // The gating table above is keyed by name, so it goes stale
        // silently if a tool is renamed. This checks it against the real
        // registry rather than a copy of the list.
        let caps = Caps {
            model_id: "test/model".to_owned(),
            max_context: 8192,
            granted_context: 8192,
            native_tools: false,
            thinking: false,
            grammar: false,
            vision: false,
            logprobs: false,
            params_b: 32.0,
            quant: "q4k".to_owned(),
            device: dark_contract::Device::Cpu,
            measured_tok_s: None,
        };

        for tool in registry::resolve(&caps, None).tools {
            let schema = tool.schema();
            let kind = action_kind_of(&schema.name, schema.mutating);
            if schema.mutating {
                assert_ne!(
                    kind,
                    ActionKind::Read,
                    "{} mutates, so it must never be gated as a read",
                    schema.name
                );
            } else {
                assert_eq!(
                    kind,
                    ActionKind::Read,
                    "{} does not mutate, so it is a read",
                    schema.name
                );
            }
        }
    }

    #[test]
    fn the_tool_set_carries_every_gated_tool() {
        let caps = Caps {
            model_id: "test/model".to_owned(),
            max_context: 8192,
            granted_context: 8192,
            native_tools: false,
            thinking: false,
            grammar: false,
            vision: false,
            logprobs: false,
            params_b: 32.0,
            quant: "q4k".to_owned(),
            device: dark_contract::Device::Cpu,
            measured_tok_s: None,
        };

        let gated = registry::resolve(&caps, None);
        let expected = gated.tools.len();
        let set = tool_set(gated);

        assert_eq!(
            set.schemas().len(),
            expected,
            "no tool is lost between the registry and the turn loop"
        );
    }
}
