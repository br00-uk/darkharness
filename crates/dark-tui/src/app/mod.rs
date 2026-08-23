//! The application shell: two panes, a command bar, and a function-key bar.
//!
//! This module renders [`dark_contract::Event`] values and produces
//! [`dark_contract::Intent`] values; it never reaches into the runtime
//! (Rule 14 in `CLAUDE.md` — `dark-tui` depends on `dark-contract` only).
//!
//! [`App`] is the state machine: [`App::apply_event`] folds in what the
//! harness reports, [`App::handle_key`] and [`App::handle_mouse`] fold in
//! what the person does, and [`render::render`] draws the result to a
//! [`ratatui::Frame`]. [`run`] wires the three together into the shell's
//! main loop, using [`bridge::try_recv`] to poll the event bus without a
//! `tokio` runtime — see that module's documentation for why.

pub mod bridge;
mod keys;
pub mod layout;
pub mod pane;
pub mod render;
pub mod state;
pub mod zone;

pub use pane::{Focus, LeftPane, RightPane};
pub use render::render;
pub use state::{
    App, Header, LagState, LastError, MIN_SIDE_BY_SIDE_COLUMNS, MIN_SIDE_BY_SIDE_ROWS,
    PendingConfirm,
};
pub use zone::{ZoneId, ZoneRegistry};

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use dark_contract::{EventRx, Intent};
use ratatui::Terminal;
use ratatui::backend::Backend;

use bridge::PollOutcome;

/// How long [`run`] waits for a terminal input event before checking the
/// event bus and redrawing. Sixteen milliseconds keeps the shell inside
/// roughly one 60 Hz frame; see task unit `H3`'s frame budget, which this
/// loop's cadence should not exceed even before that unit's own animation
/// lands.
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Drives the shell until the person quits.
///
/// Each pass: draws one frame, waits up to [`INPUT_POLL_INTERVAL`] for a
/// keyboard or mouse event, then drains whatever the event bus has
/// buffered. An [`Intent`] that either side produces goes to `intents`
/// immediately.
///
/// # Errors
///
/// Returns an error when the terminal fails to draw a frame or when reading
/// a `crossterm` input event fails.
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut EventRx,
    intents: &Sender<Intent>,
) -> std::io::Result<()> {
    while !app.should_quit() {
        app.tick(Instant::now());
        terminal.draw(|frame| render(app, frame))?;

        if ratatui::crossterm::event::poll(INPUT_POLL_INTERVAL)? {
            let intent = match ratatui::crossterm::event::read()? {
                ratatui::crossterm::event::Event::Key(key) => app.handle_key(key),
                ratatui::crossterm::event::Event::Mouse(mouse) => {
                    app.handle_mouse(mouse.column, mouse.row, mouse.kind)
                }
                ratatui::crossterm::event::Event::Resize(columns, rows) => {
                    app.set_size(columns, rows);
                    None
                }
                ratatui::crossterm::event::Event::FocusGained
                | ratatui::crossterm::event::Event::FocusLost
                | ratatui::crossterm::event::Event::Paste(_) => None,
            };
            if let Some(intent) = intent {
                // A shell with nobody listening on the other end — a
                // headless replay drive, for instance — is not an error;
                // see `dark_contract::EventTx::send`'s equivalent rule.
                let _ = intents.send(intent);
            }
        }

        loop {
            match bridge::try_recv(events) {
                PollOutcome::Event(received) => app.apply_event(received, Instant::now()),
                PollOutcome::Pending => break,
                PollOutcome::Closed => return Ok(()),
            }
        }
    }
    Ok(())
}
