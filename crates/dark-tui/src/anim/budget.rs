//! The fog map's frame budget: what to drop when a frame runs long.
//!
//! See task unit `H3`, rule 9: "Set a frame budget of 8 milliseconds. When a
//! frame exceeds it, remove the shimmer first. Then remove the gradient.
//! Keep the layout."

use std::time::Duration;

/// How much decoration the fog map draws this frame.
///
/// Every level keeps the layout — the ring each ticket sits on, and the
/// glyph and name it shows — since task unit `H3` never lists the layout
/// among the things a slow frame may drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailLevel {
    /// Every decoration: the shimmer and the radial disk gradient.
    #[default]
    Full,
    /// The radial disk gradient, without the shimmer.
    NoShimmer,
    /// Neither decoration.
    LayoutOnly,
}

/// Tracks how long recent frames took, and degrades decoration in response.
///
/// The degrade in [`FrameBudget::record`] never recovers on its own: a
/// single slow frame — a page fault, a burst of terminal input, a garbage
/// collection pause in an unrelated process sharing the machine — is
/// exactly the kind of one-off spike this rule exists to hide, not to make
/// permanent, and reversing the degrade every time a fast frame follows a
/// slow one would flicker the decoration on and off. Build a fresh
/// [`FrameBudget`] (for example at the start of a session) to reset it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameBudget {
    /// How long a frame may take before it counts as slow.
    budget: Duration,
    /// How much decoration the fog map draws now.
    detail: DetailLevel,
}

impl FrameBudget {
    /// Builds a tracker at the given per-frame budget.
    ///
    /// Task unit `H3` sets this budget to 8 milliseconds; see
    /// [`crate::views::fogmap::FRAME_BUDGET`].
    #[must_use]
    pub const fn new(budget: Duration) -> Self {
        Self {
            budget,
            detail: DetailLevel::Full,
        }
    }

    /// Records how long the most recent frame took to draw, degrading
    /// [`FrameBudget::detail`] by one step when it ran over budget.
    pub fn record(&mut self, elapsed: Duration) {
        if elapsed > self.budget {
            self.detail = match self.detail {
                DetailLevel::Full => DetailLevel::NoShimmer,
                DetailLevel::NoShimmer | DetailLevel::LayoutOnly => DetailLevel::LayoutOnly,
            };
        }
    }

    /// Returns how much decoration the caller should draw now.
    #[must_use]
    pub const fn detail(&self) -> DetailLevel {
        self.detail
    }

    /// Returns the budget one frame must stay inside to avoid degrading.
    #[must_use]
    pub const fn budget(&self) -> Duration {
        self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> FrameBudget {
        FrameBudget::new(Duration::from_millis(8))
    }

    #[test]
    fn a_fast_frame_keeps_full_detail() {
        let mut b = budget();
        b.record(Duration::from_millis(2));
        assert_eq!(b.detail(), DetailLevel::Full);
    }

    #[test]
    fn a_frame_exactly_at_the_budget_does_not_degrade() {
        let mut b = budget();
        b.record(Duration::from_millis(8));
        assert_eq!(b.detail(), DetailLevel::Full);
    }

    #[test]
    fn the_first_slow_frame_removes_the_shimmer_first() {
        let mut b = budget();
        b.record(Duration::from_millis(9));
        assert_eq!(b.detail(), DetailLevel::NoShimmer);
    }

    #[test]
    fn a_second_slow_frame_removes_the_gradient_too() {
        let mut b = budget();
        b.record(Duration::from_millis(9));
        b.record(Duration::from_millis(9));
        assert_eq!(b.detail(), DetailLevel::LayoutOnly);
    }

    #[test]
    fn the_layout_only_level_never_degrades_further() {
        let mut b = budget();
        for _ in 0..10 {
            b.record(Duration::from_millis(50));
        }
        assert_eq!(b.detail(), DetailLevel::LayoutOnly);
    }

    #[test]
    fn a_fast_frame_after_a_slow_one_does_not_undo_the_degrade() {
        let mut b = budget();
        b.record(Duration::from_millis(9));
        b.record(Duration::from_millis(1));
        assert_eq!(
            b.detail(),
            DetailLevel::NoShimmer,
            "a single fast frame must not flicker the decoration back on"
        );
    }
}
