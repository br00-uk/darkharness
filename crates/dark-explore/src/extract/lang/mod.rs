//! Per-language classification: turning a raw query capture into a
//! [`ClassifiedDef`], and a raw `@import` node into a full
//! [`super::types::Import`].
//!
//! Each submodule owns the languages that share a family's conventions
//! closely enough to read naturally together; nothing besides this file
//! dispatches on [`Language`] outside that grouping.

mod c_family;
mod data;
mod jvm;
mod scripting;
mod systems;
mod web;

use tree_sitter::Node;

use super::paths::RepoPaths;
use super::types::{DefKind, Import};
use crate::syntax::Language;

/// The language-aware classification of one definition capture.
pub(crate) struct ClassifiedDef {
    /// The definition's category, refined from the query's own
    /// (necessarily coarser) capture name.
    pub kind: DefKind,
    /// Whether the definition is visible outside its file.
    pub exported: bool,
    /// Whether `F3`'s per-language table names this shape interface-like.
    pub is_interface_like: bool,
}

/// Classifies one definition capture. `raw_kind` is the [`DefKind`] the
/// query's own capture name gave; a language adapter may refine it further
/// (Go's `type_spec`, for example, arrives as [`DefKind::TypeAlias`] and
/// leaves as [`DefKind::Interface`], [`DefKind::Class`], or unchanged).
pub(crate) fn classify_def(
    language: Language,
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    match language {
        Language::Rust => systems::classify_rust(raw_kind, node, name_node, source),
        Language::Go => systems::classify_go(raw_kind, node, name_node, source),
        Language::TypeScript | Language::Tsx => web::classify_typescript(raw_kind, node, source),
        Language::JavaScript => web::classify_javascript(raw_kind, node, source),
        Language::Python => scripting::classify_python(raw_kind, node, name_node, source),
        Language::Ruby => scripting::classify_ruby(raw_kind),
        Language::Java => jvm::classify_java(raw_kind, node, name_node, source),
        Language::CSharp => jvm::classify_csharp(raw_kind, node, name_node, source),
        Language::C => c_family::classify_c(raw_kind, node, name_node, source),
        Language::Cpp => c_family::classify_cpp(raw_kind, node, name_node, source),
        Language::Sql => data::classify_sql(raw_kind),
        Language::Markdown => data::classify_markdown(raw_kind),
    }
}

/// Parses one `@import` capture into a full [`Import`], resolving it
/// against `repo` when the language adapter can do so lexically.
///
/// Returns `None` only when the node carries nothing usable (an `#include`
/// whose path is a macro expansion, for example): a real import that
/// merely fails to resolve still returns `Some`, with
/// [`Import::resolved_to`] left `None`.
pub(crate) fn parse_import(
    language: Language,
    node: Node<'_>,
    name_node: Option<Node<'_>>,
    source: &[u8],
    repo: &RepoPaths<'_>,
) -> Option<Import> {
    match language {
        Language::Rust => systems::import_rust(node, source, repo),
        Language::Go => systems::import_go(node, source),
        Language::TypeScript | Language::Tsx => web::import_typescript(node, source, repo),
        Language::JavaScript => web::import_javascript(node, source, repo),
        Language::Python => scripting::import_python(node, source, repo),
        Language::Ruby => scripting::import_ruby(node, source, repo),
        Language::Java => jvm::import_java(node, source),
        Language::CSharp => jvm::import_csharp(node, source),
        Language::C => c_family::import_c(node, source, repo),
        Language::Cpp => c_family::import_cpp(node, source, repo),
        Language::Sql => None,
        Language::Markdown => data::import_markdown(node, name_node, source, repo),
    }
}
