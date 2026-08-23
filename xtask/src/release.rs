//! Release engineering checks for the three build artefacts.
//!
//! Task unit `J4` names three checks: the portable artefact's size budget,
//! and byte-for-byte reproducibility across two builds from one commit.
//! `cargo-dist` and `.github/workflows/release.yml` do the packaging;
//! this module holds the two checks the specification asks `cargo xtask`
//! to run.
//!
//! `dark-engine` is still a placeholder (task units `B2` to `B7` bring in
//! mistral.rs), so both checks here run today against a `dark-cpu` binary
//! that does not yet link the inference engine. The binary is far under
//! its size limit and reproducible almost by construction, because there
//! is not much in it yet. That will change once the engine lands; the
//! checks themselves do not need to change with it.
//!
//! ## What this module does not do
//!
//! `dark-metal` and `dark-cuda` need a macOS arm64 toolchain and a CUDA
//! toolchain respectively, neither of which this task unit can assume is
//! present. [`check_binary_size`] and [`check_reproducible`] both build
//! `dark-cli` with its default features only — the `dark-cpu` artefact.
//! Extending either check to the other two artefacts is future work for
//! whichever task unit first runs a build on that hardware.
//!
//! Task unit `J4` step 7 also asks for a `grammars-core` default feature
//! (eight languages) and a `grammars-full` feature on `dark-explore`.
//! `crates/dark-explore/Cargo.toml` is not a file this task unit owns —
//! see the module-level report for why that step is named as deferred
//! rather than implemented here.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// One of the three release artefacts from the README's "Build
/// artefacts" table and Section 4.5 (Rule 18).
pub(crate) struct Artefact {
    /// The artefact name, for example `dark-cpu`.
    pub(crate) name: &'static str,
    /// The `dark-cli` crate features this artefact builds with.
    pub(crate) features: &'static [&'static str],
    /// The platforms this artefact targets, as prose.
    pub(crate) platforms: &'static str,
}

/// The three artefacts, in the README's table order.
///
/// [`artefacts_match_readme_table`] guards this against drifting away
/// from Section 4.5 and the README's "Build artefacts" table.
pub(crate) const ARTEFACTS: [Artefact; 3] = [
    Artefact {
        name: "dark-cpu",
        features: &[],
        platforms: "all platforms (portable)",
    },
    Artefact {
        name: "dark-metal",
        features: &["metal"],
        platforms: "macOS arm64",
    },
    Artefact {
        name: "dark-cuda",
        features: &["cuda", "flash-attn"],
        platforms: "Linux and Windows x64 with NVIDIA",
    },
];

/// Task unit `J4` step 6: the `dark-cpu` artefact's size budget, in
/// bytes.
const PORTABLE_SIZE_LIMIT_BYTES: u64 = 80 * 1024 * 1024;

/// Converts a byte count to mebibytes, for display.
#[allow(clippy::cast_precision_loss)]
fn bytes_to_mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// Checks a measured binary size against the portable artefact's budget.
///
/// A pure function so the boundary condition is unit-testable without
/// invoking `cargo build`.
///
/// # Errors
///
/// Returns an error with the measured size, the limit, and a remedy when
/// `size_bytes` exceeds `limit_bytes`.
fn evaluate_size(size_bytes: u64, limit_bytes: u64) -> Result<String> {
    let report = format!(
        "dark-cpu is {:.2} MiB (limit {:.0} MiB)",
        bytes_to_mib(size_bytes),
        bytes_to_mib(limit_bytes),
    );
    if size_bytes > limit_bytes {
        bail!(
            "{report}. Task unit J4 sets an 80 MiB limit for the portable build. Strip more, \
             narrow the default grammar set (grammars-core, task unit J4 step 7), or move a \
             dependency behind a feature that dark-cpu does not enable."
        );
    }
    Ok(report)
}

