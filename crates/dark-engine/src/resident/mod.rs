//! The resident set manager: which models are in memory, and why.
//!
//! Memory is the dominant limit (section 4.1). This module estimates a
//! model's footprint before it loads, refuses a load that does not fit,
//! never evicts a pinned model or one holding a turn lease, and computes
//! [`dark_contract::Caps::granted_context`] from what remains after the
//! weights. See [`set::ResidentSet`] for the state machine and
//! [`degrade::climb`] for the four-step degradation ladder.
//!
//! Every type here is plain data or a pure function of it: nothing in this
//! module opens a file, reaches mistral.rs, or touches a device. A test
//! drives the whole lifecycle — load, evict, lease, degrade, refuse — with
//! no model file and no accelerator.
//!
//! # What is deferred to real hardware
//!
//! The build specification's "done when" for this task unit asks for the
//! estimator to land within 10% of *measured* memory for five models. That
//! needs weights on disk and a machine to load them onto, which this
//! sandbox has neither of. [`estimate`]'s tests instead pin the formula
//! against five published Qwen3 configurations (hand-computed, independent
//! of the code under test) and against the two illustrative figures
//! section 4.1's prose gives. Confirming those five estimates against a
//! real load's measured memory is the honest next step, on a machine that
//! has one.

pub mod degrade;
pub mod estimate;
pub mod model_key;
mod set;

pub use degrade::{DegradeRequest, Outcome as DegradeOutcome, QuantOption, Step as DegradeStep};
pub use estimate::ModelConfig;
pub use model_key::{ModelKey, TurnId};
pub use set::{BeginLoadRequest, DEFAULT_MIN_CONTEXT, LoadPlan, ResidentSet};
