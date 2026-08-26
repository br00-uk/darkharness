//! Extracts one file's imports, definitions, and references, resolving
//! references against definitions in the same file where a lexical scope
//! walk finds one. Cross-file resolution (`super::resolve`) runs
//! afterwards, over every file's output at once.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::syntax::{Language, ParsedFile};

use super::lang;
use super::paths::RepoPaths;
use super::query::{self, RawKind};
use super::scope;
use super::types::{
    Def, DefKind, FileSymbols, Import, Ref, ResolutionConfidence, ResolvedSymbol, Span,
};
use super::util::node_text;

/// Ranks a definition kind for [`dedup_defs_by_node`]: when two query
/// patterns capture the very same node (a Rust function nested in a
/// `declaration_list`, a Python method inside a class body), the more
/// specific kind wins.
fn def_priority(kind: DefKind) -> u8 {
    match kind {
        DefKind::Method => 1,
        _ => 0,
    }
}

/// Collapses raw definition captures that share one underlying node —
/// see [`def_priority`] — down to one candidate per node, keeping the
/// higher-priority kind. Stable: the first candidate for a given node sets
/// its position in the output, so run order never changes which position a
/// surviving node occupies.
fn dedup_defs_by_node<'t>(
    candidates: Vec<(tree_sitter::Node<'t>, tree_sitter::Node<'t>, DefKind)>,
) -> Vec<(tree_sitter::Node<'t>, tree_sitter::Node<'t>, DefKind)> {
    let mut index_of: HashMap<usize, usize> = HashMap::new();
    let mut out: Vec<(tree_sitter::Node<'t>, tree_sitter::Node<'t>, DefKind)> = Vec::new();
    for candidate in candidates {
        let id = candidate.0.id();
        if let Some(&existing) = index_of.get(&id) {
            if def_priority(candidate.2) > def_priority(out[existing].2) {
                out[existing] = candidate;
            }
        } else {
            index_of.insert(id, out.len());
            out.push(candidate);
        }
    }
    out
}

/// One definition together with the scope it was found in, before the
/// final, deterministic sort assigns it a stable index.
struct ScopedDef {
    def: Def,
    scope_id: usize,
}

/// Extracts `parsed`'s imports, definitions, and references.
///
/// `all_paths` is every path [`crate::discover::Snapshot`] found (parsed or
/// not): import resolution checks membership in this set rather than the
/// filesystem, so it agrees with the same tree discovery already committed
/// to. See Rule 29.
/// The three candidate lists that one pass over the raw tags produces.
struct Candidates<'tree> {
    defs: Vec<(Node<'tree>, Node<'tree>, DefKind)>,
    refs: Vec<(Node<'tree>, Node<'tree>)>,
    imports: Vec<(Node<'tree>, Option<Node<'tree>>)>,
}

