//! What each pane shows, and which pane holds focus.

/// What the left pane shows.
///
/// `F2` jumps straight to [`LeftPane::Map`]. `Ctrl+Left`/`Ctrl+Right` cycle
/// through every variant while the left pane holds focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeftPane {
    /// The fog map (task unit `H3`).
    #[default]
    Map,
    /// The repository file tree.
    Files,
    /// The seam graph between crates.
    Seams,
    /// The documentation packs.
    Packs,
}

impl LeftPane {
    /// Every variant, in cycle order.
    const ORDER: [Self; 4] = [Self::Map, Self::Files, Self::Seams, Self::Packs];

    /// Returns the next pane in cycle order.
    #[must_use]
    pub fn next(self) -> Self {
        cycle(&Self::ORDER, self, true)
    }

    /// Returns the previous pane in cycle order.
    #[must_use]
    pub fn prev(self) -> Self {
        cycle(&Self::ORDER, self, false)
    }

    /// Returns the title this pane shows in its border.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Map => "MAP",
            Self::Files => "FILES",
            Self::Seams => "SEAMS",
            Self::Packs => "PACKS",
        }
    }
}

/// What the right pane shows.
///
/// `F4` jumps straight to [`RightPane::Diff`], `F5` to [`RightPane::Explore`].
/// `Ctrl+Left`/`Ctrl+Right` cycle through every variant while the right pane
/// holds focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightPane {
    /// The running turn's transcript (task unit `H4`).
    #[default]
    Transcript,
    /// A unified diff for a mutating tool result (task unit `H4`).
    Diff,
    /// A rendered documentation page.
    Doc,
    /// A repository analysis result.
    Explore,
}

impl RightPane {
    /// Every variant, in cycle order.
    const ORDER: [Self; 4] = [Self::Transcript, Self::Diff, Self::Doc, Self::Explore];

    /// Returns the next pane in cycle order.
    #[must_use]
    pub fn next(self) -> Self {
        cycle(&Self::ORDER, self, true)
    }

    /// Returns the previous pane in cycle order.
    #[must_use]
    pub fn prev(self) -> Self {
        cycle(&Self::ORDER, self, false)
    }

    /// Returns the title this pane shows in its border.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Transcript => "TRANSCRIPT",
            Self::Diff => "DIFF",
            Self::Doc => "DOC",
            Self::Explore => "EXPLORE",
        }
    }
}

/// Moves `current` by `delta` positions through `order`, wrapping at each
/// end. `forward` of `true` returns the next entry; `false` returns the
/// previous one. Staying in `usize` throughout, rather than moving to a
/// signed type and back, needs no cast at all.
fn cycle<T: Copy + PartialEq>(order: &[T], current: T, forward: bool) -> T {
    let Some(index) = order.iter().position(|&entry| entry == current) else {
        return current;
    };
    let len = order.len();
    let next_index = if forward {
        (index + 1) % len
    } else {
        (index + len - 1) % len
    };
    order[next_index]
}

/// Which region of the shell has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    /// The left pane.
    #[default]
    Left,
    /// The right pane.
    Right,
    /// The command bar at the bottom of the screen.
    Command,
}

impl Focus {
    /// Returns the next region in tab order.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Command,
            Self::Command => Self::Left,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_pane_cycles_forward_through_every_variant_and_wraps() {
        let mut pane = LeftPane::Map;
        let mut seen = vec![pane];
        for _ in 0..3 {
            pane = pane.next();
            seen.push(pane);
        }
        assert_eq!(
            seen,
            vec![
                LeftPane::Map,
                LeftPane::Files,
                LeftPane::Seams,
                LeftPane::Packs
            ]
        );
        assert_eq!(pane.next(), LeftPane::Map);
    }

    #[test]
    fn left_pane_prev_undoes_next() {
        let pane = LeftPane::Seams;
        assert_eq!(pane.next().prev(), pane);
        assert_eq!(pane.prev().next(), pane);
    }

    #[test]
    fn left_pane_prev_wraps_at_the_start() {
        assert_eq!(LeftPane::Map.prev(), LeftPane::Packs);
    }

    #[test]
    fn right_pane_cycles_forward_through_every_variant_and_wraps() {
        let mut pane = RightPane::Transcript;
        let mut seen = vec![pane];
        for _ in 0..3 {
            pane = pane.next();
            seen.push(pane);
        }
        assert_eq!(
            seen,
            vec![
                RightPane::Transcript,
                RightPane::Diff,
                RightPane::Doc,
                RightPane::Explore
            ]
        );
        assert_eq!(pane.next(), RightPane::Transcript);
    }

    #[test]
    fn focus_cycles_left_right_command_and_wraps() {
        assert_eq!(Focus::Left.next(), Focus::Right);
        assert_eq!(Focus::Right.next(), Focus::Command);
        assert_eq!(Focus::Command.next(), Focus::Left);
    }

    #[test]
    fn every_left_pane_has_a_distinct_title() {
        let titles: Vec<&str> = LeftPane::ORDER.iter().map(|p| p.title()).collect();
        let unique: std::collections::HashSet<&str> = titles.iter().copied().collect();
        assert_eq!(titles.len(), unique.len());
    }

    #[test]
    fn every_right_pane_has_a_distinct_title() {
        let titles: Vec<&str> = RightPane::ORDER.iter().map(|p| p.title()).collect();
        let unique: std::collections::HashSet<&str> = titles.iter().copied().collect();
        assert_eq!(titles.len(), unique.len());
    }
}
