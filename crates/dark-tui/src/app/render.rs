//! Draws an [`App`] to a [`Frame`].

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph};

use crate::app::layout::{self, AppLayout};
use crate::app::pane::Focus;
use crate::app::state::{App, Header};
use crate::app::zone::ZoneId;
use crate::theme::Theme;

/// The function-key bar, in order. See the key table in task unit `H1`.
const FUNCTION_KEYS: [(u8, &str); 10] = [
    (1, "Help"),
    (2, "Map"),
    (3, "View"),
    (4, "Diff"),
    (5, "Explore"),
    (6, "Lexicon"),
    (7, "Ticket"),
    (8, "Resolve"),
    (9, "Menu"),
    (10, "Quit"),
];

/// Draws the whole shell into `frame`, and rebuilds the mouse zone
/// registry to match.
///
/// Reads and writes [`App`]: it records the frame size
/// ([`App::set_size`](crate::app::state::App::set_size), which
/// [`App::should_stack_panes`](crate::app::state::App::should_stack_panes)
/// then uses) and clears and repopulates the zone registry so a mouse click
/// against this frame hit-tests correctly.
pub fn render(app: &mut App, frame: &mut Frame<'_>) {
    let size = frame.area();
    app.set_size(size.width, size.height);
    app.zones_mut().clear();

    let theme = *app.theme();
    let stacked = app.should_stack_panes();
    let computed = layout::compute(size, stacked);

    frame.render_widget(Block::new().style(theme.background()), size);

    if computed.outer.width == 0 || computed.outer.height == 0 {
        return;
    }

    render_outer(frame, app, &theme, computed.outer);
    render_pane(
        frame,
        computed.left_pane,
        app.left_pane().title(),
        app.focus() == Focus::Left,
        &theme,
        None,
    );
    render_pane(
        frame,
        computed.right_pane,
        app.right_pane().title(),
        app.focus() == Focus::Right,
        &theme,
        dropped_output_glyph(app),
    );
    render_command_bar(frame, app, &theme, &computed);
    render_function_keys(frame, computed.function_keys, &theme);

    app.zones_mut()
        .register(header_rect(computed.outer), ZoneId::Header);
    app.zones_mut()
        .register(computed.left_pane, ZoneId::LeftPane);
    app.zones_mut()
        .register(computed.right_pane, ZoneId::RightPane);
    app.zones_mut()
        .register(computed.command_bar, ZoneId::CommandBar);
    register_function_key_zones(app, computed.function_keys);

    if app.is_help_visible() {
        render_help_overlay(frame, &theme, size);
    } else if app.is_menu_visible() {
        render_menu_overlay(frame, &theme, size);
    }
}

/// The outer border and its title: the session, the resident model, the
/// context budget, and the measured rate. See task unit `H1`'s mock-up.
fn render_outer(frame: &mut Frame<'_>, app: &App, theme: &Theme, area: Rect) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.unfocused_border())
        .title(Line::from(header_spans(app, theme)))
        .style(theme.panel());
    frame.render_widget(block, area);
}

/// Builds the header title line: `darkharness ─ myrepo ─ ◆ LOCAL model ─
/// ctx N% ─ N tok/s`.
///
/// The event contract carries no git branch (see the `H1`/`H2` report), so,
/// unlike the mock-up in `PRD.md`, this omits a branch segment rather than
/// show one the harness never sent.
fn header_spans(app: &App, theme: &Theme) -> Vec<Span<'static>> {
    let header: &Header = app.header();
    let text_style = theme.style(theme.palette().text);
    let dim_style = theme.text_dim();

    let mut spans = vec![Span::styled(" darkharness ", text_style)];
    if let Some(name) = header.repo_name() {
        spans.push(Span::styled("─ ", dim_style));
        spans.push(Span::styled(name, text_style));
        spans.push(Span::raw(" "));
    }

    if let Some((model, progress)) = &header.loading {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "progress is a fraction 0.0..=1.0; the percentage always fits u16"
        )]
        let percent = (progress.clamp(0.0, 1.0) * 100.0).round() as u16;
        spans.push(Span::styled("─ ", dim_style));
        spans.push(Span::styled(
            format!("◆ LOADING {model} {percent}% "),
            theme.model_loading_style(*progress),
        ));
    } else if let Some(model) = current_model(header) {
        spans.push(Span::styled("─ ", dim_style));
        spans.push(Span::styled(format!("◆ LOCAL {model} "), theme.ok()));
    }

    if header.ctx_granted > 0 {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "ctx_fraction is a fraction 0.0..=1.0; the percentage always fits u16"
        )]
        let percent = (header.ctx_fraction() * 100.0).round() as u16;
        spans.push(Span::styled("─ ", dim_style));
        spans.push(Span::styled(format!("ctx {percent}% "), text_style));
    }

    let rate = app.tokens_per_sec();
    if rate > 0.0 {
        spans.push(Span::styled("─ ", dim_style));
        spans.push(Span::styled(format!("{rate:.0} tok/s "), text_style));
    }

    spans
}

