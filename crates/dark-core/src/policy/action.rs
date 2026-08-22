//! What the turn loop asks the policy to gate.

use std::path::PathBuf;

use dark_contract::ConfirmPrompt;

/// The kind of action a [`super::PolicyConfig`] value gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    /// Reading a file or another action with no lasting effect.
    Read,
    /// Writing to a file.
    Write,
    /// Running a command.
    Exec,
}

impl ActionKind {
    /// Returns the lowercase name, for example `"write"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Exec => "exec",
        }
    }
}

/// One action that [`super::Policy`] must allow, confirm, or deny.
///
/// Each variant carries the exact detail a person must see before they
/// approve it, never a summary. See task unit `A4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A read, or another action with no lasting effect.
    Read {
        /// What is being read, for example a file path.
        what: String,
    },
    /// A file write.
    Write {
        /// The file that changes.
        path: PathBuf,
        /// The exact unified diff.
        diff: String,
        /// Whether `path` resolves outside the repository root.
        ///
        /// The caller computes this, typically by canonicalising `path`
        /// against the repository root before it follows a symbolic link.
        /// [`super::Policy`] always denies a write with this set, regardless
        /// of the configured policy value. See Rule 34.
        outside_root: bool,
    },
    /// A command execution.
    Exec {
        /// The exact command line.
        command: String,
        /// The working directory the command runs in.
        cwd: PathBuf,
        /// Whether a shell interprets `command`.
        shell: bool,
    },
}

impl Action {
    /// Returns the [`ActionKind`] that a [`super::PolicyConfig`] value gates.
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::Read { .. } => ActionKind::Read,
            Self::Write { .. } => ActionKind::Write,
            Self::Exec { .. } => ActionKind::Exec,
        }
    }

    /// Builds the [`ConfirmPrompt`] a person must see to approve this action.
    ///
    /// A write carries its exact unified diff. A command carries its exact
    /// command line. Neither carries a summary. See Do step 3 of task unit
    /// `A4`.
    pub fn to_prompt(&self) -> ConfirmPrompt {
        match self {
            Self::Read { what } => ConfirmPrompt::Other {
                summary: "Read".to_owned(),
                detail: what.clone(),
            },
            Self::Write { path, diff, .. } => ConfirmPrompt::Write {
                path: path.clone(),
                diff: diff.clone(),
            },
            Self::Exec {
                command,
                cwd,
                shell,
            } => ConfirmPrompt::Exec {
                command: command.clone(),
                cwd: cwd.clone(),
                shell: *shell,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_matches_the_variant() {
        assert_eq!(
            Action::Read {
                what: "src/lib.rs".into()
            }
            .kind(),
            ActionKind::Read
        );
        assert_eq!(
            Action::Write {
                path: "a.rs".into(),
                diff: String::new(),
                outside_root: false,
            }
            .kind(),
            ActionKind::Write
        );
        assert_eq!(
            Action::Exec {
                command: "ls".into(),
                cwd: ".".into(),
                shell: false,
            }
            .kind(),
            ActionKind::Exec
        );
    }

    #[test]
    fn write_prompt_carries_the_exact_diff_not_a_summary() {
        let diff = "@@ -1,2 +1,2 @@\n-old line\n+new line\n";
        let action = Action::Write {
            path: PathBuf::from("src/main.rs"),
            diff: diff.to_owned(),
            outside_root: false,
        };
        match action.to_prompt() {
            ConfirmPrompt::Write { path, diff: shown } => {
                assert_eq!(path, PathBuf::from("src/main.rs"));
                assert_eq!(shown, diff);
            }
            other => panic!("unexpected prompt: {other:?}"),
        }
    }

    #[test]
    fn exec_prompt_carries_the_exact_command_not_a_summary() {
        let action = Action::Exec {
            command: "rm -rf build && cargo build --release".to_owned(),
            cwd: PathBuf::from("/repo"),
            shell: true,
        };
        match action.to_prompt() {
            ConfirmPrompt::Exec {
                command,
                cwd,
                shell,
            } => {
                assert_eq!(command, "rm -rf build && cargo build --release");
                assert_eq!(cwd, PathBuf::from("/repo"));
                assert!(shell);
            }
            other => panic!("unexpected prompt: {other:?}"),
        }
    }
}
