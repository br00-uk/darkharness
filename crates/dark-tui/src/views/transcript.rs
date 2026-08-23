//! The transcript view: output as it arrives.
//!
//! [`Transcript`] folds the events one running turn produces into a small
//! ordered log of [`Segment`]s, and [`Transcript::render`] draws that log.
//! Nothing here reads a clock or owns a channel: [`Transcript::apply_event`]
//! is a plain fold, so a caller drives it exactly the way
//! [`crate::app::App::apply_event`] is driven, from whatever already polls
//! the event bus.
//!
//! # Coalescing token deltas
//!
//! Task unit `H4`, rule 2, asks to "coalesce token deltas on a 16-millisecond
//! tick" rather than redraw for each token. [`Transcript::apply_event`]
//! appends a [`dark_contract::Event::TokenDelta`] or
//! [`dark_contract::Event::ReasonDelta`] in amortised constant time and
//! draws nothing, so calling it many times between two redraws costs a few
//! string appends, not a few redraws. The coalescing this rule asks for
//! therefore falls out of the shell's existing redraw cadence —
//! [`crate::app::INPUT_POLL_INTERVAL`] already polls the terminal and the
//! event bus every 16 milliseconds — rather than needing a second timer
//! here. See task unit `H3`'s "do not spawn a thread or a timer of your
//! own," which applies equally to this view.
//!
//! # What this view cannot fully do
//!
//! Task unit `H4` names three renderers this crate has no dependency on:
//! `ansi-to-tui` for tool output, `tree-sitter-highlight` for code, and
//! `termimad` or `pulldown-cmark` for Markdown. `dark-tui`'s `Cargo.toml`
//! declares `dark-contract` and `ratatui` only, and this task unit does not
//! own that file. [`ansi_to_lines`], [`highlight_code_line`], and
//! [`render_markdown`] are hand-rolled, reduced substitutes: a small SGR
//! parser, a keyword-and-string heuristic, and a Markdown-lite renderer.
//! Each covers the common case well enough to read, and documents the gap
//! rather than silently claiming the full library's behaviour.

use dark_contract::Event;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};

use crate::theme::Theme;
use crate::views::diff::{UnifiedDiff, render_lines as render_diff_lines};

/// One piece of a turn's transcript, in the order it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// What the person submitted.
    User {
        /// The submitted text.
        text: String,
    },
    /// The model's visible output, accumulated from
    /// [`Event::TokenDelta`].
    Assistant {
        /// The text accumulated so far.
        text: String,
    },
    /// The model's thinking output, accumulated from
    /// [`Event::ReasonDelta`]. Collapsed by default; see
    /// [`Transcript::render`]'s `expanded_thinking` parameter.
    Reasoning {
        /// The text accumulated so far.
        text: String,
        /// How many `ReasonDelta` events have arrived for this segment —
        /// the live count task unit `H4` asks the collapsed line to show.
        token_count: usize,
    },
    /// The model asked for a tool call.
    ToolCall {
        /// The call identifier, matched against a later [`Segment::ToolResult`].
        id: String,
        /// The tool name.
        name: String,
        /// The arguments, formatted as compact JSON.
        args: String,
    },
    /// A line of progress from a running tool.
    ToolProgress {
        /// The call this progress line belongs to.
        call_id: String,
        /// The line of output.
        line: String,
    },
    /// A tool finished.
    ToolResult {
        /// The call this result answers.
        call_id: String,
        /// The tool name.
        name: String,
        /// Whether the call failed.
        is_error: bool,
        /// The one-line headline.
        headline: String,
        /// The full text the tool returned.
        content: String,
        /// Whether the tool produced a diff. When true, [`Transcript::render`]
        /// parses `content` as a unified diff and renders it with
        /// [`crate::views::diff`]'s styling rather than as plain text.
        has_diff: bool,
    },
    /// The lossy channel dropped output. See
    /// [`Transcript::record_lag`].
    LagWarning {
        /// How many events the channel dropped.
        dropped: u64,
    },
}

