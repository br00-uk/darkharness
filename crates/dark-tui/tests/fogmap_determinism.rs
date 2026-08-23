//! Determinism tests for the fog map (task unit `H3`).
//!
//! "Done when: The same map produces identical bytes twice." This drives
//! [`dark_tui::views::fogmap`]'s public API exactly as a caller outside the
//! crate would, from a snapshot through to a rendered frame, and checks
//! that nothing along the way — the layout, the ring relaxation, the
//! shimmer, the pulse — reads a clock or any other source that could change
//! between two runs over the same input. [`fogmap::compute_layout`]'s own
//! unit tests (`crates/dark-tui/src/views/fogmap.rs`) cover the layout
//! algorithm's properties in more detail; this file is the end-to-end
//! check `cargo nextest run -p dark-tui --test fogmap_determinism` names.

use dark_tui::anim::DetailLevel;
use dark_tui::theme::{ColorLevel, Theme, TicketState};
use dark_tui::views::fogmap::{FogMap, FogMapData, Ticket, compute_layout};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

fn ticket(id: &str, state: TicketState, blocked_by: &[&str]) -> Ticket {
    Ticket {
        id: id.to_owned(),
        name: format!("{id} name"),
        state,
        blocked_by: blocked_by.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// A map with a mix of states, at least one ticket on every ring: the
/// destination, a frontier ticket, a claimed one (which pulses), a blocked
/// one, an unspecified one, and one outside the map's scope.
fn sample_data() -> FogMapData {
    FogMapData {
        destination: "T-100".to_owned(),
        tickets: vec![
            ticket("T-100", TicketState::Resolved, &["T-018", "T-019"]),
            ticket("T-018", TicketState::Frontier, &["T-021"]),
            ticket("T-019", TicketState::Claimed, &["T-021", "T-022"]),
            ticket("T-021", TicketState::Blocked, &[]),
            ticket("T-022", TicketState::Blocked, &[]),
            ticket("T-fog", TicketState::Fog, &[]),
            ticket("T-scope", TicketState::OutOfScope, &[]),
        ],
    }
}

fn render(data: &FogMapData, theme: &Theme, phase: f32, shimmer_time: f32) -> Buffer {
    let layout = compute_layout(data);
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
    let widget = FogMap::new(&layout, theme)
        .phase(phase)
        .shimmer_time(shimmer_time)
        .detail(DetailLevel::Full);
    terminal
        .draw(|frame| frame.render_widget(widget, frame.area()))
        .expect("render must not fail against a TestBackend");
    terminal.backend().buffer().clone()
}

#[test]
fn the_same_map_produces_identical_bytes_twice() {
    let data = sample_data();
    let theme = Theme::new(ColorLevel::TrueColor);
    let first = render(&data, &theme, 0.4, 12.75);
    let second = render(&data, &theme, 0.4, 12.75);
    assert_eq!(first, second, "identical input must render identical bytes");
}

#[test]
fn the_same_map_produces_identical_bytes_across_many_repeated_computations() {
    // Guards against any hidden non-determinism that a single repeat could
    // miss — an unordered hash-map iteration, for instance, which does not
    // always disagree with itself on the very next call.
    let data = sample_data();
    let theme = Theme::new(ColorLevel::TrueColor);
    let baseline = render(&data, &theme, 0.0, 0.0);
    for _ in 0..25 {
        assert_eq!(render(&data, &theme, 0.0, 0.0), baseline);
    }
}

#[test]
fn layout_alone_is_deterministic_independent_of_rendering() {
    let data = sample_data();
    let a = compute_layout(&data);
    let b = compute_layout(&data);
    assert_eq!(a, b);
}

#[test]
fn determinism_holds_at_every_colour_level() {
    let data = sample_data();
    for level in [
        ColorLevel::TrueColor,
        ColorLevel::Ansi256,
        ColorLevel::Ansi16,
        ColorLevel::None,
    ] {
        let theme = Theme::new(level);
        let first = render(&data, &theme, 0.5, 3.3);
        let second = render(&data, &theme, 0.5, 3.3);
        assert_eq!(first, second, "rendering at {level:?} must be stable");
    }
}

#[test]
fn determinism_holds_across_every_detail_level() {
    let data = sample_data();
    let theme = Theme::new(ColorLevel::TrueColor);
    let layout = compute_layout(&data);
    for detail in [
        DetailLevel::Full,
        DetailLevel::NoShimmer,
        DetailLevel::LayoutOnly,
    ] {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        let widget = FogMap::new(&layout, &theme)
            .detail(detail)
            .shimmer_time(5.0);
        terminal
            .draw(|frame| frame.render_widget(widget, frame.area()))
            .expect("first render must not fail");
        let first = terminal.backend().buffer().clone();

        let widget = FogMap::new(&layout, &theme)
            .detail(detail)
            .shimmer_time(5.0);
        terminal
            .draw(|frame| frame.render_widget(widget, frame.area()))
            .expect("second render must not fail");
        let second = terminal.backend().buffer().clone();

        assert_eq!(first, second, "detail level {detail:?} must render stably");
    }
}

#[test]
fn a_frozen_phase_and_shimmer_time_never_read_the_wall_clock() {
    // If either read a real clock instead of the parameter it was given,
    // two renders separated by real elapsed time would disagree.
    let data = sample_data();
    let theme = Theme::new(ColorLevel::TrueColor);
    let before = render(&data, &theme, 0.6, 9.0);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let after = render(&data, &theme, 0.6, 9.0);
    assert_eq!(
        before, after,
        "elapsed wall-clock time must not change a pinned frame"
    );
}

#[test]
fn resizing_and_rendering_the_map_never_panics() {
    let data = sample_data();
    let theme = Theme::new(ColorLevel::TrueColor);
    let layout = compute_layout(&data);
    for (width, height) in [(1, 1), (10, 5), (40, 10), (80, 24), (200, 60), (300, 90)] {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        terminal
            .draw(|frame| {
                let widget = FogMap::new(&layout, &theme);
                frame.render_widget(widget, frame.area());
            })
            .unwrap_or_else(|err| panic!("render at {width}x{height} failed: {err}"));
    }
}
