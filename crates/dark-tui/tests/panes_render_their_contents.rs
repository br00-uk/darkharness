//! The panes must draw their contents, not only their borders.
//!
//! `dark-tui`'s three views — [`dark_tui::views::transcript`],
//! [`dark_tui::views::diff`], and [`dark_tui::views::fogmap`] — each had
//! thorough unit tests while nothing in [`dark_tui::app`] ever called them,
//! so the shell rendered a header, two empty bordered boxes, and a
//! function-key bar for every session. Unit tests on a view cannot catch
//! that: the gap was between the view and the application, and only a test
//! that drives [`dark_tui::app::App`] itself crosses it.
//!
//! So these tests assert against the frame the application actually draws.

use dark_contract::{
    ConfirmPrompt, Event, Received, RoleClass, ToolCall, ToolResultSummary, Usage,
};
use dark_tui::app::App;
use dark_tui::app::render::render;
use dark_tui::theme::{ColorLevel, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::time::Instant;

/// Renders `app` at `width` by `height` and returns the frame as plain
/// text, one string per row.
fn frame_text(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("a TestBackend always builds");
    terminal
        .draw(|frame| render(app, frame))
        .expect("render must not fail against a TestBackend");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect()
}

/// Returns the whole frame as one string, for a `contains` assertion.
fn flat(app: &mut App, width: u16, height: u16) -> String {
    frame_text(app, width, height).join("\n")
}

fn app() -> App {
    App::new(Theme::new(ColorLevel::TrueColor))
}

fn feed(app: &mut App, event: Event) {
    app.apply_event(Received::Event(event), Instant::now());
}

fn turn_start(app: &mut App) {
    feed(
        app,
        Event::TurnStart {
            turn: "t1".to_owned(),
            class: RoleClass::Worker,
            model: "test-model".to_owned(),
        },
    );
}

#[test]
fn the_transcript_pane_shows_what_the_person_submitted() {
    let mut app = app();
    turn_start(&mut app);
    feed(
        &mut app,
        Event::UserMessage {
            turn: "t1".to_owned(),
            text: "find the policy gate".to_owned(),
        },
    );

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("find the policy gate"),
        "the submitted text must appear in the frame:\n{frame}"
    );
}

#[test]
fn the_transcript_pane_shows_the_model_output() {
    let mut app = app();
    turn_start(&mut app);
    for word in ["Policy", "::", "decide", " is", " the", " gate"] {
        feed(
            &mut app,
            Event::TokenDelta {
                turn: "t1".to_owned(),
                text: word.to_owned(),
            },
        );
    }

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("decide"),
        "streamed output must appear in the frame:\n{frame}"
    );
}

#[test]
fn the_transcript_pane_shows_a_tool_call_and_its_result() {
    let mut app = app();
    turn_start(&mut app);
    feed(
        &mut app,
        Event::ToolCall {
            turn: "t1".to_owned(),
            call: ToolCall {
                id: "c1".to_owned(),
                name: "grep".to_owned(),
                args: serde_json::json!({ "pattern": "fn decide" }),
            },
        },
    );
    feed(
        &mut app,
        Event::ToolResult {
            turn: "t1".to_owned(),
            call_id: "c1".to_owned(),
            result: ToolResultSummary {
                name: "grep".to_owned(),
                is_error: false,
                bytes: 42,
                headline: "1 match".to_owned(),
                has_diff: false,
            },
            content: "policy/mod.rs:118".to_owned(),
        },
    );

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("grep"),
        "the tool name must appear:\n{frame}"
    );
    assert!(
        frame.contains("1 match"),
        "the result headline must appear:\n{frame}"
    );
}

#[test]
fn a_finished_turn_stays_readable_in_the_pane() {
    let mut app = app();
    turn_start(&mut app);
    feed(
        &mut app,
        Event::UserMessage {
            turn: "t1".to_owned(),
            text: "the first question".to_owned(),
        },
    );
    feed(
        &mut app,
        Event::TurnEnd {
            turn: "t1".to_owned(),
            usage: Usage::default(),
            wall_ms: 10,
        },
    );

    assert!(
        flat(&mut app, 120, 30).contains("the first question"),
        "a turn that ended must stay on screen: the pane is the conversation, \
         not just the running turn"
    );
}

#[test]
fn an_empty_transcript_says_so_rather_than_drawing_a_blank_box() {
    let mut app = app();
    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("Nothing yet"),
        "an empty pane must explain itself:\n{frame}"
    );
}

#[test]
fn a_confirmation_shows_the_exact_diff_over_the_panes() {
    // Task unit `H4`, rule 8: "Show the exact diff or the exact command in
    // a confirmation modal. Never show a summary."
    let mut app = app();
    turn_start(&mut app);
    feed(
        &mut app,
        Event::ConfirmReq {
            id: "q1".to_owned(),
            prompt: ConfirmPrompt::Write {
                path: "src/policy.rs".into(),
                diff: "@@ -1 +1 @@\n-old line\n+new line\n".to_owned(),
            },
        },
    );

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("CONFIRM"),
        "the modal must be drawn:\n{frame}"
    );
    assert!(
        frame.contains("new line"),
        "the exact diff must be shown, not a summary:\n{frame}"
    );
}

#[test]
fn the_diff_pane_shows_the_diff_a_confirmation_carried() {
    let mut app = app();
    turn_start(&mut app);
    feed(
        &mut app,
        Event::ConfirmReq {
            id: "q1".to_owned(),
            prompt: ConfirmPrompt::Write {
                path: "src/policy.rs".into(),
                diff: "@@ -1 +1 @@\n-old line\n+added by the tool\n".to_owned(),
            },
        },
    );
    // Answering the confirmation leaves the diff available to read.
    let answered = app.answer_confirm(dark_contract::Allow::Once);
    assert!(answered.is_some(), "answering must produce an intent");
    assert!(
        !app.is_awaiting_confirm(),
        "the modal must close once answered"
    );
    app.set_right_pane(dark_tui::app::pane::RightPane::Diff);

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("added by the tool"),
        "the diff pane must show the diff:\n{frame}"
    );
}

