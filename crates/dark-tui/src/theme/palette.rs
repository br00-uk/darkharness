//! The accretion-disk palette.
//!
//! The model is an accretion disk seen near edge-on. The limb that turns
//! toward the viewer is bright and blue. The limb that turns away is dim and
//! red.

use ratatui::style::Color;

/// One named colour in the accretion-disk palette.
///
/// Each field is a token. Code names a token, never a raw colour, so a
/// reader can tell what a colour means without checking a hex value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// `#05060A`. The application background.
    pub singularity: Color,
    /// `#0B0D14`. The panel background.
    pub horizon: Color,
    /// `#FFF4D6`. Selection, cursor, and the focused border.
    pub photon_ring: Color,
    /// `#FFC15E`. The bright inner edge of the disk. Used for a load sweep.
    pub disk_inner: Color,
    /// `#FF7A18`. The primary accent. Active work.
    pub disk_mid: Color,
    /// `#C2410C`. The outer edge of the disk.
    pub disk_outer: Color,
    /// `#7C2D12`. Resolved and historical work.
    pub ember: Color,
    /// `#7DD3FC`. The approaching limb: the frontier, takeable now.
    pub doppler_blue: Color,
    /// `#38536B`. The receding limb: blocked, waiting.
    pub doppler_dim: Color,
    /// `#1E293B`. Dithered: not yet specified.
    pub fog: Color,
    /// `#0F172A`. Out of scope, outside the disk.
    pub void: Color,
    /// `#E2E8F0`. Primary text.
    pub text: Color,
    /// `#64748B`. De-emphasised text.
    pub text_dim: Color,
    /// `#F43F5E`. A failure, or the dark-mode warning.
    pub danger: Color,
    /// `#34D399`. Success.
    pub ok: Color,
    /// `#FBBF24`. A caution.
    pub warn: Color,
}

impl Palette {
    /// Builds the one palette that the harness uses.
    ///
    /// See task unit `H2` in `PRD.md` for the source of every value.
    #[must_use]
    pub const fn accretion_disk() -> Self {
        Self {
            singularity: Color::Rgb(5, 6, 10),
            horizon: Color::Rgb(11, 13, 20),
            photon_ring: Color::Rgb(255, 244, 214),
            disk_inner: Color::Rgb(255, 193, 94),
            disk_mid: Color::Rgb(255, 122, 24),
            disk_outer: Color::Rgb(194, 65, 12),
            ember: Color::Rgb(124, 45, 18),
            doppler_blue: Color::Rgb(125, 211, 252),
            doppler_dim: Color::Rgb(56, 83, 107),
            fog: Color::Rgb(30, 41, 59),
            void: Color::Rgb(15, 23, 42),
            text: Color::Rgb(226, 232, 240),
            text_dim: Color::Rgb(100, 116, 139),
            danger: Color::Rgb(244, 63, 94),
            ok: Color::Rgb(52, 211, 153),
            warn: Color::Rgb(251, 191, 36),
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::accretion_disk()
    }
}

/// Extracts the red, green, and blue components of a colour.
///
/// Returns `None` for a colour that carries no rgb triple, for example
/// [`Color::Reset`] or a named ANSI colour.
const fn rgb_components(color: Color) -> Option<(u8, u8, u8)> {
    match color {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

/// Blends two colours by a fraction `t`.
///
/// `t` of `0.0` returns `from`. `t` of `1.0` returns `to`. A `t` outside
/// `0.0..=1.0` is clamped first. Both colours must carry an rgb triple; a
/// colour that does not falls back to a step at the midpoint, which keeps
/// the function total under every colour level.
#[must_use]
pub fn gradient(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (rgb_components(from), rgb_components(to)) {
        (Some((r0, g0, b0)), Some((r1, g1, b1))) => {
            Color::Rgb(lerp_u8(r0, r1, t), lerp_u8(g0, g1, t), lerp_u8(b0, b1, t))
        }
        _ => {
            if t < 0.5 {
                from
            } else {
                to
            }
        }
    }
}

/// Linearly interpolates one colour channel.
///
/// The result of `round` on a value clamped to `0.0..=255.0` always fits in
/// a `u8`, so the cast below never truncates or loses a sign.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0.0..=255.0 immediately before the cast"
)]
fn lerp_u8(from: u8, to: u8, t: f32) -> u8 {
    let from = f32::from(from);
    let to = f32::from(to);
    let value = from + (to - from) * t;
    value.round().clamp(0.0, 255.0) as u8
}

/// Moves a colour toward grey by a fraction `amount`.
///
/// `amount` of `0.0` returns the colour unchanged. `amount` of `1.0` returns
/// its perceptual grey. A colour with no rgb triple is returned unchanged,
/// since a named ANSI colour has no grey point to move toward.
#[must_use]
pub fn desaturate(color: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    let Some((r, g, b)) = rgb_components(color) else {
        return color;
    };
    // ITU-R BT.601 luma weights, the usual choice for a perceptual grey.
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    let grey = luma.round().clamp(0.0, 255.0);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "grey is clamped to 0.0..=255.0 immediately before the cast"
    )]
    let grey = grey as u8;
    gradient(color, Color::Rgb(grey, grey, grey), amount)
}

