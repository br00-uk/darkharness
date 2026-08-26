//! The `rust` and `go` grammar adapters.

use tree_sitter::Node;

use super::ClassifiedDef;
use crate::extract::paths::RepoPaths;
use crate::extract::types::{DefKind, Import, Span};
use crate::extract::util::{has_word, node_text, text_before_name};

// ---------------------------------------------------------------- Rust ---

/// Classifies a Rust definition. `F3`'s table names only `trait` as
/// interface-like for Rust, so [`DefKind::Interface`] is the only kind that
/// sets `is_interface_like`.
pub(crate) fn classify_rust(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let exported = has_word(text_before_name(&node, &name_node, source), "pub");
    ClassifiedDef {
        kind: raw_kind,
        exported,
        is_interface_like: raw_kind == DefKind::Interface,
    }
}

/// Recursively collects the local names a `use` tree binds, and notes
/// whether any branch is a `use_wildcard`.
///
/// `use_as_clause`'s alias is its own last named child in every shape the
/// grammar allows (`path as alias`), so reading it that way needs no field
/// name for `use_as_clause` itself — `tree-sitter-rust`'s node-types.json
/// gives that node no named fields of its own.
fn collect_use_names(
    node: Node<'_>,
    source: &[u8],
    out: &mut Vec<String>,
    has_wildcard: &mut bool,
) {
    match node.kind() {
        "use_wildcard" => *has_wildcard = true,
        "identifier" | "type_identifier" => out.push(node_text(&node, source).to_string()),
        "scoped_identifier" => {
            if let Some(name) = node.child_by_field_name("name") {
                out.push(node_text(&name, source).to_string());
            }
        }
        "use_as_clause" => {
            let mut cursor = node.walk();
            if let Some(alias) = node.named_children(&mut cursor).last() {
                out.push(node_text(&alias, source).to_string());
            }
        }
        "self" | "super" | "crate" => {}
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_use_names(child, source, out, has_wildcard);
            }
        }
    }
}

/// Reads the full path segments of a single, unbranched `use` target
/// (`crate::a::b::C`, with no `{...}` list and no `*`).
///
/// Returns `None` when `node` is a shape this function does not walk (a
/// list, a wildcard): those imports still contribute names via
/// [`collect_use_names`], but F2 does not attempt to resolve them to one
/// file, because they may name more than one.
fn simple_use_path(node: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut cursor = node;
    loop {
        match cursor.kind() {
            "crate" => {
                segments.push("crate".to_string());
                break;
            }
            "self" | "super" => {
                segments.push(cursor.kind().to_string());
                break;
            }
            "identifier" | "type_identifier" => {
                segments.push(node_text(&cursor, source).to_string());
                break;
            }
            "scoped_identifier" => {
                let name = cursor.child_by_field_name("name")?;
                let path = cursor.child_by_field_name("path")?;
                let mut prefix = simple_use_path(path, source)?;
                prefix.push(node_text(&name, source).to_string());
                return Some(prefix);
            }
            "use_as_clause" => {
                cursor = cursor.named_child(0)?;
            }
            _ => return None,
        }
    }
    segments.reverse();
    Some(segments)
}

/// Returns the directory of the workspace crate `segment` names, when this
/// repository holds one.
///
/// A Rust path leads with either `crate` — this crate — or the name of
/// another crate, and in a workspace that other crate is usually a sibling
/// of this one. `use dark_contract::Engine` from `crates/dark-cli/src/`
/// means `crates/dark-contract`, with the identifier's underscores back to
/// the hyphens a directory name uses.
///
/// Two candidates are tried, both by direct lookup rather than a scan: a
/// sibling of the importing file's own crate directory (`crates/<name>`),
/// and a crate directory at the repository root (`<name>`). Both are
/// confirmed against a `Cargo.toml` in [`RepoPaths::all`], and the caller
/// then confirms the file it builds from this as well, so a first segment
/// that names an external crate resolves to nothing rather than to the
/// wrong file.
///
/// A package whose name differs from its directory name is not found. That
/// costs a resolution; it never produces a wrong one.
fn workspace_crate_dir(segment: &str, repo: &RepoPaths<'_>) -> Option<std::path::PathBuf> {
    let dir_name = segment.replace('_', "-");
    let own_crate = repo.nearest_ancestor_with("Cargo.toml")?;

    let mut candidates = Vec::with_capacity(2);
    if let Some(parent) = own_crate.parent() {
        candidates.push(parent.join(&dir_name));
    }
    candidates.push(std::path::PathBuf::from(&dir_name));

    candidates
        .into_iter()
        .find(|dir| repo.all.contains(&dir.join("Cargo.toml")))
}

