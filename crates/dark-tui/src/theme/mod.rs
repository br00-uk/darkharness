//! Colour that shows state.
//!
//! `dark` in this module names the network policy, not a light-versus-dark
//! UI preference: this crate has one palette, the accretion disk described
//! in task unit `H2`. "Dark mode" is [`Event::DarkChanged`], the event that
//! fires when the harness starts or stops blocking network egress. The
//! person must never be unsure about that state, so the whole palette
//! desaturates and the status bar reddens while it holds.
//!
//! [`Event::DarkChanged`]: dark_contract::Event::DarkChanged

pub mod density;
pub mod level;
pub mod palette;

use std::time::Duration;

use ratatui::style::{Modifier, Style};

pub use density::{DENSITY_RAMP, density_char};
pub use level::ColorLevel;
pub use palette::{Palette, desaturate, gradient, pulse};

/// How a ticket, a map node, or a piece of work is doing.
///
/// See the state table in task unit `H2`. Task unit `H3` draws the fog map
/// from these states; this crate only names the colour for each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TicketState {
    /// Takeable now. Every blocking edge into it resolved.
    Frontier,
    /// A session claimed this ticket and is working it.
    Claimed,
    /// Work finished and landed.
    Resolved,
    /// At least one blocking edge into it is still open.
    Blocked,
    /// Not yet specified.
    Fog,
    /// Outside the map's scope.
    OutOfScope,
}

/// The full colour token layer.
///
/// A `Theme` combines the fixed [`Palette`] with two things that change at
/// run time: the [`ColorLevel`] the terminal supports, and how far the
/// dark-mode transition has run. Every method that returns a [`Style`] folds
/// both in, so a caller never downgrades or tints a colour by hand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// The named colours before degradation or tinting.
    palette: Palette,
    /// What the terminal can show.
    level: ColorLevel,
    /// How far the dark-mode transition has run, `0.0` to `1.0`.
    ///
    /// `0.0` is network-open. `1.0` is fully dark: network egress is
    /// blocked. A value between the two is mid-transition.
    dark_progress: f32,
}

/// How long the dark-mode palette transition takes.
///
/// Task unit `H3` drives this with its spring integrator; until that lands,
/// a caller may step [`Theme::dark_progress`] linearly against this
/// duration and still show the same start and end state.
pub const DARK_TRANSITION: Duration = Duration::from_millis(400);

impl Theme {
    /// Builds a theme at a given colour level, with dark mode off.
    #[must_use]
    pub const fn new(level: ColorLevel) -> Self {
        Self::with_palette(Palette::charmtone(), level)
    }

    /// Builds a theme over an explicit palette, with dark mode off.
    ///
    /// [`Theme::new`] chooses [`Palette::charmtone`]. Pass
    /// [`Palette::accretion_disk`] here for the palette task unit `H2`
    /// specifies.
    #[must_use]
    pub const fn with_palette(palette: Palette, level: ColorLevel) -> Self {
        Self {
            palette,
            level,
            dark_progress: 0.0,
        }
    }

    /// Builds a theme at the colour level that [`ColorLevel::detect`] finds.
    #[must_use]
    pub fn detect() -> Self {
        Self::new(ColorLevel::detect())
    }

    /// Returns the colour level this theme renders at.
    #[must_use]
    pub const fn level(&self) -> ColorLevel {
        self.level
    }

    /// Returns how far the dark-mode transition has run.
    #[must_use]
    pub const fn dark_progress(&self) -> f32 {
        self.dark_progress
    }

    /// Sets how far the dark-mode transition has run.
    ///
    /// The value clamps to `0.0..=1.0`, so a caller may pass an
    /// elapsed-over-total ratio that overshoots slightly without checking it
    /// first.
    pub fn set_dark_progress(&mut self, progress: f32) {
        self.dark_progress = progress.clamp(0.0, 1.0);
    }

    /// Returns true once the dark-mode transition has fully run.
    #[must_use]
    pub fn is_dark(&self) -> bool {
        self.dark_progress >= 1.0
    }

    /// Resolves a raw palette colour to what the terminal can show now.
    ///
    /// This desaturates toward the current [`Theme::dark_progress`], then
    /// downgrades to the current [`ColorLevel`]. Every other method on this
    /// type that returns a colour or a [`Style`] goes through this one, so a
    /// caller never needs to call [`palette::desaturate`] or
    /// [`ColorLevel::downgrade`] directly.
    #[must_use]
    pub fn resolve(&self, raw: ratatui::style::Color) -> ratatui::style::Color {
        self.level.downgrade(desaturate(raw, self.dark_progress))
    }

    /// A foreground-only style for a raw palette colour.
    #[must_use]
    pub fn style(&self, raw: ratatui::style::Color) -> Style {
        Style::default().fg(self.resolve(raw))
    }

    /// The application background.
    #[must_use]
    pub fn background(&self) -> Style {
        Style::default().bg(self.resolve(self.palette.singularity))
    }

    /// A panel's background and default text colour.
    #[must_use]
    pub fn panel(&self) -> Style {
        Style::default()
            .bg(self.resolve(self.palette.horizon))
            .fg(self.resolve(self.palette.text))
    }

    /// De-emphasised text, for a caption or a secondary line.
    #[must_use]
    pub fn text_dim(&self) -> Style {
        Style::default().fg(self.resolve(self.palette.text_dim))
    }

    /// The border and glyph colour for whichever pane holds focus.
    #[must_use]
    pub fn focused_border(&self) -> Style {
        Style::default().fg(self.resolve(self.palette.photon_ring))
    }

    /// The border colour for a pane that does not hold focus.
    #[must_use]
    pub fn unfocused_border(&self) -> Style {
        Style::default().fg(self.resolve(self.palette.text_dim))
    }

