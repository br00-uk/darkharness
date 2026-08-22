//! The provenance of one resolved value.

use std::path::PathBuf;

/// Where a resolved value came from.
///
/// The five variants match the five-source resolution order in the build
/// specification (Section 6, task unit `J2`): a later source in that list
/// overrides an earlier one. `dark config explain <key>` reports this
/// alongside the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The built-in default that the caller supplied to [`crate::resolve`].
    Default,
    /// `$DARK_HOME/config.toml`. Carries the file's full path.
    DarkHomeFile(PathBuf),
    /// `<repo>/.dark/config.toml`. Carries the file's full path.
    ProjectFile(PathBuf),
    /// An environment variable with the `DARK_` prefix. Carries the
    /// variable's name.
    EnvVar(String),
    /// A command-line flag. Carries the dotted key that the flag set.
    Flag(String),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default => write!(f, "built-in default"),
            Self::DarkHomeFile(path) => write!(f, "{} (dark home file)", path.display()),
            Self::ProjectFile(path) => write!(f, "{} (project file)", path.display()),
            Self::EnvVar(name) => write!(f, "environment variable {name}"),
            Self::Flag(key) => write!(f, "command-line flag ({key})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_kind_and_the_detail() {
        assert_eq!(Source::Default.to_string(), "built-in default");
        assert_eq!(
            Source::EnvVar("DARK_POLICY_WRITE".to_string()).to_string(),
            "environment variable DARK_POLICY_WRITE"
        );
        assert_eq!(
            Source::Flag("policy.write".to_string()).to_string(),
            "command-line flag (policy.write)"
        );
    }
}
