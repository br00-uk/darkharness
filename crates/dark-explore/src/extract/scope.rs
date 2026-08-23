//! Lexical scope, computed structurally from the parse tree rather than
//! from a query capture.
//!
//! F2, "Do" item 3 asks extraction to "use tree-sitter scopes inside a
//! file." [`SCOPE_KINDS`] names, per language, the node kinds that
//! introduce a new lexical scope: a function body, a class body, a block.
//! [`enclosing_scope_id`] and [`scope_chain`] both walk a node's ancestors
//! against that table; nothing here inspects a query capture, so it works
//! the same way regardless of which `tags.scm` pattern found the node.

use tree_sitter::Node;

use crate::syntax::Language;

/// The node kinds that introduce a new lexical scope, for one language.
///
/// This is deliberately coarse: it does not need to model every construct
/// that a full compiler's scope resolver would (a `match` arm's bindings, a
/// Python comprehension's own scope), because F2 only tracks the
/// definitions `tags.scm` captures (functions, methods, types — not local
/// variables), and those live at a much coarser grain than a full scope
/// resolver needs to reach. See F2, "Do not" — "do not build a type
/// checker."
fn scope_kinds(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &[
            "function_item",
            "block",
            "closure_expression",
            "impl_item",
            "trait_item",
            "mod_item",
        ],
        Language::Go => &[
            "function_declaration",
            "method_declaration",
            "block",
            "func_literal",
        ],
        Language::TypeScript | Language::Tsx | Language::JavaScript => &[
            "function_declaration",
            "generator_function_declaration",
            "generator_function",
            "function_expression",
            "arrow_function",
            "method_definition",
            "statement_block",
            "class_body",
        ],
        Language::Python => &["function_definition", "class_definition", "block", "lambda"],
        Language::Java => &[
            "class_body",
            "interface_body",
            "enum_body",
            "method_declaration",
            "block",
        ],
        Language::CSharp => &["declaration_list", "block", "method_declaration"],
        Language::Ruby => &[
            "method",
            "singleton_method",
            "class",
            "module",
            "block",
            "do_block",
        ],
        Language::C | Language::Cpp => &[
            "function_definition",
            "compound_statement",
            "class_specifier",
            "struct_specifier",
            "namespace_definition",
        ],
        Language::Sql | Language::Markdown => &[],
    }
}

/// Returns the ids of every ancestor of `node` that introduces a new
/// lexical scope, innermost first, always ending with `root_id`.
///
/// `root_id` should be the id of the tree's root node: it stands for "the
/// file's own top-level scope," which every chain reaches eventually
/// because [`tree_sitter::Node::parent`] returns `None` at the root.
pub(crate) fn scope_chain(language: Language, node: Node<'_>, root_id: usize) -> Vec<usize> {
    let kinds = scope_kinds(language);
    let mut chain = Vec::new();
    let mut current = node.parent();
    while let Some(ancestor) = current {
        if kinds.contains(&ancestor.kind()) {
            chain.push(ancestor.id());
        }
        current = ancestor.parent();
    }
    if chain.last() != Some(&root_id) {
        chain.push(root_id);
    }
    chain
}

/// Returns the id of the innermost scope that encloses `node` — the scope
/// `node` is declared or used *in*, not any scope `node` itself introduces.
pub(crate) fn enclosing_scope_id(language: Language, node: Node<'_>, root_id: usize) -> usize {
    scope_chain(language, node, root_id)
        .into_iter()
        .next()
        .unwrap_or(root_id)
}
