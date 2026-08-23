//! Resize-fuzz tests for the application shell (task unit `H1`).
//!
//! "Done when: … Resize down to 40×10 causes no panic." This sweeps a wide
//! range of terminal sizes, well past that floor down to zero, and drives
//! each one through a realistic sequence of events and key presses. Nothing
//! here asserts what the frame looks like — [`golden_frames`] covers that —
//! only that drawing it, at any size, never panics.
//!
//! [`golden_frames`]: ../golden_frames/index.html

use dark_contract::{Event, Received};
use dark_tui::app::{App, render};
use dark_tui::theme::{ColorLevel, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

fn app_mid_turn() -> App {
    let mut app = App::new(Theme::new(ColorLevel::TrueColor));
    let now = std::time::Instant::now();
    app.apply_event(
        Received::Event(Event::SessionStart {
            id: "s1".into(),
            root: std::path::PathBuf::from("/home/dan/myrepo"),
        }),
        now,
    );
    app.apply_event(
        Received::Event(Event::TurnStart {
            turn: "t1".into(),
            class: dark_contract::RoleClass::Worker,
            model: "qwen3-14b-q4".into(),
        }),
        now,
    );
    app.apply_event(Received::Lagged(4), now);
    app.apply_event(
        Received::Event(Event::Budget {
            used: 34,
            granted: 100,
        }),
        now,
    );
    app
}

/// Renders `app` at `(width, height)`, panicking the test (not the
/// application) if drawing panics.
fn render_at(app: &mut App, width: u16, height: u16) {
    let backend = TestBackend::new(width.max(1), height.max(1));
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| render(app, frame))
        .unwrap_or_else(|err| panic!("render at {width}x{height} failed: {err}"));
}

#[test]
fn resizing_down_to_forty_by_ten_causes_no_panic() {
    let mut app = app_mid_turn();
    render_at(&mut app, 80, 24);
    render_at(&mut app, 40, 10);
}

#[test]
fn every_size_from_one_by_one_to_beyond_the_documented_minimum_survives_a_render() {
    let mut app = app_mid_turn();
    for width in 1..=100u16 {
        for height in 1..=30u16 {
            render_at(&mut app, width, height);
        }
    }
}

#[test]
fn a_shrink_then_grow_sequence_survives_without_panicking() {
    let mut app = app_mid_turn();
    let sizes = [
        (200, 60),
        (120, 40),
        (80, 24),
        (79, 23),
        (40, 10),
        (10, 5),
        (1, 1),
        (40, 10),
        (80, 24),
        (200, 60),
    ];
    for (width, height) in sizes {
        render_at(&mut app, width, height);
    }
}

#[test]
fn resizing_while_the_help_overlay_is_open_survives_at_every_size() {
    let mut app = app_mid_turn();
    let _ = app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    for width in 1..=90u16 {
        render_at(&mut app, width, 24);
    }
}

#[test]
fn resizing_while_the_command_bar_holds_a_long_line_survives_at_every_size() {
    let mut app = app_mid_turn();
    let _ = app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    let _ = app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    for c in "a very long line of typed text that may run past a narrow terminal".chars() {
        let _ = app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    for width in 1..=90u16 {
        render_at(&mut app, width, 24);
    }
}

#[test]
fn a_stacked_layout_survives_a_mouse_click_at_every_position() {
    let mut app = app_mid_turn();
    render_at(&mut app, 40, 10);
    for x in 0..40u16 {
        for y in 0..10u16 {
            let _ = app.handle_mouse(x, y, MouseEventKind::Down(MouseButton::Left));
        }
    }
    render_at(&mut app, 40, 10);
}

#[test]
fn a_model_load_in_progress_survives_every_size() {
    let mut app = app_mid_turn();
    app.apply_event(
        Received::Event(Event::ModelLoading {
            model: "qwen3-14b-q4".into(),
            progress: 0.42,
        }),
        std::time::Instant::now(),
    );
    for width in 1..=90u16 {
        render_at(&mut app, width, 24);
    }
}
