//! The application's state, and how an [`Event`] changes it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use dark_contract::{Allow, ErrCode, Event, Intent, Received, ResidencySnapshot};

use crate::app::pane::{Focus, LeftPane, RightPane};
use crate::app::zone::ZoneRegistry;
use crate::theme::{DARK_TRANSITION, Theme};
use crate::views::diff::{ConfirmDetail, UnifiedDiff};
use crate::views::fogmap::{FogMapState, Layout};
use crate::views::transcript::Transcript;

/// The narrowest terminal size the shell renders side-by-side panes at.
///
/// Below this, [`crate::app::layout::compute`] stacks the panes instead of
/// clipping them. See task unit `H1`, "Support 80 columns by 24 rows."
pub const MIN_SIDE_BY_SIDE_COLUMNS: u16 = 80;

/// The narrowest terminal height the shell renders side-by-side panes at.
pub const MIN_SIDE_BY_SIDE_ROWS: u16 = 24;

/// What a person must still confirm before the harness runs it.
///
/// `H1` tracks that a request is pending; a later task unit renders its
/// detail in a modal (see task unit `H4`, "Show the exact diff or the exact
/// command in a confirmation modal.").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConfirm {
    /// The identifier that answers this request.
    pub id: String,
    /// The exact change this request is asking about.
    ///
    /// Task unit `H4`, rule 8, asks the modal to show "the exact diff or
    /// the exact command … never a summary", so the request's own
    /// [`dark_contract::ConfirmPrompt`] is kept whole rather than reduced
    /// to a line of text here.
    pub detail: ConfirmDetail,
}

/// The last error the harness reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastError {
    /// The stable code.
    pub code: ErrCode,
    /// The message for a person to read.
    pub message: String,
    /// The action that clears the error, when the harness named one.
    pub remedy: Option<String>,
}

/// What the lossy channel has dropped during the running turn.
///
/// The application must never mistake a dropped [`Event::TokenDelta`] or
/// [`Event::ReasonDelta`] for the end of a turn, and it must say when it
/// dropped output rather than rendering a silent gap. This type is the
/// record that makes that possible: [`App::apply_event`] updates it on
/// every [`Received::Lagged`], and a view reads
/// [`LagState::dropped_this_turn`] to decide whether to show a warning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LagState {
    /// Events the lossy channel dropped since the current turn started.
    pub dropped_this_turn: u64,
    /// Events the lossy channel has dropped in total, across every turn.
    pub dropped_total: u64,
}

impl LagState {
    /// Returns true when the current turn has lost output.
    #[must_use]
    pub const fn has_dropped_output(self) -> bool {
        self.dropped_this_turn > 0
    }

    /// Starts a new turn, clearing the per-turn counter.
    ///
    /// [`LagState::dropped_total`] is never cleared: it is the harness's
    /// running account of how much streaming output the person never saw.
    fn start_turn(&mut self) {
        self.dropped_this_turn = 0;
    }

    /// Records that the lossy channel dropped `n` events.
    fn record_lag(&mut self, n: u64) {
        self.dropped_this_turn = self.dropped_this_turn.saturating_add(n);
        self.dropped_total = self.dropped_total.saturating_add(n);
    }
}

/// What the status bar shows about the session, the model, and the budget.
#[derive(Debug, Clone, Default)]
pub struct Header {
    /// The session identifier, once [`Event::SessionStart`] arrives.
    pub session_id: Option<String>,
    /// The repository root, once [`Event::SessionStart`] arrives.
    pub repo_root: Option<PathBuf>,
    /// The git branch that is checked out, when there is one.
    ///
    /// `None` covers a directory that git does not track and a detached
    /// head. The header then shows the repository name alone, with no
    /// branch segment beside it.
    pub branch: Option<String>,
    /// What is in memory now.
    pub resident: ResidencySnapshot,
    /// The model that is loading now, and its progress, while one is.
    pub loading: Option<(String, f32)>,
    /// Tokens in use in the context window.
    pub ctx_used: usize,
    /// Tokens that the resident set manager granted.
    pub ctx_granted: usize,
    /// Whether the harness is blocking network egress now.
    pub dark: bool,
}

