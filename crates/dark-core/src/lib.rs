//! Session, turn loop, context assembly, and the event bus.
//!
//! This crate holds the engine as `dyn Engine`, so it builds and tests against
//! `dark-engine-fake`. See Rule 17.

pub mod context;
mod jsonl;
pub mod policy;
pub mod session;
pub mod telemetry;
pub mod turn;
