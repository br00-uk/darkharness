//! `dark doctor`: checks the installation and names a remedy for anything
//! that needs one.
//!
//! Every check in this module runs with no network. That is not an
//! implementation detail. The primary requirement of the whole harness is
//! that a person disconnects the network after `dark setup` completes and
//! keeps working; `dark doctor` is the command that tells them whether
//! that is true, so it must not itself reach for the network to answer.
//!
//! Several checks depend on task units that have not landed yet:
//! `dark-engine` does not load a model until `B2` to `B7`; `dark tune`
//! writes a hardware profile in `B6`; documentation packs exist from `G1`
//! to `G5`; the tree-sitter grammars that `/explore` uses arrive with
//! `F1`. Each of those checks reports [`Status::Pending`] and names the
//! task unit, instead of reporting a pass it cannot back up. See
//! [`Finding`] for the shape every check returns, and the function-level
//! documentation below for which check is real today and which is
//! pending.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use dark_agentsmd::{AgentsMdConfig, WorkingSet, explain, resolve as resolve_chain};

/// The outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// The check passed.
    Ok,
    /// The check passed, but the message is worth reading.
    Warn,
    /// The check failed. Apply the remedy before you disconnect.
    Fail,
    /// The check cannot run yet. It waits on a task unit that has not
    /// landed.
    Pending,
}

impl Status {
    /// Returns the label this status prints at the start of a report line.
    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Pending => "PENDING",
        }
    }
}

/// One line of `dark doctor` output: one check, its outcome, and — for
/// anything short of [`Status::Ok`] — the remedy that clears it.
#[derive(Debug, Clone)]
pub(crate) struct Finding {
    /// The check name. Matches the `Check` column of the task unit `J3`
    /// table in the build specification.
    pub(crate) check: &'static str,
    /// Whether the check passed, warned, failed, or waits on a task unit.
    pub(crate) status: Status,
    /// What the check found, for a person to read.
    pub(crate) message: String,
    /// The action that clears the finding. `None` only when `status` is
    /// [`Status::Ok`].
    pub(crate) remedy: Option<String>,
    /// Whether the remedy needs the network to carry out, for example a
    /// download or a package install.
    ///
    /// `dark doctor --offline` reads this field to decide whether a
    /// finding blocks `OFFLINE READY`: a person cannot clear a
    /// network-needing finding without reconnecting, so it must block; a
    /// finding whose remedy is local (edit a file, wait for a task unit
    /// to land, rebuild from an already-fetched cache) does not.
    pub(crate) network_remedy: bool,
}

impl Finding {
    /// Builds a passing finding.
    fn ok(check: &'static str, message: impl Into<String>) -> Self {
        Self {
            check,
            status: Status::Ok,
            message: message.into(),
            remedy: None,
            network_remedy: false,
        }
    }

    /// Builds a finding that passed with a caveat.
    fn warn(check: &'static str, message: impl Into<String>, remedy: impl Into<String>) -> Self {
        Self {
            check,
            status: Status::Warn,
            message: message.into(),
            remedy: Some(remedy.into()),
            network_remedy: false,
        }
    }

    /// Builds a failing finding. Set `network_remedy` when a person needs
    /// the network to apply `remedy`.
    fn fail(
        check: &'static str,
        message: impl Into<String>,
        remedy: impl Into<String>,
        network_remedy: bool,
    ) -> Self {
        Self {
            check,
            status: Status::Fail,
            message: message.into(),
            remedy: Some(remedy.into()),
            network_remedy,
        }
    }

    /// Builds a finding for a check that cannot run yet because
    /// `task_unit` has not landed.
    fn pending(
        check: &'static str,
        message: impl Into<String>,
        task_unit: &'static str,
        network_remedy: bool,
    ) -> Self {
        Self {
            check,
            status: Status::Pending,
            message: message.into(),
            remedy: Some(format!("Wait for task unit {task_unit}.")),
            network_remedy,
        }
    }
}

/// The accelerator that darkharness cares about: the device kinds
/// `dark-engine` loads a model onto (see [`dark_contract::Device`]), or
/// none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Accelerator {
    /// No usable graphics processor.
    None,
    /// An NVIDIA graphics processor, driven through CUDA.
    Cuda,
    /// Apple Silicon, driven through Metal.
    Metal,
}

