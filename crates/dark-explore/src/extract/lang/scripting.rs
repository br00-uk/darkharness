//! The `python` and `ruby` grammar adapters.

use std::path::PathBuf;

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::paths::{RepoPaths, with_extension_appended};
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::node_text;

// -------------------------------------------------------------- Python ---

/// `true` when `class_definition`'s `superclasses` argument list names
/// `Protocol` or `ABC`, qualified or not (`typing.Protocol`, `abc.ABC`,
/// `Protocol`, `ABC`). This is F3's Python rule for interface-like: a
/// `Protocol` subclass or an `ABC` subclass.
fn has_protocol_or_abc_base(node: Node<'_>, source: &[u8]) -> bool {
    let Some(bases) = node.child_by_field_name("superclasses") else {
        return false;
    };
    let text = node_text(&bases, source);
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .any(|token| {
            let last = token.rsplit('.').next().unwrap_or(token);
            last == "Protocol" || last == "ABC"
        })
}

pub(crate) fn classify_python(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let name = node_text(&name_node, source);
    let exported = !name.starts_with('_');
    let is_interface_like = raw_kind == DefKind::Class && has_protocol_or_abc_base(node, source);
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like,
    }
}

/// Resolves a relative import's dotted text (`.`, `.foo`, `..foo.bar`)
/// against the importing file's own directory.
///
/// One leading dot means "this package"; each further dot climbs one
/// directory. `remainder`, when not empty, is dotted-joined onto that
/// directory before the `.py` and `__init__.py` probes.
fn resolve_relative_module(module_text: &str, repo: &RepoPaths<'_>) -> Option<PathBuf> {
    let dots = module_text.chars().take_while(|&c| c == '.').count();
    if dots == 0 {
        return None;
    }
    let remainder = &module_text[dots..];
    let mut base = repo.file.parent()?.to_path_buf();
    for _ in 1..dots {
        base = base.parent()?.to_path_buf();
    }
    let candidate_dir = if remainder.is_empty() {
        base
    } else {
        base.join(remainder.replace('.', "/"))
    };
    let as_module = with_extension_appended(&candidate_dir, "py");
    if repo.all.contains(&as_module) {
        return Some(as_module);
    }
    let as_package = candidate_dir.join("__init__.py");
    if repo.all.contains(&as_package) {
        return Some(as_package);
    }
    None
}

/// Resolves an absolute dotted module path (`a.b.c`) against every
/// discovered file, requiring a unique match. See F2, "do not report a
/// guessed reference as resolved."
fn resolve_absolute_module(module_text: &str, repo: &RepoPaths<'_>) -> Option<PathBuf> {
    let segments: Vec<&str> = module_text.split('.').filter(|s| !s.is_empty()).collect();
    repo.resolve_unique_suffix(&segments, &["py"], &["__init__.py"])
}

fn resolve_python_module(module_text: &str, repo: &RepoPaths<'_>) -> Option<PathBuf> {
    if module_text.starts_with('.') {
        resolve_relative_module(module_text, repo)
    } else {
        resolve_absolute_module(module_text, repo)
    }
}

/// Reads the bound local name of one `import` target: `dotted_name` or
/// `aliased_import`'s `alias` field, falling back to the last dotted
/// segment of the aliased import's own path when it carries no alias
/// (which does not happen in valid Python, but a fallback costs nothing).
fn bound_name(node: Node<'_>, source: &[u8]) -> String {
    match node.kind() {
        "aliased_import" => node.child_by_field_name("alias").map_or_else(
            || last_dotted_segment(node, source),
            |n| node_text(&n, source).to_string(),
        ),
        _ => last_dotted_segment(node, source),
    }
}

fn last_dotted_segment(node: Node<'_>, source: &[u8]) -> String {
    node_text(&node, source)
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn module_text_of(node: Node<'_>, source: &[u8]) -> String {
    match node.kind() {
        "aliased_import" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string())
            .unwrap_or_default(),
        _ => node_text(&node, source).to_string(),
    }
}

pub(crate) fn import_python(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    let raw = node_text(&node, source).trim().to_string();
    let range = Span::from_node(&node);

    if node.kind() == "import_from_statement" {
        let module_node = node.child_by_field_name("module_name")?;
        let module_text = node_text(&module_node, source).to_string();
        let mut imported_names = Vec::new();
        let mut cursor = node.walk();
        for name_node in node.children_by_field_name("name", &mut cursor) {
            imported_names.push(bound_name(name_node, source));
        }
        let resolved_to = resolve_python_module(&module_text, repo);
        return Some(Import {
            raw,
            range,
            imported_names,
            module_binding: None,
            resolved_to,
        });
    }

    // `import_statement`: one or more whole-module bindings, `import a.b.c`
    // or `import a.b.c as x`, comma-separated when more than one.
    let mut cursor = node.walk();
    let names: Vec<Node<'_>> = node.children_by_field_name("name", &mut cursor).collect();
    if let [only] = names.as_slice() {
        let module_text = module_text_of(*only, source);
        let resolved_to = resolve_python_module(&module_text, repo);
        return Some(Import {
            raw,
            range,
            imported_names: Vec::new(),
            module_binding: Some(bound_name(*only, source)),
            resolved_to,
        });
    }
    let imported_names = names.iter().map(|n| bound_name(*n, source)).collect();
    Some(Import {
        raw,
        range,
        imported_names,
        module_binding: None,
        resolved_to: None,
    })
}

// ---------------------------------------------------------------- Ruby ---

/// Ruby has no file-scoped export keyword: a `private` call changes what
/// the *interpreter* allows at runtime, not what is visible to a reader of
/// another file. The `ruby` adapter reports every definition `exported`
/// rather than modelling that runtime state.
pub(crate) fn classify_ruby(raw_kind: DefKind) -> ClassifiedDef {
    ClassifiedDef {
        kind: raw_kind,
        exported: true,
        is_interface_like: false,
    }
}

fn ruby_string_content(node: &Node<'_>, source: &[u8]) -> String {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "string_content" {
            return node_text(&child, source).to_string();
        }
    }
    node_text(node, source)
        .trim_matches(|c| c == '\'' || c == '"')
        .to_string()
}

pub(crate) fn import_ruby(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    let method = node.child_by_field_name("method")?;
    let method_name = node_text(&method, source).to_string();
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let string_node = arguments
        .named_children(&mut cursor)
        .find(|n| n.kind() == "string")?;
    let raw = ruby_string_content(&string_node, source);

    // `require_relative` is a file-path specifier; plain `require` names a
    // load-path or gem entry that a lexical, file-path-only pass cannot
    // place. See F2, "do not report a guessed reference as resolved."
    let resolved_to = if method_name == "require_relative" {
        repo.resolve_relative(&raw, &["rb"], &[])
    } else {
        None
    };

    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names: Vec::new(),
        module_binding: None,
        resolved_to,
    })
}
