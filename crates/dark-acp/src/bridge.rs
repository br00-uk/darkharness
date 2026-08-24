//! Translates between the protocol's vocabulary and this harness's.
//!
//! Everything here is a pure function of its input. That is deliberate:
//! it is the part of speaking to a foreign agent that can be tested
//! without one, and the part most likely to be wrong in a way that
//! matters — a permission ask shown as the wrong kind of action, or a
//! refusal read as an approval.
//!
//! # The one rule that shapes this module
//!
//! An answer this harness cannot express must never widen a permission.
//! Where a mapping is uncertain, it resolves towards refusing: see
//! [`chosen_option`], which picks nothing at all rather than guessing
//! which of the agent's options a person meant.

use std::path::PathBuf;

use dark_contract::{Allow, ConfirmPrompt};

/// What an agent asked permission to do, in the terms the protocol uses.
///
/// A protocol permission request carries a title, an optional kind, and
/// a list of options the client picks from. This is that, reduced to
/// what the mapping needs, so the mapping can be tested without building
/// a protocol message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionAsk {
    /// The one-line title the agent gave.
    pub title: String,
    /// The kind of action, when the agent named one: `edit`, `execute`,
    /// `read`, `delete`, `move`, `search`, `fetch`, or another word.
    pub kind: Option<String>,
    /// The path the action touches, when it names one.
    pub path: Option<PathBuf>,
    /// The exact diff, when the agent supplied one.
    pub diff: Option<String>,
    /// The exact command, when the action runs one.
    pub command: Option<String>,
    /// Anything further the agent said about the action.
    pub detail: String,
}

impl PermissionAsk {
    /// Builds an ask with only a title, for a request that carried
    /// nothing else.
    #[must_use]
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            kind: None,
            path: None,
            diff: None,
            command: None,
            detail: String::new(),
        }
    }
}

/// Turns an agent's permission request into the prompt this harness
/// shows.
///
/// The mapping is by what the request actually carries, not by the kind
/// word alone: an ask that carries a diff is a write however it is
/// labelled, and one that carries a command is an execution. That keeps
/// a person seeing the real change — task unit `A4`'s requirement that a
/// confirmation shows the exact diff and never a summary — even when an
/// agent labels its actions differently from this harness.
///
/// An ask that carries neither becomes [`ConfirmPrompt::Other`], which
/// shows the title and everything the agent said. That is weaker than a
/// diff, and it is the truth: there was no diff to show.
#[must_use]
pub fn to_prompt(ask: &PermissionAsk) -> ConfirmPrompt {
    if let (Some(path), Some(diff)) = (ask.path.as_ref(), ask.diff.as_ref()) {
        return ConfirmPrompt::Write {
            path: path.clone(),
            diff: diff.clone(),
        };
    }

    if let Some(command) = ask.command.as_ref() {
        return ConfirmPrompt::Exec {
            command: command.clone(),
            // The agent runs in the repository root this session opened.
            // A protocol permission request carries no working directory
            // of its own, so naming one here would be an invention.
            cwd: ask.path.clone().unwrap_or_default(),
            // The agent runs its own command; whether a shell reads it
            // is the agent's business and is not stated in the request.
            shell: false,
        };
    }

    ConfirmPrompt::Other {
        summary: ask.title.clone(),
        detail: detail_of(ask),
    }
}

/// Assembles everything known about an ask that has no diff and no
/// command, so the person deciding sees all of it.
fn detail_of(ask: &PermissionAsk) -> String {
    let mut lines = Vec::new();
    if let Some(kind) = &ask.kind {
        lines.push(format!("kind: {kind}"));
    }
    if let Some(path) = &ask.path {
        lines.push(format!("path: {}", path.display()));
    }
    if !ask.detail.is_empty() {
        lines.push(ask.detail.clone());
    }
    lines.join("\n")
}

/// One option an agent offered in a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Option_ {
    /// The identifier to send back when this option is chosen.
    pub id: String,
    /// What the option does, as the agent described it.
    pub name: String,
    /// The protocol's own classification: `allow_once`, `allow_always`,
    /// `reject_once`, or `reject_always`.
    pub kind: String,
}

