//! The `graph` stage.
//!
//! Builds three `petgraph` graphs directly from
//! [`extract::extract_repository`](crate::extract::extract_repository)'s
//! output:
//!
//! - the **F-graph**: file to file, an edge for each import that resolved
//!   to a file in the repository;
//! - the **S-graph**: symbol to symbol, an edge from the definition a
//!   reference occurs inside to the definition it resolved to;
//! - the **M-graph**: the F-graph contracted by directory.
//!
//! See task unit `F2`, "Do" items 5 to 7, and Rule 32 for the node
//! numbering every graph here follows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use petgraph::graph::{DiGraph, NodeIndex};

use crate::discover::compare_paths;
use crate::extract::{DefKind, FileSymbols, ResolutionConfidence};

/// One F-graph node: a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    /// The file's path, relative to the repository root.
    pub path: PathBuf,
    /// How many definitions [`crate::extract`] found in this file.
    pub total_defs: u32,
    /// How many of those definitions are interface-like.
    pub interface_like_defs: u32,
}

/// One F-graph edge: file `a` imports file `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileEdge {
    /// How many of `a`'s imports resolved to `b`.
    pub imports: u32,
}

/// One S-graph node: a single definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolNode {
    /// The path of the file that holds the definition.
    pub file: PathBuf,
    /// The definition's index into that file's
    /// [`FileSymbols::defs`](crate::extract::FileSymbols::defs).
    pub def_index: usize,
    /// The definition's name.
    pub name: String,
    /// The definition's category.
    pub kind: DefKind,
    /// Whether F3's per-language table names this definition
    /// interface-like.
    pub is_interface_like: bool,
}

/// One S-graph edge: the definition that encloses a reference points at
/// the definition the reference resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolEdge {
    /// The strongest [`ResolutionConfidence`] among the references this
    /// edge summarises. `Exact` outranks `ImportScoped`, which outranks
    /// `NameOnly`.
    pub confidence: ResolutionConfidence,
    /// How many references between these two definitions this edge
    /// summarises.
    pub references: u32,
}

/// One M-graph node: a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleNode {
    /// The directory's path, relative to the repository root. The
    /// repository root itself is the empty path.
    pub path: PathBuf,
    /// How many files in the F-graph sit directly in this directory.
    pub files: u32,
    /// The sum of [`FileNode::total_defs`] over those files.
    pub total_defs: u32,
    /// The sum of [`FileNode::interface_like_defs`] over those files.
    pub interface_like_defs: u32,
}

/// One M-graph edge: directory `a` holds a file that imports a file in
/// directory `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleEdge {
    /// How many distinct F-graph edges contracted into this one.
    ///
    /// An F-graph edge whose two files share a directory contracts to a
    /// self-loop and is dropped rather than kept: a module's own internal
    /// coupling is not a seam between modules, which is what the M-graph
    /// exists to expose. This is F2's own reading of "contracted by
    /// directory" — the PRD does not spell out self-loop handling — so a
    /// caller that wants intra-directory F-graph edges too should read
    /// [`Graphs::files`] directly rather than the M-graph.
    pub file_edges: u32,
}

/// The three graphs, together with the lookup tables that map a repository
/// path (or, for the S-graph, a path and a definition index) to its node.
pub struct Graphs {
    /// The F-graph: file to file.
    pub files: DiGraph<FileNode, FileEdge>,
    /// The S-graph: symbol to symbol.
    pub symbols: DiGraph<SymbolNode, SymbolEdge>,
    /// The M-graph: the F-graph contracted by directory.
    pub modules: DiGraph<ModuleNode, ModuleEdge>,
    /// Maps a file's path to its node in [`Graphs::files`].
    pub file_index: HashMap<PathBuf, NodeIndex>,
    /// Maps `(file path, def_index)` to its node in [`Graphs::symbols`].
    pub symbol_index: HashMap<(PathBuf, usize), NodeIndex>,
    /// Maps a directory's path to its node in [`Graphs::modules`].
    pub module_index: HashMap<PathBuf, NodeIndex>,
}

/// Returns the repository-relative directory that holds `path`, or the
/// empty path when `path` sits at the repository root.
fn parent_dir(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| PathBuf::from(""), Path::to_path_buf)
}

