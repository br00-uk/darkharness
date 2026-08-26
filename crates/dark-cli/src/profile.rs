//! `.dark/profile.json`: which door out of discovery a repository took.
//!
//! `dark extend` and `dark refactor` each prepare a repository for an
//! agent, and `dark plan` then charts with that preparation in view. What
//! the two doors decided has to outlive the command that decided it, so it
//! is written down.
//!
//! This is deliberately small. It records the choice and enough to tell
//! whether the choice is still current — nothing that could be recomputed
//! from the analysis, which is already on disk beside it.

use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Which door was taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    /// Keep the language and the style; add to what is here.
    Extend,
    /// Change the language, the architecture, or both.
    Refactor,
}

/// What a door decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Profile {
    /// Which door.
    pub(crate) mode: Mode,
    /// The language the repository is written in now.
    pub(crate) language: Option<String>,
    /// The language it is being taken to, when that differs.
    pub(crate) target_language: Option<String>,
    /// The architectural pattern chosen, when one was.
    pub(crate) pattern: Option<String>,
    /// The tree this was decided against.
    ///
    /// A profile written for a tree that has since moved on is stale: the
    /// modules it summarised may not exist. Recorded so a reader can say
    /// so rather than quietly applying old advice.
    pub(crate) tree_sha: String,
}

impl Profile {
    /// The file a repository's profile lives in.
    pub(crate) fn path(root: &Path) -> std::path::PathBuf {
        root.join(".dark").join("profile.json")
    }

    /// Writes this profile under `root`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be written.
    pub(crate) fn write(&self, root: &Path) -> Result<()> {
        let path = Self::path(root);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        let mut text =
            serde_json::to_string_pretty(self).context("cannot serialise the profile")?;
        text.push('\n');
        std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
    }

    /// Reads the profile under `root`, when one exists.
    ///
    /// # Errors
    ///
    /// Returns an error when the file exists and cannot be read or parsed.
    pub(crate) fn read(root: &Path) -> Result<Option<Self>> {
        let path = Self::path(root);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("cannot parse {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(anyhow::anyhow!("cannot read {}: {err}", path.display())),
        }
    }

    /// Returns true when this profile was decided against `tree_sha`.
    pub(crate) fn is_current(&self, tree_sha: &str) -> bool {
        self.tree_sha == tree_sha
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn extend() -> Profile {
        Profile {
            mode: Mode::Extend,
            language: Some("Rust".to_owned()),
            target_language: None,
            pattern: None,
            tree_sha: "abc".to_owned(),
        }
    }

    #[test]
    fn a_profile_round_trips() {
        let dir = TempDir::new().unwrap();
        extend().write(dir.path()).unwrap();
        assert_eq!(Profile::read(dir.path()).unwrap(), Some(extend()));
    }

    #[test]
    fn a_repository_with_no_profile_reads_none() {
        let dir = TempDir::new().unwrap();
        assert_eq!(Profile::read(dir.path()).unwrap(), None);
    }

    #[test]
    fn a_profile_knows_whether_it_is_still_current() {
        assert!(extend().is_current("abc"));
        assert!(!extend().is_current("def"));
    }

    #[test]
    fn a_refactor_profile_records_both_languages() {
        let dir = TempDir::new().unwrap();
        let profile = Profile {
            mode: Mode::Refactor,
            language: Some("Python".to_owned()),
            target_language: Some("Rust".to_owned()),
            pattern: Some("service split".to_owned()),
            tree_sha: "abc".to_owned(),
        };
        profile.write(dir.path()).unwrap();
        let read = Profile::read(dir.path()).unwrap().expect("written");
        assert_eq!(read.mode, Mode::Refactor);
        assert_eq!(read.target_language.as_deref(), Some("Rust"));
        assert_eq!(read.pattern.as_deref(), Some("service split"));
    }

    #[test]
    fn a_corrupt_profile_is_an_error_not_a_silent_none() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".dark")).unwrap();
        std::fs::write(Profile::path(dir.path()), "not json").unwrap();
        assert!(Profile::read(dir.path()).is_err());
    }
}
