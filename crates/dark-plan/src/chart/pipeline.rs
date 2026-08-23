//! The charting pipeline: runs the seven stages, checkpointing after each
//! one, so a killed run resumes instead of restarting.
//!
//! Task unit `E1`. See the module documentation on `crate::chart` for the
//! stage table this orchestrates, and `crate::chart::stages` for why stages
//! 4 to 7 are trait objects rather than calls into `extract.rs`,
//! `sharpen.rs`, `size.rs`, and `wire.rs` directly.

use std::time::{SystemTime, UNIX_EPOCH};

use dark_contract::{Engine, ErrCode, Error, Result, RoleClass};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::axes::{AxisAnswer, AxisOutcome, AxisSets, run_axis_sweep};
use crate::chart::checkpoint::{Checkpoint, CheckpointStore, Stage};
use crate::chart::destination::{DestinationRecord, run_destination};
use crate::chart::gate::authorize_charting;
use crate::chart::sampling::StageSampling;
use crate::chart::seed::SeedReport;
use crate::chart::stages::{
    Candidate, ExtractOutput, Extractor, SharpenOutcome, Sharpener, SizeOutcome, Sizer, WireAnswer,
    Wirer,
};
use crate::chart::ticket::{ChartedEdge, ChartedTicket, FogPatch, ScopeExclusion};
use crate::wire::WireRepairReport;

/// Fresh input for stage 1, when a charting run starts from nothing rather
/// than resuming.
#[derive(Debug, Clone, Copy)]
pub struct DestinationInput<'a> {
    /// The idea a person brought to `/plan`.
    pub idea: &'a str,
    /// The resolved `AGENTS.md` chain, or an empty string when there is
    /// none.
    pub agents_md: &'a str,
    /// A short repository summary.
    pub repo_summary: &'a str,
}

/// The settings one charting run needs, beyond the engine and the model.
#[derive(Debug, Clone)]
pub struct ChartConfig {
    /// The role class charting talks to. Both a large `Worker` model and an
    /// `Architect` model may chart, so the caller — who resolved the
    /// profile — names the class, rather than this pipeline assuming one.
    pub role_class: RoleClass,
    /// The `deliberate`, `extract`, and `classify` sampling settings.
    pub sampling: StageSampling,
    /// The model identifier. Named in the charting-gate refusal and the
    /// cost estimate.
    pub model_id: String,
    /// Whether the resolved profile allows this model to chart. See
    /// `crate::chart::gate`.
    pub allow_charting: bool,
    /// The token budget one ticket must fit. Task unit `E5` owns the
    /// formula (`granted_context * 0.55`); this pipeline only carries the
    /// computed number through to [`Sizer::size`].
    pub ticket_budget_tokens: usize,
}

/// What a charting run produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ChartRun {
    /// Stage 5 (sharpen) found no fog: every candidate was already sharp
    /// enough to answer directly. Task unit `E1`, Do step 6: "Tell the
    /// person that the work fits one session and needs no map."
    NoMapNeeded {
        /// The destination stage 1 settled on. Charting still runs stage 1,
        /// even when it turns out no map follows.
        destination: DestinationRecord,
        /// A sentence to show the person.
        note: String,
    },
    /// Charting produced a map.
    /// A finished map.
    ///
    /// Boxed because a charted run carries the whole map and its wiring
    /// repair, and the other variant carries one string: leaving it unboxed
    /// makes every `ChartRun` as large as the largest one.
    Charted(Box<ChartOutput>),
}

/// A finished charting run.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartOutput {
    /// The map's identifier.
    pub map_id: String,
    /// What stage 1 settled on.
    pub destination: DestinationRecord,
    /// What stage 2 read from the repository.
    pub seed: SeedReport,
    /// What stage 3 answered, one entry per axis.
    pub axis_answers: Vec<AxisAnswer>,
    /// What stage 4 (extract) found to be out of scope.
    pub out_of_scope: Vec<ScopeExclusion>,
    /// The tickets the map ends with, after stage 6 (size) may have split
    /// some. Ordered by [`ChartedTicket::ordinal`].
    pub tickets: Vec<ChartedTicket>,
    /// The blocking edges stage 7 (wire) produced, resolved to ticket
    /// identifiers and then repaired.
    ///
    /// These are the repaired edges, not the raw ones: a cycle stops the
    /// frontier permanently, so the pipeline never hands back an edge set it
    /// has not run [`crate::wire::repair_wiring`] over. See
    /// [`ChartOutput::wire_repair`] for what the repair changed.
    pub edges: Vec<ChartedEdge>,
    /// What the wiring repair changed on the way to [`ChartOutput::edges`].
    ///
    /// A caller that wants to tell a person which cycles the harness broke,
    /// or which tickets block an unusual number of others, reads this.
    pub wire_repair: WireRepairReport,
    /// The fog patches stage 5 (sharpen) produced, merged one per axis (Do
    /// step 5 of task unit `E4`).
    pub fog: Vec<FogPatch>,
}

/// Runs the seven-stage charting pipeline against one engine.
///
/// Holds no state of its own between calls beyond what `checkpoints`
/// stores: two pipelines built with the same `checkpoints` and the same
/// `map_id` see the same progress, which is what makes
/// [`ChartPipeline::resume`] possible after the process that started
/// charting is gone.
pub struct ChartPipeline<'a> {
    engine: &'a dyn Engine,
    config: ChartConfig,
    axis_sets: &'a AxisSets,
    checkpoints: &'a dyn CheckpointStore,
}