/// Picks the option that carries out `allow`.
///
/// Returns `None` when the agent offered nothing that matches, which the
/// caller must treat as a refusal rather than a free choice — see the
/// module documentation. Selecting some other option because it was
/// first in the list would turn a person's "no" into a "yes", which is
/// the one mistake this mapping must never make.
#[must_use]
pub fn chosen_option(allow: Allow, options: &[Option_]) -> Option<&Option_> {
    let wanted: &[&str] = match allow {
        // "Always" falls back to "once": allowing a single action is
        // narrower than what was asked for, so it can never widen a
        // permission. The reverse fallback would.
        Allow::Always => &["allow_always", "allow_once"],
        Allow::Once => &["allow_once"],
        Allow::Deny => &["reject_once", "reject_always"],
    };

    wanted
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, kind: &str) -> Option_ {
        Option_ {
            id: id.to_owned(),
            name: id.to_owned(),
            kind: kind.to_owned(),
        }
    }

    fn all_four() -> Vec<Option_> {
        vec![
            option("a1", "allow_once"),
            option("a2", "allow_always"),
            option("r1", "reject_once"),
            option("r2", "reject_always"),
        ]
    }

    #[test]
    fn an_ask_with_a_diff_is_a_write_and_shows_the_diff() {
        let ask = PermissionAsk {
            path: Some(PathBuf::from("src/lib.rs")),
            diff: Some("@@ -1 +1 @@\n-a\n+b\n".to_owned()),
            ..PermissionAsk::titled("Edit src/lib.rs")
        };

        match to_prompt(&ask) {
            ConfirmPrompt::Write { path, diff } => {
                assert_eq!(path, PathBuf::from("src/lib.rs"));
                assert!(diff.contains("+b"), "the exact diff is shown: {diff}");
            }
            other => panic!("expected a write prompt, got {other:?}"),
        }
    }

    #[test]
    fn an_ask_with_a_diff_is_a_write_whatever_the_agent_calls_it() {
        // Task unit A4 wants the person to see the real change. An agent
        // that labels a diff-bearing action something unexpected must
        // not cost them that.
        let ask = PermissionAsk {
            kind: Some("mutate_the_codebase".to_owned()),
            path: Some(PathBuf::from("a.rs")),
            diff: Some("@@ diff @@".to_owned()),
            ..PermissionAsk::titled("something")
        };

        assert!(matches!(to_prompt(&ask), ConfirmPrompt::Write { .. }));
    }

    #[test]
    fn an_ask_with_a_command_is_an_execution() {
        let ask = PermissionAsk {
            command: Some("cargo test".to_owned()),
            ..PermissionAsk::titled("Run the tests")
        };

        match to_prompt(&ask) {
            ConfirmPrompt::Exec { command, .. } => assert_eq!(command, "cargo test"),
            other => panic!("expected an exec prompt, got {other:?}"),
        }
    }

    #[test]
    fn an_ask_with_neither_shows_everything_it_did_carry() {
        let ask = PermissionAsk {
            kind: Some("fetch".to_owned()),
            path: Some(PathBuf::from("https://example.invalid")),
            detail: "the agent wants to read a web page".to_owned(),
            ..PermissionAsk::titled("Fetch a page")
        };

        match to_prompt(&ask) {
            ConfirmPrompt::Other { summary, detail } => {
                assert_eq!(summary, "Fetch a page");
                assert!(detail.contains("fetch"), "detail: {detail}");
                assert!(detail.contains("example.invalid"), "detail: {detail}");
                assert!(detail.contains("web page"), "detail: {detail}");
            }
            other => panic!("expected an other prompt, got {other:?}"),
        }
    }

    #[test]
    fn a_diff_without_a_path_is_not_reported_as_a_write() {
        // `ConfirmPrompt::Write` promises a path. Inventing one would
        // tell a person a file changes that this harness cannot name.
        let ask = PermissionAsk {
            diff: Some("@@ diff @@".to_owned()),
            ..PermissionAsk::titled("Edit something")
        };

        assert!(matches!(to_prompt(&ask), ConfirmPrompt::Other { .. }));
    }

    #[test]
    fn allowing_once_picks_the_once_option() {
        let options = all_four();
        let chosen = chosen_option(Allow::Once, &options).expect("an option matches");
        assert_eq!(chosen.kind, "allow_once");
    }

    #[test]
    fn allowing_always_picks_the_always_option() {
        let options = all_four();
        let chosen = chosen_option(Allow::Always, &options).expect("an option matches");
        assert_eq!(chosen.kind, "allow_always");
    }

    #[test]
    fn denying_picks_a_refusal() {
        let options = all_four();
        let chosen = chosen_option(Allow::Deny, &options).expect("an option matches");
        assert!(chosen.kind.starts_with("reject"), "kind: {}", chosen.kind);
    }

    #[test]
    fn allowing_always_narrows_to_once_when_the_agent_offers_no_always() {
        // Narrowing is safe. The reverse would turn a single approval
        // into a standing one.
        let options = vec![option("a1", "allow_once"), option("r1", "reject_once")];
        let chosen = chosen_option(Allow::Always, &options).expect("an option matches");
        assert_eq!(chosen.kind, "allow_once");
    }

    #[test]
    fn allowing_once_never_widens_to_always() {
        let options = vec![option("a2", "allow_always"), option("r1", "reject_once")];
        assert!(
            chosen_option(Allow::Once, &options).is_none(),
            "a single approval must never become a standing one"
        );
    }

    #[test]
    fn a_refusal_with_no_refusing_option_chooses_nothing() {
        // The caller cancels instead. Picking an allow option here would
        // turn a person's "no" into a "yes".
        let options = vec![option("a1", "allow_once"), option("a2", "allow_always")];
        assert!(
            chosen_option(Allow::Deny, &options).is_none(),
            "no option here carries out a refusal"
        );
    }

    #[test]
    fn no_options_at_all_chooses_nothing_for_every_answer() {
        for allow in [Allow::Once, Allow::Always, Allow::Deny] {
            assert!(chosen_option(allow, &[]).is_none(), "answer: {allow:?}");
        }
    }
}
