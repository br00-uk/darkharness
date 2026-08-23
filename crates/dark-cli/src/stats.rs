//! `dark stats`: aggregates `$DARK_HOME/telemetry.jsonl` and renders it for
//! a person to read.
//!
//! Task unit `J6`, step 5, asks for the prefix cache hit rate first,
//! because it is the strongest predictor of perceived speed (see
//! [`dark_core::telemetry::TelemetryRecord::cache_hit_rate`]): a cache
//! miss forces the full prefill that costs 15 to 30 seconds on a 32B
//! model (`CLAUDE.md`, "Constraints that shape the code"). This module
//! puts that figure first in both the chart and the summary table.
//!
//! Every number here comes from
//! [`dark_core::telemetry::TelemetryRecord`]: counts and durations only,
//! never a prompt, a reply, or a tool result's content. This command reads
//! a file on disk and prints what it finds; it opens no network
//! connection, the same as every other command in this binary.

use std::path::Path;

use dark_core::telemetry::{TelemetryRecord, read_records};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};

/// The width, in character cells, of the rendered chart.
const CHART_WIDTH: u16 = 72;
/// The height, in character cells, of the rendered chart.
const CHART_HEIGHT: u16 = 16;
/// How many of the most recent turns the chart plots.
const CHART_TURN_WINDOW: usize = 40;

/// The aggregate figures `dark stats` reports, folded from every
/// [`TelemetryRecord`] in `telemetry.jsonl`.
///
/// Every rate is a ratio of sums, not a mean of ratios: summing the
/// numerators and the denominators first weights every token, load, and
/// tool call equally, so one long turn does not count for less than ten
/// short ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Summary {
    /// How many turns telemetry has recorded.
    pub(crate) turns: usize,
    /// Total prompt tokens across every recorded turn.
    pub(crate) prompt_tokens: u64,
    /// Total completion tokens across every recorded turn.
    pub(crate) completion_tokens: u64,
    /// Total prompt tokens the engine served from its cache.
    pub(crate) cached_tokens: u64,
    /// Total wall time across every recorded turn, in milliseconds.
    pub(crate) turn_ms: u64,
    /// Model loads across every recorded turn.
    pub(crate) model_loads: u32,
    /// Total time those loads took, in milliseconds.
    pub(crate) model_load_ms: u64,
    /// Tool calls across every recorded turn.
    pub(crate) tool_calls: u32,
    /// The part of `tool_calls` that failed.
    pub(crate) tool_failures: u32,
    /// Frame-budget overruns across every recorded turn.
    pub(crate) frame_overruns: u32,
}

impl Summary {
    /// Folds `records` into one [`Summary`].
    ///
    /// Returns every figure at zero for an empty slice, rather than an
    /// `Option`: a person who has not yet run a turn is a valid state for
    /// `dark stats`, not an error. [`Summary::render`] shows a distinct
    /// message for that case instead of an empty table.
    fn from_records(records: &[TelemetryRecord]) -> Self {
        let mut summary = Self {
            turns: records.len(),
            prompt_tokens: 0,
            completion_tokens: 0,
            cached_tokens: 0,
            turn_ms: 0,
            model_loads: 0,
            model_load_ms: 0,
            tool_calls: 0,
            tool_failures: 0,
            frame_overruns: 0,
        };
        for record in records {
            summary.prompt_tokens += record.prompt_tokens as u64;
            summary.completion_tokens += record.completion_tokens as u64;
            summary.cached_tokens += record.cached_tokens as u64;
            summary.turn_ms += record.turn_ms;
            summary.model_loads += record.model_loads;
            summary.model_load_ms += record.model_load_ms;
            summary.tool_calls += record.tool_calls;
            summary.tool_failures += record.tool_failures;
            summary.frame_overruns += record.frame_overruns;
        }
        summary
    }

    /// Returns the prefix cache hit rate across every recorded turn,
    /// between `0.0` and `1.0`. `0.0` when no prompt token was recorded.
    #[allow(clippy::cast_precision_loss)]
    fn cache_hit_rate(&self) -> f64 {
        if self.prompt_tokens == 0 {
            return 0.0;
        }
        (self.cached_tokens as f64) / (self.prompt_tokens as f64)
    }

    /// Returns the generation rate across every recorded turn, in tokens
    /// per second. `0.0` when no wall time was recorded.
    #[allow(clippy::cast_precision_loss)]
    fn tokens_per_second(&self) -> f64 {
        if self.turn_ms == 0 {
            return 0.0;
        }
        (self.completion_tokens as f64) / (self.turn_ms as f64 / 1000.0)
    }

    /// Returns the mean turn duration, in milliseconds. `0.0` when no turn
    /// was recorded.
    #[allow(clippy::cast_precision_loss)]
    fn mean_turn_ms(&self) -> f64 {
        if self.turns == 0 {
            return 0.0;
        }
        (self.turn_ms as f64) / (self.turns as f64)
    }

