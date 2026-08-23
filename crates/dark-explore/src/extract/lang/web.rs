//! The `javascript` and `typescript` grammar adapters. The TSX grammar
//! shares TypeScript's node kinds for everything this module reads, so
//! `classify_typescript` and `import_typescript` serve both.

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::paths::RepoPaths;
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::node_text;

const JS_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs"];
const JS_INDEX_NAMES: &[&str] = &["index.ts", "index.tsx", "index.js", "index.jsx"];

/// Walks upward from `node` and reports whether an `export_statement`
/// encloses it. `export` in JavaScript and TypeScript wraps the
/// declaration it exports (`export_statement declaration: (...)`)
/// rather than modifying it in place, and the arrow-function pattern
/// captures its `variable_declarator`, two levels below the wrapper, so
/// this walks to the root rather than checking one parent.
fn has_export_ancestor(node: Node<'_>) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == "export_statement" {
            return true;
        }
        current = n.parent();
    }
    false
}

pub(crate) fn classify_typescript(
    raw_kind: DefKind,
    node: Node<'_>,
    _source: &[u8],
) -> ClassifiedDef {
    let exported = has_export_ancestor(node);
    let is_interface_like = match raw_kind {
        DefKind::Interface => true,
        DefKind::Class => node.kind() == "abstract_class_declaration",
        DefKind::TypeAlias => node
            .child_by_field_name("value")
            .is_some_and(|value| value.kind() == "object_type"),
        _ => false,
    };
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like,
    }
}

pub(crate) fn classify_javascript(
    raw_kind: DefKind,
    node: Node<'_>,
    _source: &[u8],
) -> ClassifiedDef {
    ClassifiedDef {
        kind: raw_kind,
        exported: has_export_ancestor(node),
        is_interface_like: false,
    }
}

/// Strips the surrounding quote characters from a JS/TS string literal's
/// own source text. A plain trim is enough here: F2 only needs the module
/// specifier's text, not a fully unescaped string value.
fn string_literal_text(node: &Node<'_>, source: &[u8]) -> String {
    node_text(node, source)
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string()
}

/// Recursively reads an `import_clause` or `export_clause` subtree,
/// collecting bound local names and the whole-module binding (a default
/// import, or a `* as ns` namespace import).
fn collect_names(
    node: Node<'_>,
    source: &[u8],
    names: &mut Vec<String>,
    module_binding: &mut Option<String>,
) {
    match node.kind() {
        "identifier" => *module_binding = Some(node_text(&node, source).to_string()),
        "namespace_import" => {
            if let Some(id) = node.named_child(0) {
                *module_binding = Some(node_text(&id, source).to_string());
            }
        }
        "import_specifier" | "export_specifier" => {
            let bound = node
                .child_by_field_name("alias")
                .or_else(|| node.child_by_field_name("name"));
            if let Some(bound) = bound {
                names.push(node_text(&bound, source).to_string());
            }
        }
        "import_clause" | "named_imports" | "export_clause" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_names(child, source, names, module_binding);
            }
        }
        _ => {}
    }
}

fn import_js_like(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    let source_node = node.child_by_field_name("source")?;
    let raw = string_literal_text(&source_node, source);

    let mut imported_names = Vec::new();
    let mut module_binding = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_names(child, source, &mut imported_names, &mut module_binding);
    }

    // A bare package specifier (`"react"`, `"@scope/pkg"`) resolves through
    // `node_modules` or a `tsconfig.json` path mapping, neither of which a
    // lexical pass can see; only a relative specifier is attempted.
    let resolved_to = if raw.starts_with('.') {
        repo.resolve_relative(&raw, JS_EXTENSIONS, JS_INDEX_NAMES)
    } else {
        None
    };

    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names,
        module_binding,
        resolved_to,
    })
}

pub(crate) fn import_typescript(
    node: Node<'_>,
    source: &[u8],
    repo: &RepoPaths<'_>,
) -> Option<Import> {
    import_js_like(node, source, repo)
}

pub(crate) fn import_javascript(
    node: Node<'_>,
    source: &[u8],
    repo: &RepoPaths<'_>,
) -> Option<Import> {
    import_js_like(node, source, repo)
}