impl<'a> ChartPipeline<'a> {
    /// Builds a pipeline. Nothing runs until [`ChartPipeline::chart`] or
    /// [`ChartPipeline::resume`] is called.
    #[must_use]
    pub fn new(
        engine: &'a dyn Engine,
        config: ChartConfig,
        axis_sets: &'a AxisSets,
        checkpoints: &'a dyn CheckpointStore,
    ) -> Self {
        Self {
            engine,
            config,
            axis_sets,
            checkpoints,
        }
    }

    /// Charts `map_id` from stage 1, using `destination_input` to settle
    /// the destination.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineUnsupported`] when
    /// [`ChartConfig::allow_charting`] is `false` — a 4B model must not
    /// chart a map. Returns an error when the engine fails, or when a
    /// stage's output cannot be parsed. See [`ChartPipeline::resume`] for
    /// picking up after either kind of failure.
    pub async fn chart(
        &self,
        map_id: &str,
        destination_input: DestinationInput<'_>,
        seed: SeedReport,
        stages: &StageImpls<'_>,
    ) -> Result<ChartRun> {
        self.run(map_id, Some(destination_input), seed, stages)
            .await
    }

    /// Resumes charting `map_id`, replaying every checkpoint already
    /// recorded and continuing from the first stage that has none.
    ///
    /// Task unit `E1`, Do step 5: "Support `dark map chart --resume
    /// <map-id> --from-stage <n>`." This method finds `<n>` itself, from
    /// what [`CheckpointStore::load`] returns, rather than the caller
    /// naming it — the checkpoint store is the source of truth for how far
    /// a run got, and a caller-supplied stage number could disagree with
    /// it after a crash mid-checkpoint-write.
    ///
    /// `seed` is used only when stage 2 has no checkpoint yet; once stage 2
    /// has run, its checkpointed value is authoritative, and this
    /// parameter is ignored, so a caller may pass the same
    /// [`SeedReport`] it would have used for a fresh run without
    /// checking first.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineGenerate`] when no checkpoint exists for
    /// stage 1 and the caller has no way to supply fresh destination input
    /// through this method — resume from before stage 1 finishes is not
    /// meaningful; use [`ChartPipeline::chart`] instead. Otherwise, the
    /// same errors as [`ChartPipeline::chart`].
    pub async fn resume(
        &self,
        map_id: &str,
        seed: SeedReport,
        stages: &StageImpls<'_>,
    ) -> Result<ChartRun> {
        self.run(map_id, None, seed, stages).await
    }

    /// Runs (or resumes) charting. See [`ChartPipeline::chart`] and
    /// [`ChartPipeline::resume`].
    ///
    /// Delegates to one method per stage, all sharing the same shape
    /// through [`ChartPipeline::stage_or_checkpoint`]: replay a checkpoint
    /// if one exists, otherwise compute the stage and checkpoint the
    /// result.
    async fn run(
        &self,
        map_id: &str,
        destination_input: Option<DestinationInput<'_>>,
        seed_input: SeedReport,
        stages: &StageImpls<'_>,
    ) -> Result<ChartRun> {
        authorize_charting(&self.config.model_id, self.config.allow_charting)?;

        let destination = self.stage_destination(map_id, destination_input).await?;
        let seed = self.stage_seed(map_id, seed_input).await?;
        let axis_answers = self.stage_axis_sweep(map_id, &destination, &seed).await?;
        let extracted = self
            .stage_extract(map_id, stages, &destination, &axis_answers)
            .await?;
        let sharpened = self.stage_sharpen(map_id, stages, &extracted).await?;

        let sharp_candidates: Vec<Candidate> = sharpened
            .iter()
            .filter(|record| record.outcome == SharpenOutcome::Ticket)
            .map(|record| record.candidate.clone())
            .collect();
        let fog_candidates: Vec<Candidate> = sharpened
            .iter()
            .filter(|record| record.outcome == SharpenOutcome::Fog)
            .map(|record| record.candidate.clone())
            .collect();

        // Do step 6: "After stage 4, test for fog. When no fog exists,
        // stop." Fog is stage 5's output, not stage 4's (see the stage
        // table); this pipeline reads that step as "after extraction and
        // sharpening finish," fusing stages 4 and 5 into the one gate the
        // step describes. Flagged in the task report as read this way,
        // because the literal stage number does not match the table.
        if fog_candidates.is_empty() {
            return Ok(ChartRun::NoMapNeeded {
                destination,
                note: "This work fits one session. It needs no map.".to_owned(),
            });
        }

        let tickets = self.stage_size(map_id, stages, &sharp_candidates).await?;
        let wired = self.stage_wire(map_id, stages, &tickets).await?;

        // Repair before handing the edges back. `Wirer::wire` is called
        // once per ticket and cannot see the whole edge set, so a cycle,
        // a duplicate, or an implied edge only becomes visible here. A
        // cycle stops the frontier permanently, so this is not optional
        // and it is not the caller's job to remember. See task unit `E6`,
        // Do step 2.
        let raw_edges = resolve_edges(&tickets, &wired);
        let wire_repair = crate::wire::repair_wiring(&tickets, raw_edges)?;
        let edges = wire_repair.edges.clone();
        let fog = merge_fog_by_axis(&fog_candidates);
        let out_of_scope = extracted
            .out_of_scope
            .into_iter()
            .map(|item| ScopeExclusion {
                gist: item.gist,
                reason: item.reason,
                ticket_id: None,
            })
            .collect();

        Ok(ChartRun::Charted(Box::new(ChartOutput {
            map_id: map_id.to_owned(),
            destination,
            seed,
            axis_answers,
            out_of_scope,
            tickets,
            edges,
            wire_repair,
            fog,
        })))
    }

