//! Repository analysis: symbols, graphs, metrics, and seams.
//!
//! Stages 1 to 5 use no model and must produce identical bytes for the same
//! commit and configuration. Sort paths with a byte comparator, fix every
//! seed and visit order, and keep a timestamp out of hashed output. See
//! Rules 29 to 32 and task units `F1` to `F5`.

pub mod discover;
pub mod extract;
pub mod graph;
pub mod narrate;
pub mod output;
pub mod seam;
pub mod syntax;
