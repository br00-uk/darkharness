//! `dark pack`: manages documentation packs under `$DARK_HOME/packs`.
//!
//! `list`, `export`, `import`, and `rm` need no model and are wired here
//! against [`dark_lexicon::cli`]. `add`, `refresh`, and `reindex` need an
//! [`dark_lexicon::index::Embedder`], which needs a loaded embedding model
//! — task units `B2` to `B7` — so they still answer "not yet", with a
//! message naming exactly what they are waiting for.
//!
//! Every function here is a local filesystem read or write; nothing in
//! this module opens a network connection.

use std::path::{Path, PathBuf};

use dark_lexicon::cli;
use dark_lexicon::pack::PackManifest;

use crate::PackAction;

/// Returns `$DARK_HOME/packs`, the root every pack lives under (Section
/// 5.3).
fn packs_root() -> PathBuf {
    crate::dark_home().join("packs")
}

/// Renders one line describing a pack, for `dark pack list`.
fn render_manifest_line(manifest: &PackManifest) -> String {
    format!(
        "{:<32} {:<12} {:>6} chunk(s)  staleness {:<6} embed {}",
        manifest.pack.pack_id(),
        manifest.pack.ecosystem,
        manifest.ingest.chunks,
        manifest.staleness.policy,
        manifest.embed.model,
    )
}

/// Runs `dark pack list`.
///
/// # Errors
///
/// Returns an error when `$DARK_HOME/packs` exists but cannot be listed.
fn run_list() -> anyhow::Result<()> {
    let manifests = cli::list(&packs_root()).map_err(crate::contract_error)?;
    if manifests.is_empty() {
        println!("no packs under {}.", packs_root().display());
        return Ok(());
    }
    for manifest in &manifests {
        println!("{}", render_manifest_line(manifest));
    }
    Ok(())
}

/// Runs `dark pack export <pack> -o <file>`.
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::PackNotFound`] when no pack has this
/// identifier. Returns an error when the archive cannot be written.
fn run_export(pack: &str, output: &Path) -> anyhow::Result<()> {
    cli::export(&packs_root(), pack, output).map_err(crate::contract_error)?;
    println!("wrote {}", output.display());
    Ok(())
}

/// Runs `dark pack import <file>`.
///
/// # Errors
///
/// Returns an error when the archive cannot be read, its manifest fails to
/// parse, or the pack directory cannot be written.
fn run_import(file: &Path) -> anyhow::Result<()> {
    let manifest = cli::import(&packs_root(), file).map_err(crate::contract_error)?;
    println!(
        "imported {} from {}",
        manifest.pack.pack_id(),
        file.display()
    );
    Ok(())
}

/// Runs `dark pack rm <pack>`.
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::PackNotFound`] when no pack has this
/// identifier. Returns an error when the directory cannot be removed.
fn run_rm(pack: &str) -> anyhow::Result<()> {
    cli::rm(&packs_root(), pack).map_err(crate::contract_error)?;
    println!("removed {pack}");
    Ok(())
}

/// An [`Embedder`](dark_lexicon::index::Embedder) backed by the real
/// engine.
///
/// [`dark_lexicon::index::Embedder::embed`] is synchronous — pack
/// indexing is a batch pipeline with nothing to interleave — while
/// [`dark_contract::Engine::embed`] is asynchronous. This adapter bridges
/// the two by driving the engine's future on a runtime handle, which is
/// why it lives in the composition root: it needs both the engine and the
/// runtime that owns it.
///
/// Every call must therefore come from a thread that is **not** inside
/// the runtime; [`with_embedder`] is the one place that arranges that.
struct EngineEmbedder<'a> {
    /// The engine that produces the vectors.
    engine: &'a dyn dark_contract::Engine,
    /// The runtime the engine's futures run on.
    handle: tokio::runtime::Handle,
}

impl dark_lexicon::index::Embedder for EngineEmbedder<'_> {
    fn embed(
        &self,
        texts: &[String],
        purpose: dark_contract::EmbedPurpose,
    ) -> dark_contract::Result<Vec<Vec<f32>>> {
        self.handle
            .block_on(self.engine.embed(texts.to_vec(), purpose))
    }
}

