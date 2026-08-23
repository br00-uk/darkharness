//! A damped-spring integrator for animation.
//!
//! Task unit `H3` names three uses: camera movement, pane transitions, and
//! the dark-mode colour change. [`Spring`] itself does not know which of
//! those it drives — it advances one number toward one target, and a caller
//! runs one instance for each thing that moves.

/// A one-dimensional damped harmonic oscillator.
///
/// [`Spring::new`] takes a fixed frame time, angular frequency, and damping
/// ratio, and folds them into four coefficients that [`Spring::update`]
/// applies without a branch or a call to a trigonometric function. Building
/// a fresh [`Spring`] is the way to change any of the three inputs — the
/// coefficients are not meant to be recomputed on every frame.
///
/// # Numerical method
///
/// The underlying equation of motion is the damped harmonic oscillator
/// ```text
/// x'' + 2 * damping * freq * x' + freq^2 * (x - target) = 0
/// ```
/// [`Spring::new`] discretises it with semi-implicit (symplectic) Euler at
/// the fixed `dt` supplied, rather than the exact closed-form solution
/// (which branches on whether `damping` is below, at, or above `1.0`, and
/// needs `sin`, `cos`, and `exp`). Semi-implicit Euler needs none of that —
/// [`Spring::update`] is four multiplies and four adds — and stays close to
/// the exact solution and numerically stable for the small `freq * dt` a
/// 60-hertz-class redraw loop produces (task unit `H3` suggests `freq` of
/// `6.0`; at a 16-millisecond tick, `freq * dt` is about `0.1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// The current position's coefficient in the position update.
    pos_pos: f32,
    /// The current velocity's coefficient in the position update.
    pos_vel: f32,
    /// The current position's coefficient in the velocity update.
    vel_pos: f32,
    /// The current velocity's coefficient in the velocity update.
    vel_vel: f32,
}

impl Spring {
    /// Builds a spring for a fixed frame time, frequency, and damping ratio.
    ///
    /// `dt` is the time each [`Spring::update`] call advances, in seconds.
    /// `freq` is the natural angular frequency, in radians each second —
    /// task unit `H3` suggests `6.0`. `damping` is the damping ratio: `1.0`
    /// is critical, below `1.0` bounces past the target, above `1.0`
    /// approaches it slower than critical.
    #[must_use]
    pub fn new(dt: f32, freq: f32, damping: f32) -> Self {
        let spring = freq * freq * dt;
        let damp = 2.0 * damping * freq * dt;
        let vel_vel = 1.0 - damp;
        let vel_pos = -spring;
        let pos_vel = dt * vel_vel;
        let pos_pos = 1.0 - dt * spring;
        Self {
            pos_pos,
            pos_vel,
            vel_pos,
            vel_vel,
        }
    }

    /// Advances `pos` and `vel` by one frame toward `target`.
    ///
    /// The caller owns `pos` and `vel` between calls — a [`Spring`] holds
    /// no state of its own beyond its four coefficients, so the same
    /// instance can drive as many independent `(pos, vel)` pairs as a
    /// caller needs, one call each.
    pub fn update(&self, pos: &mut f32, vel: &mut f32, target: f32) {
        let (p, v) = (*pos, *vel);
        *pos = p.mul_add(self.pos_pos, v * self.pos_vel) + target * (1.0 - self.pos_pos);
        *vel = p.mul_add(self.vel_pos, v * self.vel_vel) + target * -self.vel_pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    const FREQ: f32 = 6.0;

    fn simulate(damping: f32, target: f32, steps: usize) -> Vec<f32> {
        let spring = Spring::new(DT, FREQ, damping);
        let mut pos = 0.0_f32;
        let mut vel = 0.0_f32;
        let mut trace = Vec::with_capacity(steps);
        for _ in 0..steps {
            spring.update(&mut pos, &mut vel, target);
            trace.push(pos);
        }
        trace
    }

    #[test]
    fn critical_damping_settles_at_the_target_without_overshooting() {
        let trace = simulate(1.0, 1.0, 240);
        for &p in &trace {
            assert!(p <= 1.02, "critical damping overshot the target: {p}");
        }
        let last = *trace.last().expect("240 steps produced 240 values");
        assert!((last - 1.0).abs() < 0.01, "spring never settled: {last}");
    }

    #[test]
    fn light_damping_bounces_past_the_target() {
        let trace = simulate(0.15, 1.0, 240);
        assert!(
            trace.iter().any(|&p| p > 1.05),
            "an underdamped spring should overshoot the target at least once"
        );
    }

    #[test]
    fn heavy_damping_approaches_slower_than_critical() {
        let critical = simulate(1.0, 1.0, 20);
        let heavy = simulate(6.0, 1.0, 20);
        let critical_last = *critical.last().expect("20 steps produced 20 values");
        let heavy_last = *heavy.last().expect("20 steps produced 20 values");
        assert!(
            heavy_last < critical_last,
            "an overdamped spring ({heavy_last}) should trail a critically damped one \
             ({critical_last}) after the same number of steps"
        );
    }

    #[test]
    fn a_spring_already_at_the_target_and_at_rest_stays_put() {
        let spring = Spring::new(DT, FREQ, 1.0);
        let mut pos = 1.0;
        let mut vel = 0.0;
        for _ in 0..10 {
            spring.update(&mut pos, &mut vel, 1.0);
        }
        assert!((pos - 1.0).abs() < 1e-6);
        assert!(vel.abs() < 1e-6);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "this checks that two identical computations agree bit for bit, which is the \
                  point of the test — a tolerance-based comparison would hide the very drift it \
                  exists to catch"
    )]
    fn updating_is_a_pure_function_of_the_stored_coefficients_and_the_inputs() {
        // No clock, no thread, no interior mutability: the same starting
        // state fed through the same spring twice must agree exactly. See
        // task unit `H3`: "do not make a frame depend on wall-clock time in
        // a way a golden test cannot pin."
        let spring = Spring::new(DT, FREQ, 0.6);
        let (mut pos_a, mut vel_a) = (0.2_f32, -0.1_f32);
        let (mut pos_b, mut vel_b) = (0.2_f32, -0.1_f32);
        for _ in 0..30 {
            spring.update(&mut pos_a, &mut vel_a, 1.0);
            spring.update(&mut pos_b, &mut vel_b, 1.0);
        }
        assert_eq!(pos_a, pos_b);
        assert_eq!(vel_a, vel_b);
    }

    #[test]
    fn a_negative_target_pulls_the_position_negative() {
        let trace = simulate(1.0, -1.0, 120);
        let last = *trace.last().expect("120 steps produced 120 values");
        assert!((last + 1.0).abs() < 0.01, "spring never settled: {last}");
    }
}
