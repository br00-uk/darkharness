//! `cargo xtask airgap`: task unit `J5`, the air-gap test.
//!
//! Section 3.2 is the whole project's reason to exist: after `dark setup`
//! completes, a person disconnects the network and keeps working. This
//! module is the test that proves that claim rather than assumes it.
//!
//! # What "genuinely denies the network" means here
//!
//! A test that runs with no network available proves nothing if nothing
//! it does would have needed the network anyway. This module instead:
//!
//! 1. builds the real `dark` binary;
//! 2. puts a real Linux network namespace around every command it runs,
//!    with `unshare --net` — the same technique
//!    `crates/dark-tools/src/exec/netns.rs` already uses for dark mode's
//!    child processes, except that module treats the wrap as best
//!    effort and silently falls back when it is unavailable. This
//!    module is the opposite of best effort: [`ensure_isolation_works`]
//!    proves the namespace has no route to a real external address
//!    before it trusts it for anything else, and refuses to run at all
//!    when it cannot get one;
//! 3. runs the scripted session task unit `J5` step 3 names, inside that
//!    namespace, against the real binary;
//! 4. classifies every failure. A step that fails because a network call
//!    was attempted and refused is this test's one and only reason to
//!    fail it. A step that fails because `dark-cli` has not wired up the
//!    command yet — most of them, today — is not a network problem, and
//!    is reported as [`Outcome::Pending`], by name, not hidden.
//!
//! # What this module cannot prove yet
//!
//! Task unit `J5`'s own "Needs" line names `A2`, `A3`, `D4`, `E7`, `F4`,
//! `G5`, and `J3`. Most of that work has landed at the crate level (this
//! is a fast-moving workspace; check `git log` rather than trust a
//! stale summary) — but `crates/dark-cli/src/main.rs`, the composition
//! root that would let a scripted session actually call any of it,
//! still bails out of `run`, `explore`, `seams`, `map`, `pack`, and
//! most other subcommands with "is not implemented yet" (checked
//! directly, not assumed — see the `not_yet` function there). No task
//! unit in the PRD names "wire `dark-cli`'s dispatch to the crates
//! task units `A2`, `D4`, `E7`, `F3`, and `G5` already built" as its own
//! step; until one does, or an existing one picks it up, none of the
//! five scripted actions in `J5` step 3 can complete for real, because
//! the command surface they need does not exist at the CLI layer yet.
//! [`SESSION`]'s five steps are written against the command surface
//! Section 3.5 documents, so no change is needed here once that surface
//! lands; today, every one of them reports [`Outcome::Pending`], and
//! this test still passes, because "Assert that no step reports a
//! network error" (task unit `J5` step 4) is satisfied by a pending
//! step, which is a different, honest, and expected kind of failure.
//!
//! `dark setup` cannot download a real model yet either (task units `B2`
//! to `B7`, not landed as of this module's writing);
//! `testdata/airgap/fixtures/` stands in for its output. See
//! `testdata/airgap/README.md`.

use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// The hidden task name `main.rs` dispatches to run just the network
/// probe. Not part of the advertised `cargo xtask` task list: `run`
/// invokes it by re-executing this same `xtask` binary inside a network
/// namespace, so the probe's connection attempt happens on the far side
/// of `unshare --net`, not in the parent process.
pub(crate) const PROBE_TASK: &str = "airgap-probe-network";

/// A real, routable address that this test never expects to reach.
///
/// One of Cloudflare's public DNS resolvers. Any address the harness
/// does not run reaches the same conclusion; this one needs no DNS
/// lookup, so a denied namespace fails the connection immediately
/// instead of failing a name lookup first — the interesting failure
/// mode is "no route", not "no resolver".
const PROBE_TARGET: &str = "1.1.1.1:443";

/// How long [`probe_network_subcommand`] waits for a connection attempt.
///
/// A namespace with `CLONE_NEWNET` and nothing else has no default
/// route, so a connection attempt fails immediately with "network is
/// unreachable" — this bound exists only as a backstop against a sandbox
/// that behaves unexpectedly, not because the expected path is slow.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one scripted-session command may run before this test kills
/// it and reports a failure.
const STEP_TIMEOUT_SECS: u64 = 60;

