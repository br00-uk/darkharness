//! Animation for the fog map (task unit `H3`).
//!
//! Nothing in this module owns a thread or a timer. Every function and
//! every [`Spring::update`] call takes the elapsed time as a parameter and
//! returns a new value, so a caller supplies the same tick it already
//! advances the rest of the shell with (see
//! [`crate::app::App::tick`](../app/struct.App.html#method.tick)), and a
//! golden-frame test can pin any moment in an animation exactly by choosing
//! what it passes in — see task unit `H3`: "do not make a frame depend on
//! wall-clock time in a way a golden test cannot pin."
//!
//! - [`Spring`] integrates a damped harmonic oscillator, for camera
//!   movement, pane transitions, and the dark-mode colour change.
//! - [`shimmer`] and [`phase_offset_for`] give the fog map's slow luminance
//!   shimmer.
//! - [`FrameBudget`] tracks the fog map's 8-millisecond frame budget and
//!   says what decoration to drop when a frame runs over it.
//! - [`AnimationGate`] says whether the fog map should animate at all —
//!   `TERM=dumb`, a non-terminal output, `DARK_NO_ANIM`, a window without
//!   focus, or three consecutive slow frames all turn it off.
//! - [`stable_hash`] gives a fixed, cross-build hash that both the fog
//!   map's layout and its shimmer phase use to stay deterministic.

pub mod budget;
pub mod gate;
pub mod hash;
pub mod shimmer;
pub mod spring;

pub use budget::{DetailLevel, FrameBudget};
pub use gate::AnimationGate;
pub use hash::stable_hash;
pub use shimmer::{SHIMMER_HZ, phase_offset_for, shimmer};
pub use spring::Spring;
