//! Turns the event bus into [`TelemetryRecord`] lines.
//!
//! [`TelemetryRecorder`] watches the same [`Event`] stream
//! [`crate::session::transcript::TranscriptWriter`] watches, and folds it
//! into one [`TelemetryRecord`] per turn: [`Event::TurnStart`] opens the
//! turn's counters, [`Event::ModelLoading`] and [`Event::ToolResult`] feed
//! them, and [`Event::TurnEnd`] closes the turn and writes the line.
//!
//! # The frame-budget gap
//!
//! Step 1 of task unit `J6` asks this module to record frame-budget
//! overruns. `dark-tui` is the only crate that measures one — see
//! `dark-tui::anim::budget::FrameBudget` — and Rule 14 confines `dark-tui`
//! to a dependency on `dark-contract` alone, while [`Event`] carries
//! nothing that names a frame overrun and, in any case, flows only from
//! `dark-core` to `dark-tui`, never the other way; [`dark_contract::Intent`]
//! is the channel that runs from `dark-tui` back to `dark-core`, and it
//! carries no such variant either.
//!
//! Nothing in the workspace can call
//! [`TelemetryRecorder::record_frame_overrun`] today. Wiring it needs
//! either a new `Intent` variant carrying the count back from the terminal
//! application, or a frame-overrun event added to [`Event`] itself, and
//! either change belongs to `dark-contract`, which this task unit does not
//! own — `dark-contract` also changes only between waves (`CLAUDE.md`),
//! and other agents are compiling against it as this module lands. This
//! module exposes the counting method and the record field regardless, so
//! that later change needs only to add the call, not the field.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use dark_contract::{Event, Result};

use super::record::TelemetryRecord;
use super::writer::TelemetryWriter;

/// Accumulates one turn's counters until [`Event::TurnEnd`] closes it.
#[derive(Debug, Default)]
struct TurnAccumulator {
    /// Model loads that finished during this turn.
    model_loads: u32,
    /// Total time those loads took, in milliseconds.
    model_load_ms: u64,
    /// Tool calls that finished during this turn.
    tool_calls: u32,
    /// The part of `tool_calls` that failed.
    tool_failures: u32,
    /// Frame-budget overruns reported during this turn. See the module
    /// documentation for why nothing feeds this yet.
    frame_overruns: u32,
}

/// Watches the event bus and writes one [`TelemetryRecord`] per turn to
/// `telemetry.jsonl`.
#[derive(Debug)]
pub struct TelemetryRecorder {
    writer: TelemetryWriter,
    turn: TurnAccumulator,
    /// Models whose load is in progress, keyed by model identifier, with
    /// the instant the first sub-`1.0` [`Event::ModelLoading`] for that
    /// model arrived. [`Event`] carries a load's progress but not its
    /// start time, so this recorder measures load duration by wall clock
    /// as the events arrive, the same way it must for every other duration
    /// here.
    loads_in_progress: HashMap<String, Instant>,
}

impl TelemetryRecorder {
    /// Wraps an open [`TelemetryWriter`].
    #[must_use]
    pub fn new(writer: TelemetryWriter) -> Self {
        Self {
            writer,
            turn: TurnAccumulator::default(),
            loads_in_progress: HashMap::new(),
        }
    }

    /// Returns the path this recorder appends to.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        self.writer.path()
    }

    /// Records one frame-budget overrun for the turn in progress.
    ///
    /// See the module documentation: nothing in the workspace calls this
    /// today, because no event or intent carries the count from
    /// `dark-tui` into `dark-core`. The method exists so that gap is the
    /// only piece of wiring a later change needs to add.
    pub fn record_frame_overrun(&mut self) {
        self.turn.frame_overruns += 1;
    }

    /// Feeds one event from the bus into the accumulator, writing a
    /// [`TelemetryRecord`] when the event is [`Event::TurnEnd`].
    ///
    /// Reads only the fields [`Event`] documents as counts and durations:
    /// [`Event::TurnEnd`]'s `usage` and `wall_ms`, [`Event::ModelLoading`]'s
    /// `model` and `progress`, and [`Event::ToolResult`]'s
    /// `result.is_error`. It never reads [`Event::UserMessage`]'s `text`,
    /// [`Event::TokenDelta`]'s or [`Event::ReasonDelta`]'s `text`, or
    /// [`Event::ToolResult`]'s `content` — see the `telemetry` module
    /// documentation and step 2 of task unit `J6`.
    ///
    /// # Errors
    ///
    /// Returns an error when writing the record fails.
    pub async fn on_event(&mut self, event: &Event) -> Result<()> {
        match event {
            Event::TurnStart { .. } => {
                self.turn = TurnAccumulator::default();
            }
            Event::ModelLoading { model, progress } => {
                if *progress >= 1.0 {
                    if let Some(start) = self.loads_in_progress.remove(model) {
                        self.turn.model_loads += 1;
                        self.turn.model_load_ms += millis_saturating(start.elapsed());
                    }
                } else {
                    self.loads_in_progress
                        .entry(model.clone())
                        .or_insert_with(Instant::now);
                }
            }
            Event::ToolResult { result, .. } => {
                self.turn.tool_calls += 1;
                if result.is_error {
                    self.turn.tool_failures += 1;
                }
            }
            Event::TurnEnd { usage, wall_ms, .. } => {
                let record = TelemetryRecord {
                    turn_ms: *wall_ms,
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    cached_tokens: usage.cached_tokens,
                    model_loads: self.turn.model_loads,
                    model_load_ms: self.turn.model_load_ms,
                    tool_calls: self.turn.tool_calls,
                    tool_failures: self.turn.tool_failures,
                    frame_overruns: self.turn.frame_overruns,
                };
                self.writer.record(&record).await?;
                self.turn = TurnAccumulator::default();
            }
            _ => {}
        }
        Ok(())
    }
}

