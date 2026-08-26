//! The in-session command table: what `/plan`, `/explore` and the rest do.
//!
//! # The bug this closes
//!
//! `dark_contract::Intent::Command` and `Intent::Submit` were handled by
//! the same match arm, so every slash command a person typed was sent to
//! the model as ordinary text. `/plan "add a health check"` did not chart
//! a map — it asked a language model to talk about charting one, and the
//! model, having no tool for it, obliged. Every command in the build
//! specification's in-session table behaved that way.
//!
//! A command is not a prompt. This module says which is which, and what
//! each one does.
//!
//! # Why the outcome is a value rather than an action
//!
//! [`dispatch`] returns an [`Outcome`] instead of running anything. Some
//! commands are answered entirely inside the shell (`/help` prints a
//! table), some change session state the caller owns (`/godark`), and some
//! genuinely are a prompt for the model (`/think`, until a later task unit
//! gives it its own path). The caller holds the session; this decides.

/// What the shell should do with a submission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Not a command. Send this text to the model as a turn.
    Prompt(String),
    /// A command answered here. Show this text; run no turn.
    Answered(String),
    /// A command the shell must act on itself.
    Act(Action),
    /// A command that exists in the build specification's table but has no
    /// implementation yet. Carries the name so the notice can say which.
    NotYet(String),
    /// A word starting with a slash that names no command.
    Unknown(String),
}

/// A command the shell acts on rather than answering in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Chart a map for this idea.
    Chart(String),
    /// Take a ticket from the newest map.
    Work(Option<String>),
    /// Analyse the repository.
    Explore,
    /// Show the seam report.
    Seams,
    /// Enter or leave dark mode.
    Dark(bool),
    /// Show the resident set.
    Residency,
    /// Compact the context now.
    Compact,
    /// Clear the conversation.
    Clear,
    /// Leave.
    Quit,
}

/// The in-session command table, as `PRD.md` Section 3.5 lists it.
///
/// The third column is what a person sees for `/help`, so it is written
/// for them rather than describing the implementation.
const TABLE: [(&str, &str, &str); 14] = [
    ("/plan", "<idea>", "Chart a map towards an idea."),
    ("/plan work", "[ticket]", "Take a ticket from the map."),
    ("/explore", "", "Analyse this repository."),
    ("/seams", "", "Show the seam report."),
    ("/docs", "<lib> <topic>", "Search the Lexicon."),
    ("/map", "", "Open the fog map."),
    ("/godark", "", "Block network egress."),
    ("/golight", "", "Allow network egress."),
    ("/residency", "", "Show what is in memory."),
    ("/model", "<class> <id>", "Override a role class."),
    ("/think", "on|off|auto", "Set the thinking mode."),
    ("/compact", "", "Compact the context now."),
    ("/clear", "", "Forget the conversation."),
    ("/quit", "", "Leave."),
];

/// Decides what `text` means.
///
/// Text that does not start with a slash is a prompt, always — a person
/// writing about a path or a fraction must never have it read as a
/// command.
pub(crate) fn dispatch(text: &str) -> Outcome {
    let trimmed = text.trim();
    if !trimmed.starts_with('/') {
        return Outcome::Prompt(text.to_owned());
    }

    let (head, rest) = split_first_word(trimmed);
    let rest = rest.trim();

    match head {
        "/help" => Outcome::Answered(help_text()),
        "/quit" | "/exit" => Outcome::Act(Action::Quit),
        "/clear" => Outcome::Act(Action::Clear),
        "/compact" => Outcome::Act(Action::Compact),
        "/godark" => Outcome::Act(Action::Dark(true)),
        "/golight" => Outcome::Act(Action::Dark(false)),
        "/residency" => Outcome::Act(Action::Residency),
        "/explore" => Outcome::Act(Action::Explore),
        "/seams" => Outcome::Act(Action::Seams),
        "/plan" => plan(rest),
        // Named in the specification's table, with no path behind them
        // yet. Saying so beats sending the words to the model, which
        // would answer as though it had done the thing.
        "/map" | "/docs" | "/model" | "/think" | "/ticket" | "/claim" | "/resolve" | "/fog"
        | "/lexicon" => Outcome::NotYet(head.to_owned()),
        _ => Outcome::Unknown(head.to_owned()),
    }
}

/// Reads `/plan`'s argument: `work` takes a ticket, anything else is an
/// idea to chart.
fn plan(rest: &str) -> Outcome {
    let (first, tail) = split_first_word(rest);
    if first == "work" {
        let ticket = tail.trim();
        return Outcome::Act(Action::Work(
            (!ticket.is_empty()).then(|| ticket.to_owned()),
        ));
    }
    if rest.is_empty() {
        return Outcome::Answered(
            "/plan needs an idea to chart a way towards. Try: /plan add a health check\n\
             /plan work takes the first takeable ticket from the newest map."
                .to_owned(),
        );
    }
    // A quoted idea is accepted because the specification writes it that
    // way (`/plan "<idea>"`), and typing the quotes should not put them
    // in the destination.
    Outcome::Act(Action::Chart(unquote(rest).to_owned()))
}

