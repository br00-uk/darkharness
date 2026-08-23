//! Converts engine output to [`dark_contract::Chunk`] and cancels cleanly
//! (task unit `B4`).
//!
//! [`request`] and [`response`] are pure conversions to and from real
//! mistral.rs types, tested directly with no loaded model. [`accumulate`]
//! reassembles [`dark_contract::Chunk::ToolCallDelta`] fragments by index.
//! [`concurrency`] limits how many sequences run at once, from resident-set
//! headroom, and its `Permit` is what makes a cancelled turn's memory
//! return to baseline (`crates/dark-engine/tests/cancel_leak.rs`). [`live`]
//! is the seam that actually drives a `mistralrs::Model`'s stream — see its
//! module documentation for why it cannot be exercised by a test here.

pub mod accumulate;
pub mod concurrency;
pub mod live;
pub mod request;
pub mod response;

pub use accumulate::Accumulator;
pub use concurrency::{Limiter, Permit, max_concurrent_sequences};
