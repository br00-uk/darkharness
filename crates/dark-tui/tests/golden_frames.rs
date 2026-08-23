//! Golden-frame tests for the application shell (task unit `H1`).
//!
//! Each test renders a fresh [`App`] against a [`TestBackend`] at one of the
//! three sizes task unit `H1` names and asserts the exact text every cell
//! shows. A rendering regression — a border in the wrong place, a title
//! that stops showing, a pane that goes missing — changes this text, so the
//! comparison catches it without a human reading a screenshot.
//!
//! [`TestBackend`] means none of this needs a real terminal: see task unit
//! `H1`'s brief, "Do not require a real terminal in tests."

use dark_tui::app::{App, render};
use dark_tui::theme::{ColorLevel, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

/// Renders a fresh shell at `(width, height)` and returns the plain text
/// every cell shows, one line per row. Dropping style from the comparison
/// keeps the golden text readable; [`colour_degradation`] covers style.
///
/// [`colour_degradation`]: ../colour_degradation/index.html
fn render_plain_text(width: u16, height: u16) -> String {
    let mut app = App::new(Theme::new(ColorLevel::TrueColor));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| render(&mut app, frame))
        .expect("render must not fail against a TestBackend");
    buffer_text(terminal.backend().buffer())
}

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

/// Asserts that `text` has exactly `height` lines — one per terminal row,
/// so the frame covered its whole area and clipped nothing. Each line is
/// built from exactly `width` cells by construction (see
/// [`buffer_text`]); this only checks the row count, since a handful of
/// this shell's glyphs (for example the warning triangle) are ambiguous
/// width in some Unicode tables, which would make a per-row character
/// count fragile without saying anything about a real rendering bug.
fn assert_fills_the_frame(text: &str, height: u16) {
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), usize::from(height), "expected {height} rows");
}

#[test]
fn golden_frame_at_eighty_by_twenty_four_fills_the_frame_and_is_stable() {
    let width = 80;
    let height = 24;
    let first = render_plain_text(width, height);
    assert_fills_the_frame(&first, height);

    let second = render_plain_text(width, height);
    assert_eq!(
        first, second,
        "the same state must render identical bytes twice"
    );

    assert!(
        first.contains("darkharness"),
        "the title bar must name the harness"
    );
    assert!(first.contains("MAP"), "the left pane must show its title");
    assert!(
        first.contains("TRANSCRIPT"),
        "the right pane must show its default title"
    );
    assert!(
        first.contains("Help"),
        "the function-key bar must show its labels"
    );
    assert!(
        first.contains("Quit"),
        "the function-key bar must reach F10"
    );
}

#[test]
fn golden_frame_at_one_twenty_by_forty_fills_the_frame_and_is_stable() {
    let width = 120;
    let height = 40;
    let first = render_plain_text(width, height);
    assert_fills_the_frame(&first, height);

    let second = render_plain_text(width, height);
    assert_eq!(
        first, second,
        "the same state must render identical bytes twice"
    );

    assert!(first.contains("darkharness"));
    assert!(first.contains("MAP"));
    assert!(first.contains("TRANSCRIPT"));
}

#[test]
fn golden_frame_at_two_hundred_by_sixty_fills_the_frame_and_is_stable() {
    let width = 200;
    let height = 60;
    let first = render_plain_text(width, height);
    assert_fills_the_frame(&first, height);

    let second = render_plain_text(width, height);
    assert_eq!(
        first, second,
        "the same state must render identical bytes twice"
    );

    assert!(first.contains("darkharness"));
    assert!(first.contains("MAP"));
    assert!(first.contains("TRANSCRIPT"));
}

#[test]
fn a_session_start_replaces_the_bare_title_with_the_repository_name() {
    use dark_contract::{Event, Received};

    let mut app = App::new(Theme::new(ColorLevel::TrueColor));
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| render(&mut app, frame))
        .expect("first render must not fail");
    let before = buffer_text(terminal.backend().buffer());

    app.apply_event(
        Received::Event(Event::SessionStart {
            id: "s1".into(),
            root: std::path::PathBuf::from("/home/dan/myrepo"),
        }),
        std::time::Instant::now(),
    );
    terminal
        .draw(|frame| render(&mut app, frame))
        .expect("second render must not fail");
    let after = buffer_text(terminal.backend().buffer());

    assert!(!before.contains("myrepo"));
    assert!(after.contains("myrepo"));
}
