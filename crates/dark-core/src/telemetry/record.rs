//! One line of `$DARK_HOME/telemetry.jsonl`: the measurements the harness
//! takes for one turn, and nothing else.
//!
//! A [`TelemetryRecord`] carries counts and durations only. See the
//! `telemetry` module documentation for why it never carries the text a
//! person typed, an assistant's reply, or a tool result's content.

use serde::{Deserialize, Serialize};

/// One turn's measurements, ready to append to `telemetry.jsonl`.
///
/// Every field is a count or a duration. None of them can be traced back to
/// what was said in the turn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TelemetryRecord {
    /// How long the turn took, in milliseconds. Mirrors
    /// [`dark_contract::Event::TurnEnd`]'s `wall_ms`.
    pub turn_ms: u64,
    /// Tokens in the request. Mirrors [`dark_contract::Usage::prompt_tokens`].
    pub prompt_tokens: usize,
    /// Tokens the model generated, thinking included. Mirrors
    /// [`dark_contract::Usage::completion_tokens`].
    pub completion_tokens: usize,
    /// Prompt tokens the engine served from its cache. Mirrors
    /// [`dark_contract::Usage::cached_tokens`].
    ///
    /// [`TelemetryRecord::cache_hit_rate`] turns this into the ratio that
    /// `dark stats` shows first: the strongest predictor of perceived
    /// speed (task unit `J6`, step 5).
    pub cached_tokens: usize,
    /// Model loads that finished during this turn.
    pub model_loads: u32,
    /// Total time those loads took, in milliseconds.
    pub model_load_ms: u64,
    /// Tool calls that finished during this turn.
    pub tool_calls: u32,
    /// The part of `tool_calls` that failed.
    pub tool_failures: u32,
    /// Frame-budget overruns reported during this turn.
    ///
    /// See `dark_core::telemetry::TelemetryRecorder::record_frame_overrun`
    /// for why this count arrives out of band from the event bus, rather
    /// than from an [`dark_contract::Event`] variant.
    pub frame_overruns: u32,
}

impl TelemetryRecord {
    /// Returns the generation rate for this turn, in tokens per second.
    ///
    /// Returns `0.0` when the turn took no measurable time, rather than
    /// dividing by zero.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn tokens_per_second(&self) -> f64 {
        if self.turn_ms == 0 {
            return 0.0;
        }
        (self.completion_tokens as f64) / (self.turn_ms as f64 / 1000.0)
    }

    /// Returns the prefix cache hit rate: the share of prompt tokens the
    /// engine served from its cache, between `0.0` and `1.0`.
    ///
    /// `dark stats` shows this figure first (task unit `J6`, step 5): it is
    /// the strongest predictor of perceived speed, because a cache miss is
    /// what forces the full prefill that costs 15 to 30 seconds on a 32B
    /// model (see `CLAUDE.md`, "Constraints that shape the code").
    ///
    /// Returns `0.0` when the turn used no prompt tokens.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn cache_hit_rate(&self) -> f64 {
        if self.prompt_tokens == 0 {
            return 0.0;
        }
        (self.cached_tokens as f64) / (self.prompt_tokens as f64)
    }

    /// Returns the share of this turn's tool calls that failed, between
    /// `0.0` and `1.0`.
    ///
    /// Returns `0.0` when the turn made no tool calls.
    #[must_use]
    pub fn tool_failure_rate(&self) -> f64 {
        if self.tool_calls == 0 {
            return 0.0;
        }
        f64::from(self.tool_failures) / f64::from(self.tool_calls)
    }

    /// Returns the average duration of one model load during this turn, in
    /// milliseconds.
    ///
    /// Returns `0.0` when no model load finished during this turn.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn mean_model_load_ms(&self) -> f64 {
        if self.model_loads == 0 {
            return 0.0;
        }
        (self.model_load_ms as f64) / f64::from(self.model_loads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> TelemetryRecord {
        TelemetryRecord {
            turn_ms: 2000,
            prompt_tokens: 1000,
            completion_tokens: 400,
            cached_tokens: 750,
            model_loads: 2,
            model_load_ms: 3000,
            tool_calls: 4,
            tool_failures: 1,
            frame_overruns: 3,
        }
    }

    #[test]
    fn tokens_per_second_divides_completion_tokens_by_wall_time() {
        assert!((record().tokens_per_second() - 200.0).abs() < 1e-9);
    }

    #[test]
    fn tokens_per_second_is_zero_for_a_zero_length_turn() {
        let mut r = record();
        r.turn_ms = 0;
        assert!((r.tokens_per_second() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn cache_hit_rate_is_the_share_of_prompt_tokens_served_from_cache() {
        assert!((record().cache_hit_rate() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn cache_hit_rate_is_zero_when_the_turn_used_no_prompt_tokens() {
        let mut r = record();
        r.prompt_tokens = 0;
        r.cached_tokens = 0;
        assert!((r.cache_hit_rate() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn tool_failure_rate_divides_failures_by_calls() {
        assert!((record().tool_failure_rate() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn tool_failure_rate_is_zero_when_no_tool_ran() {
        let mut r = record();
        r.tool_calls = 0;
        r.tool_failures = 0;
        assert!((r.tool_failure_rate() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn mean_model_load_ms_divides_load_time_by_load_count() {
        assert!((record().mean_model_load_ms() - 1500.0).abs() < 1e-9);
    }

    #[test]
    fn mean_model_load_ms_is_zero_when_no_load_finished() {
        let mut r = record();
        r.model_loads = 0;
        r.model_load_ms = 0;
        assert!((r.mean_model_load_ms() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let r = record();
        let line = serde_json::to_string(&r).unwrap();
        let back: TelemetryRecord = serde_json::from_str(&line).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn a_record_serialises_to_only_the_documented_fields() {
        // Guards against a future field slipping in that carries prompt
        // text or file content: every key here must be a count or a
        // duration, and this list must match the struct exactly.
        let value = serde_json::to_value(record()).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "cached_tokens",
                "completion_tokens",
                "frame_overruns",
                "model_load_ms",
                "model_loads",
                "prompt_tokens",
                "tool_calls",
                "tool_failures",
                "turn_ms",
            ]
        );
    }
}
