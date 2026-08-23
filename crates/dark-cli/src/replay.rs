//! `dark replay <session>`: replays a recorded session through the
//! terminal application shell.
//!
//! [`dark_core::session::read_events`] reads
//! `$DARK_HOME/sessions/<ulid>/transcript.jsonl` (Section 5.3), and
//! [`dark_tui::replay`] drives the same [`dark_tui::app::App`] a live
//! session drives, one event at a time. When standard output is a real
//! terminal this command runs the live loop
//! ([`dark_tui::replay::run`]), so a person can watch the replay and use
//! every key the live shell answers. When it is not — piped output, a
//! script, continuous integration — [`dark_tui::replay::Player::play_to_end`]
//! drains the whole recording with no terminal at all, and this command
//! prints a one-line summary instead. Both paths read the transcript from
//! disk; neither opens a network connection.

use std::io::{self, IsTerminal, Stdout};

use dark_contract::Event;
use dark_core::session::{read_events, transcript_path};
use dark_tui::app::App;
use dark_tui::replay::{Mode, Player, Recording, run as run_live};
use dark_tui::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ulid::Ulid;

/// Parses `session` as a ULID.
fn parse_session_id(session: &str) -> anyhow::Result<Ulid> {
    Ulid::from_string(session)
        .map_err(|err| anyhow::anyhow!("{session:?} is not a valid session identifier: {err}"))
}

/// Reads and rebuilds the events for `id` under `sessions_root`, on a
/// small single-threaded runtime.
///
/// `dark-cli` otherwise has no async code; this mirrors
/// `stats::block_on_read`'s reasoning exactly — nothing here needs to
/// interleave with anything else, so the smallest runtime that can drive
/// one future to completion is enough.
///
/// # Errors
///
/// Returns [`dark_contract::ErrCode::SessionNotFound`] when no transcript
/// exists for `id`. Returns an error when the transcript cannot be read or
/// contains corrupt, non-final-line JSON.
fn block_on_read(sessions_root: &std::path::Path, id: Ulid) -> anyhow::Result<Vec<Event>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|err| anyhow::anyhow!("could not start the transcript reader: {err}"))?;
    runtime
        .block_on(read_events(sessions_root, id))
        .map_err(crate::contract_error)
}

/// Sets up a real terminal for the live replay loop: raw mode and the
/// alternate screen.
fn init_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

/// Restores the terminal that [`init_terminal`] set up, whether or not the
/// live loop returned an error.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
}

/// Runs the live replay loop against a real terminal, restoring the
/// terminal afterwards whether or not the loop itself failed.
fn run_live_replay(recording: Recording) -> anyhow::Result<()> {
    let mut terminal = init_terminal()?;
    let mut app = App::new(Theme::detect());

    let outcome = run_live(&mut terminal, &mut app, recording, Mode::Play(1.0));
    let restored = restore_terminal(&mut terminal);

    outcome?;
    restored?;
    Ok(())
}

/// Drains the whole recording with no terminal, and prints a one-line
/// summary — the path this command takes when standard output is not a
/// terminal (piped output, a script, continuous integration).
fn run_headless_replay(recording: Recording) {
    let event_count = recording.len();
    let mut app = App::new(Theme::detect());
    let mut player = Player::new(recording);
    player.play_to_end(&mut app);

    let header = app.header();
    print!("replayed {event_count} event(s)");
    if let Some(session_id) = &header.session_id {
        print!(", session {session_id}");
    }
    if let Some(repo_root) = &header.repo_root {
        print!(", repo {}", repo_root.display());
    }
    if let Some(branch) = &header.branch {
        print!(" ({branch})");
    }
    println!(".");
    if app.is_turn_active() {
        println!("note: the recording ends mid-turn (no TurnEnd event).");
    }
}

/// Runs `dark replay <session>`.
///
/// # Errors
///
/// Returns an error when `session` does not parse as a ULID, or when
/// [`read_events`] fails — most commonly
/// [`dark_contract::ErrCode::SessionNotFound`] when no transcript exists
/// for it.
pub(crate) fn run_command(session: &str) -> anyhow::Result<()> {
    let id = parse_session_id(session)?;
    let sessions_root = crate::dark_home().join("sessions");
    let events = block_on_read(&sessions_root, id)?;
    let recording = Recording::new(events);

    if io::stdout().is_terminal() {
        run_live_replay(recording)
    } else {
        run_headless_replay(recording);
        Ok(())
    }
}

// Referenced only so a reader can jump straight to the path this command
// reads from; every actual read goes through `read_events` above.
#[allow(dead_code)]
const _TRANSCRIPT_PATH_DOC_POINTER: fn(&std::path::Path, Ulid) -> std::path::PathBuf =
    transcript_path;

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{RoleClass, Usage};
    use dark_core::session::TranscriptWriter;
    use tempfile::TempDir;

    #[test]
    fn parse_session_id_accepts_a_valid_ulid() {
        let id = Ulid::new();
        assert_eq!(parse_session_id(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn parse_session_id_rejects_garbage() {
        let err = parse_session_id("not-a-ulid").unwrap_err();
        assert!(err.to_string().contains("not a valid session identifier"));
    }

    #[tokio::test]
    async fn block_on_read_reports_session_not_found_for_a_missing_transcript() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        // block_on_read builds its own runtime, so call it from a
        // synchronous context via spawn_blocking to avoid nesting runtimes.
        let sessions_root = tmp.path().to_path_buf();
        let err = tokio::task::spawn_blocking(move || block_on_read(&sessions_root, id))
            .await
            .unwrap()
            .unwrap_err();
        assert!(err.to_string().contains("E_SESSION_NOT_FOUND"));
    }

    #[tokio::test]
    async fn run_headless_replay_drains_a_recorded_transcript_without_a_terminal() {
        let tmp = TempDir::new().unwrap();
        let id = Ulid::new();
        let mut writer = TranscriptWriter::open(tmp.path(), id).await.unwrap();
        writer
            .record(&Event::SessionStart {
                id: id.to_string(),
                root: std::path::PathBuf::from("/repo"),
                branch: Some("main".to_owned()),
            })
            .await
            .unwrap();
        writer
            .record(&Event::TurnStart {
                turn: "t1".to_owned(),
                class: RoleClass::Worker,
                model: "test-model".to_owned(),
            })
            .await
            .unwrap();
        writer
            .record(&Event::TokenDelta {
                turn: "t1".to_owned(),
                text: "hello".to_owned(),
            })
            .await
            .unwrap();
        writer
            .record(&Event::TurnEnd {
                turn: "t1".to_owned(),
                usage: Usage::default(),
                wall_ms: 5,
            })
            .await
            .unwrap();
        drop(writer);

        let sessions_root = tmp.path().to_path_buf();
        let events = tokio::task::spawn_blocking(move || block_on_read(&sessions_root, id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(events.len(), 4);

        // The headless path prints to stdout and returns nothing to
        // assert on directly; running it to completion without panicking
        // is the property this test pins — `App` and `Player` already
        // carry their own coverage for the state this drives.
        run_headless_replay(Recording::new(events));
    }
}