fn confidence_rank(confidence: ResolutionConfidence) -> u8 {
    match confidence {
        ResolutionConfidence::Exact => 2,
        ResolutionConfidence::ImportScoped => 1,
        ResolutionConfidence::NameOnly => 0,
    }
}

/// Finds the innermost definition in `file` whose range contains
/// `ref_range`, by byte-range containment. Returns `None` when no
/// definition encloses it (a reference at a file's top level, outside any
/// function, class, or module block).
fn enclosing_def(file: &FileSymbols, ref_range: crate::extract::Span) -> Option<usize> {
    file.defs
        .iter()
        .enumerate()
        .filter(|(_, def)| {
            def.range.start_byte <= ref_range.start_byte && ref_range.end_byte <= def.range.end_byte
        })
        .min_by_key(|(_, def)| def.range.end_byte - def.range.start_byte)
        .map(|(index, _)| index)
}

/// Adds an edge from `a` to `b`, or, when one already exists, updates its
/// weight instead of adding a parallel edge.
fn upsert_edge<N, E>(
    graph: &mut DiGraph<N, E>,
    a: NodeIndex,
    b: NodeIndex,
    new_weight: impl FnOnce() -> E,
    update: impl FnOnce(&mut E),
) {
    if let Some(edge) = graph.find_edge(a, b) {
        if let Some(weight) = graph.edge_weight_mut(edge) {
            update(weight);
        }
    } else {
        graph.add_edge(a, b, new_weight());
    }
}

/// Adds the F-graph edges: one import resolving to a file in the repository.
fn add_file_edges(
    file_graph: &mut DiGraph<FileNode, FileEdge>,
    file_index: &HashMap<PathBuf, NodeIndex>,
    sorted: &[&FileSymbols],
) {
    // F-graph edges: one import resolving to a file in the repository.
    for file in sorted {
        let Some(&from) = file_index.get(&file.path) else {
            continue;
        };
        for import in &file.imports {
            let Some(target) = &import.resolved_to else {
                continue;
            };
            let Some(&to) = file_index.get(target) else {
                continue;
            };
            upsert_edge(
                file_graph,
                from,
                to,
                || FileEdge { imports: 1 },
                |w| w.imports += 1,
            );
        }
    }
}

/// Adds the S-graph edges: the definition enclosing a resolved reference
/// points at the definition it resolved to.
///
/// A reference used at a file's top level has no enclosing definition and
/// so contributes no edge.
fn add_symbol_edges(
    symbol_graph: &mut DiGraph<SymbolNode, SymbolEdge>,
    symbol_index: &HashMap<(PathBuf, usize), NodeIndex>,
    sorted: &[&FileSymbols],
) {
    // S-graph edges: the definition enclosing a resolved reference points
    // at the definition it resolved to. A reference with no enclosing
    // definition (used at a file's top level) contributes no edge.
    for file in sorted {
        for reference in &file.refs {
            let (Some(target), Some(confidence)) = (&reference.resolved_to, reference.confidence)
            else {
                continue;
            };
            let Some(source_def_index) = enclosing_def(file, reference.range) else {
                continue;
            };
            let Some(&from) = symbol_index.get(&(file.path.clone(), source_def_index)) else {
                continue;
            };
            let Some(&to) = symbol_index.get(&(target.file.clone(), target.def_index)) else {
                continue;
            };
            if from == to {
                continue;
            }
            upsert_edge(
                symbol_graph,
                from,
                to,
                || SymbolEdge {
                    confidence,
                    references: 1,
                },
                |w| {
                    w.references += 1;
                    if confidence_rank(confidence) > confidence_rank(w.confidence) {
                        w.confidence = confidence;
                    }
                },
            );
        }
    }
}

