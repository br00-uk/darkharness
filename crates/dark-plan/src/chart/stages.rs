//! The seams stages 4 to 7 plug into.
//!
//! Task units `E3` (`extract.rs`), `E4` (`sharpen.rs`), `E5` (`size.rs`),
//! and `E6` (`wire.rs`) own the real implementations of these stages, and
//! this task unit must not touch those files (see the task brief). What it
//! must do is finish a pipeline that runs all seven stages, so this module
//! defines the seam each later unit implements: one trait per stage, plus
//! the small data types that cross it.
//!
//! Each trait method returns [`BoxFuture`] instead of being declared
//! `async fn`, because [`ChartPipeline`](crate::chart::pipeline::ChartPipeline)
//! holds these as `&dyn Trait` — a native `async fn` in a trait is not
//! object-safe, and `dark-plan` has no `async-trait` dependency to paper
//! over that. [`BoxFuture`] needs nothing beyond `std`, so it costs no new
//! dependency; see its doc comment in `crate::chart::sampling`.

use dark_contract::{Engine, Result, RoleClass};
use serde::{Deserialize, Serialize};

use crate::axes::AxisAnswer;
use crate::chart::sampling::{BoxFuture, MicroSampling};
use crate::chart::ticket::{ChartedTicket, TicketKind};

/// One candidate that extraction (stage 4) found.
///
/// Mirrors one entry of the `candidates` array in the extraction schema
/// (task unit `E3`, Do step 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// The candidate's short name, 12 words or fewer.
    pub name: String,
    /// The question this candidate raises. Ends with a question mark.
    pub question: String,
    /// The axis that produced this candidate.
    pub axis: String,
    /// What kind of ticket this candidate would become.
    pub kind: TicketKind,
}

/// One thing extraction found to be past the destination's scope.
///
/// Mirrors one entry of the `out_of_scope` array in the extraction schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutOfScopeCandidate {
    /// A short summary of the excluded thing.
    pub gist: String,
    /// Why it sits outside scope.
    pub reason: String,
}

/// What stage 4 (extract) produces.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExtractOutput {
    /// The candidates extraction found.
    pub candidates: Vec<Candidate>,
    /// What extraction found to be out of scope.
    pub out_of_scope: Vec<OutOfScopeCandidate>,
}

/// Turns stage 3's axis answers into stage 4's candidates.
///
/// Task unit `E3` implements this over one generation, constrained to the
/// schema in its Do step 2, with the deterministic repair checks in its Do
/// step 3.
pub trait Extractor: Send + Sync {
    /// Runs stage 4 over every stage 3 answer that was not "nothing here".
    ///
    /// `destination` is the settled destination text from stage 1 (see
    /// [`crate::chart::destination::DestinationRecord::destination`]). Task
    /// unit `E3`'s "no question restates the destination" check compares
    /// each candidate's question against it.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine fails, or when the deterministic
    /// checks task unit `E3` names never pass after its retry budget.
    fn extract<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        destination: &'a str,
        answers: &'a [AxisAnswer],
    ) -> BoxFuture<'a, Result<ExtractOutput>>;
}

/// What stage 5 (sharpen) decided about one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharpenOutcome {
    /// The question can be stated precisely now. It becomes a ticket.
    Ticket,
    /// The question cannot yet be phrased sharply. It becomes fog.
    Fog,
}

/// Tests one candidate for fog.
///
/// Task unit `E4` implements this: one generation, temperature 0, one word
/// of output, with the deterministic exclusions in its Do step 4 applied
/// before the model is asked at all.
pub trait Sharpener: Send + Sync {
    /// Tests `candidate`.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine fails.
    fn sharpen<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        candidate: &'a Candidate,
    ) -> BoxFuture<'a, Result<SharpenOutcome>>;
}

/// What stage 6 (size) decided about one ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SizeOutcome {
    /// The ticket fits one session as it stands.
    Ok,
    /// The ticket holds more than one decision. These candidates replace it.
    Split(Vec<Candidate>),
}

/// Tests whether one candidate fits inside the ticket budget.
///
/// Task unit `E5` implements this: the ticket budget itself
/// (`granted_context * 0.55`) is computed elsewhere and passed in as
/// `budget_tokens`, because that formula belongs to `E5`, not to this
/// pipeline.
pub trait Sizer: Send + Sync {
    /// Tests `candidate` against `budget_tokens`.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine fails.
    fn size<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        candidate: &'a Candidate,
        budget_tokens: usize,
    ) -> BoxFuture<'a, Result<SizeOutcome>>;
}

/// What stage 7 (wire) answered for one ticket.
///
/// `blocked_by` holds ticket *names*, not identifiers, matching the
/// build specification's own example prompt ("Answer with names, or
/// NONE"): a model reasons about tickets by name, not by an opaque
/// identifier it never chose. The pipeline resolves these names into
/// [`crate::chart::ticket::ChartedEdge`] values keyed by
/// [`ChartedTicket::id`], since every ticket already has one by the time
/// stage 7 runs (see `crate::chart::pipeline`, which assigns identifiers
/// once sizing finishes, so Do step 7's "a ticket needs an identifier
/// before another ticket can reference it" holds from the start of stage
/// 7, not only after it).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WireAnswer {
    /// The names of the tickets that must resolve before this one can.
    pub blocked_by: Vec<String>,
}

/// Asks which other tickets must resolve first.
///
/// Task unit `E6` implements this: one generation per ticket, with the
/// deterministic repairs in its Do step 2 (cycle breaking, transitive
/// reduction, out-degree capping, and the frontier check) applied after
/// every ticket has answered.
pub trait Wirer: Send + Sync {
    /// Asks which of `other_names` must block `ticket`.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine fails.
    fn wire<'a>(
        &'a self,
        engine: &'a dyn Engine,
        class: RoleClass,
        sampling: MicroSampling,
        ticket: &'a ChartedTicket,
        other_names: &'a [String],
    ) -> BoxFuture<'a, Result<WireAnswer>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_candidate_carries_the_extraction_schema_fields() {
        let candidate = Candidate {
            name: "pack staleness policy".to_owned(),
            question: "How does a pack declare its staleness policy?".to_owned(),
            axis: "lifecycle, migration and backfill".to_owned(),
            kind: TicketKind::Grilling,
        };
        assert!(candidate.question.ends_with('?'));
        assert!(candidate.name.split_whitespace().count() <= 12);
    }

    #[test]
    fn size_outcome_split_carries_replacement_candidates() {
        let outcome = SizeOutcome::Split(vec![Candidate {
            name: "a".to_owned(),
            question: "a?".to_owned(),
            axis: "x".to_owned(),
            kind: TicketKind::Task,
        }]);
        match outcome {
            SizeOutcome::Split(parts) => assert_eq!(parts.len(), 1),
            SizeOutcome::Ok => panic!("expected a split"),
        }
    }
}