/// The running turn's transcript.
///
/// Builds empty with [`Transcript::new`]. [`Transcript::apply_event`] folds
/// in what the harness reports; a fresh [`Transcript`] at the start of each
/// turn keeps one turn's content from bleeding into the next, matching how
/// [`crate::app::state::App`] itself resets its own per-turn state on
/// [`Event::TurnStart`].
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    segments: Vec<Segment>,
    open_reasoning: bool,
    open_assistant: bool,
}

impl Transcript {
    /// Builds an empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the accumulated segments, in the order they happened.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Returns true when nothing has happened yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Folds in one event.
    ///
    /// An event this view has no use for — `TurnStart`, `TurnEnd`, and
    /// everything [`crate::app::state::App`] already handles on its own —
    /// is ignored rather than matched explicitly, so a future
    /// `dark-contract` addition to the `#[non_exhaustive]` `Event` enum
    /// needs no change here.
    pub fn apply_event(&mut self, event: &Event) {
        match event {
            Event::UserMessage { text, .. } => {
                self.segments.push(Segment::User { text: text.clone() });
                self.open_assistant = false;
                self.open_reasoning = false;
            }
            Event::TokenDelta { text, .. } => self.push_assistant_text(text),
            Event::ReasonDelta { text, .. } => self.push_reasoning_text(text),
            Event::ToolCall { call, .. } => {
                self.segments.push(Segment::ToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.to_string(),
                });
                self.open_assistant = false;
                self.open_reasoning = false;
            }
            Event::ToolProgress { call_id, line, .. } => {
                self.segments.push(Segment::ToolProgress {
                    call_id: call_id.clone(),
                    line: line.clone(),
                });
            }
            Event::ToolResult {
                call_id,
                result,
                content,
                ..
            } => {
                self.segments.push(Segment::ToolResult {
                    call_id: call_id.clone(),
                    name: result.name.clone(),
                    is_error: result.is_error,
                    headline: result.headline.clone(),
                    content: content.clone(),
                    has_diff: result.has_diff,
                });
            }
            _ => {}
        }
    }

    /// Records that the lossy channel dropped `dropped` events.
    ///
    /// See task unit `H4`, rule 3: "Show a warning glyph when the lossy
    /// channel reports a lag." [`crate::app::render`] already shows this in
    /// the pane's border title; this places a second, inline marker at the
    /// point in the transcript where the drop happened, so a person reading
    /// back through the turn sees exactly where the gap is, not only that
    /// one exists somewhere.
    pub fn record_lag(&mut self, dropped: u64) {
        self.segments.push(Segment::LagWarning { dropped });
        self.open_assistant = false;
        self.open_reasoning = false;
    }

    /// Appends to the open [`Segment::Assistant`], opening one first if none
    /// is open at the tail of the log.
    fn push_assistant_text(&mut self, text: &str) {
        if self.open_assistant {
            if let Some(Segment::Assistant { text: existing }) = self.segments.last_mut() {
                existing.push_str(text);
                return;
            }
        }
        self.segments.push(Segment::Assistant {
            text: text.to_owned(),
        });
        self.open_assistant = true;
        self.open_reasoning = false;
    }

    /// Appends to the open [`Segment::Reasoning`], opening one first if none
    /// is open at the tail of the log.
    fn push_reasoning_text(&mut self, text: &str) {
        if self.open_reasoning {
            if let Some(Segment::Reasoning {
                text: existing,
                token_count,
            }) = self.segments.last_mut()
            {
                existing.push_str(text);
                *token_count += 1;
                return;
            }
        }
        self.segments.push(Segment::Reasoning {
            text: text.to_owned(),
            token_count: 1,
        });
        self.open_reasoning = true;
        self.open_assistant = false;
    }
}