/// Brings a session up and hands `body` an embedder backed by its model.
///
/// The runtime is built here and `body` runs outside it, on this thread,
/// so [`EngineEmbedder`]'s `block_on` is never called from inside the
/// runtime — which would panic.
fn with_embedder<T>(
    body: impl FnOnce(
        &dyn dark_lexicon::index::Embedder,
        &dyn dark_contract::Engine,
        &dark_contract::Caps,
    ) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    use anyhow::Context as _;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the harness runtime")?;

    let bus = dark_contract::EventBus::new();
    let harness = runtime.block_on(crate::harness::bring_up(crate::harness::BringUp {
        root: crate::repo_root()?,
        dark_home: crate::dark_home(),
        preferred_model: None,
        policy: dark_core::policy::PolicyConfig::default(),
        mode: dark_core::policy::RunMode::Headless { yes: false },
        events: bus.tx(),
        tier_override: None,
    }))?;

    // The embedding model's own caps, not the worker's: a pack records
    // which model produced its vectors, and vectors from two different
    // models are not comparable.
    let caps = runtime
        .block_on(harness.engine.caps(dark_contract::RoleClass::Embed))
        .map_err(crate::contract_error)?;

    let embedder = EngineEmbedder {
        engine: harness.engine.as_ref(),
        handle: runtime.handle().clone(),
    };
    body(&embedder, harness.engine.as_ref(), &caps)
}

/// The staleness policy a pack takes when nothing names one.
const DEFAULT_STALENESS: &str = "90d";

/// Builds the [`SourceInput`](cli::SourceInput) for `source`.
///
/// Only the source kinds whose material is already on this machine are
/// built here. `sitemap` needs an HTTP fetcher, which is
/// `dark-airlock`'s to construct (Rule 13) and which the primary
/// requirement keeps off a working machine's path; it is reported by
/// name rather than half-wired.
///
/// # Errors
///
/// Returns an error when `--source` names a kind this command cannot
/// obtain material for, and when no kind is given and `source` is not a
/// local directory.
fn source_input(source: &str, source_kind: Option<&str>) -> anyhow::Result<cli::SourceInput> {
    let kind = match source_kind {
        Some(text) => cli::SourceKind::parse(text).map_err(crate::contract_error)?,
        None => cli::SourceKind::detect(source).ok_or_else(|| {
            anyhow::anyhow!(
                "{source} is not a local directory, so the source kind cannot be guessed. Pass \
                 --source-kind with one of: llms-txt, docsrs, git, localdir, openapi, manpage."
            )
        })?,
    };

    match kind {
        cli::SourceKind::Localdir => Ok(cli::SourceInput::Localdir {
            root: std::path::PathBuf::from(source),
        }),
        cli::SourceKind::Git => Ok(cli::SourceInput::Git {
            worktree_root: std::path::PathBuf::from(source),
            url_template: None,
        }),
        cli::SourceKind::LlmsTxt => Ok(cli::SourceInput::LlmsTxt {
            path: source.to_owned(),
            url: None,
            text: read_source_file(source, "the llms.txt file to ingest")?,
        }),
        cli::SourceKind::Openapi => Ok(cli::SourceInput::Openapi {
            json_text: read_source_file(source, "the OpenAPI document")?,
            base_url: None,
        }),
        cli::SourceKind::Manpage => Ok(cli::SourceInput::Manpage {
            name: default_name(source),
            rendered_text: read_source_file(source, "the rendered manual page")?,
        }),
        cli::SourceKind::Docsrs => Ok(cli::SourceInput::Docsrs {
            json_text: read_source_file(source, "the cargo doc JSON output")?,
            base_url: None,
        }),
        cli::SourceKind::Sitemap => anyhow::bail!(
            "a sitemap source needs to fetch pages over the network, which this command does not \
             do. Fetch the pages first and add them with --source-kind localdir."
        ),
    }
}

