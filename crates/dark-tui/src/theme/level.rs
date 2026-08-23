//! Colour degradation for terminals that cannot show every colour.
//!
//! Not every terminal understands a 24-bit colour. This module detects what
//! the terminal supports and converts a colour down to that level.

use ratatui::style::Color;

/// The fixed 16-colour ANSI palette, in the order that [`Color`] enumerates
/// its own named variants.
///
/// These are the conventional values that most terminal emulators use for
/// the standard 16 colours. [`ColorLevel::downgrade`] measures distance
/// against this table to pick the closest match.
const ANSI16: [(Color, (u8, u8, u8)); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (205, 49, 49)),
    (Color::Green, (13, 188, 121)),
    (Color::Yellow, (229, 229, 16)),
    (Color::Blue, (36, 114, 200)),
    (Color::Magenta, (188, 63, 188)),
    (Color::Cyan, (17, 168, 205)),
    (Color::Gray, (229, 229, 229)),
    (Color::DarkGray, (102, 102, 102)),
    (Color::LightRed, (241, 76, 76)),
    (Color::LightGreen, (35, 209, 139)),
    (Color::LightYellow, (245, 245, 67)),
    (Color::LightBlue, (59, 142, 234)),
    (Color::LightMagenta, (214, 112, 214)),
    (Color::LightCyan, (41, 184, 219)),
    (Color::White, (255, 255, 255)),
];

/// How many distinct colours the terminal can show.
///
/// The application picks a level once at start-up (see [`ColorLevel::detect`])
/// and downgrades the whole palette to it. Every level still conveys state
/// through more than colour alone; see [`crate::theme::density`] for the
/// fallback that carries the fog map's meaning without colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorLevel {
    /// 24-bit colour. Every token keeps its exact value.
    #[default]
    TrueColor,
    /// The 256-colour xterm palette.
    Ansi256,
    /// The 16-colour ANSI palette.
    Ansi16,
    /// No colour. The application relies on glyphs and density characters.
    None,
}

impl ColorLevel {
    /// Detects the level from the environment.
    ///
    /// `NO_COLOR` (see <https://no-color.org>) forces [`ColorLevel::None`]
    /// whenever it is set, regardless of its value. Otherwise `COLORTERM` of
    /// `truecolor` or `24bit` selects [`ColorLevel::TrueColor`]. A `TERM` of
    /// `dumb`, or a `TERM` that names a 256-colour terminal, refines the
    /// remaining case. Every other environment falls back to
    /// [`ColorLevel::Ansi16`], the level that the most terminals support.
    #[must_use]
    pub fn detect() -> Self {
        Self::detect_from(|key| std::env::var(key).ok())
    }

    /// Detects the level from an arbitrary variable lookup.
    ///
    /// [`ColorLevel::detect`] calls this with a lookup backed by the real
    /// process environment. Reading `std::env` directly from the tests below
    /// would race between tests, and mutating it needs `unsafe`, which this
    /// crate forbids, so the tests call this function with a fake lookup
    /// instead.
    fn detect_from(lookup: impl Fn(&str) -> Option<String>) -> Self {
        if lookup("NO_COLOR").is_some() {
            return Self::None;
        }
        if let Some(colorterm) = lookup("COLORTERM")
            && (colorterm == "truecolor" || colorterm == "24bit")
        {
            return Self::TrueColor;
        }
        let term = lookup("TERM").unwrap_or_default();
        if term == "dumb" {
            return Self::None;
        }
        if term.contains("256color") {
            return Self::Ansi256;
        }
        Self::Ansi16
    }

    /// Converts a colour down to this level.
    ///
    /// A colour that already fits the level, for example a named ANSI
    /// colour passed to [`ColorLevel::Ansi16`], returns unchanged.
    #[must_use]
    pub fn downgrade(self, color: Color) -> Color {
        match self {
            Self::TrueColor => color,
            Self::Ansi256 => downgrade_to_256(color),
            Self::Ansi16 => downgrade_to_16(color),
            Self::None => Color::Reset,
        }
    }
}

