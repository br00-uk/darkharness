//! The `c` and `cpp` grammar adapters.

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::paths::RepoPaths;
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::{has_word, node_text, text_before_name};

/// `true` when `word` appears, as a whole token, in the source text
/// between `node`'s start and `name_node`'s start. `static` on a free
/// function is the only modifier the `c` and `cpp` adapters read this way:
/// a C++ `static` *member* function means something else (class-scoped
/// rather than instance-scoped, not file-local), so [`classify_cpp`] only
/// applies this check to a free [`DefKind::Function`], never a
/// [`DefKind::Method`].
fn has_modifier(node: &Node<'_>, name_node: &Node<'_>, source: &[u8], word: &str) -> bool {
    has_word(text_before_name(node, name_node, source), word)
}

fn string_literal_inner(node: &Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(&child, source).to_string();
        }
    }
    node_text(node, source).trim_matches('"').to_string()
}

fn import_include(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    let path_node = node.child_by_field_name("path")?;
    let (raw, resolved_to) = match path_node.kind() {
        "string_literal" => {
            let content = string_literal_inner(&path_node, source);
            let resolved = repo.resolve_relative(&content, &["h", "hpp", "hh", "hxx"], &[]);
            (content, resolved)
        }
        // `<...>` names a system or a compiler-search-path header, which a
        // lexical, repository-only pass cannot place. See F2, "do not
        // report a guessed reference as resolved."
        "system_lib_string" => {
            let text = node_text(&path_node, source);
            (
                text.trim_matches(|c| c == '<' || c == '>').to_string(),
                None,
            )
        }
        _ => return None,
    };
    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names: Vec::new(),
        module_binding: None,
        resolved_to,
    })
}

// ------------------------------------------------------------------- C ---

pub(crate) fn classify_c(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let exported = match raw_kind {
        DefKind::Function => !has_modifier(&node, &name_node, source, "static"),
        _ => true,
    };
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like: false,
    }
}

pub(crate) fn import_c(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    import_include(node, source, repo)
}

// ----------------------------------------------------------------- C++ ---

pub(crate) fn classify_cpp(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let exported = match raw_kind {
        DefKind::Function => !has_modifier(&node, &name_node, source, "static"),
        _ => true,
    };
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like: false,
    }
}

pub(crate) fn import_cpp(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    if node.kind() == "using_declaration" {
        let raw = node_text(&node, source)
            .trim_end_matches(';')
            .trim()
            .to_string();
        return Some(Import {
            raw,
            range: Span::from_node(&node),
            imported_names: Vec::new(),
            module_binding: None,
            resolved_to: None,
        });
    }
    import_include(node, source, repo)
}