#[test]
fn a_long_transcript_shows_its_newest_lines() {
    let mut app = app();
    turn_start(&mut app);
    for n in 0..200 {
        feed(
            &mut app,
            Event::TokenDelta {
                turn: "t1".to_owned(),
                text: format!("line {n}\n"),
            },
        );
    }

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("line 199"),
        "the newest output must be visible:\n{frame}"
    );
    assert!(
        !frame.contains("line 0\n"),
        "the oldest output must have scrolled off:\n{frame}"
    );
}

#[test]
fn scrolling_back_reaches_the_older_lines() {
    let mut app = app();
    turn_start(&mut app);
    for n in 0..200 {
        feed(
            &mut app,
            Event::TokenDelta {
                turn: "t1".to_owned(),
                text: format!("line {n}\n"),
            },
        );
    }
    app.scroll_back(150);

    let frame = flat(&mut app, 120, 30);
    assert!(
        frame.contains("line 4"),
        "scrolling back must reach older output:\n{frame}"
    );

    app.scroll_to_tail();
    assert!(flat(&mut app, 120, 30).contains("line 199"));
}

#[test]
fn every_pane_draws_something_inside_its_border() {
    // Whatever a pane is set to, it must never render as an empty box: a
    // person cannot tell "nothing to show" from "not built" that way, and
    // that indistinguishability is exactly what hid the missing wiring.
    use dark_tui::app::pane::{LeftPane, RightPane};

    for right in [
        RightPane::Transcript,
        RightPane::Diff,
        RightPane::Doc,
        RightPane::Explore,
    ] {
        let mut app = app();
        app.set_right_pane(right);
        let rows = frame_text(&mut app, 120, 30);
        // Row 2 is the first row inside the pane borders.
        let inside: String = rows[2..rows.len() - 3].join("");
        assert!(
            inside.chars().any(char::is_alphabetic),
            "{right:?} drew nothing inside its border"
        );
    }

    for left in [
        LeftPane::Map,
        LeftPane::Files,
        LeftPane::Seams,
        LeftPane::Packs,
    ] {
        let mut app = app();
        app.set_left_pane(left);
        let rows = frame_text(&mut app, 120, 30);
        let inside: String = rows[2..rows.len() - 3].join("");
        assert!(
            inside.chars().any(char::is_alphabetic),
            "{left:?} drew nothing inside its border"
        );
    }
}

#[test]
fn the_map_pane_draws_a_layout_once_one_is_set() {
    // The fog map was the last view nothing reached: complete, tested in
    // isolation, and never handed a `Layout`. `App` cannot build one —
    // Rule 14 keeps this crate on `dark-contract` — so the composition
    // root computes it and sets it, and this is the seam that proves the
    // set arrives on screen.
    use dark_tui::theme::TicketState;
    use dark_tui::views::fogmap::{FogMapData, Ticket, compute_layout};

    let data = FogMapData {
        destination: "T3".to_owned(),
        tickets: vec![
            Ticket {
                id: "T1".to_owned(),
                name: "read the seams".to_owned(),
                state: TicketState::Frontier,
                blocked_by: vec![],
            },
            Ticket {
                id: "T2".to_owned(),
                name: "write the tool".to_owned(),
                state: TicketState::Claimed,
                blocked_by: vec!["T1".to_owned()],
            },
            Ticket {
                id: "T3".to_owned(),
                name: "ship it".to_owned(),
                state: TicketState::Blocked,
                blocked_by: vec!["T2".to_owned()],
            },
        ],
    };

    let mut app = app();
    let before = flat(&mut app, 120, 30);
    assert!(
        before.contains("No map loaded"),
        "with no map the pane must say so:\n{before}"
    );

    app.set_map(compute_layout(&data));
    let after = flat(&mut app, 120, 30);
    assert!(
        !after.contains("No map loaded"),
        "the placeholder must give way to the map:\n{after}"
    );
    assert!(
        after.contains("ship it") || after.contains("T3"),
        "the map must name what it draws:\n{after}"
    );
}

#[test]
fn a_map_changed_event_asks_for_the_map_it_names() {
    // `dark-tui` cannot open a map store, so it records the request and
    // the shell's loop hands it to a loader the composition root supplies.
    let mut app = app();
    assert!(app.take_map_request().is_none());

    feed(
        &mut app,
        Event::MapChanged {
            map_id: "01J0MAP".to_owned(),
        },
    );

    assert_eq!(app.take_map_request().as_deref(), Some("01J0MAP"));
    assert!(
        app.take_map_request().is_none(),
        "one MapChanged must cost one load, not one per redraw"
    );
}

#[test]
fn a_selection_that_the_new_map_does_not_hold_is_dropped() {
    use dark_tui::theme::TicketState;
    use dark_tui::views::fogmap::{FogMapData, Ticket, compute_layout};

    let one = |id: &str| FogMapData {
        destination: id.to_owned(),
        tickets: vec![Ticket {
            id: id.to_owned(),
            name: id.to_owned(),
            state: TicketState::Frontier,
            blocked_by: vec![],
        }],
    };

    let mut app = app();
    app.set_map(compute_layout(&one("T1")));
    app.map_state_mut().select("T1");
    assert_eq!(app.map_state().selected(), Some("T1"));

    // A map that no longer holds T1 must not keep it highlighted.
    app.set_map(compute_layout(&one("T9")));
    assert_eq!(app.map_state().selected(), None);
}
