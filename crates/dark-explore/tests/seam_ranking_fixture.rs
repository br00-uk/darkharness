//! `F3` done-when: "The fixture repository ranks its two known seams above
//! its known poor boundary."
//!
//! The fixture is a small Rust crate built to make each ranking claim
//! forced, not lucky:
//!
//! - Three dense clusters of files, so Louvain separates them: a core
//!   triangle (`engine`, `model`, `util`), an output triangle (`report`,
//!   `render`, `layout`), and a storage triangle (`store`, `backend`,
//!   `iface`).
//! - **Known seam 1**: `engine -> iface`, the only edge joining core to
//!   storage. It is a bridge, it crosses a community boundary, its target
//!   holds one bare trait so its abstractness is 1, and the two files never
//!   share a commit.
//! - **Known seam 2**: `report -> engine`, the only edge joining output to
//!   core. A bridge crossing a boundary, with a test referencing its
//!   target.
//! - **Known poor boundary**: `store -> backend`. It sits inside one
//!   community, its target is concrete, and every commit that touches one
//!   file touches the other, so its inverse co-change is zero. Its history
//!   is what condemns it: structure alone would call it ordinary.

use std::path::Path;
use std::process::Command;

use dark_explore::discover::{DiscoverOptions, discover};
use dark_explore::extract::extract_repository;
use dark_explore::graph::build;
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

/// Commits the named files, creating one co-change observation.
fn commit(root: &Path, files: &[&str], message: &str) {
    for file in files {
        git(root, &["add", file]);
    }
    git(root, &["commit", "-qm", message, "--allow-empty"]);
}

/// Writes the source files: three dense triangles and the two seams.
fn write_sources(root: &Path) {
    write(root, "Cargo.toml", "[package]\nname = \"fixture\"\n");

    // The core triangle.
    write(
        root,
        "src/engine.rs",
        "use crate::model::step;\nuse crate::iface::Storage;\n\
         pub fn run(s: &dyn Storage) { step(); let _ = s; }\n",
    );
    write(
        root,
        "src/model.rs",
        "use crate::util::helper;\npub fn step() { helper(); }\n",
    );
    write(
        root,
        "src/util.rs",
        "use crate::engine::run;\npub fn helper() {}\npub fn reenter() { let _ = run; }\n",
    );

    // The output triangle.
    write(
        root,
        "src/report.rs",
        "use crate::render::draw;\nuse crate::engine::run;\n\
         pub fn publish() { draw(); let _ = run; }\n",
    );
    write(
        root,
        "src/render.rs",
        "use crate::layout::grid;\npub fn draw() { grid(); }\n",
    );
    write(
        root,
        "src/layout.rs",
        "use crate::report::publish;\npub fn grid() {}\npub fn back() { let _ = publish; }\n",
    );

    // The storage triangle. `iface` holds one bare trait, so its
    // abstractness is exactly 1.
    write(root, "src/iface.rs", "pub trait Storage {}\n");
    write(
        root,
        "src/store.rs",
        "use crate::iface::Storage;\nuse crate::backend::write_block;\n\
         pub fn save(s: &dyn Storage) { write_block(); let _ = s; }\n",
    );
    write(
        root,
        "src/backend.rs",
        "use crate::iface::Storage;\npub fn write_block() {}\n\
         pub fn open(s: &dyn Storage) { let _ = s; }\n",
    );

    // A test file, so the `T` term has something to say about `engine`.
    write(
        root,
        "src/engine_test.rs",
        "use crate::engine::run;\npub fn exercises_run() { let _ = run; }\n",
    );
}

