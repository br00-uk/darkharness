//! The `java` and `csharp` grammar adapters.

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::{has_word, node_text, text_before_name};

/// Reads whether `word` (`public`, `abstract`) appears as a whole token in
/// the source text between `node`'s start and `name_node`'s start.
///
/// `class_declaration`, `method_declaration`, and `interface_declaration`
/// all give their `modifiers` (Java) or `modifier` (C#) child node a span
/// that starts *before* the declaration's own keyword, so this range always
/// covers every modifier that precedes the name, regardless of how many
/// there are or which named field the grammar files them under.
fn has_modifier(node: &Node<'_>, name_node: &Node<'_>, source: &[u8], word: &str) -> bool {
    has_word(text_before_name(node, name_node, source), word)
}

fn trimmed_statement(node: &Node<'_>, source: &[u8], keyword: &str) -> String {
    let text = node_text(node, source).trim_end_matches(';').trim();
    text.strip_prefix(keyword)
        .unwrap_or(text)
        .trim()
        .to_string()
}

// ---------------------------------------------------------------- Java ---

pub(crate) fn classify_java(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let exported = has_modifier(&node, &name_node, source, "public");
    let is_interface_like = match raw_kind {
        DefKind::Interface => true,
        DefKind::Class => has_modifier(&node, &name_node, source, "abstract"),
        _ => false,
    };
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like,
    }
}

/// Java resolves an import through the classpath, which a lexical,
/// file-path-only pass over the repository tree cannot see: a package
/// name's conventional correspondence to a directory path is exactly
/// that — a convention, not a guarantee — so [`Import::resolved_to`] stays
/// `None`. See F2, "do not report a guessed reference as resolved."
pub(crate) fn import_java(node: Node<'_>, source: &[u8]) -> Import {
    Import {
        raw: trimmed_statement(&node, source, "import"),
        range: Span::from_node(&node),
        imported_names: Vec::new(),
        module_binding: None,
        resolved_to: None,
    }
}

// --------------------------------------------------------------- C# ---

pub(crate) fn classify_csharp(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let exported = has_modifier(&node, &name_node, source, "public");
    let is_interface_like = match raw_kind {
        DefKind::Interface => true,
        DefKind::Class => has_modifier(&node, &name_node, source, "abstract"),
        _ => false,
    };
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like,
    }
}

/// C# resolves a `using` through the project's assembly references, which
/// a lexical pass over the repository tree cannot see. See
/// [`import_java`]'s documentation: the same reasoning applies.
pub(crate) fn import_csharp(node: Node<'_>, source: &[u8]) -> Import {
    Import {
        raw: trimmed_statement(&node, source, "using"),
        range: Span::from_node(&node),
        imported_names: Vec::new(),
        module_binding: None,
        resolved_to: None,
    }
}
