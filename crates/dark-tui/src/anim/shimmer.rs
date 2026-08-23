//! The slow shimmer applied to the fog map's luminance.
//!
//! See task unit `H3`, rule 8: "Add a slow shimmer. Apply a phase-offset
//! sine to the luminance of each cell at 0.15 Hz."

use std::f32::consts::TAU;

use super::hash::stable_hash;

/// How many full cycles the shimmer completes each second.
pub const SHIMMER_HZ: f32 = 0.15;

/// Returns a shimmer multiplier for one cell, in `-1.0..=1.0`.
///
/// `time_secs` is the elapsed time since the shimmer's clock started, in
/// seconds. A caller drives this from the same tick that already advances
/// the rest of the shell (see task unit `H3`'s "driven by the tick the
/// application already has") — never from a clock this function reads for
/// itself, which is why the function takes the time rather than holding it.
/// `phase_offset`, in radians, staggers one cell from another so the whole
/// map does not brighten and dim in lockstep; see [`phase_offset_for`].
#[must_use]
pub fn shimmer(time_secs: f32, phase_offset: f32) -> f32 {
    (TAU * SHIMMER_HZ).mul_add(time_secs, phase_offset).sin()
}

/// Derives a stable phase offset for an identifier, in `0.0..TAU`.
///
/// Two calls with the same `id` return the same offset, so the shimmer on
/// the same ticket looks the same from run to run — the same determinism
/// rule that [`crate::views::fogmap::compute_layout`] holds its layout to.
#[must_use]
pub fn phase_offset_for(id: &str) -> f32 {
    // Prime, so consecutive ids do not alias. Reducing to a fixed-size
    // fraction before the cast below keeps the value exactly representable
    // in an `f32`, so this never loses precision it would need to stay
    // deterministic.
    const BUCKETS: u64 = 1_000_003;
    let hash = stable_hash(id);
    #[allow(
        clippy::cast_precision_loss,
        reason = "hash % BUCKETS is below 1_000_003, far inside f32's exact integer range"
    )]
    let fraction = (hash % BUCKETS) as f32 / BUCKETS as f32;
    fraction * TAU
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_starts_at_the_sine_of_the_phase_offset() {
        assert!((shimmer(0.0, 0.0) - 0.0).abs() < 1e-6);
        let expected = std::f32::consts::FRAC_PI_2.sin();
        assert!((shimmer(0.0, std::f32::consts::FRAC_PI_2) - expected).abs() < 1e-6);
    }

    #[test]
    fn shimmer_stays_within_one() {
        let mut t = 0.0_f32;
        while t < 20.0 {
            let value = shimmer(t, 1.3);
            assert!(
                (-1.0..=1.0).contains(&value),
                "{value} out of range at t={t}"
            );
            t += 0.037;
        }
    }

    #[test]
    fn shimmer_completes_one_cycle_every_1_over_0_15_hz_seconds() {
        let period = 1.0 / SHIMMER_HZ;
        let start = shimmer(0.0, 0.4);
        let one_period_later = shimmer(period, 0.4);
        assert!((start - one_period_later).abs() < 1e-3);
    }

    #[test]
    fn phase_offset_is_stable_for_the_same_id() {
        assert!((phase_offset_for("T-018") - phase_offset_for("T-018")).abs() < f32::EPSILON);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "this checks that two independent hashes did not collide, not that a computed \
                  float matches an expected one within tolerance"
    )]
    fn phase_offset_differs_for_different_ids_in_the_common_case() {
        assert_ne!(phase_offset_for("T-018"), phase_offset_for("T-019"));
    }

    #[test]
    fn phase_offset_stays_in_the_documented_range() {
        for id in ["", "a", "T-001", "a very long ticket identifier indeed"] {
            let offset = phase_offset_for(id);
            assert!(
                (0.0..TAU).contains(&offset),
                "{offset} out of range for {id:?}"
            );
        }
    }
}