/// Runs [`probe_network_subcommand`]'s connection attempt and reports
/// whether the network was reachable.
///
/// Exits the process directly (`main.rs` dispatches straight to this
/// function for [`PROBE_TASK`]) so its exit code alone tells the parent,
/// running this same binary through `unshare --net`, whether the
/// namespace denied the connection.
pub(crate) fn probe_network_subcommand() -> Result<()> {
    let target: SocketAddr = PROBE_TARGET.parse().context("parsing the probe address")?;
    match TcpStream::connect_timeout(&target, PROBE_TIMEOUT) {
        Ok(_stream) => {
            println!("REACHABLE");
            bail!("connected to {PROBE_TARGET}; the network namespace did not deny it");
        }
        Err(err) => {
            println!("DENIED: {err}");
            Ok(())
        }
    }
}

/// One action from task unit `J5` step 3's scripted session.
struct Step {
    /// The action's name, matching the PRD's bullet list.
    name: &'static str,
    /// The `dark` invocations that exercise it. More than one command
    /// covers a bullet that names two actions together (`/explore` and
    /// the seam report).
    commands: &'static [&'static [&'static str]],
}

/// The five actions task unit `J5` step 3 names, in order, each mapped
/// to the closest command Section 3.5 documents for it today.
///
/// `chart a map` and `work one research ticket` are `/plan` and
/// `/plan work`, in-session commands with no standalone CLI subcommand,
/// so both go through `dark run "<prompt>" --dark` (Section 3.5). `run
/// /explore and read the seam report` is one bullet naming two actions;
/// both have real CLI subcommands (`dark explore`, `dark seams`), so it
/// runs both. `docs_get` is a tool the model calls during a turn, not a
/// command a person types, so the closest scripted equivalent is the
/// `/docs` slash command that asks for it. `edit a file and run a test`
/// has no slash command of its own; it is an ordinary agentic turn, so
/// it is one `dark run` prompt that asks for both, against
/// `testdata/airgap/fixtures/repo` (see that fixture's own README, and
/// this module's header, for why against a fixture crate and not this
/// workspace).
const SESSION: [Step; 5] = [
    Step {
        name: "chart a map with /plan",
        commands: &[&["run", "/plan \"add a health check endpoint\"", "--dark"]],
    },
    Step {
        name: "work one research ticket",
        commands: &[&["run", "/plan work", "--dark"]],
    },
    Step {
        name: "run /explore and read the seam report",
        commands: &[&["explore"], &["seams"]],
    },
    Step {
        name: "retrieve documentation with docs_get",
        commands: &[&["run", "/docs anyhow error-handling", "--dark"]],
    },
    Step {
        name: "edit a file and run a test",
        commands: &[&[
            "run",
            "edit src/lib.rs to add a doc comment on `add`, then run cargo test",
            "--dark",
        ]],
    },
];

/// What running one scripted command produced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The command exited successfully.
    Done,
    /// The command failed because the task unit named here has not
    /// landed — `dark-cli`'s own `not_yet` message, matched by name, not
    /// swallowed.
    Pending(String),
    /// Dark mode's policy layer blocked the action (`E_POLICY_DARK`):
    /// the strongest evidence this test can see that the harness
    /// refused a network-shaped action on its own, before the kernel
    /// ever had to.
    PolicyBlocked,
    /// The command failed in a way that looks like a network error.
    /// This is the one outcome that fails the whole test: task unit
    /// `J5` step 4 asks this test to assert that no step reports one.
    NetworkFailure(String),
    /// The command failed for a reason that is neither of the above —
    /// a real bug, not a documented, expected gap.
    OtherFailure(String),
}