impl Header {
    /// Returns the repository directory name, for the title bar.
    #[must_use]
    pub fn repo_name(&self) -> Option<String> {
        self.repo_root
            .as_ref()
            .and_then(|root| root.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// Returns the fraction of the granted context window in use, `0.0` to
    /// `1.0`. Returns `0.0` when nothing was granted yet, rather than
    /// dividing by zero.
    #[must_use]
    pub fn ctx_fraction(&self) -> f32 {
        if self.ctx_granted == 0 {
            return 0.0;
        }
        #[allow(
            clippy::cast_precision_loss,
            reason = "context sizes are far below f32's exact integer range"
        )]
        let fraction = self.ctx_used as f32 / self.ctx_granted as f32;
        fraction.clamp(0.0, 1.0)
    }
}

/// A measured generation rate.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TokenRate {
    /// Tokens generated since the running turn started.
    count: usize,
    /// When the running turn started.
    started_at: Option<Instant>,
    /// The most recently computed rate, in tokens each second.
    tokens_per_sec: f32,
}

impl TokenRate {
    /// Starts timing a new turn.
    fn start(&mut self, now: Instant) {
        self.count = 0;
        self.started_at = Some(now);
        self.tokens_per_sec = 0.0;
    }

    /// Records one generated token and recomputes the rate.
    fn record_token(&mut self, now: Instant) {
        self.count += 1;
        let Some(started_at) = self.started_at else {
            return;
        };
        let elapsed = now.saturating_duration_since(started_at).as_secs_f32();
        if elapsed > 0.0 {
            #[allow(
                clippy::cast_precision_loss,
                reason = "a turn's token count is far below f32's exact integer range"
            )]
            let count = self.count as f32;
            self.tokens_per_sec = count / elapsed;
        }
    }

    /// Sets the rate directly, for example from a turn's final usage.
    fn set(&mut self, tokens_per_sec: f32) {
        self.tokens_per_sec = tokens_per_sec.max(0.0);
    }

    /// Returns the most recently computed rate.
    #[must_use]
    pub const fn tokens_per_sec(self) -> f32 {
        self.tokens_per_sec
    }
}

/// The dark-mode transition in progress, if one is.
#[derive(Debug, Clone, Copy)]
struct DarkTransition {
    /// Where the transition is heading: `true` desaturates and reddens.
    target: bool,
    /// When the transition started.
    started_at: Instant,
}

/// The full state of the terminal application shell.
///
/// An [`App`] renders through [`crate::app::render::render`] and changes
/// through [`App::apply_event`] (from the harness) and [`App::handle_key`]
/// or [`App::handle_mouse`] (from the person). It holds no channel of its
/// own; the caller supplies an [`dark_contract::EventRx`] to poll (see
/// [`crate::app::bridge`]) and an [`dark_contract::Intent`] sink to send to.
#[derive(Debug, Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent toggle a person controls directly (a pane's focus, an \
              overlay, whether thinking is expanded, whether a quit is armed); they do not form \
              a state machine that collapses into fewer fields, since any subset can be set at \
              once"
)]
pub struct App {
    left_pane: LeftPane,
    right_pane: RightPane,
    transcript: Transcript,
    scrollback: usize,
    last_diff: Option<UnifiedDiff>,
    map: Option<Layout>,
    map_state: FogMapState,
    map_requested: Option<String>,
    focus: Focus,
    zones: ZoneRegistry,
    theme: Theme,
    header: Header,
    command_input: String,
    thinking_expanded: bool,
    turn_active: bool,
    turn_id: Option<String>,
    lag: LagState,
    token_rate: TokenRate,
    reasoning_token_count: usize,
    help_visible: bool,
    menu_visible: bool,
    quit_confirm_armed: bool,
    should_quit: bool,
    last_error: Option<LastError>,
    last_notice: Option<String>,
    pending_confirms: Vec<PendingConfirm>,
    dark_transition: Option<DarkTransition>,
    size: (u16, u16),
}

