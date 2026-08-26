//! A type used as a type is a reference, and a workspace sibling's crate
//! name resolves to that crate.
//!
//! Both were missing, and together they made `dark blast` a call graph that
//! stopped at the crate boundary. A trait used as `dyn Engine` across four
//! crates reported that nothing in the repository referenced it, which is
//! the opposite of the truth and exactly the answer a blast radius exists
//! to give.
//!
//! These tests work at the graph, not the report: the S-graph edge is the
//! thing that was absent, and a count in a report can be right for the
//! wrong reason.

use std::path::Path;

use dark_explore::discover::{DiscoverOptions, discover};
use dark_explore::extract::extract_repository;
use dark_explore::graph::{Graphs, build};
use dark_explore::syntax::{Cache, parse_snapshot};
use petgraph::Direction;
use petgraph::graph::NodeIndex;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, content).expect("write");
}

fn analyse(root: &Path) -> Graphs {
    let snapshot = discover(root, &DiscoverOptions::default()).expect("discover");
    let (parsed, _cache) = parse_snapshot(&Cache::new(), root, &snapshot).expect("parse");
    build(&extract_repository(&snapshot, &parsed))
}

fn symbol(graphs: &Graphs, file: &str, name: &str) -> NodeIndex {
    graphs
        .symbols
        .node_indices()
        .find(|&n| graphs.symbols[n].file == Path::new(file) && graphs.symbols[n].name == name)
        .unwrap_or_else(|| panic!("no symbol {name} in {file}"))
}

/// The names of every definition that references `target`, directly.
fn referencing_names(graphs: &Graphs, target: NodeIndex) -> Vec<String> {
    let mut names: Vec<String> = graphs
        .symbols
        .neighbors_directed(target, Direction::Incoming)
        .map(|n| graphs.symbols[n].name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn a_trait_named_in_a_type_position_is_referenced_by_the_definition_naming_it() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"one\"\n");
    write(root, "src/engine.rs", "pub trait Engine {}\n");
    write(
        root,
        "src/holder.rs",
        "use crate::engine::Engine;\n\
         pub struct Holder { pub engine: Box<dyn Engine> }\n\
         pub fn take(engine: &dyn Engine) {}\n",
    );

    let graphs = analyse(root);
    let engine = symbol(&graphs, "src/engine.rs", "Engine");
    let names = referencing_names(&graphs, engine);

    assert!(
        names.contains(&"Holder".to_owned()),
        "a struct holding `Box<dyn Engine>` references Engine: {names:?}"
    );
    assert!(
        names.contains(&"take".to_owned()),
        "a function taking `&dyn Engine` references Engine: {names:?}"
    );
}

#[test]
fn a_definitions_own_name_is_not_a_reference_to_itself() {
    // `rust.scm` captures every `type_identifier`, which necessarily
    // includes the name each type definition declares. Those must not
    // become references, or every type would reference itself and the
    // reach of every symbol would be wrong.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"one\"\n");
    write(
        root,
        "src/lib.rs",
        "pub struct Alone {}\npub trait Solo {}\n",
    );

    let graphs = analyse(root);
    for name in ["Alone", "Solo"] {
        let node = symbol(&graphs, "src/lib.rs", name);
        assert!(
            referencing_names(&graphs, node).is_empty(),
            "{name} must not reference itself"
        );
    }
}

#[test]
fn a_type_from_a_sibling_crate_in_the_workspace_resolves() {
    // `use dark_contract::Engine` from another crate: the first segment
    // names a workspace sibling, not this crate, so `crate::`-only path
    // resolution left every cross-crate reference unresolved and a blast
    // radius stopped dead at the crate boundary.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(
        root,
        "crates/dark-contract/Cargo.toml",
        "[package]\nname = \"dark-contract\"\n",
    );
    write(
        root,
        "crates/dark-contract/src/lib.rs",
        "pub trait Engine {}\n",
    );
    write(
        root,
        "crates/dark-core/Cargo.toml",
        "[package]\nname = \"dark-core\"\n",
    );
    write(
        root,
        "crates/dark-core/src/lib.rs",
        "use dark_contract::Engine;\npub struct Session { pub engine: Box<dyn Engine> }\n",
    );

    let graphs = analyse(root);
    let engine = symbol(&graphs, "crates/dark-contract/src/lib.rs", "Engine");
    let names = referencing_names(&graphs, engine);

    assert!(
        names.contains(&"Session".to_owned()),
        "a type used across a crate boundary must still be a reference: {names:?}"
    );
}

#[test]
fn a_first_segment_naming_no_workspace_crate_resolves_to_nothing() {
    // The sibling-crate lookup must not become a way to match an unrelated
    // same-named local type: `serde` is not in this workspace, so
    // `serde::Engine` names nothing here, and the local `Engine` must not
    // be offered in its place.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(
        root,
        "crates/mine/Cargo.toml",
        "[package]\nname = \"mine\"\n",
    );
    write(root, "crates/mine/src/engine.rs", "pub trait Engine {}\n");
    write(
        root,
        "crates/mine/src/user.rs",
        "use serde::Engine;\npub struct User { pub engine: Box<dyn Engine> }\n",
    );

    let graphs = analyse(root);
    let engine = symbol(&graphs, "crates/mine/src/engine.rs", "Engine");
    assert!(
        !referencing_names(&graphs, engine).contains(&"User".to_owned()),
        "an import naming a crate outside the workspace must not resolve to a local type"
    );
}

#[test]
fn a_path_leading_with_a_type_references_that_type() {
    // `Event::TurnStart` reaches `TurnStart` through `Event`, so a change
    // to `Event` reaches the definition that names the variant.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "Cargo.toml", "[package]\nname = \"one\"\n");
    write(root, "src/event.rs", "pub enum Event { Start, Stop }\n");
    write(
        root,
        "src/user.rs",
        "use crate::event::Event;\npub fn begin() -> Event { Event::Start }\n",
    );

    let graphs = analyse(root);
    let event = symbol(&graphs, "src/event.rs", "Event");
    assert!(
        referencing_names(&graphs, event).contains(&"begin".to_owned()),
        "a function naming `Event::Start` references Event"
    );
}