/// Builds the history: clusters arrive separately, then the poor boundary
/// earns its coupling.
fn write_history(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "Fixture"]);

    // Each cluster arrives in its own commit, so the initial history does
    // not read as everything co-changing with everything.
    commit(
        root,
        &[
            "Cargo.toml",
            "src/engine.rs",
            "src/model.rs",
            "src/util.rs",
            "src/engine_test.rs",
        ],
        "core",
    );
    commit(
        root,
        &["src/report.rs", "src/render.rs", "src/layout.rs"],
        "output",
    );
    commit(
        root,
        &["src/iface.rs", "src/store.rs", "src/backend.rs"],
        "storage",
    );

    // The poor boundary earns its coupling: store and backend move
    // together, every time.
    for round in 0..4 {
        write(
            root,
            "src/store.rs",
            &format!(
                "use crate::iface::Storage;\nuse crate::backend::write_block;\n\
                 pub fn save(s: &dyn Storage) {{ write_block(); let _ = s; }}\n// round {round}\n"
            ),
        );
        write(
            root,
            "src/backend.rs",
            &format!(
                "use crate::iface::Storage;\npub fn write_block() {}{}\n\
                 pub fn open(s: &dyn Storage) {{ let _ = s; }}\n// round {round}\n",
                "{", "}"
            ),
        );
        commit(
            root,
            &["src/store.rs", "src/backend.rs"],
            &format!("store and backend, round {round}"),
        );
    }

    // The seam sides change separately.
    write(
        root,
        "src/engine.rs",
        "use crate::model::step;\nuse crate::iface::Storage;\n\
         pub fn run(s: &dyn Storage) { step(); let _ = s; }\n// tuned\n",
    );
    commit(root, &["src/engine.rs"], "engine alone");
}

fn build_fixture(root: &Path) {
    write_sources(root);
    write_history(root);
}

#[test]
fn the_two_known_seams_rank_above_the_known_poor_boundary() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let snapshot = discover(root, &DiscoverOptions::default()).expect("discover");
    let (parsed, _cache) = parse_snapshot(&Cache::new(), root, &snapshot).expect("parse");
    let files = extract_repository(&snapshot, &parsed);
    let graphs = build(&files);
    let cochange = CoChange::read(root, Window::default()).expect("history reads");

    let analysis = analyse(&graphs, &cochange, &Weights::default()).expect("analyse");

    let score_of = |from: &str, to: &str| -> f64 {
        let from_node = graphs.file_index[Path::new(from)];
        let to_node = graphs.file_index[Path::new(to)];
        analysis
            .seams
            .iter()
            .find(|s| s.from == from_node && s.to == to_node)
            .unwrap_or_else(|| panic!("no scored edge {from} -> {to}"))
            .score
    };

    let seam_one = score_of("src/engine.rs", "src/iface.rs");
    let seam_two = score_of("src/report.rs", "src/engine.rs");
    let poor = score_of("src/store.rs", "src/backend.rs");

    assert!(
        seam_one > poor,
        "engine -> iface ({seam_one:.3}) must outrank store -> backend ({poor:.3})"
    );
    assert!(
        seam_two > poor,
        "report -> engine ({seam_two:.3}) must outrank store -> backend ({poor:.3})"
    );

    // The two seams are the only edges joining their clusters, so both are
    // bridges and carry the hard flag; the poor boundary sits inside a
    // triangle and does not.
    let hard_of = |from: &str, to: &str| -> bool {
        let from_node = graphs.file_index[Path::new(from)];
        let to_node = graphs.file_index[Path::new(to)];
        analysis
            .seams
            .iter()
            .find(|s| s.from == from_node && s.to == to_node)
            .expect("the edge is scored")
            .hard
    };
    assert!(hard_of("src/engine.rs", "src/iface.rs"));
    assert!(hard_of("src/report.rs", "src/engine.rs"));
    assert!(!hard_of("src/store.rs", "src/backend.rs"));
}

#[test]
fn the_analysis_is_identical_across_repeated_runs() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let run = || {
        let snapshot = discover(root, &DiscoverOptions::default()).expect("discover");
        let (parsed, _cache) = parse_snapshot(&Cache::new(), root, &snapshot).expect("parse");
        let files = extract_repository(&snapshot, &parsed);
        let graphs = build(&files);
        let cochange = CoChange::read(root, Window::default()).expect("history reads");
        analyse(&graphs, &cochange, &Weights::default()).expect("analyse")
    };

    let first = run();
    let second = run();
    assert_eq!(first, second, "Rules 29 to 32: same input, same bytes");
}
