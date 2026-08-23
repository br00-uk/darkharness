//! `dark session`: lists, replays, and continues recorded sessions
//! (task unit `A1`).
//!
//! A session is a directory under `$DARK_HOME/sessions/<ulid>/` holding
//! `transcript.jsonl`: one JSON object per [`dark_contract::Event`], in
//! the order the events happened. That file is the whole record — a
//! session has no other state — so every action here is a read of it,
//! and none of them needs a model or a network.
//!
//! # `resume` and what it can honestly do
//!
//! [`dark_core::session::Session::replay`] rebuilds a session's messages
//! from its transcript, which is what continuing a conversation needs.
//! Handing those messages to a live turn also needs a loaded model, and
//! the terminal application to show it in — so `resume` prints what it
//! rebuilt and names that gap, rather than pretending to continue a
//! conversation it cannot yet display.

use std::path::Path;

use anyhow::{Context as _, Result};
use dark_contract::{Event, Role};
use dark_core::session::{read_events, transcript_path};
use ulid::Ulid;

use crate::SessionAction;

/// Runs the `dark session` subcommand named by `action`.
pub(crate) fn run_command(action: SessionAction) -> Result<()> {
    match action {
        SessionAction::List => list(),
        // `dark session replay` and `dark replay` name the same action.
        // One implementation, in `crate::replay`, answers both.
        SessionAction::Replay { session } => crate::replay::run_command(&session),
        SessionAction::Resume { session } => resume(&session),
    }
}

/// One recorded session, as its transcript describes it.
#[derive(Debug)]
struct Recorded {
    /// The session identifier, which names its directory.
    id: Ulid,
    /// The repository the session worked in, from its `SessionStart`.
    root: Option<String>,
    /// The branch that was checked out, when the transcript recorded one.
    branch: Option<String>,
    /// How many turns the session ran to completion.
    turns: usize,
    /// The first thing the person asked, shortened for a list.
    opening: Option<String>,
}

/// How much of the opening message a listing shows.
const OPENING_WIDTH: usize = 60;

/// Shortens `text` to [`OPENING_WIDTH`] characters, on a character
/// boundary, with an ellipsis when it was cut.
///
/// Counts characters rather than bytes: cutting a multi-byte character in
/// half would produce invalid text in the middle of a listing.
fn shorten(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim();

    if trimmed.chars().count() <= OPENING_WIDTH {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(OPENING_WIDTH - 1).collect();
    format!("{kept}…")
}

/// Reads one session's transcript into a [`Recorded`].
async fn describe(sessions_root: &Path, id: Ulid) -> Result<Recorded> {
    let events = read_events(sessions_root, id)
        .await
        .map_err(crate::contract_error)?;

    let mut recorded = Recorded {
        id,
        root: None,
        branch: None,
        turns: 0,
        opening: None,
    };

    for event in &events {
        match event {
            Event::SessionStart { root, branch, .. } => {
                recorded.root = Some(root.display().to_string());
                recorded.branch.clone_from(branch);
            }
            Event::UserMessage { text, .. } => {
                if recorded.opening.is_none() {
                    recorded.opening = Some(shorten(text));
                }
            }
            Event::TurnEnd { .. } => recorded.turns += 1,
            _ => {}
        }
    }
    Ok(recorded)
}

/// Returns every session identifier under `sessions_root`, newest last.
///
/// A ULID leads with its millisecond timestamp, so sorting the directory
/// names puts the sessions in time order with no clock read and no file
/// timestamp. Two sessions started inside the same millisecond sort by
/// their random tail rather than their true order, which no listing this
/// command produces would notice.
///
/// A directory whose name parses as a ULID but does not print back as
/// itself is skipped. `Ulid::from_string` accepts a string whose leading
/// character is outside the range a real timestamp reaches and
/// normalises it, so such a name would be listed under an identifier
/// that names no directory — and every later read of it, including
/// `dark replay`, would fail to find the transcript. Nothing this
/// harness writes has such a name.
fn session_ids(sessions_root: &Path) -> Result<Vec<Ulid>> {
    if !sessions_root.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(sessions_root)
        .with_context(|| format!("could not read {}", sessions_root.display()))?;

    let mut ids: Vec<Ulid> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| {
            let id = Ulid::from_string(&name).ok()?;
            (id.to_string() == name).then_some(id)
        })
        .collect();
    ids.sort_unstable();
    Ok(ids)
}

