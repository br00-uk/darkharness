//! The replay harness: drive the shell from a recorded transcript instead
//! of a live engine (task unit `H5`).
//!
//! The goal is in the task unit's own words: "Develop and test the
//! interface without the engine." A recorded session and a live one must
//! look the same on screen, so this module drives the same [`App`] a live
//! session drives, through the same entry points: [`App::apply_event`] and
//! [`App::tick`]. A replay that took a different route — poking at private
//! fields, or building its own copy of what [`App`] already does — would
//! stop proving anything about the real interface. See [`Player::step`].
//!
//! # Where the transcript comes from
//!
//! `dark-core`'s session module writes one JSON [`Event`] per line to
//! `$DARK_HOME/sessions/<id>/transcript.jsonl` (see
//! `dark_core::session::transcript`). `dark-tui` depends on `dark-contract`
//! only (Rule 14 in `CLAUDE.md`), so this module cannot call that reader
//! directly, and it does not carry its own copy of a JSON parser either:
//! `dark-tui`'s `Cargo.toml` declares `dark-contract` and `ratatui` and
//! nothing else, and a hand-rolled parser for [`Event`] would have to match
//! `serde`'s externally-tagged wire format field for field — including a
//! `serde_json::Value` inside [`dark_contract::ToolCall::args`] — a second
//! copy of logic `serde_json` already gets right, that this crate has no
//! dependency on and is not asked to add one.
//!
//! [`Recording`] therefore takes events already parsed, as plain data. The
//! caller that owns the file — `dark-cli`'s `replay` command, which already
//! depends on `dark-core` — reads the transcript and hands the events here.
//! This keeps the dependency rule intact and the parsing logic in the one
//! place that already has it right.
//!
//! # Determinism
//!
//! "A recorded transcript reproduces the same frames every time." [`App`]
//! reads the wall clock nowhere on its own — every time-based transition
//! takes an [`Instant`] as an argument (see task unit `H3`'s spring and the
//! dark-mode transition) — so a caller that always passes the same
//! sequence of instants gets the same sequence of frames. [`Player`] is
//! that caller: it keeps its own virtual clock, seeded once from a real
//! [`Instant`] and advanced afterwards only by a fixed, configuration-free
//! [`BASE_TICK`] per step. Nothing in [`Player::step`] or
//! [`Player::advance`] reads [`Instant::now`] again, so replaying the same
//! [`Recording`] twice — even at different [`Player::speed`] settings —
//! drives [`App`] through the identical sequence of relative time
//! offsets, and therefore the identical sequence of frames.
//!
//! [`Player::speed`] does not touch that virtual clock at all. It scales
//! only [`Player::wall_sleep`], the real pause a live playback loop —
//! [`run`] — waits between two steps. Speed is a courtesy to the person
//! watching, not an input to [`App`]; separating the two is what keeps
//! playback speed from ever being able to change a golden frame.

use std::io;
use std::time::{Duration, Instant};

use dark_contract::{Event, Received};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::app::{App, render};

/// The virtual time [`Player::step`] and [`Player::advance`] each add to
/// the clock they pass to [`App::tick`], at every speed.
///
/// This matches [`crate::app::run`]'s own redraw cadence
/// (`INPUT_POLL_INTERVAL`), so a turn replayed at speed `1.0` reaches every
/// time-based transition — the 400 millisecond dark-mode fade, task unit
/// `H3`'s spring — at the same relative progress a live session would have
/// shown it at.
pub const BASE_TICK: Duration = Duration::from_millis(16);

/// An ordered list of events, already parsed from a transcript.
///
/// Holds no timing of its own: `dark-core`'s transcript file records one
/// [`Event`] per line and nothing else, no timestamp among them (see this
/// module's documentation, "Where the transcript comes from"), so a
/// [`Recording`] carries only the order events happened in. [`Player`]
/// supplies the timing a replay needs, from its own virtual clock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recording {
    events: Vec<Event>,
}

impl Recording {
    /// Builds a recording from an ordered list of events.
    #[must_use]
    pub const fn new(events: Vec<Event>) -> Self {
        Self { events }
    }

    /// Returns the events, in the order they happened.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Returns the number of events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns true when the recording holds no events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl From<Vec<Event>> for Recording {
    fn from(events: Vec<Event>) -> Self {
        Self::new(events)
    }
}