/// Converts a byte string that may contain ANSI SGR escape sequences into
/// styled lines.
///
/// See this module's "What this view cannot fully do": this crate has no
/// `ansi-to-tui` dependency, so this is a compact hand-rolled parser
/// covering `reset`, the text-style modifiers, the sixteen named colours,
/// and the 256-colour extended codes. An escape sequence this parser does
/// not recognise is consumed and dropped rather than shown as literal
/// bytes, so stray tool output never corrupts the display; the SGR state
/// carries across a line break, matching how a real terminal behaves.
#[must_use]
pub fn ansi_to_lines(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut style = Style::default();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' if chars.peek() == Some(&'[') => {
                chars.next(); // consume '['
                let mut params = String::new();
                let mut terminated = false;
                for pc in chars.by_ref() {
                    if pc == 'm' {
                        terminated = true;
                        break;
                    }
                    if pc.is_ascii_digit() || pc == ';' {
                        params.push(pc);
                    } else {
                        // An unrecognised final byte: stop collecting and
                        // drop the whole sequence rather than guess.
                        break;
                    }
                }
                if terminated {
                    if !buf.is_empty() {
                        current_line.push(Span::styled(std::mem::take(&mut buf), style));
                    }
                    apply_sgr(&mut style, &params);
                }
            }
            '\n' => {
                if !buf.is_empty() {
                    current_line.push(Span::styled(std::mem::take(&mut buf), style));
                }
                lines.push(Line::from(std::mem::take(&mut current_line)));
            }
            other => buf.push(other),
        }
    }
    if !buf.is_empty() {
        current_line.push(Span::styled(buf, style));
    }
    if !current_line.is_empty() {
        lines.push(Line::from(current_line));
    }
    lines
}

/// Applies one SGR parameter list (the digits between `\x1b[` and `m`) to
/// `style`.
fn apply_sgr(style: &mut Style, params: &str) {
    if params.is_empty() {
        *style = Style::default();
        return;
    }
    let mut nums = params.split(';').map(|p| p.parse::<u16>().unwrap_or(0));
    while let Some(code) = nums.next() {
        match code {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => {
                *style = style
                    .remove_modifier(Modifier::BOLD)
                    .remove_modifier(Modifier::DIM);
            }
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(ansi_16(code - 30, false)),
            90..=97 => *style = style.fg(ansi_16(code - 90, true)),
            40..=47 => *style = style.bg(ansi_16(code - 40, false)),
            100..=107 => *style = style.bg(ansi_16(code - 100, true)),
            39 => *style = style.fg(Color::Reset),
            49 => *style = style.bg(Color::Reset),
            38 => apply_extended_colour(style, &mut nums, true),
            48 => apply_extended_colour(style, &mut nums, false),
            _ => {}
        }
    }
}