/// Resolves a Rust `use` path to a file.
///
/// A `crate::`-anchored path resolves against the nearest `Cargo.toml`
/// above the importing file. A path leading with another workspace crate's
/// name resolves against that crate's directory instead — see
/// [`workspace_crate_dir`] — which is what lets a reference to a type
/// defined in one crate and used in another be recorded as a reference at
/// all. Without it every cross-crate use in a workspace stays unresolved,
/// and a blast radius stops at the crate boundary.
///
/// `self::` and `super::` paths are not attempted: resolving them needs the
/// importing file's own logical module path within its crate, which does
/// not always match its filesystem path (a `#[path]` attribute, an inline
/// `mod` block, both break the correspondence), so a lexical guess here
/// risks reporting the wrong file. Leaving `resolved_to` `None` is the safe
/// answer; see F2, "do not report a guessed reference as resolved."
fn resolve_rust_path(segments: &[String], repo: &RepoPaths<'_>) -> Option<std::path::PathBuf> {
    let first = segments.first().map(String::as_str)?;
    let crate_dir = if first == "crate" {
        repo.nearest_ancestor_with("Cargo.toml")?
    } else {
        workspace_crate_dir(first, repo)?
    };
    let rest = &segments[1..];
    if rest.is_empty() {
        return None;
    }
    let joined = rest.join("/");
    let module_file = crate_dir.join("src").join(format!("{joined}.rs"));
    if repo.all.contains(&module_file) {
        return Some(module_file);
    }
    let module_dir_file = crate_dir.join("src").join(&joined).join("mod.rs");
    if repo.all.contains(&module_dir_file) {
        return Some(module_dir_file);
    }
    // The last segment may name an item inside the module the remaining
    // segments name, rather than a module of its own.
    if rest.len() > 1 {
        let parent = rest[..rest.len() - 1].join("/");
        let parent_file = crate_dir.join("src").join(format!("{parent}.rs"));
        if repo.all.contains(&parent_file) {
            return Some(parent_file);
        }
        let parent_mod = crate_dir.join("src").join(&parent).join("mod.rs");
        if repo.all.contains(&parent_mod) {
            return Some(parent_mod);
        }
    } else {
        let lib_rs = crate_dir.join("src/lib.rs");
        if repo.all.contains(&lib_rs) {
            return Some(lib_rs);
        }
    }
    None
}

pub(crate) fn import_rust(node: Node<'_>, source: &[u8], repo: &RepoPaths<'_>) -> Option<Import> {
    let argument = node.child_by_field_name("argument")?;
    let raw = node_text(&node, source)
        .trim_end_matches(';')
        .trim()
        .to_string();

    let mut imported_names = Vec::new();
    let mut has_wildcard = false;
    collect_use_names(argument, source, &mut imported_names, &mut has_wildcard);

    let resolved_to =
        simple_use_path(argument, source).and_then(|segments| resolve_rust_path(&segments, repo));

    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names,
        module_binding: None,
        resolved_to,
    })
}

// ------------------------------------------------------------------ Go ---

/// `true` when a Go identifier's first byte is an uppercase ASCII letter:
/// the language's own, exact rule for whether a name is exported.
fn go_is_exported(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
}

pub(crate) fn classify_go(
    raw_kind: DefKind,
    node: Node<'_>,
    name_node: Node<'_>,
    source: &[u8],
) -> ClassifiedDef {
    let name = node_text(&name_node, source);
    let exported = go_is_exported(name);

    if raw_kind != DefKind::TypeAlias {
        return ClassifiedDef {
            kind: raw_kind,
            exported,
            is_interface_like: false,
        };
    }

    // `type Foo interface { ... }` and `type Foo struct { ... }` share the
    // `type_spec` node; only the `type` field's own node kind tells them
    // apart from a plain alias.
    let type_spec = node; // the query captures `type_declaration`, whose sole named child of interest is `type_spec`; walk to it.
    let spec = find_type_spec(type_spec, &name_node);
    let kind = spec
        .and_then(|s| s.child_by_field_name("type"))
        .map(|t| t.kind())
        .map_or(DefKind::TypeAlias, |kind| match kind {
            "interface_type" => DefKind::Interface,
            "struct_type" => DefKind::Class,
            _ => DefKind::TypeAlias,
        });
    ClassifiedDef {
        kind,
        exported,
        is_interface_like: kind == DefKind::Interface,
    }
}

/// Finds the `type_spec` inside a `type_declaration` whose `name` field is
/// `name_node` — there is exactly one when a `type_declaration` groups more
/// than one spec (`type (A int; B string)`), and `name_node` alone (an
/// interior node) cannot walk back to its parent field name directly.
fn find_type_spec<'tree>(
    type_declaration: Node<'tree>,
    name_node: &Node<'tree>,
) -> Option<Node<'tree>> {
    let mut cursor = type_declaration.walk();
    type_declaration.named_children(&mut cursor).find(|child| {
        child.kind() == "type_spec"
            && child
                .child_by_field_name("name")
                .is_some_and(|n| n.id() == name_node.id())
    })
}

pub(crate) fn import_go(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let path_node = node.child_by_field_name("path")?;
    let path_text = node_text(&path_node, source);
    let raw = path_text.trim_matches('"').to_string();
    let module_binding = node
        .child_by_field_name("name")
        .map(|n| node_text(&n, source).to_string());

    // Go's module system resolves an import path through `go.mod` and the
    // module cache, neither of which a lexical, file-path-only pass can
    // see; a directory whose name merely matches the import path's last
    // segment is not evidence of the right file. `resolved_to` stays
    // `None`; see F2, "do not report a guessed reference as resolved."
    Some(Import {
        raw,
        range: Span::from_node(&node),
        imported_names: Vec::new(),
        module_binding,
        resolved_to: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_exported_reads_the_first_letter() {
        assert!(go_is_exported("Foo"));
        assert!(!go_is_exported("foo"));
        assert!(!go_is_exported(""));
    }
}