/// Splits `text` at its first run of whitespace.
fn split_first_word(text: &str) -> (&str, &str) {
    match text.find(char::is_whitespace) {
        Some(at) => (&text[..at], &text[at..]),
        None => (text, ""),
    }
}

/// Removes one matched pair of surrounding double quotes.
fn unquote(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or(trimmed)
}

/// Renders the `/help` table.
fn help_text() -> String {
    use std::fmt::Write as _;
    let width = TABLE
        .iter()
        .map(|(name, args, _)| name.len() + args.len() + usize::from(!args.is_empty()))
        .max()
        .unwrap_or(0);

    let mut out = String::from("in-session commands:\n");
    for (name, args, what) in TABLE {
        let call = if args.is_empty() {
            name.to_owned()
        } else {
            format!("{name} {args}")
        };
        let _ = writeln!(out, "  {call:<width$}  {what}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_a_prompt() {
        assert_eq!(
            dispatch("add a health check"),
            Outcome::Prompt("add a health check".to_owned())
        );
    }

    #[test]
    fn a_path_in_a_sentence_is_never_read_as_a_command() {
        // A person writing about a path or a fraction must not have it
        // taken as a command. Only a leading slash starts one.
        for text in [
            "look at src/main.rs",
            "roughly 1/2 of the files",
            "the ratio is 3/4",
        ] {
            assert!(
                matches!(dispatch(text), Outcome::Prompt(_)),
                "{text} must be a prompt"
            );
        }
    }

    #[test]
    fn plan_with_an_idea_charts() {
        assert_eq!(
            dispatch("/plan add a health check"),
            Outcome::Act(Action::Chart("add a health check".to_owned()))
        );
    }

    #[test]
    fn a_quoted_idea_loses_its_quotes() {
        // The specification writes `/plan "<idea>"`, so a person may type
        // the quotes. They must not reach the destination text.
        assert_eq!(
            dispatch("/plan \"offline pack format\""),
            Outcome::Act(Action::Chart("offline pack format".to_owned()))
        );
    }

    #[test]
    fn plan_work_takes_a_ticket() {
        assert_eq!(dispatch("/plan work"), Outcome::Act(Action::Work(None)));
        assert_eq!(
            dispatch("/plan work T-018"),
            Outcome::Act(Action::Work(Some("T-018".to_owned())))
        );
    }

    #[test]
    fn plan_with_no_argument_explains_itself() {
        let Outcome::Answered(text) = dispatch("/plan") else {
            panic!("/plan alone must answer, not chart nothing");
        };
        assert!(text.contains("needs an idea"), "{text}");
    }

    #[test]
    fn the_two_dark_mode_commands_are_opposites() {
        assert_eq!(dispatch("/godark"), Outcome::Act(Action::Dark(true)));
        assert_eq!(dispatch("/golight"), Outcome::Act(Action::Dark(false)));
    }

    #[test]
    fn help_names_every_command_in_the_table() {
        let Outcome::Answered(text) = dispatch("/help") else {
            panic!("/help answers");
        };
        for (name, _, _) in TABLE {
            assert!(text.contains(name), "{name} missing from /help:\n{text}");
        }
    }

    #[test]
    fn a_command_with_no_implementation_says_so_rather_than_becoming_a_prompt() {
        // This is the whole point. A command the harness cannot run must
        // never reach the model, which would answer as though it had.
        for text in ["/map", "/docs serde derive", "/think on"] {
            assert!(
                matches!(dispatch(text), Outcome::NotYet(_)),
                "{text} must report itself unbuilt"
            );
        }
    }

    #[test]
    fn an_unknown_command_is_named_back() {
        assert_eq!(
            dispatch("/nonsense"),
            Outcome::Unknown("/nonsense".to_owned())
        );
    }

    #[test]
    fn no_command_ever_silently_becomes_a_prompt() {
        // The defect in one assertion: anything starting with a slash is
        // answered, acted on, or reported — never passed through.
        for text in [
            "/plan x",
            "/plan work",
            "/explore",
            "/seams",
            "/godark",
            "/golight",
            "/residency",
            "/compact",
            "/clear",
            "/quit",
            "/help",
            "/map",
            "/docs a b",
            "/model worker x",
            "/think on",
            "/nonsense",
            "/",
        ] {
            assert!(
                !matches!(dispatch(text), Outcome::Prompt(_)),
                "{text} reached the model as prose"
            );
        }
    }

    #[test]
    fn leading_whitespace_does_not_hide_a_command() {
        assert_eq!(dispatch("   /explore"), Outcome::Act(Action::Explore));
    }
}