impl Accelerator {
    /// Returns the hardware class name that `dark tune` will eventually
    /// report (Rule 9).
    fn class_name(self) -> &'static str {
        match self {
            Self::None => "central processor only",
            Self::Cuda => "graphics processor (CUDA)",
            Self::Metal => "Apple Silicon (Metal)",
        }
    }
}

/// Which release artefact this binary is. See the README's "Build
/// artefacts" table and Rule 18.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildVariant {
    /// `dark-cpu`: the default, portable build.
    Cpu,
    /// `dark-cuda`: built with the `cuda` and `flash-attn` features.
    Cuda,
    /// `dark-metal`: built with the `metal` feature.
    Metal,
}

impl BuildVariant {
    /// Returns the variant that this binary was compiled as.
    fn current() -> Self {
        if cfg!(feature = "cuda") {
            Self::Cuda
        } else if cfg!(feature = "metal") {
            Self::Metal
        } else {
            Self::Cpu
        }
    }

    /// Returns the artefact name, for example `dark-cpu`.
    fn artefact_name(self) -> &'static str {
        match self {
            Self::Cpu => "dark-cpu",
            Self::Cuda => "dark-cuda",
            Self::Metal => "dark-metal",
        }
    }
}

/// The facts about this machine that the checks read.
///
/// Gathering these facts is the one place in this module that touches the
/// process environment, the filesystem, or a child process. Every check
/// function below takes a `&HostFacts` and makes its decision as a pure
/// function of it, so a test builds a `HostFacts` fixture directly and
/// never has to fake a process or an environment variable.
struct HostFacts {
    /// The detected accelerator, or [`Accelerator::None`].
    accelerator: Accelerator,
    /// The artefact that this binary is.
    build_variant: BuildVariant,
    /// Total system memory, in bytes.
    total_memory_bytes: u64,
    /// Memory available for a new allocation, in bytes.
    available_memory_bytes: u64,
    /// The `git --version` output, when Git is on `PATH`.
    git_version: Option<String>,
    /// The `TERM` environment variable.
    term: Option<String>,
    /// Whether standard output is a terminal.
    stdout_is_terminal: bool,
    /// `$DARK_HOME`.
    dark_home: PathBuf,
    /// The repository root.
    repo_root: PathBuf,
}

/// Gathers [`HostFacts`] from the real machine.
fn gather_host_facts(dark_home: PathBuf, repo_root: PathBuf) -> HostFacts {
    let (total_memory_bytes, available_memory_bytes) = memory_bytes();
    HostFacts {
        accelerator: detect_accelerator(),
        build_variant: BuildVariant::current(),
        total_memory_bytes,
        available_memory_bytes,
        git_version: detect_git(),
        term: std::env::var("TERM").ok(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
        dark_home,
        repo_root,
    }
}

/// Returns `(total, available)` system memory in bytes.
fn memory_bytes() -> (u64, u64) {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    (system.total_memory(), system.available_memory())
}

/// Detects the accelerator on this machine.
///
/// Apple Silicon is detected from the target triple this binary runs as:
/// a darkharness binary always runs natively on the machine it checks, so
/// the running process's architecture is the host's architecture. An
/// NVIDIA graphics processor is detected without spawning a process where
/// possible, by reading the driver's version file; `nvidia-smi` is the
/// fallback for a driver layout this crate does not otherwise recognise.
fn detect_accelerator() -> Accelerator {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        return Accelerator::Metal;
    }
    if nvidia_gpu_present() {
        return Accelerator::Cuda;
    }
    Accelerator::None
}

/// Reports whether an NVIDIA graphics processor is present.
fn nvidia_gpu_present() -> bool {
    if Path::new("/proc/driver/nvidia/version").is_file() {
        return true;
    }
    Command::new("nvidia-smi")
        .arg("-L")
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Runs `git --version` and returns its trimmed output, when Git is on
/// `PATH` and runs successfully.
fn detect_git() -> Option<String> {
    let output = Command::new("git").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// Counts the entries directly inside `dir` that are themselves
/// directories. Returns `0` when `dir` does not exist.
fn count_child_dirs(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count()
    })
}