    /// Returns the mean duration of one model load, in milliseconds. `0.0`
    /// when no load was recorded.
    #[allow(clippy::cast_precision_loss)]
    fn mean_model_load_ms(&self) -> f64 {
        if self.model_loads == 0 {
            return 0.0;
        }
        (self.model_load_ms as f64) / f64::from(self.model_loads)
    }

    /// Returns the tool failure rate across every recorded turn, between
    /// `0.0` and `1.0`. `0.0` when no tool call was recorded.
    fn tool_failure_rate(&self) -> f64 {
        if self.tool_calls == 0 {
            return 0.0;
        }
        f64::from(self.tool_failures) / f64::from(self.tool_calls)
    }

    /// Renders the summary as text for a person to read, the prefix cache
    /// hit rate first (task unit `J6`, step 5).
    fn render(&self) -> String {
        use std::fmt::Write as _;
        if self.turns == 0 {
            return "No telemetry recorded yet. Run a turn, then run `dark stats` again.\n"
                .to_owned();
        }
        let mut out = String::new();
        let _ = writeln!(
            out,
            "prefix cache hit rate:  {:.1}% ({} of {} prompt tokens)",
            self.cache_hit_rate() * 100.0,
            self.cached_tokens,
            self.prompt_tokens,
        );
        let _ = writeln!(out, "turns recorded:         {}", self.turns);
        let _ = writeln!(out, "mean turn duration:     {:.0} ms", self.mean_turn_ms());
        let _ = writeln!(
            out,
            "tokens in / out:        {} / {}",
            self.prompt_tokens, self.completion_tokens,
        );
        let _ = writeln!(
            out,
            "generation rate:        {:.1} tok/s",
            self.tokens_per_second()
        );
        let _ = writeln!(
            out,
            "model loads:            {} ({:.0} ms mean)",
            self.model_loads,
            self.mean_model_load_ms()
        );
        let _ = writeln!(
            out,
            "tool failure rate:      {:.1}% ({} of {} calls)",
            self.tool_failure_rate() * 100.0,
            self.tool_failures,
            self.tool_calls,
        );
        let _ = writeln!(out, "frame budget overruns:  {}", self.frame_overruns);
        out
    }
}

/// Renders the prefix cache hit rate over the most recent
/// [`CHART_TURN_WINDOW`] turns as a `ratatui` [`Chart`], and returns the
/// drawing as text.
///
/// `dark stats` runs once and exits; it draws to a
/// [`ratatui::backend::TestBackend`] instead of a live terminal, and prints
/// the resulting cell grid the same way any other line of output prints.
#[allow(clippy::cast_precision_loss)]
fn render_chart(records: &[TelemetryRecord]) -> String {
    let window = &records[records.len().saturating_sub(CHART_TURN_WINDOW)..];
    let points: Vec<(f64, f64)> = window
        .iter()
        .enumerate()
        .map(|(index, record)| (index as f64, record.cache_hit_rate() * 100.0))
        .collect();

    let last_index = (window.len().saturating_sub(1)) as f64;
    let dataset = Dataset::default()
        .name("cache hit rate")
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points);

    let chart = Chart::new(vec![dataset])
        .block(Block::default().borders(Borders::ALL).title(format!(
            "prefix cache hit rate — last {} turns",
            window.len()
        )))
        .x_axis(
            Axis::default()
                .bounds([0.0, last_index.max(1.0)])
                .labels(["oldest", "newest"]),
        )
        .y_axis(
            Axis::default()
                .bounds([0.0, 100.0])
                .labels(["0%", "50%", "100%"]),
        );

    let backend = TestBackend::new(CHART_WIDTH, CHART_HEIGHT);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| frame.render_widget(chart, frame.area()))
        .expect("drawing to a TestBackend never fails");

    buffer_to_text(terminal.backend().buffer().area, terminal.backend())
}

