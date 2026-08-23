//! The charting pipeline: charts a map in seven stages, each stage a fresh
//! sub-session. Task unit `E1`.
//!
//! | # | Stage | Mode | Micro-role |
//! | --- | --- | --- | --- |
//! | 1 | [`destination`] | Human present | `deliberate` |
//! | 2 | [`seed`] | No model | none |
//! | 3 | `crate::axes` (task unit `E2`) | Human present, one turn each axis | `deliberate` |
//! | 4 | [`stages::Extractor`] (task unit `E3`, not built here) | Automatic | `extract` |
//! | 5 | [`stages::Sharpener`] (task unit `E4`, not built here) | Automatic, one candidate each call | `classify` |
//! | 6 | [`stages::Sizer`] (task unit `E5`, not built here) | Automatic, one ticket each call | `classify` |
//! | 7 | [`stages::Wirer`] (task unit `E6`, not built here) | Automatic, one ticket each call | `classify` |
//!
//! [`pipeline::ChartPipeline`] runs all seven, checkpointing after each one
//! (Do step 4) so a killed run resumes (Do step 5). [`gate`] refuses to
//! chart at all when the model's profile says it must not (a 4B model must
//! not chart a map). [`ticket`] defines the ticket-shaped output: types
//! that line up with `dark_cartograph::journal::event` by field name,
//! without depending on `dark-cartograph` (see [`ticket`]'s module
//! documentation for why, and for the one field that does not line up
//! cleanly). [`cost`] renders the estimate Do step 9 asks the caller to
//! print before charting starts.
//!
//! Stages 4 to 7 are traits, not calls into `extract.rs`, `sharpen.rs`,
//! `size.rs`, and `wire.rs`: those files belong to task units `E3` to `E6`
//! and this task unit must not touch them. [`stages`] defines the seam;
//! [`pipeline`]'s own tests plug in small deterministic test doubles to
//! prove the orchestration — checkpointing, resume, the "no fog" early
//! stop, ticket identifier assignment, edge resolution — works correctly
//! regardless of what those four stages eventually decide.

pub mod checkpoint;
pub mod cost;
pub mod destination;
pub mod gate;
pub mod pipeline;
pub mod sampling;
pub mod seed;
pub mod stages;
pub mod ticket;

pub use checkpoint::{Checkpoint, CheckpointStore, FileCheckpointStore, Stage};
pub use cost::{CostEstimate, CostInputs};
pub use destination::{DestinationRecord, run_destination};
pub use gate::authorize_charting;
pub use pipeline::{
    ChartConfig, ChartOutput, ChartPipeline, ChartRun, DestinationInput, StageImpls,
};
pub use sampling::{Generation, MicroSampling, StageSampling, build_request, run_generation};
pub use seed::{BlastRadius, SeedModule, SeedReport, SeedSeam};
pub use stages::{
    Candidate, ExtractOutput, Extractor, OutOfScopeCandidate, SharpenOutcome, Sharpener,
    SizeOutcome, Sizer, WireAnswer, Wirer,
};
pub use ticket::{ChartedEdge, ChartedTicket, FogPatch, ScopeExclusion, TicketKind};