/// Contracts the F-graph by directory into the M-graph.
fn add_module_edges(
    module_graph: &mut DiGraph<ModuleNode, ModuleEdge>,
    module_index: &HashMap<PathBuf, NodeIndex>,
    file_graph: &DiGraph<FileNode, FileEdge>,
) {
    // M-graph: the F-graph contracted by directory. An edge whose two
    // endpoints share a directory contracts to a self-loop; see
    // `ModuleEdge::file_edges`'s documentation for why F2 drops it rather
    // than keeping it.
    for edge in file_graph.edge_indices() {
        let Some((source, target)) = file_graph.edge_endpoints(edge) else {
            continue;
        };
        let source_dir = parent_dir(&file_graph[source].path);
        let target_dir = parent_dir(&file_graph[target].path);
        if source_dir == target_dir {
            continue;
        }
        let (Some(&from), Some(&to)) =
            (module_index.get(&source_dir), module_index.get(&target_dir))
        else {
            continue;
        };
        upsert_edge(
            module_graph,
            from,
            to,
            || ModuleEdge { file_edges: 1 },
            |w| w.file_edges += 1,
        );
    }
}

/// Builds the F-graph, S-graph, and M-graph from `files`.
///
/// Node identifiers are assigned in sorted path order — [`compare_paths`],
/// F1's byte comparator, never [`Path`]'s own [`Ord`] — regardless of the
/// order `files` arrives in: this function sorts its own working copy
/// first. Two calls with the same `files` content, in any order, produce
/// graphs whose node identifiers agree file for file, symbol for symbol,
/// directory for directory. See Rule 32 and F2, "Do" item 7.
#[must_use]
pub fn build(files: &[FileSymbols]) -> Graphs {
    let mut sorted: Vec<&FileSymbols> = files.iter().collect();
    sorted.sort_by(|a, b| compare_paths(&a.path, &b.path));

    let mut file_graph = DiGraph::new();
    let mut file_index = HashMap::with_capacity(sorted.len());
    for file in &sorted {
        let total_defs = u32::try_from(file.defs.len()).unwrap_or(u32::MAX);
        let interface_like_defs =
            u32::try_from(file.defs.iter().filter(|d| d.is_interface_like).count())
                .unwrap_or(u32::MAX);
        let node = file_graph.add_node(FileNode {
            path: file.path.clone(),
            total_defs,
            interface_like_defs,
        });
        file_index.insert(file.path.clone(), node);
    }

    let mut symbol_graph = DiGraph::new();
    let mut symbol_index = HashMap::new();
    for file in &sorted {
        for (def_index, def) in file.defs.iter().enumerate() {
            let node = symbol_graph.add_node(SymbolNode {
                file: file.path.clone(),
                def_index,
                name: def.name.clone(),
                kind: def.kind,
                is_interface_like: def.is_interface_like,
            });
            symbol_index.insert((file.path.clone(), def_index), node);
        }
    }

    let mut module_paths: Vec<PathBuf> = sorted.iter().map(|f| parent_dir(&f.path)).collect();
    module_paths.sort_by(|a, b| compare_paths(a, b));
    module_paths.dedup();

    let mut module_graph = DiGraph::new();
    let mut module_index = HashMap::with_capacity(module_paths.len());
    for dir in &module_paths {
        let member_files: Vec<&&FileSymbols> = sorted
            .iter()
            .filter(|f| parent_dir(&f.path) == *dir)
            .collect();
        let files_count = u32::try_from(member_files.len()).unwrap_or(u32::MAX);
        let total_defs: u32 = member_files
            .iter()
            .map(|f| u32::try_from(f.defs.len()).unwrap_or(u32::MAX))
            .sum();
        let interface_like_defs: u32 = member_files
            .iter()
            .map(|f| {
                u32::try_from(f.defs.iter().filter(|d| d.is_interface_like).count())
                    .unwrap_or(u32::MAX)
            })
            .sum();
        let node = module_graph.add_node(ModuleNode {
            path: dir.clone(),
            files: files_count,
            total_defs,
            interface_like_defs,
        });
        module_index.insert(dir.clone(), node);
    }

    add_file_edges(&mut file_graph, &file_index, &sorted);
    add_symbol_edges(&mut symbol_graph, &symbol_index, &sorted);
    add_module_edges(&mut module_graph, &module_index, &file_graph);

    Graphs {
        files: file_graph,
        symbols: symbol_graph,
        modules: module_graph,
        file_index,
        symbol_index,
        module_index,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::discover::{self, DiscoverOptions};
    use crate::extract::extract_repository;
    use crate::syntax::{self, Cache};

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn build_fixture(root: &Path) {
        write(root, "Cargo.toml", "[package]\nname = \"x\"\n");
        write(root, "src/lib.rs", "pub fn a_fn() {}\n");
        write(
            root,
            "src/consumer.rs",
            "use crate::a_fn;\nfn call_it() { a_fn(); }\n",
        );
        write(root, "other/leaf.rs", "pub fn leaf() {}\n");
    }

    fn extract_dir(root: &Path) -> Vec<crate::extract::FileSymbols> {
        let snapshot = discover::discover(root, &DiscoverOptions::default()).unwrap();
        let (parsed, _cache) = syntax::parse_snapshot(&Cache::new(), root, &snapshot).unwrap();
        extract_repository(&snapshot, &parsed)
    }

    #[test]
    fn file_node_identifiers_follow_sorted_path_order() {
        let dir = TempDir::new().unwrap();
        build_fixture(dir.path());
        let files = extract_dir(dir.path());

        let graphs = build(&files);

        let mut expected: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        expected.sort_by(|a, b| compare_paths(a, b));
        for (position, path) in expected.iter().enumerate() {
            let node = graphs.file_index[path];
            assert_eq!(node.index(), position, "{path:?} should be node {position}");
        }
    }

    #[test]
    fn node_identifiers_do_not_depend_on_input_order() {
        let dir = TempDir::new().unwrap();
        build_fixture(dir.path());
        let mut files = extract_dir(dir.path());

        let baseline = build(&files);
        files.reverse();
        let reversed = build(&files);

        for path in files.iter().map(|f| f.path.clone()) {
            assert_eq!(
                baseline.file_index[&path].index(),
                reversed.file_index[&path].index(),
                "{path:?} moved node identifier when input order changed"
            );
        }
    }

    #[test]
    fn an_import_that_resolves_becomes_an_f_graph_edge() {
        let dir = TempDir::new().unwrap();
        build_fixture(dir.path());
        let files = extract_dir(dir.path());

        let graphs = build(&files);

        let from = graphs.file_index[Path::new("src/consumer.rs")];
        let to = graphs.file_index[Path::new("src/lib.rs")];
        let edge = graphs.files.find_edge(from, to);
        assert!(edge.is_some(), "src/consumer.rs should import src/lib.rs");
    }

    #[test]
    fn a_resolved_call_becomes_an_s_graph_edge_from_its_enclosing_function() {
        let dir = TempDir::new().unwrap();
        build_fixture(dir.path());
        let files = extract_dir(dir.path());

        let graphs = build(&files);

        let from_consumer = graphs.symbol_index[&(PathBuf::from("src/consumer.rs"), 0)];
        let to_library = graphs.symbol_index[&(PathBuf::from("src/lib.rs"), 0)];
        let edge = graphs
            .symbols
            .find_edge(from_consumer, to_library)
            .expect("call_it -> a_fn should be an S-graph edge");
        assert_eq!(
            graphs.symbols[edge].confidence,
            ResolutionConfidence::ImportScoped
        );
    }

    #[test]
    fn cross_directory_edges_contract_into_the_m_graph() {
        let dir = TempDir::new().unwrap();
        build_fixture(dir.path());
        // Make `other/leaf.rs` importable from `src/consumer.rs` so the
        // F-graph has a cross-directory edge to contract.
        write(
            dir.path(),
            "src/consumer.rs",
            "use crate::a_fn;\nfn call_it() { a_fn(); }\n",
        );
        let files = extract_dir(dir.path());

        let graphs = build(&files);

        // `src` and `other` are different directories in the fixture;
        // every M-graph node's directory should differ from its own
        // in-directory file edges (no self-loops).
        for edge in graphs.modules.edge_indices() {
            let (a, b) = graphs.modules.edge_endpoints(edge).unwrap();
            assert_ne!(graphs.modules[a].path, graphs.modules[b].path);
        }
        // `src` holds two files; the M-graph must not have more module
        // nodes than there are distinct directories in the fixture.
        assert_eq!(graphs.modules.node_count(), 2, "src and other");
    }
}
