//! The mouse zone registry: a map from a screen rectangle to what it means.
//!
//! [`render`] rebuilds the registry every frame, since the layout can change
//! on every redraw (a resize, a pane cycle, a focus change). A mouse event
//! then hit-tests against whatever the most recent frame drew.
//!
//! [`render`]: crate::app::render::render

use ratatui::layout::Rect;

/// One clickable region of the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZoneId {
    /// The title bar.
    Header,
    /// The left pane.
    LeftPane,
    /// The right pane.
    RightPane,
    /// The command bar.
    CommandBar,
    /// One slot of the function-key bar, numbered `1` to `10`.
    FunctionKey(u8),
}

/// Maps a screen rectangle to a [`ZoneId`].
///
/// A later [`ZoneRegistry::register`] call takes priority over an earlier
/// one at the same point, so a caller may register a large region first and
/// a smaller region inside it second.
#[derive(Debug, Clone, Default)]
pub struct ZoneRegistry {
    zones: Vec<(Rect, ZoneId)>,
}

impl ZoneRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self { zones: Vec::new() }
    }

    /// Removes every registered zone.
    pub fn clear(&mut self) {
        self.zones.clear();
    }

    /// Registers a rectangle as a zone.
    ///
    /// A zero-area rectangle (zero width or zero height) is dropped
    /// silently; it can never contain a point, so registering it would only
    /// waste space in the lookup.
    pub fn register(&mut self, rect: Rect, id: ZoneId) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.zones.push((rect, id));
    }

    /// Returns the zone at `(x, y)`, if any.
    ///
    /// Searches the most recently registered zones first, so an overlapping
    /// registration added later wins.
    #[must_use]
    pub fn hit_test(&self, x: u16, y: u16) -> Option<ZoneId> {
        self.zones
            .iter()
            .rev()
            .find(|(rect, _)| contains(*rect, x, y))
            .map(|(_, id)| *id)
    }

    /// Returns the number of registered zones. For tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.zones.len()
    }

    /// Returns true when no zone is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.zones.is_empty()
    }
}

/// Returns true when `(x, y)` falls inside `rect`.
const fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_inside_a_zone_hits_it() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 10, 5), ZoneId::Header);
        assert_eq!(zones.hit_test(3, 2), Some(ZoneId::Header));
    }

    #[test]
    fn a_point_outside_every_zone_misses() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 10, 5), ZoneId::Header);
        assert_eq!(zones.hit_test(20, 20), None);
    }

    #[test]
    fn the_right_edge_is_exclusive() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 10, 5), ZoneId::Header);
        assert_eq!(zones.hit_test(10, 0), None);
        assert_eq!(zones.hit_test(9, 4), Some(ZoneId::Header));
    }

    #[test]
    fn a_later_registration_wins_on_overlap() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 10, 10), ZoneId::LeftPane);
        zones.register(Rect::new(2, 2, 3, 3), ZoneId::FunctionKey(1));
        assert_eq!(zones.hit_test(3, 3), Some(ZoneId::FunctionKey(1)));
        assert_eq!(zones.hit_test(8, 8), Some(ZoneId::LeftPane));
    }

    #[test]
    fn clear_removes_every_zone() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 10, 10), ZoneId::LeftPane);
        zones.clear();
        assert!(zones.is_empty());
        assert_eq!(zones.hit_test(1, 1), None);
    }

    #[test]
    fn a_zero_area_rectangle_is_never_registered() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(5, 5, 0, 10), ZoneId::RightPane);
        zones.register(Rect::new(5, 5, 10, 0), ZoneId::RightPane);
        assert!(zones.is_empty());
    }

    #[test]
    fn function_key_zones_carry_their_own_number() {
        let mut zones = ZoneRegistry::new();
        zones.register(Rect::new(0, 0, 2, 1), ZoneId::FunctionKey(3));
        zones.register(Rect::new(2, 0, 2, 1), ZoneId::FunctionKey(4));
        assert_eq!(zones.hit_test(0, 0), Some(ZoneId::FunctionKey(3)));
        assert_eq!(zones.hit_test(2, 0), Some(ZoneId::FunctionKey(4)));
    }
}