/// Applies a `38;5;N`, `38;2;R;G;B`, `48;5;N`, or `48;2;R;G;B` extended
/// colour, consuming its parameters from `nums`.
fn apply_extended_colour(style: &mut Style, nums: &mut impl Iterator<Item = u16>, fg: bool) {
    let color = match nums.next() {
        Some(5) => nums
            .next()
            .and_then(|idx| u8::try_from(idx).ok())
            .map(Color::Indexed),
        Some(2) => {
            let (r, g, b) = (nums.next(), nums.next(), nums.next());
            match (r, g, b) {
                (Some(r), Some(g), Some(b)) => {
                    match (u8::try_from(r), u8::try_from(g), u8::try_from(b)) {
                        (Ok(r), Ok(g), Ok(b)) => Some(Color::Rgb(r, g, b)),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        _ => None,
    };
    if let Some(color) = color {
        *style = if fg { style.fg(color) } else { style.bg(color) };
    }
}

/// Maps a base ANSI colour index (`0..=7`) to its named [`Color`].
const fn ansi_16(index: u16, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

/// A minimal keyword list for [`highlight_code_line`].
///
/// This is not a grammar: it is a fixed set of Rust-shaped keywords that
/// gives a reader a rough sense of structure without a `tree-sitter`
/// dependency. See this module's "What this view cannot fully do."
const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "struct", "enum", "impl", "match", "if", "else", "for", "while",
    "loop", "return", "use", "mod", "trait", "async", "await", "move", "dyn", "self", "Self",
    "const", "static", "true", "false", "as", "in",
];

/// Highlights one line of code with a small heuristic: a `//` comment runs
/// to the end of the line, a double-quoted string is one span, and a
/// keyword from [`KEYWORDS`] is another. Everything else keeps the theme's
/// default text colour.
#[must_use]
pub fn highlight_code_line(line: &str, theme: &Theme) -> Line<'static> {
    let comment_style = theme.text_dim();
    let string_style = theme.ok();
    let keyword_style = theme.style(theme.palette().doppler_blue);
    let default_style = theme.style(theme.palette().text);

    if let Some(comment_at) = line.find("//") {
        let (code, comment) = line.split_at(comment_at);
        let mut spans = highlight_words(code, keyword_style, string_style, default_style);
        spans.push(Span::styled(comment.to_owned(), comment_style));
        return Line::from(spans);
    }
    Line::from(highlight_words(
        line,
        keyword_style,
        string_style,
        default_style,
    ))
}

/// Splits `code` into keyword, string, and plain spans.
fn highlight_words(
    code: &str,
    keyword_style: Style,
    string_style: Style,
    default_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut chars = code.chars().peekable();
    let mut buf = String::new();

    let flush = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if buf.is_empty() {
            return;
        }
        let style = if KEYWORDS.contains(&buf.as_str()) {
            keyword_style
        } else {
            default_style
        };
        spans.push(Span::styled(std::mem::take(buf), style));
    };

    while let Some(c) = chars.next() {
        if c == '"' {
            flush(&mut buf, &mut spans);
            let mut string = String::from("\"");
            for sc in chars.by_ref() {
                string.push(sc);
                if sc == '"' {
                    break;
                }
            }
            spans.push(Span::styled(string, string_style));
        } else if c.is_alphanumeric() || c == '_' {
            buf.push(c);
        } else {
            flush(&mut buf, &mut spans);
            spans.push(Span::styled(c.to_string(), default_style));
        }
    }
    flush(&mut buf, &mut spans);
    spans
}

/// Renders a small, dependency-free subset of Markdown: headings, fenced
/// code blocks (with [`highlight_code_line`] applied inside), bullet lists,
/// inline code, and bold text.
///
/// See this module's "What this view cannot fully do": this is a stand-in
/// for `termimad` or a `pulldown-cmark`-based renderer, neither of which
/// this crate depends on.
#[must_use]
pub fn render_markdown(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let default_style = theme.style(theme.palette().text);
    let heading_style = theme
        .style(theme.palette().disk_mid)
        .add_modifier(Modifier::BOLD);
    let bullet_style = theme.style(theme.palette().doppler_blue);

    let mut lines = Vec::new();
    let mut in_fence = false;
    for raw_line in text.lines() {
        if let Some(stripped) = raw_line.strip_prefix("```") {
            in_fence = !in_fence;
            let _ = stripped; // the language tag, unused by this reduced highlighter
            continue;
        }
        if in_fence {
            lines.push(highlight_code_line(raw_line, theme));
            continue;
        }
        if let Some(heading) = raw_line.trim_start().strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim_start();
            lines.push(Line::styled(heading.to_owned(), heading_style));
            continue;
        }
        if let Some(item) = raw_line
            .trim_start()
            .strip_prefix("- ")
            .or_else(|| raw_line.trim_start().strip_prefix("* "))
        {
            let mut spans = vec![Span::styled("• ", bullet_style)];
            spans.extend(render_inline(item, default_style));
            lines.push(Line::from(spans));
            continue;
        }
        lines.push(Line::from(render_inline(raw_line, default_style)));
    }
    lines
}

/// Renders inline `` `code` `` and `**bold**` spans within one line of
/// plain Markdown text.
fn render_inline(text: &str, default_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '`' {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), default_style));
            }
            let mut code = String::new();
            let mut closed = false;
            for cc in chars.by_ref() {
                if cc == '`' {
                    closed = true;
                    break;
                }
                code.push(cc);
            }
            let style = default_style.add_modifier(Modifier::BOLD);
            if closed {
                spans.push(Span::styled(code, style));
            } else {
                // No closing backtick: treat the rest as plain text rather
                // than eating it silently.
                spans.push(Span::styled(format!("`{code}"), default_style));
            }
        } else if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), default_style));
            }
            let mut bold = String::new();
            let mut closed = false;
            while let Some(bc) = chars.next() {
                if bc == '*' && chars.peek() == Some(&'*') {
                    chars.next();
                    closed = true;
                    break;
                }
                bold.push(bc);
            }
            let style = default_style.add_modifier(Modifier::BOLD);
            if closed {
                spans.push(Span::styled(bold, style));
            } else {
                spans.push(Span::styled(format!("**{bold}"), default_style));
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, default_style));
    }
    spans
}