/// Blends a colour toward a bright peak and back, for a "pulsing" state.
///
/// `phase` runs `0.0..=1.0` across one pulse cycle. The caller advances
/// `phase` over time; this function holds no clock of its own, in keeping
/// with the rule that a display component never owns a clock (see
/// `CLAUDE.md`, "The context prefix must not change during a turn" — the
/// same discipline applies here: state in, colour out).
#[must_use]
pub fn pulse(base: Color, peak: Color, phase: f32) -> Color {
    let phase = phase.rem_euclid(1.0);
    // A triangle wave: 0 -> 1 over the first half, 1 -> 0 over the second.
    let t = if phase < 0.5 {
        phase * 2.0
    } else {
        (1.0 - phase) * 2.0
    };
    gradient(base, peak, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_at_zero_is_the_start_colour() {
        let p = Palette::accretion_disk();
        assert_eq!(gradient(p.horizon, p.danger, 0.0), p.horizon);
    }

    #[test]
    fn gradient_at_one_is_the_end_colour() {
        let p = Palette::accretion_disk();
        assert_eq!(gradient(p.horizon, p.danger, 1.0), p.danger);
    }

    #[test]
    fn gradient_clamps_an_out_of_range_fraction() {
        let p = Palette::accretion_disk();
        assert_eq!(gradient(p.horizon, p.danger, -5.0), p.horizon);
        assert_eq!(gradient(p.horizon, p.danger, 5.0), p.danger);
    }

    #[test]
    fn gradient_at_the_midpoint_is_between_the_two_colours() {
        let mid = gradient(Color::Rgb(0, 0, 0), Color::Rgb(100, 100, 100), 0.5);
        assert_eq!(mid, Color::Rgb(50, 50, 50));
    }

    #[test]
    fn desaturate_at_zero_is_unchanged() {
        let p = Palette::accretion_disk();
        assert_eq!(desaturate(p.disk_mid, 0.0), p.disk_mid);
    }

    #[test]
    fn desaturate_at_one_has_equal_channels() {
        let p = Palette::accretion_disk();
        let grey = desaturate(p.disk_mid, 1.0);
        let Color::Rgb(r, g, b) = grey else {
            panic!("expected an rgb colour");
        };
        assert_eq!(r, g);
        assert_eq!(g, b);
    }

    #[test]
    fn pulse_returns_to_the_base_at_the_start_and_the_end_of_a_cycle() {
        let base = Color::Rgb(10, 10, 10);
        let peak = Color::Rgb(200, 200, 200);
        assert_eq!(pulse(base, peak, 0.0), base);
        assert_eq!(pulse(base, peak, 1.0), base);
    }

    #[test]
    fn pulse_reaches_the_peak_at_the_middle_of_a_cycle() {
        let base = Color::Rgb(10, 10, 10);
        let peak = Color::Rgb(200, 200, 200);
        assert_eq!(pulse(base, peak, 0.5), peak);
    }

    #[test]
    fn a_colour_with_no_rgb_triple_is_unaffected_by_desaturate() {
        assert_eq!(desaturate(Color::Reset, 1.0), Color::Reset);
    }
}
