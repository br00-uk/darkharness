//! The tools that the harness offers to a model.
//!
//! Each module holds one family of tools. The registry decides which of them a
//! given model sees, because a small model handles fewer tools well. See task
//! unit `C4`.

pub mod exec;
pub mod fs;
pub mod registry;
pub mod search;