/// Returns the model identifier the resident set reports for the first
/// loaded model, if any is loaded.
fn current_model(header: &Header) -> Option<&str> {
    header
        .resident
        .models
        .iter()
        .find(|model| matches!(model.state, dark_contract::SlotState::Loaded))
        .map(|model| model.model_id.as_str())
}

/// Draws one bordered pane. `suffix`, when given, appends to the title —
/// used for the dropped-output warning glyph.
fn render_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &str,
    focused: bool,
    theme: &Theme,
    suffix: Option<Span<'static>>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let border_style = if focused {
        theme.focused_border()
    } else {
        theme.unfocused_border()
    };
    let mut title_spans = vec![Span::styled(format!(" {title} "), border_style)];
    if let Some(suffix) = suffix {
        title_spans.push(suffix);
    }
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Line::from(title_spans))
        .style(theme.panel());
    frame.render_widget(block, area);
}

/// The glyph [`render_pane`] appends to the transcript pane's title while
/// the lossy channel has dropped output for the running turn. See task unit
/// `H1`'s brief: "it should show that output was dropped rather than
/// silently rendering a gap."
fn dropped_output_glyph(app: &App) -> Option<Span<'static>> {
    if !app.has_dropped_output() {
        return None;
    }
    Some(Span::styled(
        format!("⚠ {} dropped ", app.lag().dropped_this_turn),
        app.theme().warn(),
    ))
}

/// The command bar: a prompt glyph, the text typed so far, and — during a
/// model load — a progress bar in its place instead. See task unit `H1`:
/// "Show a progress bar during a model load."
fn render_command_bar(frame: &mut Frame<'_>, app: &App, theme: &Theme, computed: &AppLayout) {
    let area = computed.command_bar;
    if area.width == 0 || area.height == 0 {
        return;
    }

    if let Some((model, progress)) = &app.header().loading {
        let gauge = Gauge::default()
            .gauge_style(theme.model_loading_style(*progress))
            .ratio(f64::from(progress.clamp(0.0, 1.0)))
            .label(format!("loading {model}"));
        frame.render_widget(gauge, area);
        return;
    }

    let prompt = format!("⟩ {}", app.command_input());
    let style = if app.focus() == Focus::Command {
        theme.focused_border()
    } else {
        theme.style(theme.palette().text)
    };
    frame.render_widget(Paragraph::new(Line::styled(prompt, style)), area);
    if app.focus() == Focus::Command {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "a command line is never remotely close to u16::MAX characters"
        )]
        let cursor_x = area.x + 2 + app.command_input().chars().count() as u16;
        frame.set_cursor_position((cursor_x.min(area.x + area.width.saturating_sub(1)), area.y));
    }
}