    /// Stage 1: settles the destination. Fixes the scope (Do step 3).
    async fn stage_destination(
        &self,
        map_id: &str,
        destination_input: Option<DestinationInput<'_>>,
    ) -> Result<DestinationRecord> {
        self.stage_or_checkpoint(map_id, Stage::Destination, async || {
            let input = destination_input.ok_or_else(|| {
                Error::new(
                    ErrCode::EngineGenerate,
                    format!(
                        "cannot resume {map_id}: no checkpoint exists for stage 1 \
                         (destination), and resume carries no fresh idea to settle one with"
                    ),
                )
                .with_remedy("Start this map with ChartPipeline::chart instead of resume.")
            })?;
            run_destination(
                self.engine,
                self.config.role_class,
                self.config.sampling.deliberate,
                input.idea,
                input.agents_md,
                input.repo_summary,
            )
            .await
        })
        .await
    }

    /// Stage 2: hands the caller's [`SeedReport`] through unchanged. Uses
    /// no model (Do step 1's stage table).
    async fn stage_seed(&self, map_id: &str, seed_input: SeedReport) -> Result<SeedReport> {
        self.stage_or_checkpoint(map_id, Stage::Seed, async || Ok(seed_input))
            .await
    }

    /// Stage 3: one turn per axis (task unit `E2`).
    async fn stage_axis_sweep(
        &self,
        map_id: &str,
        destination: &DestinationRecord,
        seed: &SeedReport,
    ) -> Result<Vec<AxisAnswer>> {
        self.stage_or_checkpoint(map_id, Stage::AxisSweep, async || {
            let axis_set = self.axis_sets.for_destination(destination.destination_type);
            run_axis_sweep(
                self.engine,
                self.config.role_class,
                self.config.sampling.deliberate,
                &destination.destination,
                axis_set,
                seed.seed_text().as_deref(),
            )
            .await
        })
        .await
    }

    /// Stage 4: one generation over every non-empty axis answer.
    async fn stage_extract(
        &self,
        map_id: &str,
        stages: &StageImpls<'_>,
        destination: &DestinationRecord,
        axis_answers: &[AxisAnswer],
    ) -> Result<ExtractOutput> {
        let open_answers: Vec<AxisAnswer> = axis_answers
            .iter()
            .filter(|answer| matches!(answer.outcome, AxisOutcome::Open(_)))
            .cloned()
            .collect();

        self.stage_or_checkpoint(map_id, Stage::Extract, async || {
            stages
                .extractor
                .extract(
                    self.engine,
                    self.config.role_class,
                    self.config.sampling.extract,
                    &destination.destination,
                    &open_answers,
                )
                .await
        })
        .await
    }

    /// Stage 5: one call per candidate, classifying it as a ticket or as
    /// fog.
    async fn stage_sharpen(
        &self,
        map_id: &str,
        stages: &StageImpls<'_>,
        extracted: &ExtractOutput,
    ) -> Result<Vec<SharpenRecord>> {
        self.stage_or_checkpoint(map_id, Stage::Sharpen, async || {
            let mut records = Vec::with_capacity(extracted.candidates.len());
            for candidate in &extracted.candidates {
                let outcome = stages
                    .sharpener
                    .sharpen(
                        self.engine,
                        self.config.role_class,
                        self.config.sampling.classify,
                        candidate,
                    )
                    .await?;
                records.push(SharpenRecord {
                    candidate: candidate.clone(),
                    outcome,
                });
            }
            Ok(records)
        })
        .await
    }

    /// Stage 6: may replace one candidate with several. Assigns ticket
    /// identifiers as soon as the ticket set is final — see the comment in
    /// [`ChartPipeline::run`] on why identifier assignment sits here rather
    /// than after stage 7.
    async fn stage_size(
        &self,
        map_id: &str,
        stages: &StageImpls<'_>,
        sharp_candidates: &[Candidate],
    ) -> Result<Vec<ChartedTicket>> {
        self.stage_or_checkpoint(map_id, Stage::Size, async || {
            let mut sized_candidates = Vec::new();
            for candidate in sharp_candidates {
                match stages
                    .sizer
                    .size(
                        self.engine,
                        self.config.role_class,
                        self.config.sampling.classify,
                        candidate,
                        self.config.ticket_budget_tokens,
                    )
                    .await?
                {
                    SizeOutcome::Ok => sized_candidates.push(candidate.clone()),
                    SizeOutcome::Split(parts) => sized_candidates.extend(parts),
                }
            }

            let tickets: Vec<ChartedTicket> = sized_candidates
                .into_iter()
                .enumerate()
                .map(|(ordinal, candidate)| ChartedTicket {
                    id: Ulid::new().to_string(),
                    name: candidate.name,
                    question: candidate.question,
                    ticket_type: candidate.kind,
                    hitl: candidate.kind.default_hitl(),
                    ordinal: ordinal_as_i64(ordinal),
                    axis: vec![candidate.axis],
                })
                .collect();
            Ok(tickets)
        })
        .await
    }

