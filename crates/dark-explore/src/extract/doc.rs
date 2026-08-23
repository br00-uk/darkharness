//! The generic `doc_present` heuristic: does a documentation comment
//! immediately precede a definition, with no blank line between?
//!
//! Python is not part of this engine: a Python docstring is the first
//! statement inside the definition's own body, not a comment before it. The
//! `scripting` grammar adapter checks for it directly.

use tree_sitter::Node;

use crate::syntax::Language;

use super::util::node_text;

/// One language's documentation-comment convention.
struct DocRule {
    /// The `tree-sitter` node kinds that this language's comments parse as.
    comment_kinds: &'static [&'static str],
    /// Node kinds that sit between a doc comment and its definition without
    /// breaking the association: an attribute, an annotation, a decorator.
    transparent_kinds: &'static [&'static str],
    /// A prefix that marks a comment as documentation rather than an
    /// ordinary remark. An empty slice means every comment in
    /// `comment_kinds` counts, which matches the `godoc` and Ruby `rdoc`
    /// convention of treating any adjacent comment as documentation.
    markers: &'static [&'static str],
}

fn rule(language: Language) -> Option<DocRule> {
    let rule = match language {
        Language::Rust => DocRule {
            comment_kinds: &["line_comment", "block_comment"],
            transparent_kinds: &["attribute_item"],
            markers: &["///", "//!", "/**", "/*!"],
        },
        Language::Go | Language::Ruby | Language::C | Language::Cpp => DocRule {
            comment_kinds: &["comment"],
            transparent_kinds: &[],
            markers: &[],
        },
        Language::Java => DocRule {
            comment_kinds: &["line_comment", "block_comment"],
            transparent_kinds: &[],
            markers: &["/**"],
        },
        Language::CSharp => DocRule {
            comment_kinds: &["comment"],
            transparent_kinds: &["attribute_list"],
            markers: &["///"],
        },
        Language::JavaScript | Language::TypeScript | Language::Tsx => DocRule {
            comment_kinds: &["comment"],
            transparent_kinds: &["decorator"],
            markers: &["/**"],
        },
        Language::Python | Language::Sql | Language::Markdown => return None,
    };
    Some(rule)
}

/// Returns `true` when a comment or a blank gap of at most one newline
/// separates `earlier` from `later`.
///
/// The gap between two adjacent siblings is at most a handful of bytes in
/// practice (one comment and one item, nothing else can sit between them),
/// so a plain byte scan needs no `bytecount`-style dependency to stay fast;
/// `dark-explore` may not add one regardless (Rule 16).
#[allow(clippy::naive_bytecount)]
fn adjacent(earlier: &Node<'_>, later: &Node<'_>, source: &[u8]) -> bool {
    let start = earlier.end_byte().min(source.len());
    let end = later.start_byte().min(source.len());
    if end < start {
        return false;
    }
    source[start..end].iter().filter(|&&b| b == b'\n').count() <= 1
}

/// Returns `true` when a documentation comment immediately precedes
/// `def_node`, per `language`'s convention.
pub(crate) fn doc_present(language: Language, def_node: Node<'_>, source: &[u8]) -> bool {
    let Some(rule) = rule(language) else {
        return false;
    };
    let mut boundary = def_node;
    loop {
        let Some(prev) = boundary.prev_sibling() else {
            return false;
        };
        if rule.transparent_kinds.contains(&prev.kind()) {
            boundary = prev;
            continue;
        }
        if !rule.comment_kinds.contains(&prev.kind()) {
            return false;
        }
        if !adjacent(&prev, &boundary, source) {
            return false;
        }
        if rule.markers.is_empty() {
            return true;
        }
        let text = node_text(&prev, source);
        return rule.markers.iter().any(|marker| text.starts_with(marker));
    }
}

/// Returns `true` when a Python `class_definition` or `function_definition`
/// `body` block's first statement is a bare string expression: a docstring.
pub(crate) fn python_docstring_present(body: Node<'_>) -> bool {
    let Some(first) = body.named_child(0) else {
        return false;
    };
    if first.kind() != "expression_statement" {
        return false;
    }
    matches!(first.named_child(0).map(|n| n.kind()), Some("string"))
}
