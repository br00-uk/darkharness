//! Settings for `AGENTS.md` resolution.
//!
//! The shape here mirrors the `[agents_md]` table that the harness
//! configuration file carries. `dark-config` owns the file and the table
//! around it; this module owns only the meaning of the keys inside it.

use serde::{Deserialize, Serialize};

use dark_contract::{ErrCode, Error, Result};

/// What the resolver does when the chain does not fit `budget_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum OnOverflow {
    /// Drop the nested file furthest from the working set, then truncate
    /// the repository-root file at a heading boundary, then warn naming
    /// each file and its token count. This is the only policy today.
    TruncateWarn,
}

/// Settings for the `[agents_md]` table.
///
/// ```toml
/// [agents_md]
/// enabled = true
/// budget_tokens = 1500
/// on_overflow = "truncate-warn"
/// fallback_names = ["CLAUDE.md", "GEMINI.md"]
/// honour_overrides = true
/// follow_imports = false
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentsMdConfig {
    /// Turns the resolver on. A disabled resolver returns an empty chain
    /// and never touches the tail either.
    pub enabled: bool,
    /// The token budget for the whole resolved chain.
    pub budget_tokens: usize,
    /// What to do when the chain exceeds `budget_tokens`.
    pub on_overflow: OnOverflow,
    /// File names to read in a directory that has no `AGENTS.md` and no
    /// `AGENTS.override.md`, tried in order. The resolver reads these
    /// files. It never writes to them.
    pub fallback_names: Vec<String>,
    /// Honours an `AGENTS.override.md` file. See
    /// [`FileKind::Override`](crate::chain::FileKind::Override).
    pub honour_overrides: bool,
    /// Follows an import directive inside an instruction file.
    ///
    /// The default is `false`. An import is an unbounded token cost and a
    /// path-traversal risk, so this crate never follows one, whatever this
    /// field holds. The field exists so a repository cannot silently opt
    /// into a behaviour that does not exist yet; a future task unit may
    /// implement it under a bounded, audited form.
    pub follow_imports: bool,
}

impl Default for AgentsMdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            budget_tokens: 1500,
            on_overflow: OnOverflow::TruncateWarn,
            fallback_names: vec!["CLAUDE.md".to_owned(), "GEMINI.md".to_owned()],
            honour_overrides: true,
            follow_imports: false,
        }
    }
}

impl AgentsMdConfig {
    /// Parses the `[agents_md]` table out of a TOML document.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::ToolInvalidArgs`] when `text` is not valid TOML,
    /// or does not carry an `[agents_md]` table that matches this shape.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Wrapper {
            agents_md: AgentsMdConfig,
        }
        let wrapper: Wrapper = toml::from_str(text).map_err(|err| {
            Error::new(
                ErrCode::ToolInvalidArgs,
                format!("invalid [agents_md] configuration: {err}"),
            )
        })?;
        Ok(wrapper.agents_md)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_the_documented_shape() {
        let config = AgentsMdConfig::default();
        assert!(config.enabled);
        assert_eq!(config.budget_tokens, 1500);
        assert_eq!(config.on_overflow, OnOverflow::TruncateWarn);
        assert_eq!(config.fallback_names, vec!["CLAUDE.md", "GEMINI.md"]);
        assert!(config.honour_overrides);
        assert!(!config.follow_imports);
    }

    #[test]
    fn from_toml_str_parses_the_documented_table() {
        let text = r#"
            [agents_md]
            enabled = true
            budget_tokens = 900
            on_overflow = "truncate-warn"
            fallback_names = ["CLAUDE.md", "GEMINI.md"]
            honour_overrides = true
            follow_imports = false
        "#;
        let config = AgentsMdConfig::from_toml_str(text).expect("valid config");
        assert_eq!(config.budget_tokens, 900);
    }

    #[test]
    fn from_toml_str_rejects_malformed_toml() {
        let err = AgentsMdConfig::from_toml_str("not = [valid").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolInvalidArgs);
    }
}
