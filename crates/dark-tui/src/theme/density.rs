//! ASCII density characters.
//!
//! In 16-colour mode and no-colour mode there are too few colours to show
//! the fog map's range of state. A density ramp carries the same
//! information through shape instead: a sparse character reads as empty, a
//! dense one reads as full.

/// The density ramp, from emptiest to fullest.
///
/// The fog map (task unit `H3`) walks this ramp with [`density_char`] to
/// pick a glyph for a cell when the colour level cannot carry the
/// distinction.
pub const DENSITY_RAMP: [char; 10] = [' ', '.', ':', '-', '=', '+', '*', '#', '%', '@'];

/// Picks the density character for a value.
///
/// `value` is clamped to `0.0..=1.0` first, so a value of `0.0` always
/// returns the emptiest glyph and a value of `1.0` always returns the
/// fullest one.
#[must_use]
pub fn density_char(value: f32) -> char {
    let value = value.clamp(0.0, 1.0);
    let last = DENSITY_RAMP.len() - 1;
    #[allow(
        clippy::cast_precision_loss,
        reason = "DENSITY_RAMP.len() is a small constant, far below f32's exact integer range"
    )]
    let scaled = value * last as f32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scaled is clamped to 0.0..=last as f32 immediately before the cast"
    )]
    let index = scaled.round() as usize;
    DENSITY_RAMP[index.min(last)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_is_the_emptiest_glyph() {
        assert_eq!(density_char(0.0), ' ');
    }

    #[test]
    fn one_is_the_fullest_glyph() {
        assert_eq!(density_char(1.0), '@');
    }

    #[test]
    fn out_of_range_values_clamp() {
        assert_eq!(density_char(-1.0), ' ');
        assert_eq!(density_char(2.0), '@');
    }

    #[test]
    fn the_midpoint_lands_in_the_middle_of_the_ramp() {
        // The ramp has 10 entries (indices 0..=9), so 0.5 scales to 4.5,
        // exactly between two glyphs. `density_char` rounds that half away
        // from zero, landing on index 5 rather than 4; either neighbour of
        // the true midpoint is a reasonable place to land, so this checks
        // for "close to the middle" rather than pinning the rounding rule.
        let c = density_char(0.5);
        let index = DENSITY_RAMP.iter().position(|&g| g == c).unwrap();
        #[allow(
            clippy::cast_precision_loss,
            reason = "DENSITY_RAMP.len() and a ramp index are both far below f32's exact integer \
                      range"
        )]
        let (index, middle) = (index as f32, (DENSITY_RAMP.len() - 1) as f32 / 2.0);
        let distance = (index - middle).abs();
        assert!(
            distance <= 1.0,
            "index {index} is not close to the middle ({middle})"
        );
    }

    #[test]
    fn density_is_monotonic_across_the_ramp() {
        let mut last_index = 0;
        for step in 0..=10u8 {
            let value = f32::from(step) / 10.0;
            let c = density_char(value);
            let index = DENSITY_RAMP.iter().position(|&g| g == c).unwrap();
            assert!(
                index >= last_index,
                "density must not decrease as value rises"
            );
            last_index = index;
        }
    }
}