/// Converts an rgb colour to the nearest xterm 256-colour index.
///
/// The 256-colour palette starts with the same 16 ANSI colours (indices `0`
/// to `15`), then a 6×6×6 colour cube (indices `16` to `231`), then a
/// 24-step grey ramp (indices `232` to `255`). This function targets the
/// cube and the grey ramp, since a true-colour source rarely lands exactly
/// on one of the first 16 index values anyway.
fn downgrade_to_256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let to_cube_step = |channel: u8| -> u8 {
        // The cube steps are 0, 95, 135, 175, 215, 255. Find the closest one.
        const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        STEPS
            .iter()
            .copied()
            .min_by_key(|step| channel.abs_diff(*step))
            .unwrap_or(0)
    };
    let cube = |channel: u8| -> u16 {
        match to_cube_step(channel) {
            0 => 0,
            95 => 1,
            135 => 2,
            175 => 3,
            215 => 4,
            _ => 5,
        }
    };
    let cube_r = cube(r);
    let cube_g = cube(g);
    let cube_b = cube(b);
    let cube_index = 16 + 36 * cube_r + 6 * cube_g + cube_b;

    // Also test the grey ramp, then keep whichever candidate is closer.
    let luma = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey_step = (luma.saturating_sub(8) / 10).min(23);
    let grey_level = 8 + grey_step * 10;
    let grey_index = 232 + grey_step;

    let cube_rgb = (
        to_cube_step(r).abs_diff(r),
        to_cube_step(g).abs_diff(g),
        to_cube_step(b).abs_diff(b),
    );
    let cube_distance = u32::from(cube_rgb.0) + u32::from(cube_rgb.1) + u32::from(cube_rgb.2);
    #[allow(
        clippy::cast_possible_truncation,
        reason = "grey_level fits u8: 8 + (0..=23)*10 is at most 238"
    )]
    let grey_level = grey_level as u8;
    let grey_distance = u32::from(grey_level.abs_diff(r))
        + u32::from(grey_level.abs_diff(g))
        + u32::from(grey_level.abs_diff(b));

    #[allow(
        clippy::cast_possible_truncation,
        reason = "both index values stay within 16..=255"
    )]
    if grey_distance < cube_distance {
        Color::Indexed(grey_index as u8)
    } else {
        Color::Indexed(cube_index as u8)
    }
}

/// Converts an rgb colour to the nearest of the 16 ANSI colours.
fn downgrade_to_16(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    ANSI16
        .iter()
        .min_by_key(|(_, (cr, cg, cb))| {
            let dr = i32::from(r).abs_diff(i32::from(*cr));
            let dg = i32::from(g).abs_diff(i32::from(*cg));
            let db = i32::from(b).abs_diff(i32::from(*cb));
            dr + dg + db
        })
        .map_or(Color::Reset, |(named, _)| *named)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a fake variable lookup from a fixed list of pairs.
    ///
    /// A variable absent from `vars` reads as unset, exactly like a real
    /// environment that never had it exported.
    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn no_color_forces_none_even_with_truecolor_set() {
        let lookup = env(&[("NO_COLOR", "1"), ("COLORTERM", "truecolor")]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::None);
    }

    #[test]
    fn colorterm_truecolor_selects_truecolor() {
        let lookup = env(&[("COLORTERM", "truecolor")]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::TrueColor);
    }

    #[test]
    fn a_dumb_term_selects_none() {
        let lookup = env(&[("TERM", "dumb")]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::None);
    }

    #[test]
    fn a_256color_term_selects_ansi_256() {
        let lookup = env(&[("TERM", "xterm-256color")]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::Ansi256);
    }

    #[test]
    fn an_unrecognised_term_falls_back_to_ansi_16() {
        let lookup = env(&[("TERM", "xterm")]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::Ansi16);
    }

    #[test]
    fn no_variables_at_all_falls_back_to_ansi_16() {
        let lookup = env(&[]);
        assert_eq!(ColorLevel::detect_from(lookup), ColorLevel::Ansi16);
    }

    #[test]
    fn truecolor_leaves_a_colour_unchanged() {
        let c = Color::Rgb(125, 211, 252);
        assert_eq!(ColorLevel::TrueColor.downgrade(c), c);
    }

    #[test]
    fn none_always_resets() {
        assert_eq!(
            ColorLevel::None.downgrade(Color::Rgb(255, 0, 0)),
            Color::Reset
        );
    }

    #[test]
    fn ansi_16_maps_pure_red_to_the_red_family() {
        let downgraded = ColorLevel::Ansi16.downgrade(Color::Rgb(255, 0, 0));
        assert!(matches!(downgraded, Color::Red | Color::LightRed));
    }

    #[test]
    fn ansi_16_maps_black_to_black() {
        assert_eq!(
            ColorLevel::Ansi16.downgrade(Color::Rgb(0, 0, 0)),
            Color::Black
        );
    }

    #[test]
    fn ansi_256_produces_an_indexed_colour_in_range() {
        let downgraded = ColorLevel::Ansi256.downgrade(Color::Rgb(125, 211, 252));
        let Color::Indexed(index) = downgraded else {
            panic!("expected an indexed colour, got {downgraded:?}");
        };
        assert!((16..=255).contains(&index));
    }

    #[test]
    fn ansi_256_maps_a_neutral_grey_into_the_grey_ramp() {
        let downgraded = ColorLevel::Ansi256.downgrade(Color::Rgb(128, 128, 128));
        let Color::Indexed(index) = downgraded else {
            panic!("expected an indexed colour, got {downgraded:?}");
        };
        assert!((232..=255).contains(&index));
    }

    #[test]
    fn a_named_colour_is_unaffected_by_downgrade() {
        assert_eq!(ColorLevel::Ansi16.downgrade(Color::Red), Color::Red);
        assert_eq!(ColorLevel::Ansi256.downgrade(Color::Red), Color::Red);
    }
}
