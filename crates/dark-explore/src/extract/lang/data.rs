//! The `sql` and `markdown` grammar adapters.

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::paths::RepoPaths;
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::node_text;

// ------------------------------------------------------------------ SQL --

/// Neither language sits in F3's `is_interface_like` table (§F3, "Do" item
/// 2), so both adapters return `false` unconditionally, and both report
/// every definition `exported`: SQL has no file-scoping concept, and a
/// Markdown heading is public by construction.
pub(crate) fn classify_sql(raw_kind: DefKind) -> ClassifiedDef {
    ClassifiedDef {
        kind: raw_kind,
        exported: true,
        is_interface_like: false,
    }
}

// ------------------------------------------------------------ Markdown --

pub(crate) fn classify_markdown(raw_kind: DefKind) -> ClassifiedDef {
    ClassifiedDef {
        kind: raw_kind,
        exported: true,
        is_interface_like: false,
    }
}

pub(crate) fn import_markdown(
    node: Node<'_>,
    name_node: Option<Node<'_>>,
    source: &[u8],
    repo: &RepoPaths<'_>,
) -> Option<Import> {
    let mut cursor = node.walk();
    let destination = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "link_destination")?;
    let raw = node_text(&destination, source)
        .trim_matches(|c| c == '<' || c == '>')
        .to_string();
    let resolved_to = repo.resolve_relative(&raw, &["md", "markdown"], &[]);
    let imported_names = name_node
        .map(|n| vec![node_text(&n, source).to_string()])
        .unwrap_or_default();
    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names,
        module_binding: None,
        resolved_to,
    })
}