impl FromIterator<Event> for Recording {
    fn from_iter<T: IntoIterator<Item = Event>>(iter: T) -> Self {
        Self::new(iter.into_iter().collect())
    }
}

/// Normalises a speed multiplier.
///
/// A [`Player`] must never divide by zero or stall forever, so a
/// multiplier that could cause either — zero, negative, `NaN`, or infinite
/// — falls back to `1.0` rather than reporting an error. None of the
/// `dark-contract` [`dark_contract::ErrCode`] taxonomy names "an
/// out-of-range replay speed" (and this crate does not own that taxonomy
/// to add one, see `CLAUDE.md`, "Change `dark-contract` between waves"), so
/// silently normalising, rather than manufacturing a code that does not
/// fit, is the correct choice for a cosmetic playback setting: a speed
/// this function normalises never changes which frames a replay produces,
/// only how long a live viewer waits between them.
fn normalise_speed(speed: f32) -> f32 {
    if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        1.0
    }
}

/// Drives an [`App`] through a [`Recording`], one event at a time.
///
/// [`Player`] owns the virtual clock a replay needs and nothing else: it
/// holds no reference to a terminal and draws no frame. [`run`] is the
/// thin loop that adds a terminal on top, for a person watching a replay
/// live; a test drives a [`Player`] directly instead, exactly as a golden
/// frame test drives [`App`] directly (see `golden_frames.rs`).
#[derive(Debug)]
pub struct Player {
    recording: Recording,
    cursor: usize,
    clock: Instant,
    speed: f32,
}

impl Player {
    /// Builds a player over `recording`, at speed `1.0`.
    #[must_use]
    pub fn new(recording: Recording) -> Self {
        Self::with_speed(recording, 1.0)
    }

    /// Builds a player over `recording`, at `speed` times a live session's
    /// pace. See [`Player::speed`] and [`Player::wall_sleep`] for what
    /// speed changes, and this module's documentation, "Determinism," for
    /// what it never changes.
    #[must_use]
    pub fn with_speed(recording: Recording, speed: f32) -> Self {
        Self {
            recording,
            cursor: 0,
            clock: Instant::now(),
            speed: normalise_speed(speed),
        }
    }

    /// Returns the recording this player is driving through.
    #[must_use]
    pub const fn recording(&self) -> &Recording {
        &self.recording
    }

    /// Returns the index of the next event [`Player::step`] applies.
    ///
    /// Equal to [`Recording::len`] once the player is done.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.cursor
    }

    /// Returns the number of events left to apply.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.recording.len() - self.cursor
    }

    /// Returns true once every event has been applied.
    ///
    /// True immediately for an empty [`Recording`].
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.cursor >= self.recording.len()
    }

    /// Returns the speed multiplier this player was built with, normalised
    /// by [`normalise_speed`].
    #[must_use]
    pub const fn speed(&self) -> f32 {
        self.speed
    }

    /// Returns how long a live playback loop should wait before the next
    /// step, at this player's speed.
    ///
    /// This is [`BASE_TICK`] divided by [`Player::speed`]: double the
    /// speed, half the wait. It plays no part in the virtual clock
    /// [`Player::step`] passes to [`App`] — see this module's
    /// documentation, "Determinism" — so it only ever changes how long a
    /// real person waits, never what a replay shows them once they do.
    #[must_use]
    pub fn wall_sleep(&self) -> Duration {
        BASE_TICK.div_f32(self.speed)
    }

    /// Advances the virtual clock by [`BASE_TICK`] and calls [`App::tick`].
    ///
    /// Applies no event and does not move [`Player::position`]. A step
    /// mode uses this to show an in-progress animation — the dark-mode
    /// fade, a spring settling — moving between two recorded events,
    /// exactly as [`crate::app::run`]'s own redraw loop calls
    /// [`App::tick`] once every pass regardless of whether an event
    /// arrived that pass.
    pub fn advance(&mut self, app: &mut App) {
        self.clock += BASE_TICK;
        app.tick(self.clock);
    }

    /// Applies the next recorded event to `app`, through
    /// [`App::apply_event`], then advances the clock exactly as
    /// [`Player::advance`] does.
    ///
    /// Returns the event that was applied, or `None` once
    /// [`Player::is_done`] is true, in which case `app` and the clock are
    /// left unchanged.
    pub fn step(&mut self, app: &mut App) -> Option<&Event> {
        let index = self.cursor;
        let event = self.recording.events.get(index)?.clone();
        app.apply_event(Received::Event(event), self.clock);
        self.cursor += 1;
        self.advance(app);
        self.recording.events.get(index)
    }

    /// Applies every remaining event, in order, through repeated
    /// [`Player::step`] calls.
    pub fn play_to_end(&mut self, app: &mut App) {
        while self.step(app).is_some() {}
    }
}