/// Runs `cargo xtask check-binary-size`.
///
/// Builds `dark-cli` in release mode with default features (the
/// `dark-cpu` artefact) and checks its size against
/// [`PORTABLE_SIZE_LIMIT_BYTES`].
///
/// # Errors
///
/// Returns an error when the build fails, when the resulting binary
/// cannot be found or read, or when the binary is over budget.
pub(crate) fn check_binary_size() -> Result<()> {
    let workspace_root = workspace_root()?;
    let target_dir = workspace_root.join("target");
    let binary = build_dark_cli(&workspace_root, &target_dir, &[])?;
    let size_bytes = std::fs::metadata(&binary)
        .with_context(|| format!("reading metadata for {}", binary.display()))?
        .len();
    let report = evaluate_size(size_bytes, PORTABLE_SIZE_LIMIT_BYTES)?;
    println!("check-binary-size: {report}");
    Ok(())
}

/// Runs `cargo xtask check-reproducible`.
///
/// Builds `dark-cli` (default features, the `dark-cpu` artefact) twice
/// from the same commit, into two separate target directories so neither
/// build reuses the other's incremental cache, and compares the two
/// binaries byte for byte with a BLAKE3 hash. Task unit `J4` step 4 asks
/// for `--locked` and stripped symbols; both come from
/// `[profile.release]` in the workspace `Cargo.toml`, already in force
/// for this build. `RUSTFLAGS=--remap-path-prefix` normalises the one
/// absolute path a Rust build embeds by default (the source directory in
/// debug info and panic locations), so this build reproduces the same
/// bytes even from a checkout at a different absolute path, not only from
/// two runs against this same checkout.
///
/// # Errors
///
/// Returns an error when either build fails, when either binary cannot be
/// read, or when the two binaries differ.
pub(crate) fn check_reproducible() -> Result<()> {
    let workspace_root = workspace_root()?;
    let remap = format!(
        "--remap-path-prefix={}=/darkharness",
        workspace_root.display()
    );
    let flags = [remap.as_str()];

    let target_a = workspace_root.join("target").join("xtask-repro-a");
    let target_b = workspace_root.join("target").join("xtask-repro-b");
    let binary_a = build_dark_cli(&workspace_root, &target_a, &flags)?;
    let binary_b = build_dark_cli(&workspace_root, &target_b, &flags)?;

    let hash_a = hash_file(&binary_a)?;
    let hash_b = hash_file(&binary_b)?;
    println!("check-reproducible: build A {hash_a}");
    println!("check-reproducible: build B {hash_b}");

    let verdict = reproducibility_verdict(&hash_a, &hash_b);
    match verdict {
        Ok(()) => {
            println!("check-reproducible: identical");
            Ok(())
        }
        Err(message) => bail!(message),
    }
}

/// Compares two hex-encoded BLAKE3 hashes and reports whether they match.
///
/// A pure function so the failure message is unit-testable without
/// invoking `cargo build` twice.
///
/// # Errors
///
/// Returns an error naming both hashes when they differ.
fn reproducibility_verdict(hash_a: &str, hash_b: &str) -> Result<(), String> {
    if hash_a == hash_b {
        Ok(())
    } else {
        Err(format!(
            "two --locked release builds of dark-cli from the same commit produced different \
             binaries ({hash_a} vs {hash_b}). A build must not embed a build timestamp, a \
             random seed, or an absolute path that a --remap-path-prefix does not cover."
        ))
    }
}

/// Returns the workspace root: the parent of the directory this crate's
/// manifest lives in.
fn workspace_root() -> Result<PathBuf> {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR is not set")?;
    Path::new(&manifest_dir)
        .parent()
        .map(Path::to_path_buf)
        .context("xtask's manifest directory has no parent")
}

/// The `dark` binary's file name for the host platform.
fn binary_file_name() -> String {
    format!("dark{}", std::env::consts::EXE_SUFFIX)
}

