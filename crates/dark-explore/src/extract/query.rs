//! Compiles each grammar's `tags.scm` and runs it against one parsed tree.
//!
//! A query file locates nodes; it does not classify them. Every capture
//! this module returns is either a `@name` node or one "outer" node tagged
//! [`RawKind::Def`], [`RawKind::Ref`], or [`RawKind::Import`]. The `lang`
//! module turns a [`RawTag`] into the language-aware [`super::types::Def`],
//! [`super::types::Ref`], or [`super::types::Import`] that a caller sees.

use std::sync::OnceLock;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator as _, Tree};

use super::types::DefKind;
use crate::syntax::Language;

/// The `tags.scm` text for one grammar, embedded at compile time.
fn query_source(language: Language) -> &'static str {
    match language {
        Language::Rust => include_str!("queries/rust.scm"),
        Language::Go => include_str!("queries/go.scm"),
        Language::TypeScript | Language::Tsx => include_str!("queries/typescript.scm"),
        Language::JavaScript => include_str!("queries/javascript.scm"),
        Language::Python => include_str!("queries/python.scm"),
        Language::Java => include_str!("queries/java.scm"),
        Language::CSharp => include_str!("queries/csharp.scm"),
        Language::Ruby => include_str!("queries/ruby.scm"),
        Language::C => include_str!("queries/c.scm"),
        Language::Cpp => include_str!("queries/cpp.scm"),
        Language::Sql => include_str!("queries/sql.scm"),
        Language::Markdown => include_str!("queries/markdown.scm"),
    }
}

/// One compiled query, cached for the life of the process.
///
/// Thirteen languages means thirteen slots; a plain array indexed by the
/// language's position keeps this lock-free after the first call for each
/// language, unlike a shared map.
struct Compiled {
    query: Query,
}

fn cell(language: Language) -> &'static OnceLock<Compiled> {
    static RUST: OnceLock<Compiled> = OnceLock::new();
    static GO: OnceLock<Compiled> = OnceLock::new();
    static TS: OnceLock<Compiled> = OnceLock::new();
    static TSX: OnceLock<Compiled> = OnceLock::new();
    static JS: OnceLock<Compiled> = OnceLock::new();
    static PY: OnceLock<Compiled> = OnceLock::new();
    static JAVA: OnceLock<Compiled> = OnceLock::new();
    static CSHARP: OnceLock<Compiled> = OnceLock::new();
    static RUBY: OnceLock<Compiled> = OnceLock::new();
    static C: OnceLock<Compiled> = OnceLock::new();
    static CPP: OnceLock<Compiled> = OnceLock::new();
    static SQL: OnceLock<Compiled> = OnceLock::new();
    static MD: OnceLock<Compiled> = OnceLock::new();
    match language {
        Language::Rust => &RUST,
        Language::Go => &GO,
        Language::TypeScript => &TS,
        Language::Tsx => &TSX,
        Language::JavaScript => &JS,
        Language::Python => &PY,
        Language::Java => &JAVA,
        Language::CSharp => &CSHARP,
        Language::Ruby => &RUBY,
        Language::C => &C,
        Language::Cpp => &CPP,
        Language::Sql => &SQL,
        Language::Markdown => &MD,
    }
}

fn compiled(language: Language) -> &'static Compiled {
    cell(language).get_or_init(|| {
        let query = Query::new(&language.grammar(), query_source(language))
            .unwrap_or_else(|e| panic!("{}: tags.scm failed to compile: {e}", language.name()));
        Compiled { query }
    })
}

/// What an "outer" capture (everything but `@name`) means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawKind {
    /// A definition of the given kind.
    Def(DefKind),
    /// A reference: a call, an invocation, a table use.
    Ref,
    /// An import, a `use`, a `require`, an `#include`.
    Import,
}

/// One capture pair from a query match: the outer node and, when the
/// pattern captured one, the `@name` node.
pub(crate) struct RawTag<'tree> {
    /// The node the outer capture (`@definition.*`, `@reference.*`, or
    /// `@import`) matched.
    pub node: Node<'tree>,
    /// The node the `@name` capture matched, when the pattern had one.
    /// Every `Def` and `Ref` tag carries one; an `Import` tag may not.
    pub name_node: Option<Node<'tree>>,
    /// What kind of tag this is.
    pub kind: RawKind,
}

/// Maps a capture name (for example `"definition.function"`) onto a
/// [`RawKind`], or `None` for `"name"` and for any capture this module does
/// not recognise.
fn classify_capture(capture_name: &str) -> Option<RawKind> {
    if capture_name == "import" {
        return Some(RawKind::Import);
    }
    if capture_name.starts_with("reference.") {
        return Some(RawKind::Ref);
    }
    let suffix = capture_name.strip_prefix("definition.")?;
    let kind = match suffix {
        "function" => DefKind::Function,
        "method" => DefKind::Method,
        "class" => DefKind::Class,
        "interface" => DefKind::Interface,
        "enum" => DefKind::Enum,
        "type" => DefKind::TypeAlias,
        "module" => DefKind::Module,
        "constant" => DefKind::Constant,
        "variable" => DefKind::Variable,
        "macro" => DefKind::Macro,
        "section" => DefKind::Section,
        _ => return None,
    };
    Some(RawKind::Def(kind))
}

/// Runs `language`'s `tags.scm` against `tree` and returns every tag it
/// finds, in match order (not yet sorted; callers sort by [`super::types::Span`]
/// before returning anything, per Rule 32).
pub(crate) fn run<'tree>(
    language: Language,
    tree: &'tree Tree,
    source: &[u8],
) -> Vec<RawTag<'tree>> {
    let compiled = compiled(language);
    let mut cursor = QueryCursor::new();
    let capture_names = compiled.query.capture_names();
    let mut out = Vec::new();
    let mut matches = cursor.matches(&compiled.query, tree.root_node(), source);
    while let Some(m) = matches.next() {
        let mut outer: Option<(Node<'tree>, RawKind)> = None;
        let mut name_node: Option<Node<'tree>> = None;
        for capture in m.captures {
            let capture_name = capture_names[capture.index as usize];
            if capture_name == "name" {
                name_node = Some(capture.node);
            } else if let Some(kind) = classify_capture(capture_name) {
                outer = Some((capture.node, kind));
            }
        }
        if let Some((node, kind)) = outer {
            out.push(RawTag {
                node,
                name_node,
                kind,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every grammar's `tags.scm` must at least compile. A query error
    /// (an unknown node kind, an unknown field) panics on first use per
    /// grammar; running every grammar once here catches a broken query at
    /// test time rather than the first time a file of that language is
    /// analysed.
    #[test]
    fn every_grammar_compiles_its_tags_query() {
        for language in Language::ALL {
            let _ = compiled(language);
        }
    }
}
