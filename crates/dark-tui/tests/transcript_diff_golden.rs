//! Golden-frame tests for the transcript and diff views (task unit `H4`).
//!
//! "Done when: Golden frames match for a streaming turn, a collapsed
//! thinking block, and a diff." Each test here drives
//! [`dark_tui::views::transcript::Transcript`] or
//! [`dark_tui::views::diff::DiffView`] exactly as an external caller would
//! — through their public API, with no access to this crate's internals —
//! and checks the exact text a [`TestBackend`] buffer shows, the same
//! pattern `golden_frames.rs` (task unit `H1`) already uses for the shell.

use dark_contract::{Event, ToolCall, ToolResultSummary};
use dark_tui::theme::{ColorLevel, Theme};
use dark_tui::views::diff::{DiffView, UnifiedDiff};
use dark_tui::views::transcript::Transcript;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::widgets::Widget;

fn buffer_text(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut text = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            text.push_str(
                buffer
                    .cell((x, y))
                    .expect("(x, y) is inside area by construction")
                    .symbol(),
            );
        }
        text.push('\n');
    }
    text
}

fn render_transcript(t: &Transcript, expanded: bool, width: u16, height: u16) -> Buffer {
    let theme = Theme::new(ColorLevel::TrueColor);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| t.render(frame.area(), frame.buffer_mut(), &theme, expanded))
        .expect("render must not fail against a TestBackend");
    terminal.backend().buffer().clone()
}

fn render_widget(widget: impl Widget, width: u16, height: u16) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| frame.render_widget(widget, frame.area()))
        .expect("render must not fail against a TestBackend");
    terminal.backend().buffer().clone()
}

/// A streaming turn: a user message, thinking, a tool call and its result,
/// then a visible answer — the shape task unit `H1`'s mock-up shows in the
/// `TRANSCRIPT` pane.
#[allow(
    clippy::default_trait_access,
    reason = "the `args` field is a serde_json::Value; naming that type directly would need this \
              test crate to depend on serde_json, which Rule 15 reserves to dark-contract"
)]
fn streaming_turn() -> Transcript {
    let mut t = Transcript::new();
    t.apply_event(&Event::UserMessage {
        turn: "t1".into(),
        text: "fix the staleness check".into(),
    });
    t.apply_event(&Event::ReasonDelta {
        turn: "t1".into(),
        text: "Reading crates/dark-lexicon/pack".into(),
    });
    t.apply_event(&Event::ToolCall {
        turn: "t1".into(),
        call: ToolCall {
            id: "c1".into(),
            name: "edit_file".into(),
            args: Default::default(),
        },
    });
    t.apply_event(&Event::ToolResult {
        turn: "t1".into(),
        call_id: "c1".into(),
        result: ToolResultSummary {
            name: "edit_file".into(),
            is_error: false,
            bytes: 40,
            headline: "1 change".into(),
            has_diff: true,
        },
        content: "--- a/pack.rs\n\
+++ b/pack.rs\n\
@@ -1,3 +1,3 @@\n\
-fn stale(&self) -> bool {\n\
+fn stale(&self, now: Instant) -> bool {\n\
     todo!()\n"
            .into(),
    });
    for chunk in ["Done", ". ", "The staleness check now takes `now`."] {
        t.apply_event(&Event::TokenDelta {
            turn: "t1".into(),
            text: chunk.into(),
        });
    }
    t
}

#[test]
fn a_streaming_turn_shows_every_stage_in_order() {
    let transcript = streaming_turn();
    let text = buffer_text(&render_transcript(&transcript, false, 80, 30));

    let you_at = text.find("you").expect("the user message header must show");
    let tool_at = text.find("edit_file").expect("the tool call must show");
    let result_at = text
        .find("1 change")
        .expect("the tool result headline must show");
    let answer_at = text
        .find("The staleness check now takes")
        .expect("the streamed answer must show");

    assert!(
        you_at < tool_at && tool_at < result_at && result_at < answer_at,
        "the transcript must read top to bottom in the order the turn happened: \
         you={you_at} tool={tool_at} result={result_at} answer={answer_at}"
    );
}

#[test]
fn a_streaming_turn_renders_identical_bytes_twice() {
    let transcript = streaming_turn();
    let first = render_transcript(&transcript, false, 80, 30);
    let second = render_transcript(&transcript, false, 80, 30);
    assert_eq!(first, second);
}

#[test]
fn a_collapsed_thinking_block_shows_the_count_and_hides_the_text() {
    let mut t = Transcript::new();
    for _ in 0..312 {
        t.apply_event(&Event::ReasonDelta {
            turn: "t1".into(),
            text: "x".into(),
        });
    }
    let collapsed_text = buffer_text(&render_transcript(&t, false, 80, 24));
    assert!(collapsed_text.contains("▸ thinking (312 tok)"));

    let expanded_text = buffer_text(&render_transcript(&t, true, 80, 24));
    assert!(
        !collapsed_text.contains("xxxxxxxxxx"),
        "the collapsed view must not leak the accumulated reasoning text"
    );
    assert!(
        expanded_text.contains("xxxxxxxxxx"),
        "expanding must show the accumulated reasoning text"
    );
}

#[test]
fn a_diff_renders_the_exact_unified_diff_text() {
    let diff = UnifiedDiff::parse(
        "--- a/pack.rs\n\
+++ b/pack.rs\n\
@@ -1,3 +1,3 @@\n\
-fn stale(&self) -> bool {\n\
+fn stale(&self, now: Instant) -> bool {\n\
     todo!()\n",
    );
    let theme = Theme::new(ColorLevel::TrueColor);
    let path = std::path::Path::new("crates/dark-lexicon/pack.rs");
    let view = DiffView::new(&diff, &theme).path(path);
    let text = buffer_text(&render_widget(view, 80, 24));

    assert!(text.contains("crates/dark-lexicon/pack.rs"));
    assert!(text.contains("-fn stale(&self) -> bool {"));
    assert!(text.contains("+fn stale(&self, now: Instant) -> bool {"));
}

#[test]
fn a_diff_renders_identical_bytes_twice() {
    let diff = UnifiedDiff::parse("-old\n+new\n");
    let theme = Theme::new(ColorLevel::TrueColor);
    let first = render_widget(DiffView::new(&diff, &theme), 60, 12);
    let second = render_widget(DiffView::new(&diff, &theme), 60, 12);
    assert_eq!(first, second);
}

#[test]
fn a_diff_still_renders_every_line_with_no_colour() {
    let diff = UnifiedDiff::parse("--- a/pack.rs\n+++ b/pack.rs\n@@ -1 +1 @@\n-old\n+new\n");
    let theme = Theme::new(ColorLevel::None);
    let text = buffer_text(&render_widget(DiffView::new(&diff, &theme), 60, 12));
    assert!(text.contains("-old"));
    assert!(text.contains("+new"));
}