/// Substrings that mark a failure as network-shaped: a kernel-level
/// connection failure, a DNS failure, or the OS error numbers Linux
/// reports for them (`ENETUNREACH` 101, `ETIMEDOUT` 110, `ECONNREFUSED`
/// 111, `EHOSTUNREACH` 113).
///
/// `dark-contract`'s error taxonomy (`crates/dark-contract/src/error.rs`)
/// has no dedicated network `ErrCode` today — `E_POLICY_DARK` is the
/// closest code, and it means the opposite of a network failure (the
/// policy layer caught the action first). Until a network-specific code
/// exists, this is a text match over the OS's and `reqwest`'s own error
/// messages, not a stable code; update it if the airlock ever gains one.
const NETWORK_ERROR_MARKERS: &[&str] = &[
    "network is unreachable",
    "connection refused",
    "connection timed out",
    "no route to host",
    "temporary failure in name resolution",
    "could not resolve host",
    "name or service not known",
    "dns error",
    "os error 101",
    "os error 110",
    "os error 111",
    "os error 113",
];

/// Reports whether `text` looks like a network-level failure.
fn is_network_error_text(text: &str) -> bool {
    let lower = text.to_lowercase();
    NETWORK_ERROR_MARKERS
        .iter()
        .any(|marker| lower.contains(marker))
}

/// Extracts the task unit from `dark-cli`'s `not_yet` message: "`{what}`
/// is not implemented yet. It arrives with task unit `{task_unit}`."
/// (`crates/dark-cli/src/main.rs`). Returns `None` when `text` does not
/// match that shape.
fn extract_task_unit(text: &str) -> Option<String> {
    const MARKER: &str = "It arrives with task unit ";
    let start = text.find(MARKER)? + MARKER.len();
    let rest = &text[start..];
    let end = rest.find('.').unwrap_or(rest.len());
    let unit = rest[..end].trim();
    if unit.is_empty() {
        None
    } else {
        Some(unit.to_owned())
    }
}

/// Classifies one command's result. A pure function of the exit status
/// and the combined stdout/stderr, so every case is unit-testable
/// without spawning a process.
fn classify(success: bool, combined_output: &str) -> Outcome {
    if success {
        return Outcome::Done;
    }
    if combined_output.contains("E_POLICY_DARK") {
        return Outcome::PolicyBlocked;
    }
    if let Some(unit) = extract_task_unit(combined_output) {
        return Outcome::Pending(unit);
    }
    if is_network_error_text(combined_output) {
        return Outcome::NetworkFailure(combined_output.trim().to_owned());
    }
    Outcome::OtherFailure(combined_output.trim().to_owned())
}

/// Runs `cargo xtask airgap`.
///
/// # Errors
///
/// Returns an error when this is not Linux, when `unshare --net` is not
/// available or does not deny a real connection, when the `dark` binary
/// fails to build, or when a scripted step reports a network error or an
/// unrecognised failure.
pub(crate) fn run() -> Result<()> {
    ensure_linux()?;
    let xtask_exe = std::env::current_exe().context("locating the running xtask binary")?;
    ensure_isolation_works(&xtask_exe)?;
    println!("airgap: unshare --net denies a real connection attempt. Proceeding.");

    let dark_bin = build_dark_cli()?;
    let stage = stage_fixtures()?;

    let mut done = 0usize;
    let mut pending: Vec<(String, String)> = Vec::new();
    let mut policy_blocked = 0usize;
    let mut network_failures: Vec<String> = Vec::new();
    let mut other_failures: Vec<String> = Vec::new();

    for step in &SESSION {
        for command in step.commands {
            let (success, output) =
                run_in_namespace(&dark_bin, &stage.dark_home(), &stage.repo(), command)?;
            let outcome = classify(success, &output);
            let label = match &outcome {
                Outcome::Done => "DONE".to_owned(),
                Outcome::Pending(unit) => format!("PENDING ({unit})"),
                Outcome::PolicyBlocked => "POLICY BLOCKED".to_owned(),
                Outcome::NetworkFailure(_) => "NETWORK FAILURE".to_owned(),
                Outcome::OtherFailure(_) => "OTHER FAILURE".to_owned(),
            };
            println!(
                "airgap: [{label}] {} -- dark {}",
                step.name,
                command.join(" ")
            );

            match outcome {
                Outcome::Done => done += 1,
                Outcome::Pending(unit) => pending.push((step.name.to_owned(), unit)),
                Outcome::PolicyBlocked => policy_blocked += 1,
                Outcome::NetworkFailure(message) => network_failures.push(format!(
                    "{}: dark {}: {message}",
                    step.name,
                    command.join(" ")
                )),
                Outcome::OtherFailure(message) => other_failures.push(format!(
                    "{}: dark {}: {message}",
                    step.name,
                    command.join(" ")
                )),
            }
        }
    }

    let total =
        done + pending.len() + policy_blocked + network_failures.len() + other_failures.len();
    println!(
        "airgap: {total} command(s) run; {done} done, {} pending, {policy_blocked} policy-blocked, \
         {} network failure(s), {} other failure(s)",
        pending.len(),
        network_failures.len(),
        other_failures.len(),
    );
    for (step_name, unit) in &pending {
        println!("airgap:   pending on task unit {unit}: {step_name}");
    }

    if !network_failures.is_empty() {
        bail!(
            "airgap: {} step(s) reported a network error with the network namespace denied — \
             task unit J5 step 4 fails on exactly this:\n{}",
            network_failures.len(),
            network_failures.join("\n"),
        );
    }
    if !other_failures.is_empty() {
        bail!(
            "airgap: {} step(s) failed for a reason that is neither a documented pending task \
             unit nor a network error — this is a real failure, not a gap:\n{}",
            other_failures.len(),
            other_failures.join("\n"),
        );
    }

    println!(
        "airgap: PASS. No step reported a network error. {done}/{total} scripted actions ran to \
         completion; the rest are pending named task units, not network failures. This is not \
         the same as task unit J5's Done criterion (\"the scripted session completes\") — see \
         this module's header and the J5 report for what that still needs."
    );
    Ok(())
}