/// Approximates a token count by splitting `text` on whitespace.
///
/// This is not the model's real tokenizer: no model is loaded yet, and
/// [`dark_contract::Engine::tokenize`] needs task unit `B2`. A word count
/// runs consistently over the reported budget for English prose, which is
/// what an `AGENTS.md` file is, so it is close enough to flag a chain that
/// needs attention without pretending to be exact.
fn approximate_token_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// `24 GiB`, in bytes. Rule 1's line between a machine that shares one
/// resident model across the architect, worker, and scout roles, and one
/// that need not.
const TWENTY_FOUR_GIB: u64 = 24 * 1024 * 1024 * 1024;

/// The minimum available memory, in bytes, below which even the smallest
/// supported profile does not fit.
///
/// Rule 10's central-processor default is a 4B model. At 4 bits per
/// weight, section 4.1's formula puts its weights at roughly 2 GiB before
/// the key-value cache or headroom. This threshold is deliberately a
/// little under that, so the check does not fail a machine that fits the
/// smallest profile with nothing to spare.
const MINIMUM_AVAILABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Converts a byte count to gibibytes, for display.
#[allow(clippy::cast_precision_loss)]
fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Checks the build variant against the detected accelerator. Real: this
/// needs no loaded model, only the host and the features this binary was
/// compiled with. See Rule 18.
fn check_accelerator(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Build variant against detected accelerator";
    match (facts.build_variant, facts.accelerator) {
        (BuildVariant::Cpu, Accelerator::Cuda) => Finding::fail(
            CHECK,
            "this machine has an NVIDIA graphics processor, but this binary is dark-cpu.",
            "Install the dark-cuda artefact.",
            true,
        ),
        (BuildVariant::Cpu, Accelerator::Metal) => Finding::fail(
            CHECK,
            "this machine is Apple Silicon, but this binary is dark-cpu.",
            "Install the dark-metal artefact.",
            true,
        ),
        (BuildVariant::Cuda, Accelerator::None | Accelerator::Metal) => Finding::warn(
            CHECK,
            "this binary is dark-cuda, but no NVIDIA graphics processor is present.",
            "Install dark-cpu, or run this binary on a machine with an NVIDIA graphics \
             processor.",
        ),
        (BuildVariant::Metal, Accelerator::None | Accelerator::Cuda) => Finding::warn(
            CHECK,
            "this binary is dark-metal, but this machine is not Apple Silicon.",
            "Install dark-cpu, or run this binary on Apple Silicon.",
        ),
        (BuildVariant::Cpu, Accelerator::None)
        | (BuildVariant::Cuda, Accelerator::Cuda)
        | (BuildVariant::Metal, Accelerator::Metal) => Finding::ok(
            CHECK,
            format!(
                "{} matches the detected accelerator ({}).",
                facts.build_variant.artefact_name(),
                facts.accelerator.class_name(),
            ),
        ),
    }
}

/// Checks total memory, available memory, and the budget the harness
/// would grant. Real: Section 4.1's formula is arithmetic over what
/// [`sysinfo`] reports; it needs no loaded model.
fn check_memory(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Total memory, available memory, budget";
    // Section 4.1: total = weights + kv_cache + 10% headroom. Reserving
    // that headroom out of available memory gives the budget a caller
    // should plan against before the resident set manager (Rule 4) exists
    // to enforce it directly. Integer arithmetic throughout: a byte count
    // this large loses precision the moment it crosses into an f64.
    let budget_bytes = facts.available_memory_bytes / 10 * 9;
    let sharing_note = if facts.total_memory_bytes < TWENTY_FOUR_GIB {
        "below 24 GiB, the architect, worker, and scout roles share one resident model \
         (Rule 1); this is the default, not a failure."
    } else {
        "24 GiB or more: the architect, worker, and scout roles may each hold a separate \
         resident model."
    };
    let message = format!(
        "total {:.1} GiB, available {:.1} GiB, budget {:.1} GiB (10% headroom reserved). {}",
        bytes_to_gib(facts.total_memory_bytes),
        bytes_to_gib(facts.available_memory_bytes),
        bytes_to_gib(budget_bytes),
        sharing_note,
    );
    if facts.available_memory_bytes < MINIMUM_AVAILABLE_BYTES {
        return Finding::fail(
            CHECK,
            message,
            "Reduce the context or the model size.",
            false,
        );
    }
    Finding::ok(CHECK, message)
}

