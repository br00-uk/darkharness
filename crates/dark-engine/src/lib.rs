//! The real inference engine, over mistral.rs.
//!
//! This is the only crate that may depend on `mistralrs` (Rule 12), and
//! `cargo xtask check-deps` fails the build when another one does. Every
//! other crate holds the engine as `dyn Engine` and tests against
//! `dark-engine-fake` (Rule 17).
//!
//! Memory is the limit that shapes this crate. Estimate before loading and
//! never discover a limit by allocation failure. Never evict a pinned model
//! or one that holds a turn lease. Budget against `Caps::granted_context`,
//! never `Caps::max_context`. See Rules 1 to 4 and task units `B2` to `B7`.

pub mod determinism;
pub mod embed;
pub mod load;
pub mod resident;
pub mod stream;
pub mod tune;