/// Sorts the raw tags into definitions, references, and imports.
///
/// A definition or a reference with no name node names nothing, so it is
/// dropped here rather than carried forward as an entry with an empty name.
///
/// A reference whose name node **is** a definition's own name node is
/// dropped too: `struct Session` defines `Session`, it does not reference
/// one. This matters because a grammar's reference patterns are written
/// broadly — `rust.scm` captures every `type_identifier` rather than
/// enumerating each type position — and a broad pattern necessarily also
/// matches the name a definition declares. Filtering by node identity is
/// exact, costs one pass, and holds for every grammar, so a query author
/// can write the broad pattern and rely on this.
fn partition_tags(raw_tags: Vec<query::RawTag<'_>>) -> Candidates<'_> {
    let declared_names: HashSet<usize> = raw_tags
        .iter()
        .filter(|tag| matches!(tag.kind, RawKind::Def(_)))
        .filter_map(|tag| tag.name_node.map(|node| node.id()))
        .collect();

    let mut candidates = Candidates {
        defs: Vec::new(),
        refs: Vec::new(),
        imports: Vec::new(),
    };
    for tag in raw_tags {
        match tag.kind {
            RawKind::Def(kind) => {
                if let Some(name_node) = tag.name_node {
                    candidates.defs.push((tag.node, name_node, kind));
                }
            }
            RawKind::Ref => {
                if let Some(name_node) = tag.name_node
                    && !declared_names.contains(&name_node.id())
                {
                    candidates.refs.push((tag.node, name_node));
                }
            }
            RawKind::Import => candidates.imports.push((tag.node, tag.name_node)),
        }
    }
    candidates
}

/// Classifies each definition candidate and sorts the result.
///
/// Rule 32: the order is deterministic regardless of query match order or
/// parallel arrival, so node identifiers built from it (see `graph::build`)
/// are stable across runs and platforms.
fn build_defs(
    candidates: Vec<(Node<'_>, Node<'_>, DefKind)>,
    language: Language,
    source: &[u8],
    root_id: usize,
) -> Vec<ScopedDef> {
    let mut scoped_defs: Vec<ScopedDef> = dedup_defs_by_node(candidates)
        .into_iter()
        .map(|(node, name_node, raw_kind)| {
            let name = node_text(&name_node, source).to_string();
            let classified = lang::classify_def(language, raw_kind, node, name_node, source);
            let doc_present = lang::doc_present(language, node, source);
            let scope_id = scope::enclosing_scope_id(language, node, root_id);
            ScopedDef {
                def: Def {
                    name,
                    kind: classified.kind,
                    range: Span::from_node(&node),
                    exported: classified.exported,
                    doc_present,
                    is_interface_like: classified.is_interface_like,
                },
                scope_id,
            }
        })
        .collect();

    scoped_defs.sort_by(|a, b| {
        a.def
            .range
            .cmp(&b.def.range)
            .then_with(|| a.def.name.cmp(&b.def.name))
    });
    scoped_defs
}

/// Resolves each reference candidate against the definitions in scope.
///
/// A reference that no scope in its chain defines stays unresolved, with no
/// confidence recorded. Task unit `F2`, Do step 4: an unresolved reference
/// is recorded as unresolved, never guessed at.
fn build_refs(
    candidates: Vec<(Node<'_>, Node<'_>)>,
    language: Language,
    source: &[u8],
    root_id: usize,
    path: &Path,
    defs_by_scope: &HashMap<usize, Vec<(String, usize)>>,
) -> Vec<Ref> {
    let mut refs: Vec<Ref> = candidates
        .into_iter()
        .map(|(node, name_node)| {
            let name = node_text(&name_node, source).to_string();
            let chain = scope::scope_chain(language, node, root_id);
            let found = chain.iter().find_map(|scope_id| {
                defs_by_scope
                    .get(scope_id)?
                    .iter()
                    .find_map(|(def_name, def_index)| (*def_name == name).then_some(*def_index))
            });
            Ref {
                name,
                range: Span::from_node(&name_node),
                resolved_to: found.map(|def_index| ResolvedSymbol {
                    file: path.to_path_buf(),
                    def_index,
                }),
                confidence: found.map(|_| ResolutionConfidence::Exact),
            }
        })
        .collect();
    refs.sort_by(|a, b| a.range.cmp(&b.range).then_with(|| a.name.cmp(&b.name)));
    refs
}

pub(crate) fn extract_file(parsed: &ParsedFile, all_paths: &HashSet<PathBuf>) -> FileSymbols {
    let language = parsed.language;
    let source: &[u8] = &parsed.source;
    let tree = &parsed.tree;
    let root_id = tree.root_node().id();
    let repo = RepoPaths {
        file: &parsed.path,
        all: all_paths,
    };

    let candidates = partition_tags(query::run(language, tree, source));
    let scoped_defs = build_defs(candidates.defs, language, source, root_id);

    let mut defs_by_scope: HashMap<usize, Vec<(String, usize)>> = HashMap::new();
    for (index, scoped) in scoped_defs.iter().enumerate() {
        defs_by_scope
            .entry(scoped.scope_id)
            .or_default()
            .push((scoped.def.name.clone(), index));
    }

    let refs = build_refs(
        candidates.refs,
        language,
        source,
        root_id,
        &parsed.path,
        &defs_by_scope,
    );
    let defs: Vec<Def> = scoped_defs.into_iter().map(|s| s.def).collect();

    let mut imports: Vec<Import> = candidates
        .imports
        .into_iter()
        .filter_map(|(node, name_node)| {
            lang::parse_import(language, node, name_node, source, &repo)
        })
        .collect();
    imports.sort_by(|a, b| a.range.cmp(&b.range));

    FileSymbols {
        path: parsed.path.clone(),
        language,
        imports,
        defs,
        refs,
    }
}