/// The function-key bar: `1Help 2Map 3View …`, evenly spaced across the
/// width. See task unit `H1`'s mock-up.
fn render_function_keys(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut spans = Vec::with_capacity(FUNCTION_KEYS.len() * 2);
    for (number, label) in FUNCTION_KEYS {
        spans.push(Span::styled(
            number.to_string(),
            theme.style(theme.palette().disk_mid),
        ));
        spans.push(Span::styled(format!("{label} "), theme.text_dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Splits the function-key bar into ten equal zones and registers each one.
fn register_function_key_zones(app: &mut App, area: Rect) {
    if area.width == 0 {
        return;
    }
    let count = u16::try_from(FUNCTION_KEYS.len()).unwrap_or(1);
    let slot_width = area.width / count;
    if slot_width == 0 {
        return;
    }
    for (index, (number, _)) in FUNCTION_KEYS.iter().enumerate() {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "FUNCTION_KEYS has 10 entries; index never approaches u16::MAX"
        )]
        let offset = index as u16 * slot_width;
        let zone = Rect::new(area.x + offset, area.y, slot_width, 1);
        app.zones_mut().register(zone, ZoneId::FunctionKey(*number));
    }
}

/// The top border row, as its own zone.
fn header_rect(outer: Rect) -> Rect {
    Rect::new(outer.x, outer.y, outer.width, 1)
}

/// A small centred popup, cleared first so it draws over whatever is
/// beneath it.
fn popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

/// The `F1`/`?` help overlay, listing every binding in task unit `H1`'s key
/// table.
fn render_help_overlay(frame: &mut Frame<'_>, theme: &Theme, area: Rect) {
    const LINES: [&str; 12] = [
        "F1 help    F2 map     F3 view    F4 diff    F5 explore",
        "F6 lexicon F7 ticket  F8 resolve F9 menu    F10 quit",
        "Tab focus  Ctrl+←/→ pane mode  Ctrl+P palette  Ctrl+D dark toggle",
        "Esc cancel turn  Ctrl+C quit, twice during a turn",
        "t thinking  c claim  r resolve  f fog  / filter  ? keys",
        "",
        "",
        "",
        "",
        "",
        "",
        "Esc closes this help.",
    ];
    let popup = popup_area(area, 62, 14);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.focused_border())
        .title(Line::from(Span::styled(" HELP ", theme.focused_border())))
        .style(theme.panel());
    let text_style = theme.style(theme.palette().text);
    let lines: Vec<Line<'static>> = LINES
        .iter()
        .map(|line| Line::styled((*line).to_owned(), text_style))
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// The `F9`/`Ctrl+P` menu overlay.
fn render_menu_overlay(frame: &mut Frame<'_>, theme: &Theme, area: Rect) {
    let popup = popup_area(area, 40, 6);
    frame.render_widget(Clear, popup);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(theme.focused_border())
        .title(Line::from(Span::styled(" MENU ", theme.focused_border())))
        .style(theme.panel());
    let text_style = theme.style(theme.palette().text);
    let lines = vec![
        Line::styled("/plan    plan the next ticket", text_style),
        Line::styled("/golight allow the network", text_style),
        Line::styled("Esc      close this menu", text_style),
    ];
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorLevel;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        App::new(Theme::new(ColorLevel::TrueColor))
    }

    fn render_to(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        terminal
            .draw(|frame| render(app, frame))
            .expect("render must not fail against a TestBackend");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn rendering_at_eighty_by_twenty_four_does_not_panic_and_fills_the_frame() {
        let mut app = app();
        let buffer = render_to(&mut app, 80, 24);
        assert_eq!(buffer.area, Rect::new(0, 0, 80, 24));
    }

    #[test]
    fn rendering_at_one_twenty_by_forty_does_not_panic() {
        let mut app = app();
        render_to(&mut app, 120, 40);
    }

    #[test]
    fn rendering_at_two_hundred_by_sixty_does_not_panic() {
        let mut app = app();
        render_to(&mut app, 200, 60);
    }

    #[test]
    fn the_same_state_renders_identical_bytes_twice() {
        let mut app = app();
        let first = render_to(&mut app, 80, 24);
        let second = render_to(&mut app, 80, 24);
        assert_eq!(first, second);
    }

    #[test]
    fn resizing_down_to_forty_by_ten_does_not_panic() {
        let mut app = app();
        render_to(&mut app, 40, 10);
    }

    #[test]
    fn every_size_from_zero_to_the_documented_minimum_survives_a_render() {
        let mut app = app();
        for width in 0..=80u16 {
            for height in 0..=24u16 {
                if width == 0 || height == 0 {
                    // TestBackend itself refuses a zero dimension; the
                    // layout math for it is covered directly in
                    // `layout::tests`.
                    continue;
                }
                render_to(&mut app, width, height);
            }
        }
    }

    #[test]
    fn rendering_registers_a_zone_for_every_function_key() {
        let mut app = app();
        render_to(&mut app, 80, 24);

        let fkeys = layout::compute(Rect::new(0, 0, 80, 24), false).function_keys;
        let mut found = std::collections::BTreeSet::new();
        for x in fkeys.x..fkeys.x + fkeys.width {
            if let Some(ZoneId::FunctionKey(number)) = app.zones().hit_test(x, fkeys.y) {
                found.insert(number);
            }
        }
        assert_eq!(found, (1..=10u8).collect());
    }

    #[test]
    fn a_dropped_delta_shows_a_warning_glyph_instead_of_a_silent_gap() {
        use dark_contract::{Event, Received};
        let mut app = app();
        app.apply_event(
            Received::Event(Event::TurnStart {
                turn: "t1".into(),
                class: dark_contract::RoleClass::Worker,
                model: "m".into(),
            }),
            std::time::Instant::now(),
        );
        let clean = render_to(&mut app, 80, 24);

        app.apply_event(Received::Lagged(3), std::time::Instant::now());
        let with_warning = render_to(&mut app, 80, 24);

        assert_ne!(
            clean, with_warning,
            "a dropped delta must change what is on screen"
        );
    }

    #[test]
    fn the_help_overlay_only_shows_while_visible() {
        let mut app = app();
        let without = render_to(&mut app, 80, 24);
        let _ = app.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::F(1),
            ratatui::crossterm::event::KeyModifiers::NONE,
        ));
        let with_help = render_to(&mut app, 80, 24);
        assert_ne!(without, with_help);
    }
}
