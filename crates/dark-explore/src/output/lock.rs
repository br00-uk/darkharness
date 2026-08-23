//! The `.lock` file: task unit `F4`, "Do" item 2.
//!
//! `.dark/explore/<tree-sha>.lock` sits beside the report and answers "was
//! this the tool, the configuration, and the grammars I think it was?"
//! without a reader having to re-run the analysis to find out.
//!
//! # `grammar_versions`
//!
//! F1 supports exactly thirteen languages (`crate::syntax::Language`), each
//! backed by one of the `tree-sitter-*` crates `crates/dark-explore/Cargo.toml`
//! pins — except TypeScript and TSX, which share `tree-sitter-typescript`
//! (one crate, two grammars). This module cannot read a dependency's
//! version from inside the crate that depends on it — `Cargo.toml` is not
//! available at run time, and `CARGO_PKG_VERSION` names only this crate's
//! own version — so [`grammar_versions`] is a hand-maintained constant
//! table instead, one entry per *language* (thirteen entries, matching
//! [`crate::syntax::Language::name`]'s thirteen variants) rather than one
//! per crate (twelve, since the TypeScript crate covers two languages).
//! **Keep [`GRAMMAR_VERSIONS`] in step with `Cargo.toml` by hand**: nothing
//! checks that they still agree.

use std::collections::BTreeMap;

use serde::Serialize;

/// One language's grammar crate and the version `Cargo.toml` pins.
///
/// See the module documentation for why this is hand-maintained.
const GRAMMAR_VERSIONS: &[(&str, &str)] = &[
    ("c", "0.24.2"),
    ("cpp", "0.23.4"),
    ("csharp", "0.23.5"),
    ("go", "0.25.0"),
    ("java", "0.23.5"),
    ("javascript", "0.25.0"),
    ("markdown", "0.5.3"),
    ("python", "0.25.0"),
    ("ruby", "0.23.1"),
    ("rust", "0.24.2"),
    ("sql", "0.3.11"),
    ("tsx", "0.23.2"),
    ("typescript", "0.23.2"),
];

/// Returns the thirteen-language grammar version table. See the module
/// documentation.
#[must_use]
pub fn grammar_versions() -> BTreeMap<String, String> {
    GRAMMAR_VERSIONS
        .iter()
        .map(|(language, version)| ((*language).to_owned(), (*version).to_owned()))
        .collect()
}

/// `.dark/explore/<tree-sha>.lock`'s shape: task unit `F4`, "Do" item 2.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lock {
    /// `env!("CARGO_PKG_VERSION")` of `dark-explore` itself, at the moment
    /// this report was written.
    pub tool_version: String,
    /// The same configuration hash [`super::Document::config_hash`] carries,
    /// as lowercase hexadecimal, repeated here so a reader who has only the
    /// `.lock` file can still tell two reports apart without opening the
    /// larger one.
    pub config_hash: String,
    /// The grammar version table. See the module documentation.
    pub grammar_versions: BTreeMap<String, String>,
    /// The `BLAKE3` hash of the exact JSON bytes this stage wrote to
    /// `.dark/explore/<tree-sha>.json`, as lowercase hexadecimal.
    pub output_blake3: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_table_names_all_thirteen_languages() {
        let table = grammar_versions();
        assert_eq!(table.len(), 13, "F1 supports exactly thirteen languages");
        for language in [
            "rust",
            "go",
            "typescript",
            "tsx",
            "javascript",
            "python",
            "java",
            "csharp",
            "ruby",
            "c",
            "cpp",
            "sql",
            "markdown",
        ] {
            assert!(
                table.contains_key(language),
                "{language} is missing from the grammar version table"
            );
        }
    }

    #[test]
    fn typescript_and_tsx_share_one_crate_version() {
        let table = grammar_versions();
        assert_eq!(table["typescript"], table["tsx"]);
    }
}