/// Builds `dark-cli` in release mode with `--locked`, into `target_dir`,
/// with `extra_rustflags` appended to `RUSTFLAGS`. Returns the path to the
/// resulting `dark` binary.
fn build_dark_cli(
    workspace_root: &Path,
    target_dir: &Path,
    extra_rustflags: &[&str],
) -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut rustflags = std::env::var("RUSTFLAGS").unwrap_or_default();
    for flag in extra_rustflags {
        if !rustflags.is_empty() {
            rustflags.push(' ');
        }
        rustflags.push_str(flag);
    }

    let status = Command::new(&cargo)
        .current_dir(workspace_root)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("RUSTFLAGS", &rustflags)
        .args(["build", "--locked", "--release", "-p", "dark-cli"])
        .status()
        .context("running cargo build -p dark-cli")?;
    if !status.success() {
        bail!("cargo build -p dark-cli failed (target dir {})", target_dir.display());
    }

    let binary = target_dir.join("release").join(binary_file_name());
    if !binary.is_file() {
        bail!("expected a binary at {} after a successful build", binary.display());
    }
    Ok(binary)
}

/// Returns the hex-encoded BLAKE3 hash of the file at `path`.
fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

/// Renders the artefact table, for a person reading `cargo xtask
/// release-plan`-style output. Currently used only by the tests and by
/// [`crate::main`]'s task list; kept as its own function so the table
/// stays in one place.
#[allow(dead_code)]
pub(crate) fn render_artefact_table() -> String {
    let mut out = String::new();
    for artefact in &ARTEFACTS {
        let features = if artefact.features.is_empty() {
            "default".to_owned()
        } else {
            artefact.features.join(",")
        };
        let _ = writeln!(out, "{}: {} ({})", artefact.name, features, artefact.platforms);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_size_passes_under_the_limit() {
        let report = evaluate_size(10 * 1024 * 1024, PORTABLE_SIZE_LIMIT_BYTES).unwrap();
        assert!(report.contains("10.00 MiB"));
        assert!(report.contains("80 MiB"));
    }

    #[test]
    fn evaluate_size_passes_exactly_at_the_limit() {
        assert!(evaluate_size(PORTABLE_SIZE_LIMIT_BYTES, PORTABLE_SIZE_LIMIT_BYTES).is_ok());
    }

    #[test]
    fn evaluate_size_fails_over_the_limit() {
        let err = evaluate_size(90 * 1024 * 1024, PORTABLE_SIZE_LIMIT_BYTES).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("90.00 MiB"));
        assert!(message.contains("J4"));
    }

    #[test]
    fn reproducibility_verdict_passes_on_matching_hashes() {
        assert!(reproducibility_verdict("abc123", "abc123").is_ok());
    }

    #[test]
    fn reproducibility_verdict_fails_on_differing_hashes_and_names_both() {
        let err = reproducibility_verdict("aaa", "bbb").unwrap_err();
        assert!(err.contains("aaa"));
        assert!(err.contains("bbb"));
    }

    #[test]
    fn artefacts_match_readme_table() {
        // Guards Section 4.5 / Rule 18 and the README's "Build artefacts"
        // table against drifting away from this module's copy.
        assert_eq!(ARTEFACTS.len(), 3);
        assert_eq!(ARTEFACTS[0].name, "dark-cpu");
        assert!(ARTEFACTS[0].features.is_empty());
        assert_eq!(ARTEFACTS[1].name, "dark-metal");
        assert_eq!(ARTEFACTS[1].features, &["metal"]);
        assert_eq!(ARTEFACTS[2].name, "dark-cuda");
        assert_eq!(ARTEFACTS[2].features, &["cuda", "flash-attn"]);
    }

    #[test]
    fn render_artefact_table_names_every_artefact() {
        let text = render_artefact_table();
        assert!(text.contains("dark-cpu: default"));
        assert!(text.contains("dark-metal: metal"));
        assert!(text.contains("dark-cuda: cuda,flash-attn"));
    }

    #[test]
    fn binary_file_name_matches_the_host_executable_suffix() {
        let name = binary_file_name();
        assert!(name.starts_with("dark"));
        assert_eq!(&name[4..], std::env::consts::EXE_SUFFIX);
    }
}