/// Bails when the current target is not Linux.
///
/// Network namespaces are a Linux kernel feature; `crates/dark-tools`'s
/// dark-mode wrap already scopes itself the same way, and treats it as
/// optional there. This test cannot: without a real namespace it cannot
/// tell a genuine absence of network calls from a network call that
/// happened to succeed against a real network, so it refuses to run
/// instead of reporting a pass that proves nothing.
#[cfg(not(target_os = "linux"))]
fn ensure_linux() -> Result<()> {
    bail!(
        "cargo xtask airgap needs a Linux network namespace (unshare --net) to deny the network \
         for real; this is not Linux. Run it on Linux, for example in CI."
    );
}

/// No-op on Linux; see the `cfg`-gated sibling above for why every other
/// target refuses outright instead.
// This half of the cfg split never fails, but the two must share a
// signature: the other target's `main.rs` call site does not `cfg`-split
// on the result.
#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_wraps)]
fn ensure_linux() -> Result<()> {
    Ok(())
}

/// Proves that `unshare --net` produces a namespace with no route to a
/// real external address, by running [`PROBE_TASK`] inside one and
/// checking that it reports the connection denied.
///
/// This is the one check in this module that must never be best effort:
/// task unit `J5`'s brief is explicit that a test which passes because
/// nothing happened to reach for the network proves nothing. A sandbox
/// that cannot grant `CLONE_NEWNET` (no `CAP_SYS_ADMIN`, no unprivileged
/// user namespaces) fails this outright rather than falling back to an
/// unproven assumption.
fn ensure_isolation_works(xtask_exe: &Path) -> Result<()> {
    let output = Command::new("unshare")
        .args(["--net", "--"])
        .arg(xtask_exe)
        .arg(PROBE_TASK)
        .output()
        .context(
            "running `unshare --net`. Install util-linux's `unshare`, or run this on a host \
             that has it.",
        )?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() || !combined.contains("DENIED") {
        bail!(
            "cargo xtask airgap could not prove that `unshare --net` denies a real network \
             connection (probe output: {combined:?}). This usually means the sandbox lacks \
             CAP_SYS_ADMIN and unprivileged user namespaces are disabled \
             (/proc/sys/kernel/unprivileged_userns_clone). Run as root, or grant \
             CAP_SYS_ADMIN, or enable unprivileged user namespaces. This test refuses to \
             continue without a namespace it has proven denies the network, rather than \
             report a pass that proves nothing."
        );
    }
    Ok(())
}