/// Runs `dark session list`.
fn list() -> Result<()> {
    let sessions_root = crate::dark_home().join("sessions");
    let ids = session_ids(&sessions_root)?;

    if ids.is_empty() {
        println!("no session is recorded under {}.", sessions_root.display());
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not start the transcript reader")?;

    for id in ids {
        // A transcript that will not read is reported on its own line
        // rather than stopping the listing: one corrupt session must not
        // hide the others.
        match runtime.block_on(describe(&sessions_root, id)) {
            Ok(recorded) => println!("{}", render(&recorded)),
            Err(err) => println!("{id}  (unreadable: {err})"),
        }
    }
    Ok(())
}

/// Renders one listing line.
fn render(recorded: &Recorded) -> String {
    use std::fmt::Write as _;

    let mut line = format!("{}  {} turn(s)", recorded.id, recorded.turns);
    if let Some(branch) = &recorded.branch {
        let _ = write!(line, "  [{branch}]");
    }
    if let Some(opening) = &recorded.opening {
        let _ = write!(line, "  {opening}");
    } else if let Some(root) = &recorded.root {
        let _ = write!(line, "  {root}");
    }
    line
}

/// Runs `dark session resume <session>`.
fn resume(session: &str) -> Result<()> {
    let id = Ulid::from_string(session)
        .map_err(|err| anyhow::anyhow!("{session:?} is not a valid session identifier: {err}"))?;
    let sessions_root = crate::dark_home().join("sessions");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not start the transcript reader")?;

    let root = crate::repo_root()?;
    let session_state = runtime
        .block_on(dark_core::session::Session::replay(
            &sessions_root,
            id,
            root,
        ))
        .map_err(crate::contract_error)?;

    let counts = message_counts(&session_state.messages);
    println!(
        "session {id}: rebuilt {} message(s) from {}",
        session_state.messages.len(),
        transcript_path(&sessions_root, id).display(),
    );
    println!(
        "  {} from the person, {} from the model, {} tool replies",
        counts.user, counts.assistant, counts.tool,
    );
    println!();
    println!(
        "Continuing this conversation in a live turn is not wired yet: it needs the terminal \
         application to take a rebuilt conversation as its starting point, which `dark` with no \
         subcommand does not do. Run dark replay {id} to watch it back."
    );
    Ok(())
}

/// How many messages of each role a rebuilt conversation holds.
#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    /// Messages the person sent.
    user: usize,
    /// Messages the model produced.
    assistant: usize,
    /// Replies to tool calls.
    tool: usize,
}

