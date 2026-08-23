//! The grammar for one supported language.

use std::ffi::OsStr;
use std::path::Path;

/// A language that the syntax stage can parse.
///
/// F1 supports exactly these thirteen grammars. See F1, "Do" item 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// Rust source.
    Rust,
    /// Go source.
    Go,
    /// TypeScript source, without JSX syntax.
    TypeScript,
    /// TypeScript source with JSX syntax.
    Tsx,
    /// JavaScript source.
    JavaScript,
    /// Python source.
    Python,
    /// Java source.
    Java,
    /// C# source.
    CSharp,
    /// Ruby source.
    Ruby,
    /// C source.
    C,
    /// C++ source.
    Cpp,
    /// SQL source.
    Sql,
    /// Markdown source.
    Markdown,
}

impl Language {
    /// Returns the `tree-sitter` grammar for this language.
    #[must_use]
    pub fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Ruby => tree_sitter_ruby::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Sql => tree_sitter_sequel::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }

    /// Returns the stable name of this language, for example `"rust"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Ruby => "ruby",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Sql => "sql",
            Self::Markdown => "markdown",
        }
    }

    /// Detects the language of `path` from its extension.
    ///
    /// Returns `None` when the extension belongs to no supported grammar,
    /// including when `path` has no extension. A `.h` header is treated as
    /// C, following the common convention; a project that keeps C++ headers
    /// under `.h` sees those files skipped rather than mis-parsed as C.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(OsStr::to_str)?;
        let language = match extension.to_ascii_lowercase().as_str() {
            "rs" => Self::Rust,
            "go" => Self::Go,
            "ts" | "mts" | "cts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "py" | "pyi" => Self::Python,
            "java" => Self::Java,
            "cs" => Self::CSharp,
            "rb" => Self::Ruby,
            "c" | "h" => Self::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Self::Cpp,
            "sql" => Self::Sql,
            "md" | "markdown" => Self::Markdown,
            _ => return None,
        };
        Some(language)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_every_supported_extension() {
        let cases: &[(&str, Language)] = &[
            ("a.rs", Language::Rust),
            ("a.go", Language::Go),
            ("a.ts", Language::TypeScript),
            ("a.tsx", Language::Tsx),
            ("a.js", Language::JavaScript),
            ("a.jsx", Language::JavaScript),
            ("a.py", Language::Python),
            ("a.java", Language::Java),
            ("a.cs", Language::CSharp),
            ("a.rb", Language::Ruby),
            ("a.c", Language::C),
            ("a.h", Language::C),
            ("a.cpp", Language::Cpp),
            ("a.hpp", Language::Cpp),
            ("a.sql", Language::Sql),
            ("a.md", Language::Markdown),
        ];
        for (name, expected) in cases {
            assert_eq!(
                Language::from_path(Path::new(name)),
                Some(*expected),
                "{name}"
            );
        }
    }

    #[test]
    fn returns_none_for_an_unsupported_or_missing_extension() {
        assert_eq!(Language::from_path(Path::new("a.json")), None);
        assert_eq!(Language::from_path(Path::new("README")), None);
        assert_eq!(Language::from_path(Path::new(".gitignore")), None);
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(Language::from_path(Path::new("a.RS")), Some(Language::Rust));
        assert_eq!(
            Language::from_path(Path::new("a.Py")),
            Some(Language::Python)
        );
    }

    #[test]
    fn every_grammar_loads_into_a_parser() {
        for language in [
            Language::Rust,
            Language::Go,
            Language::TypeScript,
            Language::Tsx,
            Language::JavaScript,
            Language::Python,
            Language::Java,
            Language::CSharp,
            Language::Ruby,
            Language::C,
            Language::Cpp,
            Language::Sql,
            Language::Markdown,
        ] {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&language.grammar())
                .unwrap_or_else(|e| panic!("{}: {e}", language.name()));
        }
    }
}