    /// Stage 7: one call per ticket; every ticket already has an
    /// identifier.
    ///
    /// Ticket identifiers are assigned in [`ChartPipeline::stage_size`],
    /// immediately once the ticket set is final, and checkpointed with the
    /// ticket record — not deferred to after this stage, as Do step 7's
    /// prose taken alone might suggest. `ulid::Ulid::new()` reads system
    /// entropy, so a freshly generated identifier is not reproducible
    /// across a crash-and-resume; checkpointing it right away is what
    /// makes resume produce the same map as an uninterrupted run, rather
    /// than the same map under different names. Do step 7's actual
    /// requirement — "a ticket needs an identifier before another ticket
    /// can reference it" — holds either way, since this stage runs after
    /// stage 6.
    async fn stage_wire(
        &self,
        map_id: &str,
        stages: &StageImpls<'_>,
        tickets: &[ChartedTicket],
    ) -> Result<Vec<WireRecord>> {
        self.stage_or_checkpoint(map_id, Stage::Wire, async || {
            let names: Vec<String> = tickets.iter().map(|ticket| ticket.name.clone()).collect();
            let mut records = Vec::with_capacity(tickets.len());
            for ticket in tickets {
                let other_names: Vec<String> = names
                    .iter()
                    .filter(|name| *name != &ticket.name)
                    .cloned()
                    .collect();
                let answer = stages
                    .wirer
                    .wire(
                        self.engine,
                        self.config.role_class,
                        self.config.sampling.classify,
                        ticket,
                        &other_names,
                    )
                    .await?;
                records.push(WireRecord {
                    ticket_id: ticket.id.clone(),
                    answer,
                });
            }
            Ok(records)
        })
        .await
    }

    /// Replays stage `stage`'s checkpoint when one exists; otherwise runs
    /// `compute`, checkpoints its result, and returns that.
    ///
    /// This is Do step 4 ("write a checkpoint to the journal after each
    /// stage") and Do step 5 (resume) as one piece of plumbing, shared by
    /// every stage in [`ChartPipeline::run`], rather than repeated seven
    /// times with the two branches spelled out by hand.
    async fn stage_or_checkpoint<T, F>(&self, map_id: &str, stage: Stage, compute: F) -> Result<T>
    where
        T: Serialize + DeserializeOwned,
        F: AsyncFnOnce() -> Result<T>,
    {
        if let Some(value) = self.load_checkpoint::<T>(map_id, stage)? {
            return Ok(value);
        }
        let value = compute().await?;
        self.save_checkpoint(map_id, stage, &value)?;
        Ok(value)
    }

    /// Writes one checkpoint.
    fn save_checkpoint<T: Serialize>(&self, map_id: &str, stage: Stage, value: &T) -> Result<()> {
        let payload = serde_json::to_value(value).map_err(|err| {
            Error::new(
                ErrCode::EngineGenerate,
                format!("cannot serialise the {} checkpoint: {err}", stage.as_str()),
            )
        })?;
        self.checkpoints.record(&Checkpoint {
            map_id: map_id.to_owned(),
            stage,
            recorded_at: now_ms(),
            payload,
        })
    }

    /// Reads the most recently recorded checkpoint for `stage`, when one
    /// exists.
    fn load_checkpoint<T: DeserializeOwned>(
        &self,
        map_id: &str,
        stage: Stage,
    ) -> Result<Option<T>> {
        let checkpoints = self.checkpoints.load(map_id)?;
        let Some(found) = checkpoints.into_iter().rev().find(|cp| cp.stage == stage) else {
            return Ok(None);
        };
        let value = serde_json::from_value(found.payload).map_err(|err| {
            Error::new(
                ErrCode::EngineGenerate,
                format!("cannot parse the {} checkpoint: {err}", stage.as_str()),
            )
        })?;
        Ok(Some(value))
    }
}

/// The four stage-4-to-7 implementations one charting run plugs in.
///
/// Bundled into one value so [`ChartPipeline::chart`] and
/// [`ChartPipeline::resume`] take one argument instead of four. See
/// `crate::chart::stages` for why these are trait objects.
pub struct StageImpls<'a> {
    /// Stage 4.
    pub extractor: &'a dyn Extractor,
    /// Stage 5.
    pub sharpener: &'a dyn Sharpener,
    /// Stage 6.
    pub sizer: &'a dyn Sizer,
    /// Stage 7.
    pub wirer: &'a dyn Wirer,
}

/// One candidate's stage 5 outcome, checkpointed together so resume does
/// not need to re-pair a candidate list with an outcome list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SharpenRecord {
    candidate: Candidate,
    outcome: SharpenOutcome,
}

/// One ticket's stage 7 answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WireRecord {
    ticket_id: String,
    answer: WireAnswer,
}

