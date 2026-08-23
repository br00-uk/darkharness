//! Where each region of the shell draws.
//!
//! [`compute`] is a pure function of the terminal size: same input, same
//! [`AppLayout`], every time. Nothing here reads [`App`](crate::app::App)
//! state beyond the size and the stacking decision, which keeps a layout
//! test cheap to write and its result cheap to reason about.

use ratatui::layout::{Constraint, Direction, Layout, Margin, Rect};

/// Every region [`crate::app::render::render`] draws into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    /// The whole terminal area. The outer border and the title draw here.
    pub outer: Rect,
    /// The left pane's area, border included.
    pub left_pane: Rect,
    /// The right pane's area, border included.
    pub right_pane: Rect,
    /// The command bar's single line.
    pub command_bar: Rect,
    /// The function-key bar's single line.
    pub function_keys: Rect,
    /// Whether the panes stack top-to-bottom instead of sitting side by
    /// side. See [`crate::app::state::App::should_stack_panes`].
    pub stacked: bool,
}

impl AppLayout {
    /// A layout with every region empty. [`compute`] returns this for a
    /// zero-width or zero-height area, which happens during an aggressive
    /// resize; every field stays a valid, zero-area `Rect` rather than one
    /// built from a subtraction that would otherwise wrap.
    const EMPTY: Self = Self {
        outer: Rect::ZERO,
        left_pane: Rect::ZERO,
        right_pane: Rect::ZERO,
        command_bar: Rect::ZERO,
        function_keys: Rect::ZERO,
        stacked: false,
    };
}

/// Computes the layout for `area`.
///
/// `stacked` chooses between panes side by side (task unit `H1`'s 80×24
/// mock-up) and panes stacked top to bottom, which
/// [`App::should_stack_panes`](crate::app::state::App::should_stack_panes)
/// asks for below 80 columns or 24 rows. Every constraint here is a
/// percentage, a length, or a minimum, so [`Layout::split`] always
/// succeeds — a pathologically small `area` shrinks a region to zero rather
/// than failing.
#[must_use]
pub fn compute(area: Rect, stacked: bool) -> AppLayout {
    if area.width == 0 || area.height == 0 {
        return AppLayout::EMPTY;
    }

    let outer = area;
    let inner = outer.inner(Margin::new(1, 1));

    let rows = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let panes_area = rows[0];
    let command_bar = rows[1];
    let function_keys = rows[2];

    let direction = if stacked {
        Direction::Vertical
    } else {
        Direction::Horizontal
    };
    let panes = Layout::new(
        direction,
        [Constraint::Percentage(50), Constraint::Percentage(50)],
    )
    .split(panes_area);

    AppLayout {
        outer,
        left_pane: panes[0],
        right_pane: panes[1],
        command_bar,
        function_keys,
        stacked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_area_produces_an_empty_layout_without_panicking() {
        let layout = compute(Rect::new(0, 0, 0, 0), false);
        assert_eq!(layout.outer, Rect::ZERO);
    }

    #[test]
    fn side_by_side_puts_the_right_pane_after_the_left_pane() {
        let layout = compute(Rect::new(0, 0, 80, 24), false);
        assert!(layout.left_pane.x < layout.right_pane.x);
        assert_eq!(layout.left_pane.y, layout.right_pane.y);
    }

    #[test]
    fn stacked_puts_the_right_pane_below_the_left_pane() {
        let layout = compute(Rect::new(0, 0, 80, 24), true);
        assert_eq!(layout.left_pane.x, layout.right_pane.x);
        assert!(layout.left_pane.y < layout.right_pane.y);
    }

    #[test]
    fn the_command_bar_and_function_keys_are_each_one_row() {
        let layout = compute(Rect::new(0, 0, 80, 24), false);
        assert_eq!(layout.command_bar.height, 1);
        assert_eq!(layout.function_keys.height, 1);
    }

    #[test]
    fn every_region_stays_inside_the_terminal_area_at_every_size_from_zero_to_two_hundred() {
        for width in 0..40u16 {
            for height in 0..30u16 {
                let area = Rect::new(0, 0, width, height);
                let layout = compute(area, width < 80 || height < 24);
                for region in [
                    layout.left_pane,
                    layout.right_pane,
                    layout.command_bar,
                    layout.function_keys,
                ] {
                    assert!(region.x + region.width <= area.width);
                    assert!(region.y + region.height <= area.height);
                }
            }
        }
    }

    #[test]
    fn a_large_terminal_still_lays_out_cleanly() {
        let layout = compute(Rect::new(0, 0, 200, 60), false);
        assert_eq!(layout.outer, Rect::new(0, 0, 200, 60));
        assert!(layout.left_pane.width > 0);
        assert!(layout.right_pane.width > 0);
    }
}