impl App {
    /// Builds a fresh shell. Nothing has happened yet: no session, no
    /// resident model, dark mode off.
    #[must_use]
    pub fn new(theme: Theme) -> Self {
        Self {
            left_pane: LeftPane::default(),
            right_pane: RightPane::default(),
            transcript: Transcript::new(),
            scrollback: 0,
            last_diff: None,
            map: None,
            map_state: FogMapState::new(),
            map_requested: None,
            focus: Focus::default(),
            zones: ZoneRegistry::new(),
            theme,
            header: Header::default(),
            command_input: String::new(),
            thinking_expanded: false,
            turn_active: false,
            turn_id: None,
            lag: LagState::default(),
            token_rate: TokenRate::default(),
            reasoning_token_count: 0,
            help_visible: false,
            menu_visible: false,
            quit_confirm_armed: false,
            should_quit: false,
            last_error: None,
            last_notice: None,
            pending_confirms: Vec::new(),
            dark_transition: None,
            size: (MIN_SIDE_BY_SIDE_COLUMNS, MIN_SIDE_BY_SIDE_ROWS),
        }
    }

    /// Applies one received value from the event bus.
    ///
    /// A [`Received::Lagged`] never ends a turn and never clears
    /// [`App::is_turn_active`]; it only records that the lossy channel
    /// dropped output. See [`LagState`].
    pub fn apply_event(&mut self, received: Received, now: Instant) {
        match received {
            Received::Lagged(n) => {
                self.lag.record_lag(n);
                self.transcript.record_lag(n);
            }
            Received::Event(event) => {
                // The view folds the event first, by reference: the match
                // below consumes the event's fields.
                self.transcript.apply_event(&event);
                self.apply_domain_event(event, now);
            }
        }
    }