/// Turns stage 7's by-name answers into identifier-based edges.
///
/// A blocker name that matches no ticket is dropped rather than turned
/// into an edge. The deterministic repairs task unit `E6` names — breaking
/// a cycle, reducing transitively, capping out-degree, checking the
/// frontier — belong to `wire.rs`, not to this orchestration step; this
/// function only resolves a name to an identifier, the minimum needed to
/// produce a well-formed [`ChartedEdge`] at all.
fn resolve_edges(tickets: &[ChartedTicket], wired: &[WireRecord]) -> Vec<ChartedEdge> {
    let mut edges = Vec::new();
    for record in wired {
        for blocker_name in &record.answer.blocked_by {
            if let Some(blocker) = tickets.iter().find(|ticket| &ticket.name == blocker_name) {
                edges.push(ChartedEdge {
                    blocker: blocker.id.clone(),
                    blocked: record.ticket_id.clone(),
                });
            }
        }
    }
    edges
}

/// Merges fog candidates into one patch per axis.
///
/// Task unit `E4`, Do step 5: "Write one fog patch for each axis. When
/// stage 4 produced four fog candidates on one axis, merge them into one
/// patch." Grouping is structural — the questions to merge are already
/// decided once each candidate's axis is known — so charting does the
/// merge itself rather than asking `sharpen.rs`'s `Sharpener` to.
fn merge_fog_by_axis(fog_candidates: &[Candidate]) -> Vec<FogPatch> {
    let mut by_axis: std::collections::BTreeMap<String, Vec<&str>> =
        std::collections::BTreeMap::new();
    for candidate in fog_candidates {
        by_axis
            .entry(candidate.axis.clone())
            .or_default()
            .push(candidate.question.as_str());
    }

    by_axis
        .into_iter()
        .map(|(axis, questions)| {
            let patch = if questions.len() == 1 {
                questions[0].to_owned()
            } else {
                questions
                    .iter()
                    .map(|question| format!("- {question}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            FogPatch {
                patch,
                axis: Some(axis),
            }
        })
        .collect()
}

/// Converts a ticket's position into the `i64` ordinal
/// [`ChartedTicket::ordinal`] stores.
#[allow(clippy::cast_possible_wrap)]
fn ordinal_as_i64(ordinal: usize) -> i64 {
    ordinal as i64
}

/// Returns the current time in milliseconds since the Unix epoch, the unit
/// `dark_cartograph`'s `Timestamp` uses (see
/// `dark_cartograph::journal::event::Timestamp`), so a checkpoint's
/// `recorded_at` reads the same way a journal event's timestamp would.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::checkpoint::FileCheckpointStore;
    use crate::chart::sampling::BoxFuture;
    use dark_engine_fake::script::Turn;
    use dark_engine_fake::{FakeEngine, Script};

    /// A test double for [`Extractor`] that returns a fixed
    /// [`ExtractOutput`] without calling the engine.
    struct FixedExtractor(ExtractOutput);
    impl Extractor for FixedExtractor {
        fn extract<'a>(
            &'a self,
            _engine: &'a dyn Engine,
            _class: RoleClass,
            _sampling: crate::chart::sampling::MicroSampling,
            _destination: &'a str,
            _answers: &'a [AxisAnswer],
        ) -> BoxFuture<'a, Result<ExtractOutput>> {
            let output = self.0.clone();
            Box::pin(async move { Ok(output) })
        }
    }

    /// A test double for [`Sharpener`] that classifies by a fixed set of
    /// fog names, everything else a ticket.
    struct FixedSharpener {
        fog_names: Vec<String>,
    }
    impl Sharpener for FixedSharpener {
        fn sharpen<'a>(
            &'a self,
            _engine: &'a dyn Engine,
            _class: RoleClass,
            _sampling: crate::chart::sampling::MicroSampling,
            candidate: &'a Candidate,
        ) -> BoxFuture<'a, Result<SharpenOutcome>> {
            let outcome = if self.fog_names.contains(&candidate.name) {
                SharpenOutcome::Fog
            } else {
                SharpenOutcome::Ticket
            };
            Box::pin(async move { Ok(outcome) })
        }
    }

    /// A test double for [`Sizer`] that always accepts the candidate as-is.
    struct AlwaysOkSizer;
    impl Sizer for AlwaysOkSizer {
        fn size<'a>(
            &'a self,
            _engine: &'a dyn Engine,
            _class: RoleClass,
            _sampling: crate::chart::sampling::MicroSampling,
            _candidate: &'a Candidate,
            _budget_tokens: usize,
        ) -> BoxFuture<'a, Result<SizeOutcome>> {
            Box::pin(async move { Ok(SizeOutcome::Ok) })
        }
    }

    /// A test double for [`Wirer`] that always answers `NONE`.
    struct NoBlockersWirer;
    impl Wirer for NoBlockersWirer {
        fn wire<'a>(
            &'a self,
            _engine: &'a dyn Engine,
            _class: RoleClass,
            _sampling: crate::chart::sampling::MicroSampling,
            _ticket: &'a ChartedTicket,
            _other_names: &'a [String],
        ) -> BoxFuture<'a, Result<WireAnswer>> {
            Box::pin(async move { Ok(WireAnswer::default()) })
        }
    }

    /// A test double for [`Wirer`] that blocks every ticket on the first
    /// other name it sees, so wiring produces at least one edge.
    struct BlockOnFirstWirer;
    impl Wirer for BlockOnFirstWirer {
        fn wire<'a>(
            &'a self,
            _engine: &'a dyn Engine,
            _class: RoleClass,
            _sampling: crate::chart::sampling::MicroSampling,
            _ticket: &'a ChartedTicket,
            other_names: &'a [String],
        ) -> BoxFuture<'a, Result<WireAnswer>> {
            let blocked_by = other_names.first().cloned().into_iter().collect();
            Box::pin(async move { Ok(WireAnswer { blocked_by }) })
        }
    }

    fn candidate(name: &str, axis: &str, kind: crate::chart::ticket::TicketKind) -> Candidate {
        Candidate {
            name: name.to_owned(),
            question: format!("{name}?"),
            axis: axis.to_owned(),
            kind,
        }
    }

    fn base_config() -> ChartConfig {
        ChartConfig {
            role_class: RoleClass::Architect,
            sampling: StageSampling::default(),
            model_id: "fake/qwen3-32b".to_owned(),
            allow_charting: true,
            ticket_budget_tokens: 18_000,
        }
    }

    fn destination_turn(destination_type: &str) -> Turn {
        Turn {
            text: format!("DESTINATION: A retry policy.\nTYPE: {destination_type}\n"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn charting_refuses_when_the_profile_disallows_it() {
        let engine = FakeEngine::new(Script::default());
        let axis_sets = AxisSets::builtin();
        let store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        let mut config = base_config();
        config.allow_charting = false;
        config.model_id = "fake/qwen3-4b".to_owned();

        let pipeline = ChartPipeline::new(&engine, config, &axis_sets, &store);
        let stages = StageImpls {
            extractor: &FixedExtractor(ExtractOutput::default()),
            sharpener: &FixedSharpener { fog_names: vec![] },
            sizer: &AlwaysOkSizer,
            wirer: &NoBlockersWirer,
        };

        let err = pipeline
            .chart(
                "map-1",
                DestinationInput {
                    idea: "x",
                    agents_md: "",
                    repo_summary: "",
                },
                SeedReport::default(),
                &stages,
            )
            .await
            .expect_err("a 4B model must not chart a map");

        assert_eq!(err.code, dark_contract::ErrCode::EngineUnsupported);
        assert!(err.message.contains("fake/qwen3-4b"));
        // The gate fires before any generation runs.
        assert_eq!(engine.turns_played(), 0);
    }

    #[tokio::test]
    async fn no_fog_after_sharpening_stops_with_no_map_needed() {
        let axis_set = crate::axes::AxisSet {
            axes: vec!["failure modes and error handling".to_owned()],
        };
        let mut axis_sets = AxisSets::builtin();
        axis_sets.decision = axis_set;

        let engine = FakeEngine::new(Script {
            turns: vec![
                destination_turn("decision"),
                Turn {
                    text: "Retries are capped at three.".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let extractor = FixedExtractor(ExtractOutput {
            candidates: vec![candidate(
                "retry cap",
                "failure modes and error handling",
                crate::chart::ticket::TicketKind::Task,
            )],
            out_of_scope: vec![],
        });
        let store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        let pipeline = ChartPipeline::new(&engine, base_config(), &axis_sets, &store);
        let stages = StageImpls {
            extractor: &extractor,
            sharpener: &FixedSharpener { fog_names: vec![] }, // nothing is fog
            sizer: &AlwaysOkSizer,
            wirer: &NoBlockersWirer,
        };

        let run = pipeline
            .chart(
                "map-2",
                DestinationInput {
                    idea: "x",
                    agents_md: "",
                    repo_summary: "",
                },
                SeedReport::default(),
                &stages,
            )
            .await
            .expect("charting runs");

        match run {
            ChartRun::NoMapNeeded { note, .. } => {
                assert!(note.contains("needs no map"));
            }
            ChartRun::Charted(_) => panic!("expected no map to be needed"),
        }
    }

    #[tokio::test]
    async fn a_full_run_produces_tickets_fog_and_resolved_edges() {
        let axis_set = crate::axes::AxisSet {
            axes: vec!["failure modes and error handling".to_owned()],
        };
        let mut axis_sets = AxisSets::builtin();
        axis_sets.decision = axis_set;

        let engine = FakeEngine::new(Script {
            turns: vec![
                destination_turn("decision"),
                Turn {
                    text: "Retries are undecided; staleness is also undecided.".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let extractor = FixedExtractor(ExtractOutput {
            candidates: vec![
                candidate(
                    "retry cap",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Task,
                ),
                candidate(
                    "staleness policy",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Grilling,
                ),
            ],
            out_of_scope: vec![crate::chart::stages::OutOfScopeCandidate {
                gist: "pack signing".to_owned(),
                reason: "separate effort".to_owned(),
            }],
        });
        let store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        let pipeline = ChartPipeline::new(&engine, base_config(), &axis_sets, &store);
        let stages = StageImpls {
            extractor: &extractor,
            sharpener: &FixedSharpener {
                fog_names: vec!["staleness policy".to_owned()],
            },
            sizer: &AlwaysOkSizer,
            wirer: &BlockOnFirstWirer,
        };

        let run = pipeline
            .chart(
                "map-3",
                DestinationInput {
                    idea: "x",
                    agents_md: "",
                    repo_summary: "",
                },
                SeedReport::default(),
                &stages,
            )
            .await
            .expect("charting runs");

        let ChartRun::Charted(output) = run else {
            panic!("expected a charted map");
        };

        assert_eq!(output.tickets.len(), 1);
        assert_eq!(output.tickets[0].name, "retry cap");
        assert_eq!(output.fog.len(), 1);
        assert_eq!(
            output.fog[0].axis.as_deref(),
            Some("failure modes and error handling")
        );
        assert_eq!(output.out_of_scope.len(), 1);
        assert_eq!(output.out_of_scope[0].gist, "pack signing");
        // Only one ticket survived sharpening, so BlockOnFirstWirer had no
        // other name to block on: no edge should exist.
        assert!(output.edges.is_empty());
    }

    /// `E6`: a cycle stops the frontier permanently, so the pipeline must
    /// never hand back one. `Wirer::wire` is called once per ticket and
    /// cannot see the whole edge set, so `BlockOnFirstWirer` blocks A on B
    /// and B on A without either call being able to notice. The pipeline
    /// repairs the set before it returns.
    #[tokio::test]
    async fn the_pipeline_never_returns_a_cyclical_edge_set() {
        let engine = FakeEngine::new(Script {
            turns: {
                // The destination turn, then one per axis the sweep asks
                // about. A few spare turns keep this test about wiring
                // rather than about the sweep's exact length.
                let mut turns = vec![destination_turn("decision")];
                turns.extend((0..8).map(|_| Turn {
                    text: "Retries are undecided; backoff is also undecided.".to_owned(),
                    ..Default::default()
                }));
                turns
            },
            ..Default::default()
        });
        let axis_sets = AxisSets::default();
        let extractor = FixedExtractor(ExtractOutput {
            candidates: vec![
                candidate(
                    "retry cap",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Task,
                ),
                candidate(
                    "backoff shape",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Task,
                ),
                // One candidate must stay fog, or the pipeline correctly
                // stops before wiring: no fog means no map is needed.
                candidate(
                    "staleness policy",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Grilling,
                ),
            ],
            out_of_scope: Vec::new(),
        });
        let store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        let pipeline = ChartPipeline::new(&engine, base_config(), &axis_sets, &store);
        let stages = StageImpls {
            extractor: &extractor,
            sharpener: &FixedSharpener {
                fog_names: vec!["staleness policy".to_owned()],
            },
            sizer: &AlwaysOkSizer,
            wirer: &BlockOnFirstWirer,
        };

        let run = pipeline
            .chart(
                "map-cycle",
                DestinationInput {
                    idea: "x",
                    agents_md: "",
                    repo_summary: "",
                },
                SeedReport::default(),
                &stages,
            )
            .await
            .expect("charting runs");

        let ChartRun::Charted(output) = run else {
            panic!("expected a charted map");
        };

        assert_eq!(output.tickets.len(), 2, "both candidates became tickets");
        assert!(
            !output.wire_repair.cycles_broken.is_empty(),
            "the wirer produced a cycle, so the repair must report breaking one"
        );

        // The returned edges must be acyclic. Walk them and prove it rather
        // than trusting the repair reported what it did.
        assert!(
            crate::wire::detect_cycle(&output.tickets, &output.edges).is_none(),
            "the pipeline returned a cyclical edge set: {:?}",
            output.edges
        );
    }

    #[tokio::test]
    async fn a_run_killed_after_stage_4_resumes_and_matches_an_uninterrupted_run() {
        let axis_set = crate::axes::AxisSet {
            axes: vec!["failure modes and error handling".to_owned()],
        };
        let mut axis_sets = AxisSets::builtin();
        axis_sets.decision = axis_set;

        let extractor = FixedExtractor(ExtractOutput {
            candidates: vec![
                candidate(
                    "retry cap",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Task,
                ),
                candidate(
                    "staleness policy",
                    "failure modes and error handling",
                    crate::chart::ticket::TicketKind::Grilling,
                ),
            ],
            out_of_scope: vec![],
        });
        let sharpener = FixedSharpener {
            fog_names: vec!["staleness policy".to_owned()],
        };
        let config = base_config();

        // An uninterrupted run, start to finish.
        let full_engine = FakeEngine::new(Script {
            turns: vec![
                destination_turn("decision"),
                Turn {
                    text: "Retries are undecided; staleness is also undecided.".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let full_store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        let full_pipeline =
            ChartPipeline::new(&full_engine, config.clone(), &axis_sets, &full_store);
        let stages = StageImpls {
            extractor: &extractor,
            sharpener: &sharpener,
            sizer: &AlwaysOkSizer,
            wirer: &NoBlockersWirer,
        };
        let full_run = full_pipeline
            .chart(
                "map-resume",
                DestinationInput {
                    idea: "x",
                    agents_md: "",
                    repo_summary: "",
                },
                SeedReport::default(),
                &stages,
            )
            .await
            .expect("the full run charts");

        // A run "killed" after stage 4: only run far enough to checkpoint
        // stages 1 to 4, using a fresh checkpoint store to simulate a
        // process that never got further.
        let partial_engine = FakeEngine::new(Script {
            turns: vec![
                destination_turn("decision"),
                Turn {
                    text: "Retries are undecided; staleness is also undecided.".to_owned(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let resume_store = FileCheckpointStore::new(
            std::env::temp_dir().join(format!("dark-plan-test-{}.jsonl", ulid::Ulid::new())),
        );
        // Prime the resume store with exactly the checkpoints stages 1 to 4
        // would have written, copied from the full run's store, so this
        // test does not depend on being able to interrupt run() mid-flight.
        for stage in [
            Stage::Destination,
            Stage::Seed,
            Stage::AxisSweep,
            Stage::Extract,
        ] {
            if let Some(checkpoint) = full_store
                .load("map-resume")
                .unwrap()
                .into_iter()
                .find(|cp| cp.stage == stage)
            {
                resume_store.record(&checkpoint).unwrap();
            }
        }
        drop(partial_engine); // never called: resume must not re-run stages 1 to 4.

        // Resuming must not call the engine again for stages already
        // checkpointed. A script with zero turns proves it: any call would
        // fail with "the script has 0 turn(s)".
        let resume_engine = FakeEngine::new(Script::default());
        let resume_pipeline = ChartPipeline::new(&resume_engine, config, &axis_sets, &resume_store);
        let resumed_run = resume_pipeline
            .resume("map-resume", SeedReport::default(), &stages)
            .await
            .expect("resume completes without calling the engine again");

        assert_same_decisions(&resumed_run, &full_run);
        assert_eq!(
            resume_engine.turns_played(),
            0,
            "resume must not re-run a stage that already has a checkpoint"
        );
    }

    /// Asserts that two [`ChartRun`] values record the same decisions,
    /// ignoring [`ChartedTicket::id`] and the identifiers
    /// [`ChartedEdge`] carries.
    ///
    /// A charted map's ticket identifiers are `ulid::Ulid` values, minted
    /// fresh wherever stage 6 (size) actually runs (see the note in `run`
    /// on why identifier assignment sits there). Two runs of stage 6 —
    /// even from byte-identical prior checkpoints and a byte-identical
    /// model script — mint different identifiers by design: a ULID is
    /// built to be unique per creation, not reproducible from its inputs.
    /// So "the same map" (task unit `E1`'s "Done when") is read here as
    /// the same decisions — same destination, same tickets by name and
    /// content, same blocking structure, same fog — not bit-identical
    /// identifiers. A resumed run that stopped *after* stage 6 already
    /// checkpointed its ticket identifiers would replay those exact
    /// identifiers instead of minting new ones, and would pass an even
    /// stricter, identifier-inclusive comparison; this test resumes from
    /// before stage 6 runs, so it checks the weaker, content-only
    /// guarantee that still holds in that case.
    fn assert_same_decisions(left: &ChartRun, right: &ChartRun) {
        match (left, right) {
            (
                ChartRun::NoMapNeeded {
                    destination: left_destination,
                    note: left_note,
                },
                ChartRun::NoMapNeeded {
                    destination: right_destination,
                    note: right_note,
                },
            ) => {
                assert_eq!(left_destination, right_destination);
                assert_eq!(left_note, right_note);
            }
            (ChartRun::Charted(left), ChartRun::Charted(right)) => {
                assert_eq!(left.map_id, right.map_id);
                assert_eq!(left.destination, right.destination);
                assert_eq!(left.seed, right.seed);
                assert_eq!(left.axis_answers, right.axis_answers);
                assert_eq!(left.out_of_scope, right.out_of_scope);
                assert_eq!(left.fog, right.fog);
                assert_eq!(
                    ticket_content(&left.tickets),
                    ticket_content(&right.tickets)
                );
                assert_eq!(
                    edges_by_name(&left.tickets, &left.edges),
                    edges_by_name(&right.tickets, &right.edges)
                );
            }
            _ => {
                panic!("left and right disagree on whether a map was needed: {left:?} vs {right:?}")
            }
        }
    }

    /// Projects a ticket list to the fields that must match across two runs
    /// with independently minted identifiers.
    fn ticket_content(
        tickets: &[ChartedTicket],
    ) -> Vec<(
        String,
        String,
        crate::chart::ticket::TicketKind,
        bool,
        i64,
        Vec<String>,
    )> {
        tickets
            .iter()
            .map(|ticket| {
                (
                    ticket.name.clone(),
                    ticket.question.clone(),
                    ticket.ticket_type,
                    ticket.hitl,
                    ticket.ordinal,
                    ticket.axis.clone(),
                )
            })
            .collect()
    }

    /// Resolves each edge's identifiers back to ticket names, so two edge
    /// lists built from independently minted identifiers can still compare
    /// equal.
    fn edges_by_name(tickets: &[ChartedTicket], edges: &[ChartedEdge]) -> Vec<(String, String)> {
        let name_of = |id: &str| -> String {
            tickets
                .iter()
                .find(|ticket| ticket.id == id)
                .map_or_else(String::new, |ticket| ticket.name.clone())
        };
        let mut pairs: Vec<(String, String)> = edges
            .iter()
            .map(|edge| (name_of(&edge.blocker), name_of(&edge.blocked)))
            .collect();
        pairs.sort();
        pairs
    }
}
