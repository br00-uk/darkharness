//! `cargo xtask explore-fixture`: task unit `F4`'s cross-platform hash
//! check.
//!
//! Builds a small, fixed repository fixture in a temporary directory, runs
//! `dark-explore`'s whole pipeline over it twice, and asserts the two runs
//! produce identical bytes. With `--assert-hash`, it also compares the
//! resulting hash against the one committed at
//! [`EXPECTED_HASH_PATH`], failing loudly on a mismatch instead of only
//! printing one.
//!
//! # Why no git repository
//!
//! `crate::seam::CoChange::read` shells out to `git log`, which needs a
//! real repository. This task builds the fixture with plain file writes
//! instead and analyses it with [`CoChange::default`] — an empty co-change
//! reading, coupling `0.0` for every pair, `window` left at its
//! [`Window::default`] value even though `commits_read` is `0`. F3's Do
//! step 6 asks for the co-change window itself to feed the configuration
//! hash, and it still does here; only the *history* it would have read is
//! absent, deliberately, so this check runs the same way with or without
//! `git` on the machine building it. `tests/determinism.rs`, this task
//! unit's other fixture, does exercise a real repository and real history;
//! this one exists for the narrower, cross-platform question.
//!
//! # Why the committed-hash route
//!
//! F4's brief names two ways to compare a hash across three operating
//! systems in continuous integration: upload each OS's hash as an artefact
//! and compare them in a fourth job, or commit the expected hash and have
//! every OS assert against it directly. This fixture qualifies for the
//! simpler, committed-hash route because it is genuinely platform-independent
//! by construction: every path segment it writes is plain lowercase ASCII
//! (see [`write_fixture`] for the one exception and why it is still safe),
//! it holds no absolute path anywhere, [`std::fs::write`] writes the exact
//! `\n`-terminated byte strings given to it with no platform line-ending
//! translation, and [`output::build`] itself sorts and hashes by a
//! `/`-joined path form rather than by the host's native separator (see
//! `dark_explore::output`'s own module documentation). Given that, three
//! separate CI jobs each running this task and asserting against one
//! shared, checked-in hash proves the same thing an upload-and-compare job
//! would, with far less workflow machinery.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dark_explore::discover::{self, DiscoverOptions};
use dark_explore::extract::extract_repository;
use dark_explore::graph;
use dark_explore::output::{self, Sources};
use dark_explore::seam::{self, CoChange, Weights};
use dark_explore::syntax::{self, Cache};

/// Where the expected hash lives, relative to the workspace root.
const EXPECTED_HASH_PATH: &str = "testdata/explore-fixture.hash";

/// Writes one fixture file, creating its parent directory first.
fn write(root: &Path, rel: &str, content: &str) -> Result<()> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Writes the fixture repository under `root`.
///
/// Every path below the root uses only lowercase ASCII letters, `.`, and
/// `/` — see `dark_explore::output::path`'s module documentation for why a
/// digit or an uppercase letter in a path segment can sort differently
/// under a native, per-platform byte comparator than under the `/`-joined
/// one this stage hashes with, and why that matters for a hash this task
/// commits and compares across operating systems. `Cargo.toml`, at the
/// root, is the one exception: extraction needs that literal, capitalised
/// name to find the crate root. It stays safe here because its first byte
/// (`C`, 0x43) differs from `src`'s (`s`, 0x73) before either path ever
/// reaches a separator, so the platform-dependent byte never actually
/// decides an ordering this fixture's hash depends on.
fn write_fixture(root: &Path) -> Result<()> {
    write(root, "Cargo.toml", "[package]\nname = \"fixture\"\n")?;
    write(
        root,
        "src/engine.rs",
        "use crate::model::step;\nuse crate::iface::storage;\npub fn run() { step(); }\n",
    )?;
    write(
        root,
        "src/model.rs",
        "use crate::util::helper;\npub fn step() { helper(); }\n",
    )?;
    write(root, "src/util.rs", "pub fn helper() {}\n")?;
    write(root, "src/iface.rs", "pub trait storage {}\n")?;
    Ok(())
}