impl Transcript {
    /// Renders the transcript into `area`.
    ///
    /// `expanded_thinking` shows the full text of the open
    /// [`Segment::Reasoning`] segment when true; collapsed, it shows only
    /// `▸ thinking (N tok)` with the live count — task unit `H4`, rule 1.
    /// This widget draws no border of its own; a caller renders it into the
    /// inner area of whatever pane frame already surrounds it.
    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme, expanded_thinking: bool) {
        let mut lines = Vec::new();
        for segment in &self.segments {
            lines.extend(render_segment(segment, theme, expanded_thinking));
        }
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

/// Renders one [`Segment`] to zero or more lines.
fn render_segment(segment: &Segment, theme: &Theme, expanded_thinking: bool) -> Vec<Line<'static>> {
    let text_style = theme.style(theme.palette().text);
    match segment {
        Segment::User { text } => {
            let mut lines = vec![Line::styled(
                "▸ you",
                theme.focused_border().add_modifier(Modifier::BOLD),
            )];
            lines.extend(render_markdown(text, theme));
            lines
        }
        Segment::Assistant { text } => render_markdown(text, theme),
        Segment::Reasoning { text, token_count } => {
            if expanded_thinking {
                let mut lines = vec![Line::styled(
                    format!("▾ thinking ({token_count} tok)"),
                    theme.text_dim(),
                )];
                lines.extend(
                    text.lines()
                        .map(|line| Line::styled(line.to_owned(), theme.text_dim())),
                );
                lines
            } else {
                vec![Line::styled(
                    format!("▸ thinking ({token_count} tok) ··········"),
                    theme.text_dim(),
                )]
            }
        }
        Segment::ToolCall { name, args, .. } => vec![Line::styled(
            format!("┌ {name} · {args}"),
            theme.style(theme.palette().disk_mid),
        )],
        Segment::ToolProgress { line, .. } => {
            vec![Line::styled(format!("│ {line}"), theme.text_dim())]
        }
        Segment::ToolResult {
            name,
            is_error,
            headline,
            content,
            has_diff,
            ..
        } => render_tool_result(name, *is_error, headline, content, *has_diff, theme),
        Segment::LagWarning { dropped } => vec![Line::styled(
            format!("⚠ {dropped} events dropped — output is incomplete here"),
            theme.warn(),
        )],
    }
    .into_iter()
    .map(|line| {
        if line.spans.is_empty() {
            Line::styled(String::new(), text_style)
        } else {
            line
        }
    })
    .collect()
}