/// Runs one scripted command inside a fresh `unshare --net` namespace,
/// with `$DARK_HOME` set to `dark_home` and the working directory set to
/// `cwd`. Returns whether it exited successfully and its combined
/// stdout and stderr.
fn run_in_namespace(
    dark_bin: &Path,
    dark_home: &Path,
    cwd: &Path,
    args: &[&str],
) -> Result<(bool, String)> {
    let output = Command::new("unshare")
        .args(["--net", "--", "timeout", &STEP_TIMEOUT_SECS.to_string()])
        .arg(dark_bin)
        .args(args)
        .current_dir(cwd)
        .env("DARK_HOME", dark_home)
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .output()
        .with_context(|| format!("running dark {}", args.join(" ")))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.success(), combined))
}

/// Builds `dark-cli` in debug mode (this test needs a runnable binary,
/// not a release-quality one) and returns the path to the resulting
/// `dark` binary.
fn build_dark_cli() -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let target_dir = cargo_target_dir()?;

    let status = Command::new(&cargo)
        .args(["build", "-p", "dark-cli"])
        .status()
        .context("running cargo build -p dark-cli")?;
    if !status.success() {
        bail!("cargo build -p dark-cli failed");
    }

    let binary_name = format!("dark{}", std::env::consts::EXE_SUFFIX);
    let binary = target_dir.join("debug").join(binary_name);
    if !binary.is_file() {
        bail!("expected a binary at {}", binary.display());
    }
    Ok(binary)
}

/// Returns the workspace's `target` directory, from `cargo metadata`.
fn cargo_target_dir() -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running cargo metadata")?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata output")?;
    let target_directory = metadata
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .context("cargo metadata has no target_directory")?;
    Ok(PathBuf::from(target_directory))
}

/// A staged, disposable copy of `testdata/airgap/fixtures/`.
struct Stage {
    dir: tempfile::TempDir,
}

impl Stage {
    /// The staged `$DARK_HOME`.
    fn dark_home(&self) -> PathBuf {
        self.dir.path().join("dark-home")
    }

    /// The staged repository the scripted session works against.
    fn repo(&self) -> PathBuf {
        self.dir.path().join("repo")
    }
}

/// Copies `testdata/airgap/fixtures/` into a fresh temporary directory
/// and runs `git init` on the copied `repo/`, so `dark`'s repository-root
/// detection (`crates/dark-cli/src/main.rs`'s `repo_root`) finds it.
///
/// See `testdata/airgap/README.md` for why the fixture itself checks in
/// no `.git` directory of its own.
fn stage_fixtures() -> Result<Stage> {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR is not set")?;
    let fixtures = Path::new(&manifest_dir)
        .parent()
        .context("xtask's manifest directory has no parent")?
        .join("testdata")
        .join("airgap")
        .join("fixtures");
    if !fixtures.is_dir() {
        bail!("expected fixtures at {}", fixtures.display());
    }

    let dir = tempfile::TempDir::new().context("creating a scratch directory")?;
    copy_dir_recursive(&fixtures.join("dark-home"), &dir.path().join("dark-home"))?;
    copy_dir_recursive(&fixtures.join("repo"), &dir.path().join("repo"))?;

    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir.path().join("repo"))
        .status()
        .context("running git init on the staged fixture repository")?;
    if !status.success() {
        bail!("git init failed in the staged fixture repository");
    }

    Ok(Stage { dir })
}