/// Runs the whole `/explore` pipeline once over the fixture at `root` and
/// returns the written report.
fn analyse_fixture(root: &Path) -> Result<output::Document> {
    let snapshot = discover::discover(root, &DiscoverOptions::default())?;
    let (parsed, _cache) = syntax::parse_snapshot(&Cache::new(), root, &snapshot)?;
    let files = extract_repository(&snapshot, &parsed);
    let graphs = graph::build(&files);
    let weights = Weights::default();
    let cochange = CoChange::default();
    let analysis = seam::analyse(&graphs, &cochange, &weights)?;
    let discover_options = DiscoverOptions::default();
    let tree_sha = output::tree_sha(&snapshot.files);

    Ok(output::build(&Sources {
        files: &files,
        graphs: &graphs,
        analysis: &analysis,
        cochange: &cochange,
        discover_options: &discover_options,
        weights: &weights,
        tree_sha,
    }))
}

/// Returns the workspace root, from `cargo metadata`.
fn workspace_root() -> Result<PathBuf> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
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
    let root = metadata
        .get("workspace_root")
        .and_then(serde_json::Value::as_str)
        .context("cargo metadata has no workspace_root")?;
    Ok(PathBuf::from(root))
}

/// Runs `cargo xtask explore-fixture`.
///
/// # Errors
///
/// Returns an error when the fixture cannot be written, when the pipeline
/// fails, when two runs over the same fixture disagree, or — with
/// `assert_hash` set — when the printed hash does not match
/// [`EXPECTED_HASH_PATH`].
pub(crate) fn run(assert_hash: bool) -> Result<()> {
    let dir = tempfile::TempDir::new().context("creating a scratch directory")?;
    write_fixture(dir.path())?;

    let first = analyse_fixture(dir.path())?;
    let second = analyse_fixture(dir.path())?;
    if first != second {
        bail!(
            "explore-fixture: two runs over the same fixture produced different documents — \
             Rules 29 to 32 are broken somewhere in the pipeline"
        );
    }

    let first_bytes = output::document_bytes(&first)?;
    let second_bytes = output::document_bytes(&second)?;
    if first_bytes != second_bytes {
        bail!(
            "explore-fixture: two runs produced equal documents but different bytes — a field \
             is not deterministically serialised"
        );
    }

    let hash = blake3::hash(&first_bytes);
    println!("explore-fixture: {hash}");

    if assert_hash {
        let expected_path = workspace_root()?.join(EXPECTED_HASH_PATH);
        let expected = std::fs::read_to_string(&expected_path)
            .with_context(|| format!("reading {}", expected_path.display()))?;
        let expected = expected.trim();
        let got = hash.to_string();
        if expected != got {
            bail!(
                "explore-fixture: hash mismatch. Got {got}, expected {expected} from {}. If \
                 this is a deliberate change to the analysis or the fixture, update that file \
                 to the printed hash.",
                expected_path.display()
            );
        }
        println!(
            "explore-fixture: matches the committed hash at {}",
            expected_path.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_analyses_without_error_and_finds_the_import_edge() {
        let dir = tempfile::TempDir::new().unwrap();
        write_fixture(dir.path()).unwrap();

        let document = analyse_fixture(dir.path()).unwrap();
        assert_eq!(document.stats.files, 4, "engine, model, util, iface");
        assert!(
            document
                .seams
                .iter()
                .any(|seam| seam.from == "src/engine.rs" && seam.to == "src/model.rs"),
            "seams: {:?}",
            document.seams
        );
    }

    #[test]
    fn running_the_task_twice_prints_the_same_hash() {
        // `run` itself already asserts the two in-process runs agree; this
        // pins that two separate invocations (as CI gives each OS) do too.
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        write_fixture(dir_a.path()).unwrap();
        write_fixture(dir_b.path()).unwrap();

        let a = analyse_fixture(dir_a.path()).unwrap();
        let b = analyse_fixture(dir_b.path()).unwrap();
        assert_eq!(
            output::document_bytes(&a).unwrap(),
            output::document_bytes(&b).unwrap()
        );
    }

    #[test]
    fn run_succeeds_without_asserting_a_hash() {
        run(false).expect("the fixture pipeline runs cleanly on its own");
    }
}