    /// Applies one harness event. Split out of [`App::apply_event`] only to
    /// keep that function's match arms flat.
    fn apply_domain_event(&mut self, event: Event, now: Instant) {
        match event {
            Event::SessionStart { id, root, branch } => {
                self.header.session_id = Some(id);
                self.header.repo_root = Some(root);
                self.header.branch = branch;
            }
            Event::TurnStart { turn, .. } => {
                // A new turn starts at the tail again: whatever the person
                // had scrolled back to belongs to the turn that just ended.
                self.scrollback = 0;
                self.turn_active = true;
                self.turn_id = Some(turn);
                self.lag.start_turn();
                self.token_rate.start(now);
                self.reasoning_token_count = 0;
                self.quit_confirm_armed = false;
            }
            Event::TokenDelta { turn, .. } => {
                if self.turn_id.as_deref() == Some(turn.as_str()) {
                    self.token_rate.record_token(now);
                }
            }
            Event::ReasonDelta { turn, .. } => {
                if self.turn_id.as_deref() == Some(turn.as_str()) {
                    self.reasoning_token_count += 1;
                }
            }
            Event::TurnEnd {
                turn,
                usage,
                wall_ms,
            } => {
                if self.turn_id.as_deref() == Some(turn.as_str()) {
                    self.turn_active = false;
                    if wall_ms > 0 {
                        #[allow(
                            clippy::cast_precision_loss,
                            reason = "a turn's token count and wall time are far below f32's \
                                      exact integer range"
                        )]
                        let rate = usage.completion_tokens as f32 / (wall_ms as f32 / 1000.0);
                        self.token_rate.set(rate);
                    }
                }
            }
            Event::ModelLoading { model, progress } => {
                self.header.loading = Some((model, progress));
                if progress >= 1.0 {
                    self.header.loading = None;
                }
            }
            Event::Residency(snapshot) => self.header.resident = snapshot,
            Event::Budget { used, granted } => {
                self.header.ctx_used = used;
                self.header.ctx_granted = granted;
            }
            Event::DarkChanged { dark } => {
                self.header.dark = dark;
                self.dark_transition = Some(DarkTransition {
                    target: dark,
                    started_at: now,
                });
            }
            Event::ConfirmReq { id, prompt } => {
                if let dark_contract::ConfirmPrompt::Write { diff, .. } = &prompt {
                    self.last_diff = Some(UnifiedDiff::parse(diff));
                }
                self.pending_confirms.push(PendingConfirm {
                    id,
                    detail: ConfirmDetail::from_prompt(&prompt),
                });
            }
            Event::Error { code, msg, remedy } => {
                self.last_error = Some(LastError {
                    code,
                    message: msg,
                    remedy,
                });
            }
            Event::MapChanged { map_id } => self.map_requested = Some(map_id),
            Event::Notice(text) => self.last_notice = Some(text),
            // `Event::ToolCall`, `ToolProgress`, `ToolResult` and
            // `UserMessage` are folded into the transcript above, by
            // reference, before this match consumes the event.
            // `ExploreDone` and `IndexProgress` drive views nothing has
            // wired yet; `App` only needs to not panic on them. Naming them here, rather than folding them into this
            // wildcard, would make this arm identical to the wildcard's and
            // clippy's `match_same_arms` rejects that — so the wildcard
            // alone covers both those and any variant a future
            // `dark-contract` change adds, which is the point of `Event`
            // being `#[non_exhaustive]`.
            _ => {}
        }
    }

    /// Returns the running turn's transcript, for a renderer to draw.
    #[must_use]
    pub const fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// Returns the map identifier a [`Event::MapChanged`] asked for, and
    /// clears the request.
    ///
    /// The shell cannot read a map itself (Rule 14), so it records that
    /// one changed and lets the composition root fetch it. Taking the
    /// request rather than reading it means one `MapChanged` causes one
    /// load, however many times the loop asks.
    pub fn take_map_request(&mut self) -> Option<String> {
        self.map_requested.take()
    }

    /// Sets the map the left pane draws.
    ///
    /// `dark-tui` depends on `dark-contract` alone (Rule 14), so it cannot
    /// read a map itself: the layout is computed outside and handed in.
    /// The composition root does that — it is the only place that sees
    /// both `dark-cartograph`, which stores the tickets, and this crate,
    /// which draws them.
    pub fn set_map(&mut self, layout: Layout) {
        // Whatever was selected belonged to the previous layout, and a
        // ticket that is no longer on the map cannot stay highlighted.
        if let Some(selected) = self.map_state.selected()
            && !layout.positions.iter().any(|p| p.id == selected)
        {
            self.map_state.clear_selection();
        }
        self.map = Some(layout);
    }

    /// Returns the map the left pane draws, when one has been set.
    #[must_use]
    pub const fn map(&self) -> Option<&Layout> {
        self.map.as_ref()
    }

    /// Returns the map's selection and cursor state.
    #[must_use]
    pub const fn map_state(&self) -> &FogMapState {
        &self.map_state
    }

    /// Returns the map's selection and cursor state, to move it.
    pub const fn map_state_mut(&mut self) -> &mut FogMapState {
        &mut self.map_state
    }

    /// Returns the most recent diff the harness asked about, when one has
    /// arrived. The diff pane shows this.
    #[must_use]
    pub const fn last_diff(&self) -> Option<&UnifiedDiff> {
        self.last_diff.as_ref()
    }

    /// Returns how far the transcript is scrolled back from its newest
    /// line, in visual lines. `0` is the tail.
    #[must_use]
    pub const fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// Scrolls the transcript back by `lines`.
    ///
    /// The renderer clamps this against the content it has, so this never
    /// needs to know how tall the pane is or how much content exists.
    pub fn scroll_back(&mut self, lines: usize) {
        self.scrollback = self.scrollback.saturating_add(lines);
    }

    /// Scrolls the transcript forward by `lines`, towards the newest
    /// output. Stops at the tail.
    pub fn scroll_forward(&mut self, lines: usize) {
        self.scrollback = self.scrollback.saturating_sub(lines);
    }

    /// Returns to the newest output.
    pub fn scroll_to_tail(&mut self) {
        self.scrollback = 0;
    }

    /// Advances time-based state: the dark-mode transition. Call this once
    /// each redraw, whether or not an event arrived.
    pub fn tick(&mut self, now: Instant) {
        if let Some(transition) = self.dark_transition {
            let elapsed = now.saturating_duration_since(transition.started_at);
            let ratio = duration_ratio(elapsed, DARK_TRANSITION);
            let progress = if transition.target {
                ratio
            } else {
                1.0 - ratio
            };
            self.theme.set_dark_progress(progress);
            if elapsed >= DARK_TRANSITION {
                self.dark_transition = None;
            }
        }
    }

    /// Records the terminal size the shell is drawing at.
    pub fn set_size(&mut self, columns: u16, rows: u16) {
        self.size = (columns, rows);
    }

    /// Returns the terminal size the shell last drew at.
    #[must_use]
    pub const fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Returns true when the terminal is too small to show the panes
    /// side by side.
    #[must_use]
    pub const fn should_stack_panes(&self) -> bool {
        self.size.0 < MIN_SIDE_BY_SIDE_COLUMNS || self.size.1 < MIN_SIDE_BY_SIDE_ROWS
    }

    /// Returns the pane the left pane shows.
    #[must_use]
    pub const fn left_pane(&self) -> LeftPane {
        self.left_pane
    }

    /// Returns what the right pane shows.
    #[must_use]
    pub const fn right_pane(&self) -> RightPane {
        self.right_pane
    }

    /// Returns which region has keyboard focus.
    #[must_use]
    pub const fn focus(&self) -> Focus {
        self.focus
    }

    /// Returns the shell's theme.
    #[must_use]
    pub const fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Returns the status-bar and resident-set state.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Returns the text typed into the command bar so far.
    #[must_use]
    pub fn command_input(&self) -> &str {
        &self.command_input
    }

    /// Returns true while a turn is running.
    #[must_use]
    pub const fn is_turn_active(&self) -> bool {
        self.turn_active
    }

    /// Returns true when the running turn's thinking output is expanded.
    #[must_use]
    pub const fn is_thinking_expanded(&self) -> bool {
        self.thinking_expanded
    }

    /// Returns the reasoning tokens counted for the running turn.
    #[must_use]
    pub const fn reasoning_token_count(&self) -> usize {
        self.reasoning_token_count
    }

    /// Returns the measured generation rate.
    #[must_use]
    pub const fn tokens_per_sec(&self) -> f32 {
        self.token_rate.tokens_per_sec()
    }

    /// Returns how much output the lossy channel has dropped.
    #[must_use]
    pub const fn lag(&self) -> LagState {
        self.lag
    }

    /// Returns true when the running turn has lost streaming output. A view
    /// reads this to show a warning glyph instead of a silent gap.
    #[must_use]
    pub const fn has_dropped_output(&self) -> bool {
        self.lag.has_dropped_output()
    }

    /// Returns true while the help overlay shows.
    #[must_use]
    pub const fn is_help_visible(&self) -> bool {
        self.help_visible
    }

    /// Returns true while the menu overlay shows.
    #[must_use]
    pub const fn is_menu_visible(&self) -> bool {
        self.menu_visible
    }

    /// Returns the most recent error, if the harness reported one.
    #[must_use]
    pub const fn last_error(&self) -> Option<&LastError> {
        self.last_error.as_ref()
    }

    /// Returns the most recent notice, if the harness sent one.
    #[must_use]
    pub fn last_notice(&self) -> Option<&str> {
        self.last_notice.as_deref()
    }

    /// Returns the confirmations the harness is waiting on.
    #[must_use]
    pub fn pending_confirms(&self) -> &[PendingConfirm] {
        &self.pending_confirms
    }

    /// Returns the mouse zone registry that the last frame built.
    #[must_use]
    pub const fn zones(&self) -> &ZoneRegistry {
        &self.zones
    }

    /// Returns the mouse zone registry, for [`crate::app::render::render`]
    /// to rebuild each frame.
    pub const fn zones_mut(&mut self) -> &mut ZoneRegistry {
        &mut self.zones
    }

    /// Returns true once the person has asked to leave the application.
    #[must_use]
    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Answers the oldest pending confirmation with `allow`, and returns
    /// the intent that carries that answer to the harness.
    ///
    /// Returns `None` when nothing is pending. Every issued request must
    /// be answered exactly once — an unanswered tool call breaks the chat
    /// template (task unit `A2`) — so this removes the request as it
    /// answers it, and a second press with nothing pending sends nothing
    /// rather than answering the next request by accident.
    pub fn answer_confirm(&mut self, allow: Allow) -> Option<Intent> {
        if self.pending_confirms.is_empty() {
            return None;
        }
        let pending = self.pending_confirms.remove(0);
        Some(Intent::Confirm {
            id: pending.id,
            allow,
        })
    }

    /// Returns true when the shell is waiting for a person to answer a
    /// confirmation. While this holds, the modal covers the panes and the
    /// answer keys outrank every other binding.
    #[must_use]
    pub fn is_awaiting_confirm(&self) -> bool {
        !self.pending_confirms.is_empty()
    }

    /// Sets what the left pane shows.
    pub const fn set_left_pane(&mut self, pane: LeftPane) {
        self.left_pane = pane;
    }

    /// Sets what the right pane shows.
    pub const fn set_right_pane(&mut self, pane: RightPane) {
        self.right_pane = pane;
    }

    pub(crate) const fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
    }

    pub(crate) const fn toggle_help(&mut self) {
        self.help_visible = !self.help_visible;
    }

    pub(crate) const fn toggle_menu(&mut self) {
        self.menu_visible = !self.menu_visible;
    }

    pub(crate) const fn toggle_thinking(&mut self) {
        self.thinking_expanded = !self.thinking_expanded;
    }

    pub(crate) fn push_command_char(&mut self, c: char) {
        self.command_input.push(c);
    }

    pub(crate) fn pop_command_char(&mut self) {
        self.command_input.pop();
    }

    pub(crate) fn take_command_input(&mut self) -> String {
        std::mem::take(&mut self.command_input)
    }

    pub(crate) const fn request_quit(&mut self) {
        self.should_quit = true;
    }

    /// Returns true when a second `Ctrl+C` during a turn should now quit.
    ///
    /// The first `Ctrl+C` during a running turn arms this flag instead of
    /// quitting outright, so an accidental press never discards an
    /// in-progress turn. See task unit `H1`'s key table: "Ctrl+C quit, twice
    /// during a turn."
    pub(crate) const fn is_quit_armed(&self) -> bool {
        self.quit_confirm_armed
    }

    pub(crate) const fn arm_quit(&mut self) {
        self.quit_confirm_armed = true;
    }

    pub(crate) const fn disarm_quit(&mut self) {
        self.quit_confirm_armed = false;
    }
}