/// Converts every cell in `area` of a [`TestBackend`]'s buffer to text, one
/// line per row.
fn buffer_to_text(area: Rect, backend: &TestBackend) -> String {
    let buffer = backend.buffer();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Runs `dark stats`: reads `telemetry.jsonl`, prints the prefix cache hit
/// rate chart, then the summary table.
///
/// # Errors
///
/// Returns an error when `telemetry.jsonl` exists but cannot be read or
/// parsed. See [`dark_core::telemetry::read_records`].
pub(crate) fn run_command() -> anyhow::Result<()> {
    let dark_home = crate::dark_home();
    let records = block_on_read(&dark_home)?;
    let summary = Summary::from_records(&records);

    if !records.is_empty() {
        print!("{}", render_chart(&records));
        println!();
    }
    print!("{}", summary.render());
    Ok(())
}

/// Runs [`read_records`] to completion on a single-threaded runtime.
///
/// `dark-cli` otherwise has no async code: `dark-core`'s telemetry reader
/// is `async` because it shares its file I/O with
/// [`dark_core::session::transcript`], which does need to interleave with
/// the turn loop. `dark stats` has nothing to interleave with, so it opens
/// the smallest runtime that can drive one future to completion, the same
/// way a synchronous caller elsewhere in the workspace would use
/// `futures::executor::block_on`.
fn block_on_read(dark_home: &Path) -> anyhow::Result<Vec<TelemetryRecord>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|err| anyhow::anyhow!("could not start the telemetry reader: {err}"))?;
    Ok(runtime.block_on(read_records(dark_home))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(prompt: usize, completion: usize, cached: usize, turn_ms: u64) -> TelemetryRecord {
        TelemetryRecord {
            turn_ms,
            prompt_tokens: prompt,
            completion_tokens: completion,
            cached_tokens: cached,
            model_loads: 0,
            model_load_ms: 0,
            tool_calls: 0,
            tool_failures: 0,
            frame_overruns: 0,
        }
    }

    #[test]
    fn summary_of_no_records_is_all_zero() {
        let summary = Summary::from_records(&[]);
        assert_eq!(summary.turns, 0);
        assert_eq!(summary.prompt_tokens, 0);
        assert!((summary.cache_hit_rate() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn summary_sums_numerators_and_denominators_before_dividing() {
        // A ratio of sums, not a mean of ratios: one turn with 900 of 1000
        // prompt tokens cached and one with 0 of 10 cached averages to
        // 90/1010, not to the mean of 0.9 and 0.0.
        let records = vec![record(1000, 100, 900, 2000), record(10, 5, 0, 100)];
        let summary = Summary::from_records(&records);
        assert_eq!(summary.turns, 2);
        assert_eq!(summary.prompt_tokens, 1010);
        assert_eq!(summary.cached_tokens, 900);
        assert!((summary.cache_hit_rate() - 900.0 / 1010.0).abs() < 1e-9);
    }

    #[test]
    fn tokens_per_second_divides_total_completion_tokens_by_total_wall_time() {
        let records = vec![record(0, 100, 0, 1000), record(0, 100, 0, 1000)];
        let summary = Summary::from_records(&records);
        // 200 completion tokens over 2000 ms = 100 tok/s.
        assert!((summary.tokens_per_second() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn tool_failure_rate_divides_total_failures_by_total_calls() {
        let mut a = record(0, 0, 0, 0);
        a.tool_calls = 4;
        a.tool_failures = 1;
        let mut b = record(0, 0, 0, 0);
        b.tool_calls = 6;
        b.tool_failures = 2;
        let summary = Summary::from_records(&[a, b]);
        assert_eq!(summary.tool_calls, 10);
        assert_eq!(summary.tool_failures, 3);
        assert!((summary.tool_failure_rate() - 0.3).abs() < 1e-9);
    }

    #[test]
    fn render_shows_the_cache_hit_rate_first() {
        let records = vec![record(1000, 100, 500, 2000)];
        let summary = Summary::from_records(&records);
        let text = summary.render();
        let first_line = text.lines().next().unwrap();
        assert!(
            first_line.contains("prefix cache hit rate"),
            "the first line was {first_line:?}, not the cache hit rate"
        );
        assert!(first_line.contains("50.0%"));
    }

    #[test]
    fn render_of_an_empty_summary_says_so_instead_of_printing_zeros() {
        let summary = Summary::from_records(&[]);
        let text = summary.render();
        assert!(text.contains("No telemetry recorded yet"));
        assert!(!text.contains("prefix cache hit rate"));
    }

    #[test]
    fn render_chart_produces_the_requested_dimensions() {
        let records = vec![record(100, 50, 25, 1000), record(100, 50, 75, 1000)];
        let text = render_chart(&records);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), CHART_HEIGHT as usize);
        for line in &lines {
            assert_eq!(line.chars().count(), CHART_WIDTH as usize);
        }
    }

    #[test]
    fn render_chart_names_the_turn_window_in_its_title() {
        let records: Vec<TelemetryRecord> = (0..5).map(|i| record(100, 50, i * 10, 1000)).collect();
        let text = render_chart(&records);
        assert!(text.contains("last 5 turns"));
    }

    #[test]
    fn render_chart_clips_to_the_turn_window() {
        let records: Vec<TelemetryRecord> = (0..(CHART_TURN_WINDOW + 10))
            .map(|i| record(100, 50, i % 100, 1000))
            .collect();
        let text = render_chart(&records);
        assert!(text.contains(&format!("last {CHART_TURN_WINDOW} turns")));
    }
}
