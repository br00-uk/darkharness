//! `F3` done-when: "The blast radius matches the hand-computed set."
//!
//! The fixture is a four-file chain, `a <- b <- c <- d`: each file's one
//! function calls the previous file's. The hand computation, written out so
//! the test is checkable against it:
//!
//! - **Betweenness.** Directed shortest paths: `b->a` carries the paths
//!   from `b`, `c`, and `d` to `a` (3); `c->b` carries `c->b`, `c->a`,
//!   `d->b`, `d->a` (4); `d->c` carries `d->c`, `d->b`, `d->a` (3).
//!   Min-max across the three edges: `c->b` is 1, the others 0.
//! - **Communities.** Louvain on a four-node path settles at `{a, b}` and
//!   `{c, d}`, so only `c->b` crosses a boundary.
//! - **Co-change.** Each file arrives in its own commit, so every pair's
//!   coupling is 0. With no spread, the normalised value is 0 everywhere,
//!   and every edge gets the full inverse co-change term.
//! - **Scores.** `c->b`: 0.35 + 0.25 + 0 + 0.10 + 0 = 0.70, at or above
//!   the bounding threshold of 0.6. `b->a` and `d->c`: 0.10 each.
//! - **Blast radius of `{alpha}`.** Unbounded, everything reaches it:
//!   `{alpha, beta, gamma, delta}`. Bounded, the walk stops at the one
//!   S-graph edge whose file pair is `c->b`, which is `gamma -> beta`:
//!   `{alpha, beta}`. Containment is 1 - 2/4 = 0.5.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use dark_explore::discover::{DiscoverOptions, discover};
use dark_explore::extract::extract_repository;
use dark_explore::graph::build;
use dark_explore::seam::score::{BOUNDING_THRESHOLD, blast_radius};
use dark_explore::seam::{CoChange, Weights, Window, analyse, symbol_scores};
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

fn build_fixture(root: &Path) {
    write(root, "Cargo.toml", "[package]\nname = \"radius\"\n");
    write(root, "src/a.rs", "pub fn alpha() {}\n");
    write(
        root,
        "src/b.rs",
        "use crate::a::alpha;\npub fn beta() { alpha(); }\n",
    );
    write(
        root,
        "src/c.rs",
        "use crate::b::beta;\npub fn gamma() { beta(); }\n",
    );
    write(
        root,
        "src/d.rs",
        "use crate::c::gamma;\npub fn delta() { gamma(); }\n",
    );

    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "fixture@example.invalid"]);
    git(root, &["config", "user.name", "Fixture"]);

    // One commit per file: no pair ever co-changes.
    for file in ["Cargo.toml", "src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"] {
        git(root, &["add", file]);
        git(root, &["commit", "-qm", file, "--allow-empty"]);
    }
}

#[test]
fn the_blast_radius_matches_the_hand_computed_set() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_fixture(root);

    let snapshot = discover(root, &DiscoverOptions::default()).expect("discover");
    let (parsed, _cache) = parse_snapshot(&Cache::new(), root, &snapshot).expect("parse");
    let files = extract_repository(&snapshot, &parsed);
    let graphs = build(&files);
    let cochange = CoChange::read(root, Window::default()).expect("history reads");

    let analysis = analyse(&graphs, &cochange, &Weights::default()).expect("analyse");

    // The hand computation says exactly one file edge reaches the
    // threshold: c -> b. Pin that before trusting the radius.
    let bounding: Vec<_> = analysis
        .seams
        .iter()
        .filter(|s| s.score >= BOUNDING_THRESHOLD)
        .collect();
    assert_eq!(bounding.len(), 1, "exactly one edge bounds: {bounding:#?}");
    assert_eq!(graphs.files[bounding[0].from].path, Path::new("src/c.rs"));
    assert_eq!(graphs.files[bounding[0].to].path, Path::new("src/b.rs"));

    let symbol = |file: &str, name: &str| {
        graphs
            .symbols
            .node_indices()
            .find(|&n| graphs.symbols[n].file == Path::new(file) && graphs.symbols[n].name == name)
            .unwrap_or_else(|| panic!("no symbol {name} in {file}"))
    };
    let alpha = symbol("src/a.rs", "alpha");
    let beta = symbol("src/b.rs", "beta");
    let gamma = symbol("src/c.rs", "gamma");
    let delta = symbol("src/d.rs", "delta");

    let scores = symbol_scores(&graphs, &analysis.seams);
    let start: BTreeSet<_> = [alpha].into_iter().collect();
    let radius = blast_radius(&graphs.symbols, &start, &scores, BOUNDING_THRESHOLD);

    let expected_reachable: BTreeSet<_> = [alpha, beta, gamma, delta].into_iter().collect();
    let expected_bounded: BTreeSet<_> = [alpha, beta].into_iter().collect();

    assert_eq!(radius.reachable, expected_reachable, "the unbounded set");
    assert_eq!(radius.bounded, expected_bounded, "the bounded set");
    assert_eq!(
        radius.bounding_seams.len(),
        1,
        "one seam stopped the walk: gamma -> beta"
    );
    let (stop_from, stop_to) = graphs
        .symbols
        .edge_endpoints(radius.bounding_seams[0])
        .expect("the bounding edge exists");
    assert_eq!(stop_from, gamma);
    assert_eq!(stop_to, beta);

    assert!(
        (radius.containment() - 0.5).abs() < 1e-9,
        "the seam cuts half the reach away, got {}",
        radius.containment()
    );
}
