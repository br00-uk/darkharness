//! The types that the `extract` stage produces.
//!
//! [`FileSymbols`] is the per-file result: the imports a file resolves, the
//! symbols it defines, and the references it makes. Task unit `F3` reads
//! these types directly; see the crate-level documentation for the seam
//! this module forms.

use std::path::PathBuf;

/// A byte-and-position span inside a source file.
///
/// `tree_sitter::Range` carries the same information but derives neither
/// [`serde::Serialize`] nor a total order, and `F4` needs both: a stable
/// byte order to sort tags deterministically (Rule 32), and a serialisable
/// form to write to `.dark/explore/<tree-sha>.json`. [`Span`] is the small,
/// owned type that gives both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    /// The byte offset of the first byte in the span.
    pub start_byte: usize,
    /// The byte offset one past the last byte in the span.
    pub end_byte: usize,
    /// The zero-based line of the first byte.
    pub start_row: usize,
    /// The zero-based column, in bytes, of the first byte.
    pub start_column: usize,
    /// The zero-based line of the byte one past the end of the span.
    pub end_row: usize,
    /// The zero-based column, in bytes, of the byte one past the end.
    pub end_column: usize,
}

impl Span {
    /// Converts a `tree-sitter` node's range into a [`Span`].
    #[must_use]
    pub fn from_node(node: &tree_sitter::Node<'_>) -> Self {
        let start = node.start_position();
        let end = node.end_position();
        Self {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_row: start.row,
            start_column: start.column,
            end_row: end.row,
            end_column: end.column,
        }
    }
}

/// The category that one definition belongs to.
///
/// A grammar adapter (`src/extract/lang/`) maps its own node kinds onto this
/// shared vocabulary, so a caller in `F3` reasons about one set of kinds
/// rather than thirteen grammar-specific ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    /// A free function.
    Function,
    /// A function that belongs to a type: a method, an associated function.
    Method,
    /// A concrete product type: a class, a struct, a union.
    Class,
    /// An interface-shaped type: a trait, an interface, a protocol.
    ///
    /// [`Def::is_interface_like`] is set independently of this variant.
    /// `F3`'s table names some `Interface` items as the only interface-like
    /// kind for a language (Go, Java, C#) and, for others, admits an
    /// `Interface` alongside a `Class` that also qualifies (TypeScript's
    /// `abstract class`, Python's ABC subclass).
    Interface,
    /// An enumerated type.
    Enum,
    /// A named alias for another type.
    TypeAlias,
    /// A named grouping: a module, a namespace, a package.
    Module,
    /// A top-level constant binding.
    Constant,
    /// A top-level variable binding.
    Variable,
    /// A macro definition.
    Macro,
    /// A Markdown section, keyed by its heading text.
    Section,
}

/// How confidently a reference resolved to a definition.
///
/// See F2, "Do" items 3 to 5. Never invent a fourth value that the
/// extraction pass did not earn: an unresolved reference carries no
/// confidence at all (see [`Ref::confidence`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolutionConfidence {
    /// The reference resolved inside the same file, by walking `tree-sitter`
    /// scopes from the reference outward to the file's top level.
    Exact,
    /// The reference resolved through the file's import map: the name
    /// matched an imported symbol, and the import resolved to a repository
    /// path that defines a symbol with that name.
    ImportScoped,
    /// The reference resolved by exact name match against exactly one
    /// definition anywhere in the repository, with no local scope or import
    /// evidence connecting the two.
    ///
    /// A name that matches definitions in more than one file resolves to
    /// none of them: see F2, "Do not" — "do not report a guessed reference
    /// as resolved."
    NameOnly,
}

/// One definition that a file introduces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    /// The definition's name, as written in the source.
    pub name: String,
    /// The category this definition belongs to.
    pub kind: DefKind,
    /// The span of the definition, from its leading keyword or decorator
    /// through its closing brace (or, for a language with no braces, the end
    /// of its body).
    pub range: Span,
    /// `true` when the definition is visible outside the file that declares
    /// it.
    ///
    /// The rule differs by language: a `pub` keyword in Rust, a capitalised
    /// name in Go, an `export` in JavaScript and TypeScript, a `public`
    /// modifier in Java and C#, no leading underscore in Python. Ruby has no
    /// file-scoped export keyword, so every Ruby definition reports `true`;
    /// see the `ruby` grammar adapter's module documentation.
    pub exported: bool,
    /// `true` when a documentation comment (or, in Python, a docstring)
    /// immediately precedes the definition, with no blank line between.
    pub doc_present: bool,
    /// `true` when `F3`'s per-language table names this definition's shape
    /// as interface-like. See the table in F3, "Do" item 2.
    pub is_interface_like: bool,
}

/// One name that a file resolves from another module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The literal module specifier, as written in the source: a path, a
    /// package name, a `require` argument.
    pub raw: String,
    /// The span of the whole import statement.
    pub range: Span,
    /// The names this import brings into scope.
    ///
    /// Empty when the import brings in a whole module under one name (a
    /// Go package import, a Python `import a.b.c`) rather than named
    /// members: [`Import::module_binding`] carries that name instead.
    pub imported_names: Vec<String>,
    /// The local name bound to the imported module itself, for an import
    /// that binds the module rather than named members from it.
    pub module_binding: Option<String>,
    /// The repository path this import resolves to, when the extraction
    /// pass can derive it from the text of the import alone.
    ///
    /// `None` is a true, recorded answer, not a placeholder: F2, "Do not" —
    /// "do not report a guessed reference as resolved" applies here too. A
    /// package-qualified import that only a build system can place (a Go
    /// module path, a Java package, a C# `using`) stays `None` rather than
    /// guessing at a directory that merely looks like a match.
    pub resolved_to: Option<PathBuf>,
}

/// One use of a name that may or may not be a definition in this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// The name as written at the use site.
    pub name: String,
    /// The span of the name at the use site.
    pub range: Span,
    /// The definition this reference resolves to, when resolution found
    /// one. `None` means unresolved: F2, "Do" item 4 requires recording
    /// that plainly rather than guessing.
    pub resolved_to: Option<ResolvedSymbol>,
    /// The confidence behind `resolved_to`. `None` exactly when
    /// `resolved_to` is `None`.
    pub confidence: Option<ResolutionConfidence>,
}

/// A definition that a [`Ref`] or an S-graph edge points at.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResolvedSymbol {
    /// The repository path of the file that holds the definition.
    pub file: PathBuf,
    /// The index of the definition inside that file's
    /// [`FileSymbols::defs`], after F2's deterministic sort (Rule 32).
    pub def_index: usize,
}

/// Everything F2 extracted from one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSymbols {
    /// The file path, relative to the repository root.
    pub path: PathBuf,
    /// The language `tags.scm` query ran under.
    pub language: crate::syntax::Language,
    /// The imports this file resolves, in source order.
    pub imports: Vec<Import>,
    /// The definitions this file introduces, sorted by [`Span`] (byte
    /// order), then by name. The sort is F2's own: query match order is not
    /// specified to be stable across `tree-sitter` versions, so extraction
    /// never hands a caller that order directly. See Rule 32.
    pub defs: Vec<Def>,
    /// The references this file makes, sorted the same way as `defs`.
    pub refs: Vec<Ref>,
}