/// Returns `elapsed / total`, clamped to `0.0..=1.0`. Returns `1.0` when
/// `total` is zero, so a zero-length transition completes immediately
/// rather than dividing by zero.
fn duration_ratio(elapsed: Duration, total: Duration) -> f32 {
    if total.is_zero() {
        return 1.0;
    }
    (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorLevel;
    use dark_contract::{Event, Received};

    fn app() -> App {
        App::new(Theme::new(ColorLevel::TrueColor))
    }

    fn turn_start(turn: &str) -> Received {
        Received::Event(Event::TurnStart {
            turn: turn.into(),
            class: dark_contract::RoleClass::Worker,
            model: "qwen3-14b-q4".into(),
        })
    }

    fn token(turn: &str, text: &str) -> Received {
        Received::Event(Event::TokenDelta {
            turn: turn.into(),
            text: text.into(),
        })
    }

    #[test]
    fn a_turn_start_marks_the_turn_active() {
        let mut app = app();
        assert!(!app.is_turn_active());
        app.apply_event(turn_start("t1"), Instant::now());
        assert!(app.is_turn_active());
    }

    #[test]
    fn a_lagged_receipt_does_not_end_a_running_turn() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(turn_start("t1"), now);
        assert!(app.is_turn_active());

        app.apply_event(Received::Lagged(37), now);

        assert!(
            app.is_turn_active(),
            "a dropped delta must not read as a turn end"
        );
    }

    #[test]
    fn a_lagged_receipt_is_reported_rather_than_rendered_as_a_silent_gap() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(turn_start("t1"), now);
        assert!(!app.has_dropped_output());

        app.apply_event(Received::Lagged(12), now);

        assert!(app.has_dropped_output());
        assert_eq!(app.lag().dropped_this_turn, 12);
        assert_eq!(app.lag().dropped_total, 12);
    }

    #[test]
    fn overflowing_the_real_lossy_channel_recovers_and_reports_it() {
        // Build a real bus, overflow the lossy channel through it, and drive
        // the app from what `EventRx` actually reports. This is the
        // end-to-end version of the two tests above, against the real
        // channel rather than a hand-built `Received` value.
        use crate::app::bridge::{PollOutcome, try_recv};
        use dark_contract::EventBus;

        let bus = EventBus::with_capacity(2, 64);
        let mut rx = bus.subscribe();
        let tx = bus.tx();

        tx.send(Event::TurnStart {
            turn: "t1".into(),
            class: dark_contract::RoleClass::Worker,
            model: "qwen3-14b-q4".into(),
        });
        for i in 0..50 {
            tx.send(Event::TokenDelta {
                turn: "t1".into(),
                text: i.to_string(),
            });
        }
        tx.send(Event::Budget {
            used: 10,
            granted: 100,
        });

        let mut app = app();
        let now = Instant::now();
        while let PollOutcome::Event(received) = try_recv(&mut rx) {
            app.apply_event(received, now);
        }

        assert!(app.is_turn_active(), "the turn must still read as active");
        assert!(app.has_dropped_output(), "the drop must be reported");
        assert_eq!(
            app.header().ctx_used,
            10,
            "the reliable event must still land"
        );
    }

    #[test]
    fn lag_from_a_previous_turn_does_not_carry_into_the_next() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(turn_start("t1"), now);
        app.apply_event(Received::Lagged(5), now);
        assert!(app.has_dropped_output());

        app.apply_event(
            Received::Event(Event::TurnEnd {
                turn: "t1".into(),
                usage: dark_contract::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 20,
                    reasoning_tokens: 0,
                    cached_tokens: 0,
                },
                wall_ms: 1000,
            }),
            now,
        );
        app.apply_event(turn_start("t2"), now);

        assert!(
            !app.has_dropped_output(),
            "a new turn starts with a clean lag state"
        );
        assert_eq!(
            app.lag().dropped_total,
            5,
            "the running total is never cleared"
        );
    }

    #[test]
    fn token_deltas_for_a_stale_turn_are_ignored() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(turn_start("t1"), now);
        app.apply_event(
            Received::Event(Event::TurnEnd {
                turn: "t1".into(),
                usage: dark_contract::Usage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                    reasoning_tokens: 0,
                    cached_tokens: 0,
                },
                wall_ms: 100,
            }),
            now,
        );
        // A token delta for the turn that just ended must not resurrect it.
        app.apply_event(token("t1", "late"), now + Duration::from_millis(1));
        assert!(!app.is_turn_active());
    }

    #[test]
    fn turn_end_computes_the_final_rate_from_usage_and_wall_time() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(turn_start("t1"), now);
        app.apply_event(
            Received::Event(Event::TurnEnd {
                turn: "t1".into(),
                usage: dark_contract::Usage {
                    prompt_tokens: 10,
                    completion_tokens: 41,
                    reasoning_tokens: 0,
                    cached_tokens: 0,
                },
                wall_ms: 1000,
            }),
            now,
        );
        assert!((app.tokens_per_sec() - 41.0).abs() < 0.01);
    }

    #[test]
    fn session_start_fills_the_header() {
        let mut app = app();
        app.apply_event(
            Received::Event(Event::SessionStart {
                id: "s1".into(),
                root: PathBuf::from("/home/dan/myrepo"),
                branch: None,
            }),
            Instant::now(),
        );
        assert_eq!(app.header().session_id.as_deref(), Some("s1"));
        assert_eq!(app.header().repo_name().as_deref(), Some("myrepo"));
    }

    #[test]
    fn model_loading_clears_once_progress_reaches_one() {
        let mut app = app();
        let now = Instant::now();
        app.apply_event(
            Received::Event(Event::ModelLoading {
                model: "qwen3-14b-q4".into(),
                progress: 0.5,
            }),
            now,
        );
        assert!(app.header().loading.is_some());
        app.apply_event(
            Received::Event(Event::ModelLoading {
                model: "qwen3-14b-q4".into(),
                progress: 1.0,
            }),
            now,
        );
        assert!(app.header().loading.is_none());
    }

    #[test]
    fn budget_updates_the_context_fraction() {
        let mut app = app();
        app.apply_event(
            Received::Event(Event::Budget {
                used: 34,
                granted: 100,
            }),
            Instant::now(),
        );
        assert!((app.header().ctx_fraction() - 0.34).abs() < 0.001);
    }

    #[test]
    fn dark_changed_starts_a_transition_that_completes_after_the_documented_duration() {
        let mut app = app();
        let start = Instant::now();
        app.apply_event(Received::Event(Event::DarkChanged { dark: true }), start);
        app.tick(start);
        assert!((app.theme().dark_progress() - 0.0).abs() < 0.01);

        app.tick(start + DARK_TRANSITION);
        assert!((app.theme().dark_progress() - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_second_dark_changed_reverses_a_transition_in_progress() {
        let mut app = app();
        let start = Instant::now();
        app.apply_event(Received::Event(Event::DarkChanged { dark: true }), start);
        let midpoint = start + DARK_TRANSITION / 2;
        app.tick(midpoint);
        let progress_at_midpoint = app.theme().dark_progress();
        assert!(progress_at_midpoint > 0.0 && progress_at_midpoint < 1.0);

        app.apply_event(
            Received::Event(Event::DarkChanged { dark: false }),
            midpoint,
        );
        app.tick(midpoint + DARK_TRANSITION);
        assert!((app.theme().dark_progress() - 0.0).abs() < 0.01);
    }

    #[test]
    #[allow(
        clippy::default_trait_access,
        reason = "the `args` field below is a serde_json::Value; naming that type directly would \
                  need dark-tui to depend on serde_json, which Rule 15 reserves to dark-contract"
    )]
    fn every_event_variant_the_bus_can_carry_is_handled_without_panicking() {
        let mut app = app();
        let now = Instant::now();
        let events = [
            Event::UserMessage {
                turn: "t1".into(),
                text: "hi".into(),
            },
            Event::ToolCall {
                turn: "t1".into(),
                call: dark_contract::ToolCall {
                    id: "c1".into(),
                    name: "read_file".into(),
                    // `args` is a `serde_json::Value`. Naming that crate
                    // would need dark-tui to depend on it directly (Rule 15
                    // reserves that dependency to dark-contract), so this
                    // relies on `Default` resolving through the field's
                    // declared type instead.
                    args: Default::default(),
                },
            },
            Event::ToolProgress {
                turn: "t1".into(),
                call_id: "c1".into(),
                line: "…".into(),
            },
            Event::ToolResult {
                turn: "t1".into(),
                call_id: "c1".into(),
                result: dark_contract::ToolResultSummary {
                    name: "read_file".into(),
                    is_error: false,
                    bytes: 4,
                    headline: "done".into(),
                    has_diff: false,
                },
                content: "done".into(),
            },
            Event::MapChanged {
                map_id: "m1".into(),
            },
            Event::ExploreDone {
                tree_sha: "abc".into(),
                path: PathBuf::from("x"),
            },
            Event::IndexProgress {
                pack: "p1".into(),
                done: 1,
                total: 2,
            },
        ];
        for event in events {
            app.apply_event(Received::Event(event), now);
        }
    }

    #[test]
    fn resizing_below_the_minimum_requests_a_stacked_layout() {
        let mut app = app();
        app.set_size(80, 24);
        assert!(!app.should_stack_panes());
        app.set_size(40, 10);
        assert!(app.should_stack_panes());
    }
}