/// Recursively copies the contents of `from` into `to`, creating `to`.
fn copy_dir_recursive(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &dest).with_context(|| {
                format!("copying {} to {}", entry.path().display(), dest.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reports_done_on_success() {
        assert_eq!(classify(true, ""), Outcome::Done);
    }

    #[test]
    fn classify_extracts_the_task_unit_from_dark_clis_not_yet_message() {
        let message = "dark explore is not implemented yet. It arrives with task unit F1.\n";
        assert_eq!(classify(false, message), Outcome::Pending("F1".to_owned()));
    }

    #[test]
    fn classify_extracts_a_multi_unit_task_range() {
        let message = "dark tune is not implemented yet. It arrives with task unit B2 to B5.\n";
        assert_eq!(
            classify(false, message),
            Outcome::Pending("B2 to B5".to_owned())
        );
    }

    #[test]
    fn classify_reports_policy_blocked_on_e_policy_dark() {
        let message = "the tool call failed: E_POLICY_DARK: dark mode blocked the action";
        assert_eq!(classify(false, message), Outcome::PolicyBlocked);
    }

    #[test]
    fn classify_reports_network_failure_on_a_kernel_network_error() {
        let message = "error sending request: dns error: failed to lookup address: Temporary \
                        failure in name resolution";
        assert_eq!(
            classify(false, message),
            Outcome::NetworkFailure(message.to_owned())
        );
    }

    #[test]
    fn classify_reports_network_failure_on_a_raw_os_error_number() {
        let message = "Os { code: 101, kind: NetworkUnreachable, message: \"Network is \
                        unreachable\" }";
        assert!(matches!(
            classify(false, message),
            Outcome::NetworkFailure(_)
        ));
    }

    #[test]
    fn classify_reports_other_failure_when_nothing_matches() {
        let message = "thread 'main' panicked at src/main.rs:12: index out of bounds";
        assert_eq!(
            classify(false, message),
            Outcome::OtherFailure(message.to_owned())
        );
    }

    #[test]
    fn extract_task_unit_returns_none_without_the_marker() {
        assert_eq!(extract_task_unit("a plain failure with no marker"), None);
    }

    #[test]
    fn is_network_error_text_is_case_insensitive() {
        assert!(is_network_error_text("CONNECTION REFUSED"));
        assert!(is_network_error_text(
            "Could not resolve host: example.invalid"
        ));
    }

    #[test]
    fn is_network_error_text_does_not_flag_an_unrelated_message() {
        assert!(!is_network_error_text("index out of bounds: the len is 3"));
    }

    #[test]
    fn session_has_five_steps_matching_task_unit_j5_step_3() {
        assert_eq!(SESSION.len(), 5);
        assert_eq!(SESSION[0].name, "chart a map with /plan");
        assert_eq!(SESSION[1].name, "work one research ticket");
        assert_eq!(SESSION[2].name, "run /explore and read the seam report");
        assert_eq!(SESSION[3].name, "retrieve documentation with docs_get");
        assert_eq!(SESSION[4].name, "edit a file and run a test");
    }

    #[test]
    fn every_dark_run_step_passes_the_dark_flag() {
        for step in &SESSION {
            for command in step.commands {
                if command.first() == Some(&"run") {
                    assert!(
                        command.contains(&"--dark"),
                        "a `dark run` step in the airgap session must pass --dark: {command:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn copy_dir_recursive_copies_files_and_nested_directories() {
        let src = tempfile::TempDir::new().unwrap();
        let dst = tempfile::TempDir::new().unwrap();
        fs::write(src.path().join("a.txt"), b"a").unwrap();
        fs::create_dir(src.path().join("nested")).unwrap();
        fs::write(src.path().join("nested/b.txt"), b"b").unwrap();

        copy_dir_recursive(src.path(), &dst.path().join("out")).unwrap();

        assert_eq!(fs::read(dst.path().join("out/a.txt")).unwrap(), b"a");
        assert_eq!(fs::read(dst.path().join("out/nested/b.txt")).unwrap(), b"b");
    }

    #[test]
    fn stage_fixtures_produces_a_dark_home_and_a_git_repo() {
        // A real check against this repository's own testdata/airgap
        // fixtures and a real `git init`; no process isolation needed
        // for this part, so it runs under plain `cargo nextest run -p
        // xtask` on every platform.
        let stage = stage_fixtures().unwrap();
        assert!(stage.dark_home().join("models").is_dir());
        assert!(stage.dark_home().join("packs").is_dir());
        assert!(stage.repo().join("src/lib.rs").is_file());
        assert!(stage.repo().join(".git").is_dir());
    }
}