    /// The status bar's style.
    ///
    /// Unlike every other surface, the status bar does not desaturate in
    /// dark mode: it reddens instead, so it stays the one place a glance
    /// confirms the network state. See the state table in task unit `H2`.
    #[must_use]
    pub fn status_bar(&self) -> Style {
        let bg = self.level.downgrade(gradient(
            self.palette.horizon,
            self.palette.danger,
            self.dark_progress,
        ));
        Style::default().bg(bg).fg(self.resolve(self.palette.text))
    }

    /// A danger message: a failed tool call, an unrecoverable error.
    #[must_use]
    pub fn danger(&self) -> Style {
        self.style(self.palette.danger)
    }

    /// A success message.
    #[must_use]
    pub fn ok(&self) -> Style {
        self.style(self.palette.ok)
    }

    /// A caution: a lagged event channel, a retried call.
    #[must_use]
    pub fn warn(&self) -> Style {
        self.style(self.palette.warn)
    }

    /// The style for one [`TicketState`].
    ///
    /// `phase` drives the "pulsing" claimed state and runs `0.0..=1.0`
    /// across one pulse cycle; every other state ignores it. The caller
    /// (task unit `H3`) owns the clock that advances `phase`.
    #[must_use]
    pub fn state_style(&self, state: TicketState, phase: f32) -> Style {
        let raw = match state {
            TicketState::Frontier => self.palette.doppler_blue,
            TicketState::Claimed => pulse(self.palette.disk_mid, self.palette.disk_inner, phase),
            TicketState::Resolved => self.palette.ember,
            TicketState::Blocked => self.palette.doppler_dim,
            TicketState::Fog => self.palette.fog,
            TicketState::OutOfScope => self.palette.void,
        };
        let mut style = self.style(raw);
        if state == TicketState::Claimed {
            style = style.add_modifier(Modifier::BOLD);
        }
        style
    }

    /// The sweep style for a model load in progress.
    ///
    /// `phase` runs `0.0..=1.0` across one sweep cycle.
    #[must_use]
    pub fn model_loading_style(&self, phase: f32) -> Style {
        self.style(pulse(
            self.palette.disk_outer,
            self.palette.disk_inner,
            phase,
        ))
    }

    /// Returns the raw palette that this theme resolves colours from.
    #[must_use]
    pub const fn palette(&self) -> Palette {
        self.palette
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ColorLevel::TrueColor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn resolve_is_unchanged_at_true_colour_and_zero_dark_progress() {
        let theme = Theme::new(ColorLevel::TrueColor);
        let p = theme.palette();
        assert_eq!(theme.resolve(p.disk_mid), p.disk_mid);
    }

    #[test]
    fn set_dark_progress_clamps() {
        let mut theme = Theme::new(ColorLevel::TrueColor);
        theme.set_dark_progress(5.0);
        assert!(theme.is_dark());
        theme.set_dark_progress(-5.0);
        assert!(!theme.is_dark());
    }

    #[test]
    fn full_dark_progress_desaturates_the_disk() {
        let mut theme = Theme::new(ColorLevel::TrueColor);
        theme.set_dark_progress(1.0);
        let p = theme.palette();
        let Color::Rgb(r, g, b) = theme.resolve(p.disk_mid) else {
            panic!("expected an rgb colour");
        };
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn the_status_bar_reddens_as_dark_progress_rises() {
        let mut theme = Theme::new(ColorLevel::TrueColor);
        let open = theme.status_bar().bg.unwrap();
        theme.set_dark_progress(1.0);
        let dark = theme.status_bar().bg.unwrap();
        // The status bar reddens rather than desaturating (see its own doc
        // comment), so the fully-dark background is the raw danger colour,
        // not `Theme::resolve`'s desaturated version of it.
        assert_eq!(dark, theme.palette().danger);
        assert_ne!(open, dark);
    }

    #[test]
    fn none_level_downgrades_every_resolved_colour_to_reset() {
        let theme = Theme::new(ColorLevel::None);
        let p = theme.palette();
        assert_eq!(theme.resolve(p.doppler_blue), Color::Reset);
        assert_eq!(theme.resolve(p.ember), Color::Reset);
    }

    #[test]
    fn frontier_ignores_phase_and_claimed_uses_it() {
        let theme = Theme::new(ColorLevel::TrueColor);
        let frontier_a = theme.state_style(TicketState::Frontier, 0.0);
        let frontier_b = theme.state_style(TicketState::Frontier, 0.5);
        assert_eq!(frontier_a, frontier_b);

        let claimed_a = theme.state_style(TicketState::Claimed, 0.0);
        let claimed_b = theme.state_style(TicketState::Claimed, 0.5);
        assert_ne!(claimed_a, claimed_b);
    }

    #[test]
    fn each_state_maps_to_the_documented_token() {
        let theme = Theme::new(ColorLevel::TrueColor);
        let p = theme.palette();
        assert_eq!(
            theme.state_style(TicketState::Frontier, 0.0).fg,
            Some(p.doppler_blue)
        );
        assert_eq!(
            theme.state_style(TicketState::Resolved, 0.0).fg,
            Some(p.ember)
        );
        assert_eq!(
            theme.state_style(TicketState::Blocked, 0.0).fg,
            Some(p.doppler_dim)
        );
        assert_eq!(theme.state_style(TicketState::Fog, 0.0).fg, Some(p.fog));
        assert_eq!(
            theme.state_style(TicketState::OutOfScope, 0.0).fg,
            Some(p.void)
        );
    }

    #[test]
    fn dark_transition_is_four_hundred_milliseconds() {
        assert_eq!(DARK_TRANSITION, Duration::from_millis(400));
    }
}