/// Checks the installed models' manifest hashes.
///
/// Pending: model loading, and the manifest format it writes, are task
/// unit `B2`. This reports how many model directories already exist
/// under `$DARK_HOME/models`, which needs no model and no manifest
/// schema, without claiming to have verified anything inside them.
fn check_model_manifests(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Model manifest hashes";
    let models_dir = facts.dark_home.join("models");
    let count = count_child_dirs(&models_dir);
    let message = if count == 0 {
        format!("no model directory found at {}.", models_dir.display())
    } else {
        format!(
            "{count} model director{} at {}. Hash verification needs task unit B2's manifest \
             format.",
            if count == 1 { "y" } else { "ies" },
            models_dir.display(),
        )
    };
    Finding::pending(CHECK, message, "B2", true)
}

/// Checks live generation and embedding against the loaded model.
///
/// Pending: `dark-engine` is a placeholder crate today (task units `B2`
/// to `B5` load a model, run generation, and run embedding). Do not fake
/// a live test against a model that cannot load.
fn check_live_generation() -> Finding {
    const CHECK: &str = "Live generation and embedding";
    Finding::pending(
        CHECK,
        "dark-engine cannot load a model yet, so no live generation, tool call, or embedding \
         can run.",
        "B2 to B5",
        true,
    )
}

/// Checks the measured generation rate and hardware class (Rule 9).
///
/// Pending for the measured rate: that is `dark tune`, task unit `B6`.
/// The hardware class half is real today, because it is the same
/// accelerator detection [`check_accelerator`] already ran.
fn check_measured_rate(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Measured rate and hardware class";
    Finding::pending(
        CHECK,
        format!(
            "hardware class: {}. No measured generation rate yet; dark tune has not run.",
            facts.accelerator.class_name(),
        ),
        "B6",
        false,
    )
}

/// Checks pack hashes and staleness.
///
/// Pending: the pack format (`G1`) and the pack commands (`G5`) are
/// placeholders today. This reports how many pack directories already
/// exist under `$DARK_HOME/packs`, without claiming to have verified
/// them.
fn check_pack_hashes(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Pack hashes and staleness";
    let packs_dir = facts.dark_home.join("packs");
    let count = count_child_dirs(&packs_dir);
    let message = if count == 0 {
        format!("no pack directory found at {}.", packs_dir.display())
    } else {
        format!(
            "{count} pack director{} at {}. Hash verification needs task units G1 to G5.",
            if count == 1 { "y" } else { "ies" },
            packs_dir.display(),
        )
    };
    Finding::pending(CHECK, message, "G1 to G5", true)
}

/// Checks the embedding model against pack manifests, for
/// `E_PACK_DIM_MISMATCH` (Appendix A).
///
/// Pending: this needs both an embedding model (`B5`) and pack manifests
/// (`G4`).
fn check_embedding_vs_packs() -> Finding {
    const CHECK: &str = "Embedding model against pack manifests";
    Finding::pending(
        CHECK,
        "no embedding model is loaded and no pack manifest exists yet to compare it against.",
        "B5 and G4",
        false,
    )
}

/// Checks the instruction chain token count (Rule 24). Real: task unit
/// `K3` already builds [`explain::render`] and [`explain::quality_warnings`]
/// over a chain that [`dark_agentsmd::resolve`] resolves from files on
/// disk; this check calls both, using an approximate tokenizer until
/// `B2` provides the real one.
fn check_instruction_chain(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Instruction chain token count";
    let config = AgentsMdConfig::default();
    let counter: &dyn Fn(&str) -> usize = &approximate_token_count;
    let chain = match resolve_chain(
        &facts.dark_home,
        &facts.repo_root,
        &WorkingSet::new(),
        &config,
        counter,
    ) {
        Ok(chain) => chain,
        Err(err) => {
            return Finding::fail(
                CHECK,
                format!("could not resolve the instruction chain: {err}"),
                "Check file permissions under the repository root and $DARK_HOME.",
                false,
            );
        }
    };

    let readme = std::fs::read_to_string(facts.repo_root.join("README.md")).ok();
    let quality = explain::quality_warnings(&chain, readme.as_deref());
    let rendered = explain::render(&chain, &facts.repo_root, readme.as_deref());
    let message = format!(
        "{} (token count is an approximate word count; the real tokenizer arrives with task \
         unit B2)",
        rendered.trim_end(),
    );

    if chain.warnings().is_empty() && quality.is_empty() {
        Finding::ok(CHECK, message)
    } else {
        Finding::warn(CHECK, message, "Reduce AGENTS.md.")
    }
}