/// How [`run`] advances a [`Player`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    /// Advance automatically. The field is the speed multiplier: see
    /// [`Player::with_speed`].
    Play(f32),
    /// Advance only when the person presses [`STEP_KEY`].
    Step,
}

/// The key that advances one event in [`Mode::Step`].
///
/// Chosen to match the space bar's ordinary role of "go on" in a pager or
/// a slideshow. No binding in task unit `H1`'s key table (see
/// `crates/dark-tui/src/app/keys.rs`) already uses the space bar, so this
/// adds no conflict with the live shell's own bindings, which
/// [`run`] still honours for every other key — see the function
/// documentation.
pub const STEP_KEY: KeyCode = KeyCode::Char(' ');

/// Returns true when `key` is a press of [`STEP_KEY`].
fn is_step_key(key: &KeyEvent) -> bool {
    key.kind != KeyEventKind::Release && key.code == STEP_KEY && key.modifiers.is_empty()
}

/// Drives `app` from `recording` against a live terminal, until either the
/// recording finishes or the person quits.
///
/// Every keyboard and mouse event this loop reads goes to
/// [`App::handle_key`] or [`App::handle_mouse`], exactly as
/// [`crate::app::run`] handles them for a live session — a replay is still
/// the real interface, so a person can switch panes, expand thinking, or
/// quit while watching one. The one addition is [`STEP_KEY`] in
/// [`Mode::Step`], which this loop intercepts before `app` ever sees it,
/// to mean "apply the next recorded event" instead of an ordinary
/// keystroke.
///
/// In [`Mode::Play`], the loop waits [`Player::wall_sleep`] for a keyboard
/// or mouse event on each pass; if none arrives inside that wait, it
/// applies the next event on its own. In [`Mode::Step`], the loop waits
/// indefinitely for a key, since nothing should advance until the person
/// asks.
///
/// This function reads real terminal input through
/// `ratatui::crossterm::event`, so — like [`crate::app::run`] — it needs a
/// real terminal and has no automated test; [`Player`] carries every part
/// of this module that a test can and does drive without one.
///
/// # Errors
///
/// Returns an error when the terminal fails to draw a frame or when
/// reading a `crossterm` input event fails.
pub fn run<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    recording: Recording,
    mode: Mode,
) -> io::Result<()> {
    let mut player = match mode {
        Mode::Play(speed) => Player::with_speed(recording, speed),
        Mode::Step => Player::new(recording),
    };

    loop {
        terminal.draw(|frame| render(app, frame))?;
        if app.should_quit() || player.is_done() {
            return Ok(());
        }

        let wait = match mode {
            Mode::Play(_) => player.wall_sleep(),
            // Wait for as long as `crossterm` accepts rather than forever,
            // so this loop still returns promptly once the recording ends
            // or the person quits, on a platform that cannot wait forever.
            Mode::Step => Duration::from_secs(24 * 60 * 60),
        };

        if ratatui::crossterm::event::poll(wait)? {
            match ratatui::crossterm::event::read()? {
                ratatui::crossterm::event::Event::Key(key) if is_step_key(&key) => {
                    if matches!(mode, Mode::Step) {
                        player.step(app);
                    }
                }
                ratatui::crossterm::event::Event::Key(key) => {
                    let _ = app.handle_key(key);
                }
                ratatui::crossterm::event::Event::Mouse(mouse) => {
                    let _ = app.handle_mouse(mouse.column, mouse.row, mouse.kind);
                }
                ratatui::crossterm::event::Event::Resize(columns, rows) => {
                    app.set_size(columns, rows);
                }
                ratatui::crossterm::event::Event::FocusGained
                | ratatui::crossterm::event::Event::FocusLost
                | ratatui::crossterm::event::Event::Paste(_) => {}
            }
        } else if matches!(mode, Mode::Play(_)) {
            player.step(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ColorLevel, Theme};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(Theme::new(ColorLevel::TrueColor))
    }

    fn session_start(id: &str) -> Event {
        Event::SessionStart {
            id: id.into(),
            root: PathBuf::from("/home/dan/myrepo"),
            branch: Some("main".into()),
        }
    }

    fn turn_start(turn: &str) -> Event {
        Event::TurnStart {
            turn: turn.into(),
            class: dark_contract::RoleClass::Worker,
            model: "qwen3-14b-q4".into(),
        }
    }

    fn token(turn: &str, text: &str) -> Event {
        Event::TokenDelta {
            turn: turn.into(),
            text: text.into(),
        }
    }

    fn sample_recording() -> Recording {
        Recording::new(vec![
            session_start("s1"),
            turn_start("t1"),
            token("t1", "hello"),
            token("t1", ", world"),
            Event::DarkChanged { dark: true },
            Event::TurnEnd {
                turn: "t1".into(),
                usage: dark_contract::Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    reasoning_tokens: 0,
                    cached_tokens: 0,
                },
                wall_ms: 40,
            },
        ])
    }

    fn render_frame(app: &mut App, width: u16, height: u16) -> Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        terminal
            .draw(|frame| render(app, frame))
            .expect("render must not fail against a TestBackend");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn a_fresh_recording_reports_its_length_and_emptiness() {
        let recording = sample_recording();
        assert_eq!(recording.len(), 6);
        assert!(!recording.is_empty());
        assert!(Recording::default().is_empty());
    }

    #[test]
    fn a_player_starts_at_the_first_event_with_none_applied() {
        let player = Player::new(sample_recording());
        assert_eq!(player.position(), 0);
        assert_eq!(player.remaining(), 6);
        assert!(!player.is_done());
    }

    #[test]
    fn step_applies_exactly_one_event_through_apply_event() {
        let mut app = app();
        let mut player = Player::new(sample_recording());

        let applied = player.step(&mut app).cloned();
        assert_eq!(applied, Some(session_start("s1")));
        assert_eq!(
            app.header().session_id.as_deref(),
            Some("s1"),
            "step must drive the event through App::apply_event, not a parallel path"
        );
        assert_eq!(player.position(), 1);
        assert_eq!(player.remaining(), 5);
    }

    #[test]
    fn play_to_end_drains_every_event_in_order() {
        let mut app = app();
        let mut player = Player::new(sample_recording());

        player.play_to_end(&mut app);

        assert!(player.is_done());
        assert_eq!(player.remaining(), 0);
        assert!(!app.is_turn_active(), "the recorded TurnEnd must land");
        assert_eq!(app.header().session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn step_returns_none_once_the_recording_is_exhausted() {
        let mut app = app();
        let mut player = Player::new(Recording::new(vec![session_start("s1")]));

        assert!(player.step(&mut app).is_some());
        assert!(player.is_done());
        assert!(
            player.step(&mut app).is_none(),
            "stepping past the end must not panic or wrap around"
        );
        assert!(
            player.step(&mut app).is_none(),
            "a second call past the end must still report done"
        );
    }

    #[test]
    fn an_empty_recording_is_immediately_done() {
        let mut app = app();
        let mut player = Player::new(Recording::default());
        assert!(player.is_done());
        player.play_to_end(&mut app);
        assert_eq!(player.position(), 0);
    }

    #[test]
    fn advance_moves_the_clock_without_consuming_an_event() {
        let mut app = app();
        let mut player = Player::new(sample_recording());

        player.advance(&mut app);
        player.advance(&mut app);

        assert_eq!(
            player.position(),
            0,
            "advance must not move the recording cursor"
        );
        assert_eq!(player.remaining(), 6);
    }

    #[test]
    fn invalid_speeds_normalise_to_one() {
        for speed in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let player = Player::with_speed(sample_recording(), speed);
            assert!(
                (player.speed() - 1.0).abs() < f32::EPSILON,
                "speed {speed} must normalise to 1.0, not propagate"
            );
        }
    }

    #[test]
    fn a_finite_positive_speed_is_kept_exactly() {
        let player = Player::with_speed(sample_recording(), 4.0);
        assert!((player.speed() - 4.0).abs() < f32::EPSILON);
    }

    /// Returns true when two durations differ by less than a microsecond.
    ///
    /// [`Duration::div_f32`] and [`Duration::mul_f32`] round through a
    /// floating-point intermediate, so even a `1.0` multiplier can land a
    /// few nanoseconds off the untouched value — noise, not evidence of a
    /// bug, and well below anything a person or a frame budget notices.
    fn duration_close(a: Duration, b: Duration) -> bool {
        a.abs_diff(b) < Duration::from_micros(1)
    }

    #[test]
    fn wall_sleep_scales_inversely_with_speed() {
        let base = Player::with_speed(sample_recording(), 1.0).wall_sleep();
        let fast = Player::with_speed(sample_recording(), 4.0).wall_sleep();
        let slow = Player::with_speed(sample_recording(), 0.5).wall_sleep();

        assert!(
            duration_close(base, BASE_TICK),
            "speed 1.0 must reproduce the base tick, got {base:?}"
        );
        assert!(duration_close(fast, BASE_TICK.div_f32(4.0)));
        assert!(duration_close(slow, BASE_TICK.mul_f32(2.0)));
        assert!(
            fast < base && base < slow,
            "a higher speed must mean a shorter wait"
        );
    }

    #[test]
    fn speed_never_changes_the_events_applied_or_the_frame_they_produce() {
        // "Determinism": speed is a courtesy to a live viewer, never an
        // input `App` sees, so two players racing through the same
        // recording at different speeds must land on identical state and
        // render identical frames.
        let mut slow_app = app();
        Player::with_speed(sample_recording(), 0.1).play_to_end(&mut slow_app);

        let mut fast_app = app();
        Player::with_speed(sample_recording(), 100.0).play_to_end(&mut fast_app);

        let slow_frame = render_frame(&mut slow_app, 120, 40);
        let fast_frame = render_frame(&mut fast_app, 120, 40);
        assert_eq!(slow_frame, fast_frame);
    }

    #[test]
    fn the_same_recording_replayed_twice_produces_identical_frames() {
        // The task unit's own "done when": a recorded transcript reproduces
        // the same frames every time. This exercises the property across
        // a partial replay (mid dark-mode transition, mid streaming turn)
        // as well as a full one, since a bug that only shows up
        // mid-animation would pass a full-replay-only check.
        fn frames_at_each_step(width: u16, height: u16) -> Vec<Buffer> {
            let mut app = app();
            let mut player = Player::new(sample_recording());
            let mut frames = vec![render_frame(&mut app, width, height)];
            while player.step(&mut app).is_some() {
                frames.push(render_frame(&mut app, width, height));
            }
            frames
        }

        let first_run = frames_at_each_step(80, 24);
        let second_run = frames_at_each_step(80, 24);
        assert_eq!(first_run, second_run);
        assert_eq!(first_run.len(), sample_recording().len() + 1);
    }

    #[test]
    fn replaying_the_same_recording_at_a_larger_size_is_also_deterministic() {
        let mut first = app();
        Player::new(sample_recording()).play_to_end(&mut first);
        let mut second = app();
        Player::new(sample_recording()).play_to_end(&mut second);

        assert_eq!(
            render_frame(&mut first, 200, 60),
            render_frame(&mut second, 200, 60)
        );
    }

    #[test]
    fn a_recording_round_trips_through_from_vec_and_from_iterator() {
        let events = vec![session_start("s1"), turn_start("t1")];
        let from_vec: Recording = events.clone().into();
        let from_iter: Recording = events.clone().into_iter().collect();
        assert_eq!(from_vec, Recording::new(events));
        assert_eq!(from_vec, from_iter);
    }

    #[test]
    fn step_key_recognises_a_bare_space_press_only() {
        let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(is_step_key(&space));

        let shift_space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT);
        assert!(!shift_space.modifiers.is_empty());
        assert!(!is_step_key(&shift_space));

        let other = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!is_step_key(&other));

        let mut released = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
        released.kind = KeyEventKind::Release;
        assert!(!is_step_key(&released));
    }
}
