//! Task unit `F4`'s own "done when": two runs produce identical bytes, and
//! a shuffled input order produces identical bytes too (Rule 32).
//!
//! The fixture copies the pattern `tests/seam_ranking_fixture.rs` sets: a
//! small real git repository, built fresh in a temporary directory, run
//! through the whole pipeline from discovery to the written report.
//!
//! Every path this fixture writes below the repository root uses only
//! lowercase ASCII letters, underscores, hyphens, and `.` — see
//! `crates/dark-explore/src/output/path.rs`'s module documentation for
//! why: a path component byte between `/` (0x2F) and `\` (0x5C) — a digit
//! or an uppercase letter — can sort differently under a native,
//! per-platform byte comparator than under this stage's own `/`-joined
//! one, and `graph::build`'s node numbering (F1, F2) still sorts by the
//! native comparator. `Cargo.toml` is the one exception, at the repository
//! root: extraction needs the literal, capitalised name to find the crate
//! root (`extract::paths::RepoPaths::nearest_ancestor_with`), and it is
//! still safe here — its first byte (`C`, 0x43) differs from every other
//! root entry's first byte (`s` for `src`, `t` for `tests`) before any
//! separator is ever compared, so the platform-dependent byte never
//! actually decides an ordering this fixture depends on.
use std::path::Path;
use std::process::Command;

use dark_explore::discover::{DiscoverOptions, discover};
use dark_explore::extract::extract_repository;
use dark_explore::graph::build;
use dark_explore::output::{self, Sources};
use dark_explore::seam::{CoChange, Weights, Window, analyse};
use dark_explore::syntax::{Cache, parse_snapshot};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A small fixture: two clusters of files joined by one seam, with enough
/// history for co-change to say something. Deliberately smaller than
/// `seam_ranking_fixture`'s: this test is about byte identity, not about
/// which edge outranks which.
fn build_fixture(root: &Path) {
    write(root, "Cargo.toml", "[package]\nname = \"fixture\"\n");
    write(
        root,
        "src/engine.rs",
        "use crate::model::step;\nuse crate::iface::storage;\n\
         pub fn run() { step(); }\n",
    );
    write(
        root,
        "src/model.rs",
        "use crate::util::helper;\npub fn step() { helper(); }\n",
    );
    write(root, "src/util.rs", "pub fn helper() {}\n");
    write(root, "src/iface.rs", "pub trait storage {}\n");
    write(
        root,
        "src/store.rs",
        "use crate::iface::storage;\npub fn save() {}\n",
    );
    write(
        root,
        "tests/engine_test.rs",
        "use crate::engine::run;\npub fn exercises_run() { let _ = run; }\n",
    );

    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "fixture"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial", "--allow-empty"]);

    write(
        root,
        "src/engine.rs",
        "use crate::model::step;\nuse crate::iface::storage;\n\
         pub fn run() { step(); }\n// tuned\n",
    );
    git(root, &["add", "src/engine.rs"]);
    git(root, &["commit", "-qm", "tune engine"]);
}

/// Runs the whole pipeline once, from discovery through the written
/// [`output::Document`], with `files` fed to `graph::build` in whatever
/// order the caller passes.
fn run_pipeline(root: &Path, file_order: FileOrder) -> output::Document {
    let snapshot = discover(root, &DiscoverOptions::default()).expect("discover");
    let (parsed, _cache) = parse_snapshot(&Cache::new(), root, &snapshot).expect("parse");
    let mut files = extract_repository(&snapshot, &parsed);
    if file_order == FileOrder::Reversed {
        files.reverse();
    }

    let graphs = build(&files);
    let weights = Weights::default();
    let cochange = CoChange::read(root, Window::default()).expect("history reads");
    let analysis = analyse(&graphs, &cochange, &weights).expect("analyse");
    let discover_options = DiscoverOptions::default();
    let tree_sha = output::tree_sha(&snapshot.files);

    output::build(&Sources {
        files: &files,
        graphs: &graphs,
        analysis: &analysis,
        cochange: &cochange,
        discover_options: &discover_options,
        weights: &weights,
        tree_sha,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileOrder {
    AsDiscovered,
    Reversed,
}

#[test]
fn two_runs_produce_identical_bytes_and_identical_hashes() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let first = run_pipeline(root, FileOrder::AsDiscovered);
    let second = run_pipeline(root, FileOrder::AsDiscovered);

    assert_eq!(first, second, "Rules 29 to 32: same input, same document");

    let first_bytes = output::document_bytes(&first).expect("serialise");
    let second_bytes = output::document_bytes(&second).expect("serialise");
    assert_eq!(
        first_bytes, second_bytes,
        "identical bytes, not just equal structs"
    );

    let first_lock = output::build_lock(&first, &first_bytes);
    let second_lock = output::build_lock(&second, &second_bytes);
    assert_eq!(first_lock.config_hash, second_lock.config_hash);
    assert_eq!(first_lock.output_blake3, second_lock.output_blake3);
}

#[test]
fn writing_the_same_document_twice_to_disk_produces_identical_files() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let document = run_pipeline(root, FileOrder::AsDiscovered);

    let out_a = TempDir::new().expect("tempdir");
    let out_b = TempDir::new().expect("tempdir");
    let (paths_a, lock_a) = output::write(out_a.path(), &document).expect("write a");
    let (paths_b, lock_b) = output::write(out_b.path(), &document).expect("write b");

    assert_eq!(
        std::fs::read(&paths_a.json).unwrap(),
        std::fs::read(&paths_b.json).unwrap()
    );
    assert_eq!(
        std::fs::read(&paths_a.lock).unwrap(),
        std::fs::read(&paths_b.lock).unwrap()
    );
    assert_eq!(lock_a, lock_b);
}

/// Rule 32: shuffling the input file order before `graph::build` must not
/// change the analysis, and so must not change the report.
#[test]
fn a_shuffled_input_order_produces_an_identical_document() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let baseline = run_pipeline(root, FileOrder::AsDiscovered);
    let shuffled = run_pipeline(root, FileOrder::Reversed);

    assert_eq!(
        baseline, shuffled,
        "reversing the file list before graph::build must not change the document"
    );

    let baseline_bytes = output::document_bytes(&baseline).expect("serialise");
    let shuffled_bytes = output::document_bytes(&shuffled).expect("serialise");
    assert_eq!(baseline_bytes, shuffled_bytes);
}

/// Every path this stage writes uses `/`, never the host's own separator —
/// this fixture's own assertion that its output is worth trusting on
/// Windows, not only on the platform this test happens to run on.
#[test]
fn every_path_in_the_document_uses_forward_slashes() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let document = run_pipeline(root, FileOrder::AsDiscovered);

    for module in &document.modules {
        assert!(!module.path.contains('\\'), "{}", module.path);
    }
    for seam in &document.seams {
        assert!(!seam.from.contains('\\'), "{}", seam.from);
        assert!(!seam.to.contains('\\'), "{}", seam.to);
    }
    for hotspot in &document.hotspots {
        assert!(!hotspot.path.contains('\\'), "{}", hotspot.path);
    }
}
