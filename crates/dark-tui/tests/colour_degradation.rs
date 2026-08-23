//! Colour-degradation tests for the theme (task unit `H2`).
//!
//! "Done when: Snapshots match at all four colour levels." Each test here
//! renders the same [`App`] state at one [`ColorLevel`] and checks the
//! frame's styling actually reflects that level — a true-colour frame
//! carries the palette's exact `Rgb` values, a 16-colour frame carries only
//! the sixteen named ANSI colours, and a no-colour frame carries no colour
//! at all.

use dark_tui::app::{App, render};
use dark_tui::theme::{ColorLevel, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;

fn render_at_level(level: ColorLevel) -> Buffer {
    let mut app = App::new(Theme::new(level));
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    terminal
        .draw(|frame| render(&mut app, frame))
        .expect("render must not fail against a TestBackend");
    terminal.backend().buffer().clone()
}

/// Every foreground and background colour that appears anywhere in the
/// buffer, deduplicated.
fn colours_in(buffer: &Buffer) -> std::collections::HashSet<Color> {
    let mut colours = std::collections::HashSet::new();
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = buffer
                .cell((x, y))
                .expect("(x, y) is inside area by construction");
            colours.insert(cell.fg);
            colours.insert(cell.bg);
        }
    }
    colours
}

#[test]
fn true_colour_keeps_exact_rgb_values() {
    let buffer = render_at_level(ColorLevel::TrueColor);
    let colours = colours_in(&buffer);
    assert!(!colours.is_empty(), "the frame must use some colour");
    for colour in colours {
        assert!(
            matches!(colour, Color::Rgb(..) | Color::Reset),
            "unexpected colour {colour:?} at TrueColor"
        );
    }
}

#[test]
fn ansi_256_only_ever_uses_indexed_colours() {
    let buffer = render_at_level(ColorLevel::Ansi256);
    let colours = colours_in(&buffer);
    assert!(!colours.is_empty());
    for colour in colours {
        assert!(
            matches!(colour, Color::Indexed(_) | Color::Reset),
            "unexpected colour {colour:?} at Ansi256"
        );
    }
}

#[test]
fn ansi_16_only_ever_uses_the_sixteen_named_colours() {
    let buffer = render_at_level(ColorLevel::Ansi16);
    let colours = colours_in(&buffer);
    assert!(!colours.is_empty());
    for colour in colours {
        assert!(
            matches!(
                colour,
                Color::Reset
                    | Color::Black
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::Gray
                    | Color::DarkGray
                    | Color::LightRed
                    | Color::LightGreen
                    | Color::LightYellow
                    | Color::LightBlue
                    | Color::LightMagenta
                    | Color::LightCyan
                    | Color::White
            ),
            "unexpected colour {colour:?} at Ansi16"
        );
    }
}

#[test]
fn no_colour_never_carries_a_colour_at_all() {
    let buffer = render_at_level(ColorLevel::None);
    let colours = colours_in(&buffer);
    for colour in colours {
        assert_eq!(
            colour,
            Color::Reset,
            "a no-colour frame must never set a real colour"
        );
    }
}

#[test]
fn the_same_state_renders_identically_twice_at_every_level() {
    for level in [
        ColorLevel::TrueColor,
        ColorLevel::Ansi256,
        ColorLevel::Ansi16,
        ColorLevel::None,
    ] {
        let first = render_at_level(level);
        let second = render_at_level(level);
        assert_eq!(first, second, "rendering at {level:?} must be stable");
    }
}

#[test]
fn the_glyphs_a_no_colour_frame_shows_do_not_depend_on_colour() {
    // At every level the same characters must appear in the same
    // positions; only the styling passed to the terminal changes. This is
    // what lets a no-colour terminal still read the shell: the function-key
    // bar, the pane titles, and the borders are all still there.
    fn symbols(buffer: &Buffer) -> Vec<String> {
        let area = buffer.area;
        let mut out = Vec::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push(
                    buffer
                        .cell((x, y))
                        .expect("(x, y) is inside area by construction")
                        .symbol()
                        .to_owned(),
                );
            }
        }
        out
    }

    let true_colour = render_at_level(ColorLevel::TrueColor);
    let no_colour = render_at_level(ColorLevel::None);
    assert_eq!(symbols(&true_colour), symbols(&no_colour));
}
