//! Whether the fog map animates at all.
//!
//! See task unit `H3`, rule 11: "Disable animation when any of these
//! conditions is true: `TERM` is `dumb`; the output is not a terminal;
//! `DARK_NO_ANIM` is set; the window has no focus; three consecutive frames
//! exceeded the budget."

use std::time::Duration;

/// How many consecutive over-budget frames disable animation outright.
const DISABLE_AFTER_CONSECUTIVE_SLOW_FRAMES: u8 = 3;

/// Decides whether the fog map animates this frame.
///
/// Build one with [`AnimationGate::detect`] once, at start-up. Update it
/// each frame afterwards: [`AnimationGate::set_focus`] when a
/// `crossterm::event::Event::FocusGained` or `FocusLost` arrives, and
/// [`AnimationGate::record_frame`] with how long the frame took to draw.
/// [`AnimationGate::is_animated`] folds every rule task unit `H3` lists into
/// one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationGate {
    /// Whether `TERM`, `DARK_NO_ANIM`, and the terminal check allow
    /// animation. Fixed at construction: none of the three change while the
    /// process runs.
    allowed_by_environment: bool,
    /// Whether the terminal window holds focus now.
    focused: bool,
    /// How long a frame may take before it counts as slow.
    budget: Duration,
    /// Slow frames seen in a row. A fast frame resets this to zero.
    consecutive_slow_frames: u8,
    /// Set once three consecutive frames run over budget. Never clears.
    disabled_by_slow_frames: bool,
}

impl AnimationGate {
    /// Builds a gate from the real process environment and terminal state.
    ///
    /// `is_terminal` should report whether the output stream is a terminal
    /// — see [`std::io::IsTerminal`]. This crate has no direct dependency
    /// that can answer that question for an arbitrary [`ratatui::Terminal`]
    /// backend, so the caller supplies it.
    #[must_use]
    pub fn detect(budget: Duration, is_terminal: bool) -> Self {
        Self::detect_from(budget, is_terminal, |key| std::env::var(key).ok())
    }

    /// Builds a gate from an arbitrary variable lookup.
    ///
    /// [`AnimationGate::detect`] calls this with a lookup backed by the real
    /// process environment. See
    /// [`crate::theme::ColorLevel::detect_from`] for why the tests below
    /// call this instead, with a fake lookup.
    #[must_use]
    pub fn detect_from(
        budget: Duration,
        is_terminal: bool,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let dumb_term = lookup("TERM").as_deref() == Some("dumb");
        let no_anim = lookup("DARK_NO_ANIM").is_some();
        Self {
            allowed_by_environment: is_terminal && !dumb_term && !no_anim,
            focused: true,
            budget,
            consecutive_slow_frames: 0,
            disabled_by_slow_frames: false,
        }
    }

    /// Records whether the terminal window holds focus now.
    pub fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Records how long the most recent frame took to draw.
    ///
    /// Three frames in a row over budget disables animation for the rest of
    /// this gate's life, on the theory that a terminal this slow to draw
    /// stays slow. A fast frame in between resets the count: an isolated
    /// slow frame is not evidence of that.
    pub fn record_frame(&mut self, elapsed: Duration) {
        if elapsed > self.budget {
            self.consecutive_slow_frames = self.consecutive_slow_frames.saturating_add(1);
            if self.consecutive_slow_frames >= DISABLE_AFTER_CONSECUTIVE_SLOW_FRAMES {
                self.disabled_by_slow_frames = true;
            }
        } else {
            self.consecutive_slow_frames = 0;
        }
    }

    /// Returns true when the fog map should animate this frame.
    #[must_use]
    pub const fn is_animated(&self) -> bool {
        self.allowed_by_environment && self.focused && !self.disabled_by_slow_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: Duration = Duration::from_millis(8);

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn a_real_terminal_with_no_special_variables_animates() {
        let gate = AnimationGate::detect_from(BUDGET, true, env(&[]));
        assert!(gate.is_animated());
    }

    #[test]
    fn a_dumb_terminal_never_animates() {
        let gate = AnimationGate::detect_from(BUDGET, true, env(&[("TERM", "dumb")]));
        assert!(!gate.is_animated());
    }

    #[test]
    fn output_that_is_not_a_terminal_never_animates() {
        let gate = AnimationGate::detect_from(BUDGET, false, env(&[]));
        assert!(!gate.is_animated());
    }

    #[test]
    fn dark_no_anim_disables_animation_regardless_of_its_value() {
        let gate = AnimationGate::detect_from(BUDGET, true, env(&[("DARK_NO_ANIM", "0")]));
        assert!(!gate.is_animated());
    }

    #[test]
    fn losing_focus_stops_animation_and_regaining_it_resumes() {
        let mut gate = AnimationGate::detect_from(BUDGET, true, env(&[]));
        assert!(gate.is_animated());
        gate.set_focus(false);
        assert!(!gate.is_animated());
        gate.set_focus(true);
        assert!(gate.is_animated());
    }

    #[test]
    fn three_consecutive_slow_frames_disable_animation() {
        let mut gate = AnimationGate::detect_from(BUDGET, true, env(&[]));
        gate.record_frame(Duration::from_millis(9));
        assert!(gate.is_animated(), "one slow frame must not disable it");
        gate.record_frame(Duration::from_millis(9));
        assert!(gate.is_animated(), "two slow frames must not disable it");
        gate.record_frame(Duration::from_millis(9));
        assert!(
            !gate.is_animated(),
            "three slow frames in a row must disable it"
        );
    }

    #[test]
    fn a_fast_frame_between_slow_ones_resets_the_streak() {
        let mut gate = AnimationGate::detect_from(BUDGET, true, env(&[]));
        gate.record_frame(Duration::from_millis(9));
        gate.record_frame(Duration::from_millis(9));
        gate.record_frame(Duration::from_millis(1));
        gate.record_frame(Duration::from_millis(9));
        gate.record_frame(Duration::from_millis(9));
        assert!(
            gate.is_animated(),
            "the streak must restart after a fast frame, not accumulate across it"
        );
    }

    #[test]
    fn once_disabled_by_slow_frames_regaining_focus_does_not_re_enable_it() {
        let mut gate = AnimationGate::detect_from(BUDGET, true, env(&[]));
        for _ in 0..3 {
            gate.record_frame(Duration::from_millis(9));
        }
        gate.set_focus(false);
        gate.set_focus(true);
        assert!(!gate.is_animated());
    }
}