/// Checks tree-sitter grammar versions.
///
/// Pending: `/explore`'s parsing stage, and the grammars it registers,
/// are task unit `F1`. `dark-explore` is a placeholder crate today.
fn check_tree_sitter() -> Finding {
    const CHECK: &str = "Tree-sitter grammar versions";
    Finding::pending(
        CHECK,
        "dark-explore does not register a tree-sitter grammar yet.",
        "F1",
        false,
    )
}

/// Checks that Git is on `PATH`. Real: `/explore` needs Git for co-change
/// (see the `Check` table remedy), and this runs `git --version` with no
/// network involved.
fn check_git(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Git presence";
    match &facts.git_version {
        Some(version) => Finding::ok(CHECK, format!("found {version}.")),
        None => Finding::fail(CHECK, "git is not on PATH.", "Install Git.", true),
    }
}

/// Checks terminal capability. Real: reads `TERM` and whether standard
/// output is a terminal.
fn check_terminal(facts: &HostFacts) -> Finding {
    const CHECK: &str = "Terminal capability";
    match facts.term.as_deref() {
        None | Some("") => Finding::fail(CHECK, "TERM is not set.", "Set TERM correctly.", false),
        Some("dumb") => Finding::warn(
            CHECK,
            "TERM is \"dumb\"; the full-screen terminal application needs a capable terminal.",
            "Set TERM correctly.",
        ),
        Some(term) => {
            if facts.stdout_is_terminal {
                Finding::ok(
                    CHECK,
                    format!("TERM is \"{term}\" and standard output is a terminal."),
                )
            } else {
                Finding::warn(
                    CHECK,
                    format!(
                        "TERM is \"{term}\", but standard output is not a terminal (output may \
                         be redirected)."
                    ),
                    "Run dark inside a terminal, not through a pipe or a file redirect.",
                )
            }
        }
    }
}

/// The full result of `dark doctor`: one [`Finding`] per check, in the
/// order the task unit `J3` table lists them.
#[derive(Debug, Clone)]
pub(crate) struct Report {
    /// One finding per check.
    pub(crate) findings: Vec<Finding>,
}

impl Report {
    /// Reports whether any check failed outright.
    #[must_use]
    pub(crate) fn has_failures(&self) -> bool {
        self.findings.iter().any(|f| f.status == Status::Fail)
    }

    /// Reports whether the machine is not ready to keep working with the
    /// network gone: some finding is not [`Status::Ok`] and its remedy
    /// needs the network to apply.
    #[must_use]
    pub(crate) fn offline_blocked(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.status != Status::Ok && f.network_remedy)
    }

    /// Renders the report as text for a person to read.
    ///
    /// `offline` selects the closing line: `OFFLINE READY` (or the list
    /// of blockers) when `true`, a plain pass/warn/fail/pending summary
    /// otherwise.
    #[must_use]
    pub(crate) fn render(&self, offline: bool) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for finding in &self.findings {
            let _ = writeln!(
                out,
                "[{}] {}: {}",
                finding.status.label(),
                finding.check,
                finding.message,
            );
            if let Some(remedy) = &finding.remedy {
                let _ = writeln!(out, "       remedy: {remedy}");
            }
        }
        out.push('\n');
        if offline {
            if self.offline_blocked() {
                out.push_str("NOT READY. These items need the network before you disconnect:\n");
                for finding in self
                    .findings
                    .iter()
                    .filter(|f| f.status != Status::Ok && f.network_remedy)
                {
                    let _ = writeln!(out, "- {}", finding.check);
                }
            } else {
                out.push_str("OFFLINE READY\n");
            }
        } else {
            let ok = self.count(Status::Ok);
            let warn = self.count(Status::Warn);
            let fail = self.count(Status::Fail);
            let pending = self.count(Status::Pending);
            let _ = writeln!(
                out,
                "{ok} passed, {warn} warned, {fail} failed, {pending} pending.",
            );
        }
        out
    }

    /// Counts findings with the given status.
    fn count(&self, status: Status) -> usize {
        self.findings.iter().filter(|f| f.status == status).count()
    }
}