/// Renders a [`Segment::ToolResult`]: a headline, then either a parsed diff
/// or the tool's raw (ANSI-aware) output.
fn render_tool_result(
    name: &str,
    is_error: bool,
    headline: &str,
    content: &str,
    has_diff: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let headline_style = if is_error { theme.danger() } else { theme.ok() };
    let mut lines = vec![Line::styled(
        format!("└ {name}: {headline}"),
        headline_style,
    )];
    if has_diff {
        lines.extend(render_diff_lines(&UnifiedDiff::parse(content), theme));
    } else {
        lines.extend(ansi_to_lines(content));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{ToolCall, ToolResultSummary};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn theme() -> Theme {
        Theme::new(crate::theme::ColorLevel::TrueColor)
    }

    fn token(text: &str) -> Event {
        Event::TokenDelta {
            turn: "t1".into(),
            text: text.into(),
        }
    }

    fn reason(text: &str) -> Event {
        Event::ReasonDelta {
            turn: "t1".into(),
            text: text.into(),
        }
    }

    // --- Transcript::apply_event -------------------------------------

    #[test]
    fn consecutive_token_deltas_coalesce_into_one_assistant_segment() {
        let mut t = Transcript::new();
        t.apply_event(&token("Hello"));
        t.apply_event(&token(", "));
        t.apply_event(&token("world"));
        assert_eq!(t.segments().len(), 1);
        assert_eq!(
            t.segments()[0],
            Segment::Assistant {
                text: "Hello, world".to_owned()
            }
        );
    }

    #[test]
    fn a_user_message_breaks_the_open_assistant_segment() {
        let mut t = Transcript::new();
        t.apply_event(&token("first"));
        t.apply_event(&Event::UserMessage {
            turn: "t1".into(),
            text: "next question".into(),
        });
        t.apply_event(&token("second"));
        assert_eq!(t.segments().len(), 3);
        assert_eq!(
            t.segments()[2],
            Segment::Assistant {
                text: "second".to_owned()
            }
        );
    }

    #[test]
    fn reasoning_deltas_coalesce_and_count_tokens() {
        let mut t = Transcript::new();
        t.apply_event(&reason("thinking "));
        t.apply_event(&reason("some more"));
        assert_eq!(t.segments().len(), 1);
        match &t.segments()[0] {
            Segment::Reasoning { text, token_count } => {
                assert_eq!(text, "thinking some more");
                assert_eq!(*token_count, 2);
            }
            other => panic!("expected Reasoning, got {other:?}"),
        }
    }

    #[test]
    #[allow(
        clippy::default_trait_access,
        reason = "the `args` field is a serde_json::Value; naming that type directly would need \
                  dark-tui to depend on serde_json, which Rule 15 reserves to dark-contract"
    )]
    fn a_tool_call_ends_the_open_reasoning_segment() {
        let mut t = Transcript::new();
        t.apply_event(&reason("hmm"));
        t.apply_event(&Event::ToolCall {
            turn: "t1".into(),
            call: ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: Default::default(),
            },
        });
        t.apply_event(&reason("more"));
        assert_eq!(
            t.segments().len(),
            3,
            "reasoning must not merge across the tool call"
        );
    }

    #[test]
    #[allow(
        clippy::default_trait_access,
        reason = "the `args` field is a serde_json::Value; naming that type directly would need \
                  dark-tui to depend on serde_json, which Rule 15 reserves to dark-contract"
    )]
    fn tool_progress_and_result_are_recorded_in_order() {
        let mut t = Transcript::new();
        t.apply_event(&Event::ToolCall {
            turn: "t1".into(),
            call: ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: Default::default(),
            },
        });
        t.apply_event(&Event::ToolProgress {
            turn: "t1".into(),
            call_id: "c1".into(),
            line: "reading…".into(),
        });
        t.apply_event(&Event::ToolResult {
            turn: "t1".into(),
            call_id: "c1".into(),
            result: ToolResultSummary {
                name: "read_file".into(),
                is_error: false,
                bytes: 4,
                headline: "done".into(),
                has_diff: false,
            },
            content: "done".into(),
        });
        assert_eq!(t.segments().len(), 3);
        assert!(matches!(t.segments()[0], Segment::ToolCall { .. }));
        assert!(matches!(t.segments()[1], Segment::ToolProgress { .. }));
        assert!(matches!(t.segments()[2], Segment::ToolResult { .. }));
    }

    #[test]
    fn record_lag_pushes_a_visible_marker() {
        let mut t = Transcript::new();
        t.apply_event(&token("partial"));
        t.record_lag(7);
        t.apply_event(&token("more"));
        assert_eq!(
            t.segments().len(),
            3,
            "lag must break the open assistant segment"
        );
        assert_eq!(t.segments()[1], Segment::LagWarning { dropped: 7 });
    }

    #[test]
    fn an_empty_transcript_is_empty() {
        assert!(Transcript::new().is_empty());
    }

    #[test]
    fn events_this_view_has_no_use_for_are_ignored_without_panicking() {
        let mut t = Transcript::new();
        t.apply_event(&Event::Budget {
            used: 1,
            granted: 2,
        });
        t.apply_event(&Event::Notice("hi".into()));
        assert!(t.is_empty());
    }

    // --- ansi_to_lines -------------------------------------------------

    #[test]
    fn ansi_to_lines_strips_escapes_and_keeps_the_text() {
        let lines = ansi_to_lines("\u{1b}[31mred text\u{1b}[0m plain");
        assert_eq!(lines.len(), 1);
        let plain: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(plain, "red text plain");
    }

    #[test]
    fn ansi_to_lines_applies_the_named_colour() {
        let lines = ansi_to_lines("\u{1b}[31mred\u{1b}[0m");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Red));
    }

    #[test]
    fn ansi_to_lines_splits_on_newlines_and_keeps_style_across_them() {
        let lines = ansi_to_lines("\u{1b}[32mgreen\nstill green\u{1b}[0m");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn ansi_to_lines_handles_256_colour_codes() {
        let lines = ansi_to_lines("\u{1b}[38;5;200mfoo\u{1b}[0m");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Indexed(200)));
    }

    #[test]
    fn ansi_to_lines_never_panics_on_a_truncated_escape() {
        let lines = ansi_to_lines("plain \u{1b}[31");
        let plain: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(plain.contains("plain"));
    }

    // --- highlight_code_line / render_markdown -------------------------

    #[test]
    fn highlight_code_line_separates_a_trailing_comment() {
        let theme = theme();
        let line = highlight_code_line("let x = 1; // a comment", &theme);
        let last = line.spans.last().expect("at least one span");
        assert!(last.content.contains("a comment"));
        assert_eq!(last.style.fg, theme.text_dim().fg);
    }

    #[test]
    fn highlight_code_line_styles_a_keyword_differently_from_plain_text() {
        let theme = theme();
        let line = highlight_code_line("fn go", &theme);
        let fn_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "fn")
            .unwrap();
        assert_eq!(fn_span.style.fg, Some(theme.palette().doppler_blue));
    }

    #[test]
    fn render_markdown_turns_a_heading_into_one_styled_line() {
        let theme = theme();
        let lines = render_markdown("# Title\nbody", &theme);
        assert_eq!(lines.len(), 2);
        let heading_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(heading_text, "Title");
    }

    #[test]
    fn render_markdown_renders_a_fenced_code_block_without_the_fence_markers() {
        let theme = theme();
        let lines = render_markdown("```rust\nlet x = 1;\n```\nafter", &theme);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(!text.iter().any(|l| l.contains("```")));
        assert!(text.iter().any(|l| l.contains("let x = 1;")));
        assert!(text.iter().any(|l| l == "after"));
    }

    #[test]
    fn render_markdown_turns_a_bullet_into_a_glyph_prefixed_line() {
        let theme = theme();
        let lines = render_markdown("- one\n- two", &theme);
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(first.starts_with('•'));
    }

    #[test]
    fn render_markdown_bolds_inline_code_and_double_star_text() {
        let theme = theme();
        let lines = render_markdown("plain `code` and **bold**", &theme);
        let bold_spans: Vec<&Span<'_>> = lines[0]
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .collect();
        assert_eq!(bold_spans.len(), 2);
    }

    #[test]
    fn render_markdown_never_panics_on_an_unterminated_backtick() {
        let theme = theme();
        let lines = render_markdown("plain `unterminated", &theme);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("unterminated"));
    }

    // --- Transcript::render ---------------------------------------------

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        let area = buffer.area;
        let mut text = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                text.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            text.push('\n');
        }
        text
    }

    fn render_to(
        t: &Transcript,
        width: u16,
        height: u16,
        expanded: bool,
    ) -> ratatui::buffer::Buffer {
        let theme = theme();
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        terminal
            .draw(|frame| t.render(frame.area(), frame.buffer_mut(), &theme, expanded))
            .expect("render must not fail against a TestBackend");
        terminal.backend().buffer().clone()
    }

    #[test]
    fn a_collapsed_thinking_segment_shows_the_live_count_not_the_text() {
        let mut t = Transcript::new();
        for _ in 0..312 {
            t.apply_event(&reason("x"));
        }
        let text = buffer_text(&render_to(&t, 60, 10, false));
        assert!(text.contains("thinking (312 tok)"));
    }

    #[test]
    fn expanding_thinking_shows_the_accumulated_text() {
        let mut t = Transcript::new();
        t.apply_event(&reason("the secret reasoning"));
        let collapsed = buffer_text(&render_to(&t, 60, 10, false));
        let expanded = buffer_text(&render_to(&t, 60, 10, true));
        assert!(!collapsed.contains("secret reasoning"));
        assert!(expanded.contains("secret reasoning"));
    }

    #[test]
    #[allow(
        clippy::default_trait_access,
        reason = "the `args` field is a serde_json::Value; naming that type directly would need \
                  dark-tui to depend on serde_json, which Rule 15 reserves to dark-contract"
    )]
    fn a_streaming_turn_renders_without_panicking() {
        let mut t = Transcript::new();
        t.apply_event(&Event::UserMessage {
            turn: "t1".into(),
            text: "fix the staleness check".into(),
        });
        t.apply_event(&reason("thinking about pack.rs"));
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
                bytes: 10,
                headline: "1 change".into(),
                has_diff: true,
            },
            content: "--- a/pack.rs\n+++ b/pack.rs\n@@ -1 +1 @@\n-old\n+new\n".into(),
        });
        t.apply_event(&token("Done."));
        let text = buffer_text(&render_to(&t, 80, 24, false));
        assert!(text.contains("edit_file"));
        assert!(text.contains("Done."));
    }

    #[test]
    #[allow(
        clippy::default_trait_access,
        reason = "the `args` field is a serde_json::Value; naming that type directly would need \
                  dark-tui to depend on serde_json, which Rule 15 reserves to dark-contract"
    )]
    fn a_tool_result_with_a_diff_renders_diff_styling() {
        let mut t = Transcript::new();
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
                bytes: 10,
                headline: "1 change".into(),
                has_diff: true,
            },
            content: "-old line\n+new line\n".into(),
        });
        let theme = theme();
        let mut found_removed = false;
        let mut found_added = false;
        for segment in t.segments() {
            if let Segment::ToolResult {
                content, has_diff, ..
            } = segment
            {
                assert!(has_diff);
                let diff = UnifiedDiff::parse(content);
                // `render_diff_lines` builds each `Line` with `Line::styled`, which
                // sets the *line's* style rather than its span's — ratatui applies a
                // line's style to every cell it covers when it renders (the same
                // pattern `crate::app::render` already relies on, and
                // `colour_degradation.rs` already pins), so this checks `line.style`,
                // not `line.spans[0].style`.
                for line in render_diff_lines(&diff, &theme) {
                    if line.style.fg == Some(theme.palette().ok) {
                        found_added = true;
                    }
                    if line.style.fg == Some(theme.palette().danger) {
                        found_removed = true;
                    }
                }
            }
        }
        assert!(found_added && found_removed);
    }

    #[test]
    fn a_lag_warning_is_visible_in_the_rendered_frame() {
        let mut t = Transcript::new();
        t.apply_event(&token("before"));
        t.record_lag(3);
        let text = buffer_text(&render_to(&t, 80, 24, false));
        assert!(text.contains("dropped"));
    }

    #[test]
    fn rendering_never_panics_on_a_tiny_area() {
        let mut t = Transcript::new();
        t.apply_event(&token("hello world, this is a long line of streamed text"));
        let _ = render_to(&t, 1, 1, false);
        let _ = render_to(&t, 0, 0, false);
    }

    #[test]
    fn the_same_transcript_renders_identical_bytes_twice() {
        let mut t = Transcript::new();
        t.apply_event(&token("deterministic"));
        let first = render_to(&t, 60, 10, false);
        let second = render_to(&t, 60, 10, false);
        assert_eq!(first, second);
    }
}