/// Converts a measured [`Duration`] to milliseconds, saturating at
/// [`u64::MAX`] rather than panicking on a duration too large to fit — a
/// model load never approaches that in practice.
fn millis_saturating(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::super::writer::read_records;
    use super::*;
    use dark_contract::{ErrCode, RoleClass, ToolResultSummary, Usage};
    use tempfile::TempDir;

    fn turn_start(turn: &str) -> Event {
        Event::TurnStart {
            turn: turn.to_owned(),
            class: RoleClass::Worker,
            model: "test-model".to_owned(),
        }
    }

    fn turn_end(turn: &str, usage: Usage, wall_ms: u64) -> Event {
        Event::TurnEnd {
            turn: turn.to_owned(),
            usage,
            wall_ms,
        }
    }

    fn model_loading(model: &str, progress: f32) -> Event {
        Event::ModelLoading {
            model: model.to_owned(),
            progress,
        }
    }

    fn tool_result(is_error: bool) -> Event {
        Event::ToolResult {
            turn: "t1".to_owned(),
            call_id: "c1".to_owned(),
            result: ToolResultSummary {
                name: "read_file".to_owned(),
                is_error,
                bytes: 3,
                headline: "ok".to_owned(),
                has_diff: false,
            },
            content: "irrelevant".to_owned(),
        }
    }

    async fn recorder(tmp: &TempDir) -> TelemetryRecorder {
        let writer = TelemetryWriter::open(tmp.path()).await.unwrap();
        TelemetryRecorder::new(writer)
    }

    #[tokio::test]
    async fn turn_end_writes_one_record_from_the_turns_usage() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        let usage = Usage {
            prompt_tokens: 500,
            completion_tokens: 120,
            reasoning_tokens: 10,
            cached_tokens: 400,
        };
        rec.on_event(&turn_end("t1", usage, 3000)).await.unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].turn_ms, 3000);
        assert_eq!(records[0].prompt_tokens, 500);
        assert_eq!(records[0].completion_tokens, 120);
        assert_eq!(records[0].cached_tokens, 400);
    }

    #[tokio::test]
    async fn a_completed_model_load_is_counted_and_timed() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.on_event(&model_loading("m", 0.0)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        rec.on_event(&model_loading("m", 1.0)).await.unwrap();
        rec.on_event(&turn_end("t1", Usage::default(), 100))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records[0].model_loads, 1);
        assert!(records[0].model_load_ms >= 5);
    }

    #[tokio::test]
    async fn a_load_that_never_reaches_1_0_is_not_counted() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.on_event(&model_loading("m", 0.5)).await.unwrap();
        rec.on_event(&turn_end("t1", Usage::default(), 100))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records[0].model_loads, 0);
        assert_eq!(records[0].model_load_ms, 0);
    }

    #[tokio::test]
    async fn tool_results_are_counted_and_failures_tallied_separately() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.on_event(&tool_result(false)).await.unwrap();
        rec.on_event(&tool_result(true)).await.unwrap();
        rec.on_event(&tool_result(false)).await.unwrap();
        rec.on_event(&turn_end("t1", Usage::default(), 100))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records[0].tool_calls, 3);
        assert_eq!(records[0].tool_failures, 1);
    }

    #[tokio::test]
    async fn record_frame_overrun_lands_in_the_next_turn_end() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.record_frame_overrun();
        rec.record_frame_overrun();
        rec.on_event(&turn_end("t1", Usage::default(), 100))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records[0].frame_overruns, 2);
    }

    #[tokio::test]
    async fn turn_start_resets_counters_left_over_from_an_unflushed_turn() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.on_event(&tool_result(true)).await.unwrap();
        // No TurnEnd for t1: a fresh TurnStart must not carry t1's counts
        // into t2.
        rec.on_event(&turn_start("t2")).await.unwrap();
        rec.on_event(&turn_end("t2", Usage::default(), 100))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records[0].tool_calls, 0);
        assert_eq!(records[0].tool_failures, 0);
    }

    #[tokio::test]
    async fn each_turn_produces_its_own_record() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&turn_start("t1")).await.unwrap();
        rec.on_event(&turn_end("t1", Usage::default(), 100))
            .await
            .unwrap();
        rec.on_event(&turn_start("t2")).await.unwrap();
        rec.on_event(&turn_end("t2", Usage::default(), 200))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].turn_ms, 100);
        assert_eq!(records[1].turn_ms, 200);
    }

    #[tokio::test]
    async fn an_event_that_is_none_of_the_above_is_ignored() {
        let tmp = TempDir::new().unwrap();
        let mut rec = recorder(&tmp).await;

        rec.on_event(&Event::Notice("hello".to_owned()))
            .await
            .unwrap();

        let records = read_records(tmp.path()).await.unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn path_reports_the_underlying_writers_path() {
        let tmp = TempDir::new().unwrap();
        let rec = recorder(&tmp).await;
        assert_eq!(rec.path(), super::super::writer::telemetry_path(tmp.path()));
    }

    #[test]
    fn every_error_this_module_can_return_is_tool_failed() {
        // The taxonomy has no telemetry-specific domain (see writer.rs);
        // this pins the choice so a future edit does not drift silently.
        assert_eq!(ErrCode::ToolFailed.as_str(), "E_TOOL_FAILED");
    }
}