/// Counts the messages of each role.
fn message_counts(messages: &[dark_contract::Message]) -> Counts {
    let mut counts = Counts::default();
    for message in messages {
        match message.role {
            Role::User => counts.user += 1,
            Role::Assistant => counts.assistant += 1,
            Role::Tool => counts.tool += 1,
            Role::System => {}
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use dark_contract::Message;

    use super::*;

    #[test]
    fn no_sessions_directory_lists_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            session_ids(&home.path().join("sessions"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn sessions_are_listed_oldest_first() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();

        // Explicit timestamps a second apart, rather than three calls to
        // `Ulid::new()`: within one millisecond a ULID's order comes from
        // its random tail, not the clock, so three made in a row are not
        // necessarily ordered.
        let first = Ulid::from_parts(1_700_000_000_000, 1);
        let second = Ulid::from_parts(1_700_000_001_000, 1);
        let third = Ulid::from_parts(1_700_000_002_000, 1);

        // Written out of order on purpose: the listing sorts, it does not
        // rely on what the filesystem hands back.
        for id in [third, first, second] {
            std::fs::create_dir_all(sessions.join(id.to_string())).unwrap();
        }

        assert_eq!(session_ids(&sessions).unwrap(), vec![first, second, third]);
    }

    #[test]
    fn a_directory_that_is_not_a_ulid_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        std::fs::create_dir_all(sessions.join("not-a-session")).unwrap();
        let id = Ulid::new();
        std::fs::create_dir_all(sessions.join(id.to_string())).unwrap();

        assert_eq!(session_ids(&sessions).unwrap(), vec![id]);
    }

    #[test]
    fn a_name_that_parses_but_does_not_print_back_as_itself_is_skipped() {
        // `Ulid::from_string` accepts a leading character outside the
        // range a real timestamp reaches and normalises it, so this name
        // would otherwise be listed under an identifier that names no
        // directory, and every read of it would fail.
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        let overflowing = "MJ7985CWRVM5VC5EB0TB252FSQ";
        std::fs::create_dir_all(sessions.join(overflowing)).unwrap();

        let parsed = Ulid::from_string(overflowing).expect("this name does parse");
        assert_ne!(
            parsed.to_string(),
            overflowing,
            "this test is pointless if the name round-trips"
        );
        assert!(
            session_ids(&sessions).unwrap().is_empty(),
            "a name that does not round-trip names no readable session"
        );
    }

    #[test]
    fn a_short_opening_is_left_alone() {
        assert_eq!(shorten("add a health check"), "add a health check");
    }

    #[test]
    fn a_long_opening_is_cut_with_an_ellipsis() {
        let long = "a".repeat(100);
        let shortened = shorten(&long);
        assert_eq!(shortened.chars().count(), OPENING_WIDTH);
        assert!(shortened.ends_with('…'));
    }

    #[test]
    fn a_multi_byte_opening_is_cut_on_a_character_boundary() {
        // Every character here is three bytes, so a byte-wise cut would
        // produce invalid text.
        let long = "日".repeat(100);
        let shortened = shorten(&long);
        assert_eq!(shortened.chars().count(), OPENING_WIDTH);
    }

    #[test]
    fn a_newline_in_the_opening_never_breaks_the_listing_line() {
        let shortened = shorten("first line\nsecond line");
        assert!(
            !shortened.contains('\n'),
            "a listing line must stay one line: {shortened:?}"
        );
    }

    #[test]
    fn message_counts_separate_the_roles() {
        let messages = vec![
            Message::text(Role::System, "prefix"),
            Message::text(Role::User, "do it"),
            Message::text(Role::Assistant, "done"),
            Message::tool_reply("call-0", "output"),
        ];

        assert_eq!(
            message_counts(&messages),
            Counts {
                user: 1,
                assistant: 1,
                tool: 1,
            }
        );
    }

    #[test]
    fn a_listing_line_names_the_session_and_its_turns() {
        let id = Ulid::new();
        let line = render(&Recorded {
            id,
            root: Some("/repo".to_owned()),
            branch: Some("main".to_owned()),
            turns: 3,
            opening: Some("add a health check".to_owned()),
        });

        assert!(line.contains(&id.to_string()), "line: {line}");
        assert!(line.contains("3 turn(s)"), "line: {line}");
        assert!(line.contains("main"), "line: {line}");
        assert!(line.contains("add a health check"), "line: {line}");
    }

    #[test]
    fn a_listing_line_falls_back_to_the_repository_with_no_opening() {
        let line = render(&Recorded {
            id: Ulid::new(),
            root: Some("/repo".to_owned()),
            branch: None,
            turns: 0,
            opening: None,
        });

        assert!(line.contains("/repo"), "line: {line}");
    }
}