/// Reads a source file, naming what was expected when it cannot be read.
fn read_source_file(source: &str, what: &str) -> anyhow::Result<String> {
    std::fs::read_to_string(source)
        .map_err(|err| anyhow::anyhow!("cannot read {source}: {err}. Pass the path to {what}."))
}

/// Runs `dark pack add`.
fn run_add(
    source: &str,
    source_kind: Option<&str>,
    name: Option<&str>,
    version: Option<&str>,
) -> anyhow::Result<()> {
    let input = source_input(source, source_kind)?;
    // A pack with no name takes the last path segment, which is what
    // `dark pack add ./internal-docs` should be called.
    let name = name.map_or_else(|| default_name(source), str::to_owned);
    let version = version.unwrap_or("unversioned");

    let packs = packs_root();
    let manifest = with_embedder(|embedder, engine, caps| {
        cli::add(
            &packs,
            &cli::AddRequest {
                input,
                name: &name,
                version,
                ecosystem: "",
                aliases: Vec::new(),
                staleness_policy: DEFAULT_STALENESS,
                embed: embed_block(caps),
                override_responsibility: false,
                explicit_licence: None,
            },
            &cli::AddDeps {
                engine,
                embedder,
                fetcher: None,
            },
        )
        .map_err(crate::contract_error)
    })?;

    let pack_id = manifest.pack.pack_id();
    println!("added pack {pack_id}");
    println!("  {}", packs.join(&pack_id).display());
    Ok(())
}

/// Returns the pack name to use when `--name` is absent: the last path
/// segment of `source`, or `source` itself when it has none.
fn default_name(source: &str) -> String {
    std::path::Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source)
        .to_owned()
}

/// Builds the `[embed]` block describing the model that produced a pack's
/// vectors.
///
/// Recorded in the manifest so a later `dark pack reindex` can tell
/// whether the resident model still matches the one the vectors came
/// from — vectors from two different models are not comparable.
fn embed_block(caps: &dark_contract::Caps) -> dark_lexicon::pack::EmbedBlock {
    dark_lexicon::pack::EmbedBlock {
        model: caps.model_id.clone(),
        // The width is not known until the first vector comes back;
        // `cli::add` fills it in from what the embedder produced.
        dim: 0,
        quant: "int8".to_owned(),
        query_prefix: String::new(),
        doc_prefix: String::new(),
    }
}

/// Runs `dark pack refresh`.
fn run_refresh(all: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        all,
        "dark pack refresh needs --all, or a source to add again. Refreshing one pack in place \
         needs the source it came from, which the manifest does not record."
    );
    anyhow::bail!(
        "refreshing every pack needs each pack's original source, which a manifest does not \
         record. Add each pack again with dark pack add, or run dark pack reindex --all to \
         rebuild the indexes from the documents already ingested."
    )
}

/// Runs `dark pack reindex`.
///
/// Rebuilds each pack's indexes from the documents it already holds, with
/// no re-ingest: this is the path for a pack whose vectors came from a
/// model that is no longer the resident one.
fn run_reindex(all: bool) -> anyhow::Result<()> {
    let packs = packs_root();
    let ids = cli::list(&packs).map_err(crate::contract_error)?;
    anyhow::ensure!(
        !ids.is_empty(),
        "no pack is installed under {}. Add one with dark pack add.",
        packs.display()
    );
    anyhow::ensure!(
        all,
        "dark pack reindex needs --all. {} pack(s) are installed.",
        ids.len()
    );

    with_embedder(|embedder, _engine, caps| {
        for manifest in &ids {
            let pack_id = manifest.pack.pack_id();
            let rebuilt = cli::reindex(&packs, &pack_id, &embed_block(caps), embedder)
                .map_err(crate::contract_error)?;
            println!("reindexed {pack_id} ({})", rebuilt.embed.model);
        }
        Ok(())
    })
}

