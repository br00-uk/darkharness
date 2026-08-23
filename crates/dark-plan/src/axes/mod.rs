//! Stage 3 of the charting pipeline: the axis sweep. Task unit `E2`.
//!
//! Replaces wide thinking with enumeration against a fixed list: a 32B
//! model goes deep on one thread instead of wide across the space (see the
//! weakness table in the `E` section preamble of `PRD.md`), and wide
//! thinking is hard for it, but answering one narrow question at a time is
//! not.
//!
//! [`set`] defines the three built-in axis sets and the destination type
//! that selects one. [`sweep`] runs the sweep itself, one generation per
//! axis, against `crate::chart::sampling::run_generation`.

pub mod set;
pub mod sweep;

pub use set::{AxisSet, AxisSets, DestinationType};
pub use sweep::{AxisAnswer, AxisOutcome, run_axis_sweep};