/// Runs every check in the task unit `J3` table and returns the report.
///
/// `setup` (task unit `J3`, step 8) calls this directly to print
/// `OFFLINE READY` or the list of what is missing at the end of its own
/// run, instead of shelling out to `dark doctor` as a subprocess.
pub(crate) fn report(dark_home: &Path, repo_root: &Path) -> Report {
    let facts = gather_host_facts(dark_home.to_path_buf(), repo_root.to_path_buf());
    Report {
        findings: vec![
            check_accelerator(&facts),
            check_memory(&facts),
            check_model_manifests(&facts),
            check_live_generation(),
            check_measured_rate(&facts),
            check_pack_hashes(&facts),
            check_embedding_vs_packs(),
            check_instruction_chain(&facts),
            check_tree_sitter(),
            check_git(&facts),
            check_terminal(&facts),
        ],
    }
}

/// Runs `dark doctor`, prints the report, and returns an error (so the
/// process exits with a non-zero code) when the report says the person is
/// not ready.
///
/// Plain `dark doctor` fails when any check outright fails
/// ([`Status::Fail`]); a pending check does not fail it, because that is
/// expected while task units `B2` to `B7`, `F1`, and `G1` to `G5` are
/// still open. `dark doctor --offline` fails when [`Report::offline_blocked`]
/// is true — some finding is not [`Status::Ok`] and needs the network to
/// clear.
///
/// # Errors
///
/// Returns an error when the report is not clean for the mode requested.
/// The report itself has already been printed by the time this returns
/// one.
pub(crate) fn run_command(offline: bool) -> anyhow::Result<()> {
    let dark_home = crate::dark_home();
    let repo_root = crate::repo_root()?;
    let outcome = report(&dark_home, &repo_root);
    print!("{}", outcome.render(offline));

    if offline {
        if outcome.offline_blocked() {
            anyhow::bail!(
                "dark doctor --offline found items that need the network. See the report above."
            );
        }
    } else if outcome.has_failures() {
        anyhow::bail!("dark doctor found failing checks. See the report above.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Builds a `HostFacts` fixture with every field explicit, so a test
    /// changes only what it cares about.
    fn facts(dark_home: &Path, repo_root: &Path) -> HostFacts {
        HostFacts {
            accelerator: Accelerator::None,
            build_variant: BuildVariant::Cpu,
            total_memory_bytes: 32 * 1024 * 1024 * 1024,
            available_memory_bytes: 16 * 1024 * 1024 * 1024,
            git_version: Some("git version 2.44.0".to_owned()),
            term: Some("xterm-256color".to_owned()),
            stdout_is_terminal: true,
            dark_home: dark_home.to_path_buf(),
            repo_root: repo_root.to_path_buf(),
        }
    }

    #[test]
    fn accelerator_check_passes_when_cpu_build_matches_no_accelerator() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_accelerator(&f);
        assert_eq!(finding.status, Status::Ok);
    }

    #[test]
    fn accelerator_check_fails_cpu_build_on_a_cuda_machine() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.accelerator = Accelerator::Cuda;
        let finding = check_accelerator(&f);
        assert_eq!(finding.status, Status::Fail);
        assert!(finding.network_remedy);
        assert!(finding.remedy.unwrap().contains("dark-cuda"));
    }

    #[test]
    fn accelerator_check_fails_cpu_build_on_apple_silicon() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.accelerator = Accelerator::Metal;
        let finding = check_accelerator(&f);
        assert_eq!(finding.status, Status::Fail);
        assert!(finding.remedy.unwrap().contains("dark-metal"));
    }

    #[test]
    fn accelerator_check_passes_when_cuda_build_matches_cuda_machine() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.build_variant = BuildVariant::Cuda;
        f.accelerator = Accelerator::Cuda;
        let finding = check_accelerator(&f);
        assert_eq!(finding.status, Status::Ok);
    }

    #[test]
    fn accelerator_check_warns_cuda_build_on_a_cpu_only_machine() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.build_variant = BuildVariant::Cuda;
        f.accelerator = Accelerator::None;
        let finding = check_accelerator(&f);
        assert_eq!(finding.status, Status::Warn);
        assert!(!finding.network_remedy);
    }

    #[test]
    fn memory_check_passes_with_plenty_of_memory() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_memory(&f);
        assert_eq!(finding.status, Status::Ok);
        assert!(finding.message.contains("32.0 GiB"));
    }

    #[test]
    fn memory_check_notes_shared_model_below_24_gib() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.total_memory_bytes = 16 * 1024 * 1024 * 1024;
        let finding = check_memory(&f);
        assert_eq!(
            finding.status,
            Status::Ok,
            "below 24 GiB is not a failure, Rule 1"
        );
        assert!(finding.message.contains("share one resident model"));
    }

    #[test]
    fn memory_check_fails_when_available_memory_is_critically_low() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.available_memory_bytes = 512 * 1024 * 1024;
        let finding = check_memory(&f);
        assert_eq!(finding.status, Status::Fail);
        assert_eq!(
            finding.remedy.as_deref(),
            Some("Reduce the context or the model size.")
        );
        assert!(!finding.network_remedy);
    }

    #[test]
    fn model_manifest_check_reports_pending_and_counts_model_directories() {
        let tmp = TempDir::new().unwrap();
        let models = tmp.path().join("models");
        fs::create_dir_all(models.join("Qwen__Qwen3-4B")).unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_model_manifests(&f);
        assert_eq!(finding.status, Status::Pending);
        assert!(finding.network_remedy);
        assert!(finding.message.contains('1'));
    }

    #[test]
    fn model_manifest_check_reports_no_directory_when_absent() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_model_manifests(&f);
        assert!(finding.message.contains("no model directory"));
    }

    #[test]
    fn live_generation_check_is_pending() {
        let finding = check_live_generation();
        assert_eq!(finding.status, Status::Pending);
        assert!(finding.network_remedy);
    }

    #[test]
    fn measured_rate_check_is_pending_and_not_network_blocking() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_measured_rate(&f);
        assert_eq!(finding.status, Status::Pending);
        assert!(!finding.network_remedy);
        assert!(finding.message.contains("central processor only"));
    }

    #[test]
    fn pack_hashes_check_counts_pack_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("packs").join("react@18")).unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_pack_hashes(&f);
        assert_eq!(finding.status, Status::Pending);
        assert!(finding.message.contains('1'));
    }

    #[test]
    fn embedding_vs_packs_check_is_pending_and_not_network_blocking() {
        let finding = check_embedding_vs_packs();
        assert_eq!(finding.status, Status::Pending);
        assert!(!finding.network_remedy);
    }

    #[test]
    fn tree_sitter_check_is_pending_and_not_network_blocking() {
        let finding = check_tree_sitter();
        assert_eq!(finding.status, Status::Pending);
        assert!(!finding.network_remedy);
    }

    #[test]
    fn git_check_passes_when_git_version_is_present() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_git(&f);
        assert_eq!(finding.status, Status::Ok);
    }

    #[test]
    fn git_check_fails_when_git_is_absent() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.git_version = None;
        let finding = check_git(&f);
        assert_eq!(finding.status, Status::Fail);
        assert!(finding.network_remedy);
        assert_eq!(finding.remedy.as_deref(), Some("Install Git."));
    }

    #[test]
    fn terminal_check_fails_when_term_is_unset() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.term = None;
        let finding = check_terminal(&f);
        assert_eq!(finding.status, Status::Fail);
        assert_eq!(finding.remedy.as_deref(), Some("Set TERM correctly."));
    }

    #[test]
    fn terminal_check_warns_on_dumb_term() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.term = Some("dumb".to_owned());
        let finding = check_terminal(&f);
        assert_eq!(finding.status, Status::Warn);
    }

    #[test]
    fn terminal_check_warns_when_stdout_is_not_a_terminal() {
        let tmp = TempDir::new().unwrap();
        let mut f = facts(tmp.path(), tmp.path());
        f.stdout_is_terminal = false;
        let finding = check_terminal(&f);
        assert_eq!(finding.status, Status::Warn);
    }

    #[test]
    fn terminal_check_passes_with_a_real_term_and_a_terminal() {
        let tmp = TempDir::new().unwrap();
        let f = facts(tmp.path(), tmp.path());
        let finding = check_terminal(&f);
        assert_eq!(finding.status, Status::Ok);
    }

    #[test]
    fn instruction_chain_check_passes_on_a_short_chain() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("AGENTS.md"), "be terse. use active voice.").unwrap();
        let f = facts(&home, &repo);
        let finding = check_instruction_chain(&f);
        assert_eq!(finding.status, Status::Ok);
        assert!(finding.message.contains("token"));
    }

    #[test]
    fn instruction_chain_check_warns_when_the_root_file_is_very_long() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let long = "one short line of text\n".repeat(200);
        fs::write(repo.join("AGENTS.md"), long).unwrap();
        let f = facts(&home, &repo);
        let finding = check_instruction_chain(&f);
        assert_eq!(finding.status, Status::Warn);
        assert_eq!(finding.remedy.as_deref(), Some("Reduce AGENTS.md."));
    }

    #[test]
    fn instruction_chain_check_handles_an_empty_chain() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&repo).unwrap();
        let f = facts(&home, &repo);
        let finding = check_instruction_chain(&f);
        assert_eq!(finding.status, Status::Ok);
        assert!(finding.message.contains("empty"));
    }

    #[test]
    fn report_offline_blocked_is_true_when_a_network_remedy_finding_is_not_ok() {
        let report = Report {
            findings: vec![
                Finding::ok("a", "fine"),
                Finding::fail("b", "broken", "reconnect and download", true),
            ],
        };
        assert!(report.offline_blocked());
    }

    #[test]
    fn report_offline_blocked_is_false_when_only_local_remedies_are_pending() {
        let report = Report {
            findings: vec![
                Finding::ok("a", "fine"),
                Finding::pending("b", "not built yet", "F1", false),
            ],
        };
        assert!(!report.offline_blocked());
    }

    #[test]
    fn report_has_failures_ignores_pending_and_warn() {
        let report = Report {
            findings: vec![
                Finding::warn("a", "careful", "look at it"),
                Finding::pending("b", "not built yet", "F1", false),
            ],
        };
        assert!(!report.has_failures());
    }

    #[test]
    fn report_render_offline_ready_when_nothing_blocks() {
        let report = Report {
            findings: vec![Finding::ok("a", "fine")],
        };
        assert!(report.render(true).contains("OFFLINE READY"));
    }

    #[test]
    fn report_render_lists_blockers_when_offline_blocked() {
        let report = Report {
            findings: vec![Finding::fail("a", "broken", "download it", true)],
        };
        let text = report.render(true);
        assert!(text.contains("NOT READY"));
        assert!(text.contains('a'));
        assert!(!text.contains("OFFLINE READY"));
    }

    #[test]
    fn report_render_plain_mode_summarises_counts() {
        let report = Report {
            findings: vec![
                Finding::ok("a", "fine"),
                Finding::warn("b", "careful", "look"),
                Finding::pending("c", "not yet", "B2", true),
            ],
        };
        let text = report.render(false);
        assert!(text.contains("1 passed, 1 warned, 0 failed, 1 pending."));
    }

    #[test]
    fn run_produces_one_finding_per_check_in_table_order() {
        let tmp = TempDir::new().unwrap();
        let report = report(tmp.path(), tmp.path());
        let checks: Vec<&str> = report.findings.iter().map(|f| f.check).collect();
        assert_eq!(
            checks,
            vec![
                "Build variant against detected accelerator",
                "Total memory, available memory, budget",
                "Model manifest hashes",
                "Live generation and embedding",
                "Measured rate and hardware class",
                "Pack hashes and staleness",
                "Embedding model against pack manifests",
                "Instruction chain token count",
                "Tree-sitter grammar versions",
                "Git presence",
                "Terminal capability",
            ]
        );
    }

    #[test]
    fn every_non_ok_finding_names_a_remedy() {
        let tmp = TempDir::new().unwrap();
        let report = report(tmp.path(), tmp.path());
        for finding in &report.findings {
            if finding.status != Status::Ok {
                assert!(
                    finding.remedy.is_some(),
                    "{} has no remedy for status {:?}",
                    finding.check,
                    finding.status
                );
            }
        }
    }

    #[test]
    fn count_child_dirs_is_zero_for_a_missing_directory() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(count_child_dirs(&tmp.path().join("absent")), 0);
    }

    #[test]
    fn count_child_dirs_ignores_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a-file"), "x").unwrap();
        fs::create_dir(tmp.path().join("a-dir")).unwrap();
        assert_eq!(count_child_dirs(tmp.path()), 1);
    }
}