/// Runs `dark pack <action>`.
///
/// # Errors
///
/// See [`run_list`], [`run_export`], [`run_import`], and [`run_rm`]. `add`,
/// `refresh`, and `reindex` always return an error naming the task units
/// they wait on.
pub(crate) fn run_command(action: PackAction) -> anyhow::Result<()> {
    match action {
        PackAction::Add {
            source,
            source_kind,
            name,
            version,
        } => run_add(
            &source,
            source_kind.as_deref(),
            name.as_deref(),
            version.as_deref(),
        ),
        PackAction::List => run_list(),
        PackAction::Refresh { all } => run_refresh(all),
        PackAction::Rm { pack } => run_rm(&pack),
        PackAction::Export { pack, output } => run_export(&pack, &output),
        PackAction::Import { file } => run_import(&file),
        PackAction::Reindex { all } => run_reindex(all),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{EmbedPurpose, Result as ContractResult};
    use dark_engine_fake::{FakeEngine, Script};
    use dark_lexicon::cli::{AddDeps, AddRequest, SourceInput};
    use dark_lexicon::index::Embedder;
    use dark_lexicon::pack::EmbedBlock;
    use tempfile::TempDir;

    struct FixedEmbedder;
    impl Embedder for FixedEmbedder {
        fn embed(&self, texts: &[String], _purpose: EmbedPurpose) -> ContractResult<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0; 4]).collect())
        }
    }

    fn embed_block() -> EmbedBlock {
        EmbedBlock {
            model: "test-model".to_owned(),
            dim: 4,
            quant: "int8".to_owned(),
            query_prefix: String::new(),
            doc_prefix: String::new(),
        }
    }

    /// Builds one pack under `packs_root`, through the real, public
    /// [`cli::add`] — `dark-cli` is the one crate allowed a real `&dyn
    /// Engine` fixture (Rule 17), here [`FakeEngine`] as a dev-dependency,
    /// counting tokens for [`dark_lexicon::chunk::EngineCounter`].
    fn seed_pack(packs_root: &Path, src: &Path, name: &str, version: &str) {
        std::fs::create_dir_all(src).unwrap();
        std::fs::write(
            src.join("LICENSE"),
            "MIT License\n\nPermission is hereby granted...",
        )
        .unwrap();
        std::fs::write(
            src.join("intro.md"),
            "# Introduction\nThis library does useful things.\n",
        )
        .unwrap();

        let engine = FakeEngine::new(Script::default());
        let embedder = FixedEmbedder;
        cli::add(
            packs_root,
            &AddRequest {
                input: SourceInput::Localdir {
                    root: src.to_path_buf(),
                },
                name,
                version,
                ecosystem: "crates.io",
                aliases: vec![],
                staleness_policy: "90d",
                embed: embed_block(),
                override_responsibility: false,
                explicit_licence: None,
            },
            &AddDeps {
                engine: &engine,
                embedder: &embedder,
                fetcher: None,
            },
        )
        .unwrap();
    }

    #[test]
    fn run_list_reports_no_packs_for_an_empty_root() {
        let tmp = TempDir::new().unwrap();
        run_list_against(tmp.path()).unwrap();
    }

    /// The same body as [`run_list`], parameterised on `packs_root` so the
    /// test never touches the real `$DARK_HOME`.
    fn run_list_against(root: &Path) -> anyhow::Result<()> {
        let manifests = cli::list(root).map_err(crate::contract_error)?;
        if manifests.is_empty() {
            println!("no packs under {}.", root.display());
        }
        for manifest in &manifests {
            println!("{}", render_manifest_line(manifest));
        }
        Ok(())
    }

    #[test]
    fn render_manifest_line_names_the_pack_id_and_the_embed_model() {
        let tmp = TempDir::new().unwrap();
        let packs_root = tmp.path().join("packs");
        seed_pack(&packs_root, &tmp.path().join("src"), "tokio", "1.47.0");

        let manifests = cli::list(&packs_root).unwrap();
        let line = render_manifest_line(&manifests[0]);
        assert!(line.contains("tokio@1.47.0"));
        assert!(line.contains("test-model"));
    }

    #[test]
    fn export_then_import_and_rm_round_trip_through_this_module() {
        let tmp = TempDir::new().unwrap();
        let packs_root = tmp.path().join("packs");
        seed_pack(&packs_root, &tmp.path().join("src"), "tokio", "1.47.0");

        let archive = tmp.path().join("tokio.darkpack");
        cli::export(&packs_root, "tokio@1.47.0", &archive).unwrap();
        assert!(archive.is_file());

        let other_root = tmp.path().join("other-packs");
        let manifest = cli::import(&other_root, &archive).unwrap();
        assert_eq!(manifest.pack.pack_id(), "tokio@1.47.0");

        cli::rm(&packs_root, "tokio@1.47.0").unwrap();
        assert!(!packs_root.join("tokio@1.47.0").exists());
    }

    #[test]
    fn rm_reports_pack_not_found() {
        let tmp = TempDir::new().unwrap();
        let err = run_rm_against(&tmp.path().join("packs"), "no-such-pack").unwrap_err();
        assert!(err.to_string().contains("E_PACK_NOT_FOUND"));
    }

    fn run_rm_against(root: &Path, pack: &str) -> anyhow::Result<()> {
        cli::rm(root, pack).map_err(crate::contract_error)
    }

    #[test]
    fn a_local_directory_needs_no_source_kind() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().display().to_string();

        let Ok(input) = source_input(&source, None) else {
            panic!("a directory is a localdir source");
        };
        assert!(
            matches!(input, cli::SourceInput::Localdir { .. }),
            "the PRD's own example is `dark pack add ./internal-docs`"
        );
    }

    #[test]
    fn a_bare_name_with_no_source_kind_says_which_kinds_exist() {
        let Err(err) = source_input("tokio", None) else {
            panic!("a bare name gives no way to guess the source kind");
        };
        let message = err.to_string();
        assert!(message.contains("--source-kind"), "message: {message}");
        assert!(message.contains("localdir"), "message: {message}");
    }

    #[test]
    fn an_unknown_source_kind_is_rejected() {
        let Err(err) = source_input("tokio", Some("carrier-pigeon")) else {
            panic!("this names no adapter");
        };
        assert!(err.to_string().contains("carrier-pigeon"), "message: {err}");
    }

    #[test]
    fn a_sitemap_source_says_it_would_need_the_network() {
        let Err(err) = source_input("https://example.invalid/sitemap.xml", Some("sitemap")) else {
            panic!("a sitemap source needs the network");
        };
        let message = err.to_string();
        assert!(message.contains("network"), "message: {message}");
        assert!(
            message.contains("localdir"),
            "the message names the offline way to do it: {message}"
        );
    }

    #[test]
    fn a_file_source_that_does_not_exist_names_what_was_expected() {
        let Err(err) = source_input("/no/such/openapi.json", Some("openapi")) else {
            panic!("a file that does not exist cannot be read");
        };
        assert!(err.to_string().contains("OpenAPI"), "message: {err}");
    }

    #[test]
    fn a_file_source_is_read_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("openapi.json");
        std::fs::write(&path, r#"{"openapi": "3.0.0"}"#).unwrap();

        let Ok(cli::SourceInput::Openapi { json_text, .. }) =
            source_input(&path.display().to_string(), Some("openapi"))
        else {
            panic!("expected an OpenAPI source");
        };
        assert!(json_text.contains("3.0.0"), "text: {json_text}");
    }

    #[test]
    fn a_pack_with_no_name_takes_the_last_path_segment() {
        assert_eq!(default_name("./internal-docs"), "internal-docs");
        assert_eq!(default_name("/a/b/tokio"), "tokio");
        assert_eq!(default_name("tokio"), "tokio");
    }

    #[test]
    fn refresh_without_all_says_what_it_needs() {
        let err = run_refresh(false).unwrap_err();
        assert!(err.to_string().contains("--all"), "message: {err}");
    }

    #[test]
    fn refresh_all_explains_why_it_cannot_and_names_the_alternative() {
        let err = run_refresh(true).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("dark pack reindex"),
            "the message names what to run instead: {message}"
        );
    }
}
