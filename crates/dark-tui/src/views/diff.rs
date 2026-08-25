//! The diff view: what a person approves before a write happens.
//!
//! `Tool::preview` (task unit `A4`) supplies a real unified diff before a
//! mutating tool call runs, so a confirmation shows that diff, never a
//! summary of it — see `docs/adr/0004` for why the machinery to produce one
//! exists. This module renders that diff, for the `RightPane::Diff` pane
//! and for the confirmation modal alike.

use std::path::{Path, PathBuf};

use dark_contract::ConfirmPrompt;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap};

use crate::theme::Theme;

/// What one line of a unified diff is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// A `--- a/path` or `+++ b/path` line.
    FileHeader,
    /// An `@@ -a,b +c,d @@` hunk header.
    HunkHeader,
    /// A line the diff adds.
    Added,
    /// A line the diff removes.
    Removed,
    /// A line the diff leaves unchanged, shown for context.
    Context,
}

/// One line of a parsed unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// What kind of line this is.
    pub kind: DiffLineKind,
    /// The line's exact text, the leading `+`, `-`, or space included.
    pub text: String,
}

/// Classifies one line of unified-diff text.
///
/// The file-header check runs before the added/removed checks, since a
/// `+++ b/path` line would otherwise also match the `+` prefix that marks
/// an added line.
#[must_use]
fn classify(line: &str) -> DiffLineKind {
    if line.starts_with("--- ") || line.starts_with("+++ ") {
        DiffLineKind::FileHeader
    } else if line.starts_with("@@") {
        DiffLineKind::HunkHeader
    } else if line.starts_with('+') {
        DiffLineKind::Added
    } else if line.starts_with('-') {
        DiffLineKind::Removed
    } else {
        DiffLineKind::Context
    }
}

/// A unified diff, parsed into lines a view can style.
///
/// [`UnifiedDiff::parse`] never fails: a line this crate does not recognise
/// (for example a diff with no hunks, or plain text that is not a diff at
/// all) becomes [`DiffLineKind::Context`] rather than an error, since a
/// confirmation must show the exact text regardless — see task unit `H4`,
/// rule 8: "Never show a summary."
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnifiedDiff {
    lines: Vec<DiffLine>,
}

impl UnifiedDiff {
    /// Parses `diff`, splitting on line boundaries and classifying each one.
    #[must_use]
    pub fn parse(diff: &str) -> Self {
        let lines = diff
            .lines()
            .map(|line| DiffLine {
                kind: classify(line),
                text: line.to_owned(),
            })
            .collect();
        Self { lines }
    }

    /// Returns the parsed lines, in file order.
    #[must_use]
    pub fn lines(&self) -> &[DiffLine] {
        &self.lines
    }

    /// Returns true when the diff has no lines at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Counts the lines the diff adds.
    #[must_use]
    pub fn added_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Added)
            .count()
    }

    /// Counts the lines the diff removes.
    #[must_use]
    pub fn removed_count(&self) -> usize {
        self.lines
            .iter()
            .filter(|line| line.kind == DiffLineKind::Removed)
            .count()
    }
}

/// Returns the style for one [`DiffLineKind`].
///
/// Added and removed lines use [`Theme::ok`] and [`Theme::danger`] — the
/// same tokens the rest of the shell uses for success and failure, so a
/// diff reads with the palette rather than against it. A hunk header uses
/// `disk-mid`, the palette's "active work" accent; a file header is bold
/// text; unchanged context dims.
#[must_use]
pub fn style_for(kind: DiffLineKind, theme: &Theme) -> Style {
    match kind {
        DiffLineKind::FileHeader => theme
            .style(theme.palette().text)
            .add_modifier(Modifier::BOLD),
        DiffLineKind::HunkHeader => theme.style(theme.palette().disk_mid),
        DiffLineKind::Added => theme.ok(),
        DiffLineKind::Removed => theme.danger(),
        DiffLineKind::Context => theme.text_dim(),
    }
}

/// Renders every line of `diff` as a styled [`Line`].
#[must_use]
pub fn render_lines(diff: &UnifiedDiff, theme: &Theme) -> Vec<Line<'static>> {
    diff.lines()
        .iter()
        .map(|line| Line::styled(line.text.clone(), style_for(line.kind, theme)))
        .collect()
}

/// The `RightPane::Diff` view: a unified diff, optionally headed by the
/// path it changes.
///
/// This widget draws no border of its own; a caller renders it into the
/// inner area of whatever pane frame already surrounds it.
#[derive(Debug, Clone)]
pub struct DiffView<'a> {
    diff: &'a UnifiedDiff,
    theme: &'a Theme,
    path: Option<&'a Path>,
}

impl<'a> DiffView<'a> {
    /// Builds a view over `diff`, with no path heading.
    #[must_use]
    pub const fn new(diff: &'a UnifiedDiff, theme: &'a Theme) -> Self {
        Self {
            diff,
            theme,
            path: None,
        }
    }

    /// Heads the view with the path the diff changes.
    #[must_use]
    pub const fn path(mut self, path: &'a Path) -> Self {
        self.path = Some(path);
        self
    }
}

impl Widget for DiffView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut lines = Vec::new();
        if let Some(path) = self.path {
            lines.push(Line::styled(
                path.display().to_string(),
                self.theme.focused_border().add_modifier(Modifier::BOLD),
            ));
        }
        if self.diff.is_empty() {
            lines.push(Line::styled("No changes.", self.theme.text_dim()));
        } else {
            lines.extend(render_lines(self.diff, self.theme));
        }
        // A diff line that wrapped back to the left margin reads as a
        // separate diff line, which is a lie about what the file contains.
        // The hanging indent keeps a continuation visibly a continuation.
        let wrapped = crate::views::wrap::hang(
            lines,
            &Span::raw(""),
            &Span::raw("    "),
            usize::from(area.width),
        );
        Paragraph::new(wrapped).render(area, buf);
    }
}

/// The exact detail a confirmation modal shows for one [`ConfirmPrompt`].
///
/// See task unit `H4`, rule 8: "Show the exact diff or the exact command in
/// a confirmation modal. Never show a summary." [`ConfirmDetail::Write`]
/// and [`ConfirmDetail::Exec`] hold exactly that: the diff or the command,
/// verbatim. [`ConfirmDetail::Other`] exists only because
/// [`ConfirmPrompt::Other`] itself still carries a one-line summary
/// alongside its full detail — a shape `dark-contract` owns, not this view
/// — and this renders both, the full detail included, rather than dropping
/// either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmDetail {
    /// A file write. Carries the exact unified diff.
    Write {
        /// The file that changes.
        path: PathBuf,
        /// The exact unified diff.
        diff: UnifiedDiff,
    },
    /// A command. Carries the exact command line.
    Exec {
        /// The exact command.
        command: String,
        /// The working directory.
        cwd: PathBuf,
        /// Whether a shell interprets the command.
        shell: bool,
    },
    /// Any other action, exactly as `dark-contract` described it.
    Other {
        /// The one-line summary `dark-contract` sent.
        summary: String,
        /// The full detail `dark-contract` sent.
        detail: String,
    },
}

impl ConfirmDetail {
    /// Converts a [`ConfirmPrompt`] into the detail this view renders,
    /// parsing a write's diff text.
    #[must_use]
    pub fn from_prompt(prompt: &ConfirmPrompt) -> Self {
        match prompt {
            ConfirmPrompt::Write { path, diff } => Self::Write {
                path: path.clone(),
                diff: UnifiedDiff::parse(diff),
            },
            ConfirmPrompt::Exec {
                command,
                cwd,
                shell,
            } => Self::Exec {
                command: command.clone(),
                cwd: cwd.clone(),
                shell: *shell,
            },
            ConfirmPrompt::Other { summary, detail } => Self::Other {
                summary: summary.clone(),
                detail: detail.clone(),
            },
        }
    }

    /// Renders this detail as styled lines, exact and untruncated.
    #[must_use]
    pub fn lines(&self, theme: &Theme) -> Vec<Line<'static>> {
        match self {
            Self::Write { path, diff } => {
                let mut lines = vec![Line::styled(
                    format!("write {}", path.display()),
                    theme.focused_border().add_modifier(Modifier::BOLD),
                )];
                lines.extend(render_lines(diff, theme));
                lines
            }
            Self::Exec {
                command,
                cwd,
                shell,
            } => vec![
                Line::styled(
                    format!("run: {command}"),
                    theme.danger().add_modifier(Modifier::BOLD),
                ),
                Line::styled(format!("in:  {}", cwd.display()), theme.text_dim()),
                Line::styled(
                    format!("shell: {}", if *shell { "yes" } else { "no" }),
                    theme.text_dim(),
                ),
            ],
            Self::Other { summary, detail } => {
                let mut lines = vec![Line::styled(
                    summary.clone(),
                    theme.warn().add_modifier(Modifier::BOLD),
                )];
                let text_style = theme.style(theme.palette().text);
                lines.extend(
                    detail
                        .lines()
                        .map(|line| Line::styled(line.to_owned(), text_style)),
                );
                lines
            }
        }
    }
}

/// The confirmation modal: the exact diff or the exact command, over a
/// cleared, bordered area.
///
/// See task unit `H4`, rule 8, and `docs/adr/0004`.
#[derive(Debug, Clone)]
pub struct ConfirmModal<'a> {
    detail: &'a ConfirmDetail,
    theme: &'a Theme,
}

impl<'a> ConfirmModal<'a> {
    /// Builds a modal over `detail`.
    #[must_use]
    pub const fn new(detail: &'a ConfirmDetail, theme: &'a Theme) -> Self {
        Self { detail, theme }
    }
}

impl Widget for ConfirmModal<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.theme.focused_border())
            .title(Line::from(Span::styled(
                " CONFIRM ",
                self.theme.focused_border(),
            )))
            .style(self.theme.panel());
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = self.detail.lines(self.theme);
        // The keys that answer this. Without them the modal states the
        // change and gives no way to act on it.
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("y", self.theme.ok().add_modifier(Modifier::BOLD)),
            Span::styled(" allow once   ", self.theme.text_dim()),
            Span::styled("a", self.theme.ok().add_modifier(Modifier::BOLD)),
            Span::styled(" allow always   ", self.theme.text_dim()),
            Span::styled("n", self.theme.danger().add_modifier(Modifier::BOLD)),
            Span::styled(" refuse", self.theme.text_dim()),
        ]));

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(inner, buf);
    }
}

impl ConfirmModal<'_> {
    /// How many rows and columns this modal wants, borders included.
    ///
    /// A confirmation must show the change exactly (task unit `H4`, rule
    /// 8), so it grows to fit one; but a three-line command in a modal the
    /// height of the terminal reads as a fault. The caller clamps this
    /// against the frame it has.
    #[must_use]
    pub fn wanted_size(&self) -> (u16, u16) {
        let lines = self.detail.lines(self.theme);
        let widest = lines
            .iter()
            .map(ratatui::text::Line::width)
            .max()
            .unwrap_or(0);
        // Two rows of border, a blank, and the key hints.
        let height = lines.len().saturating_add(4);
        let width = widest.max(FOOTER_WIDTH).saturating_add(2);
        (
            u16::try_from(width).unwrap_or(u16::MAX),
            u16::try_from(height).unwrap_or(u16::MAX),
        )
    }
}

/// The width of the modal's key-hint footer, so a modal never renders
/// narrower than the line that says how to answer it.
const FOOTER_WIDTH: usize = 52;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    const SAMPLE_DIFF: &str = "--- a/pack.rs\n\
+++ b/pack.rs\n\
@@ -1,3 +1,3 @@\n\
-fn stale(&self) -> bool {\n\
+fn stale(&self, now: Instant) -> bool {\n\
     todo!()\n";

    fn theme() -> Theme {
        Theme::new(crate::theme::ColorLevel::TrueColor)
    }

    #[test]
    fn parse_classifies_each_kind_of_line() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let kinds: Vec<DiffLineKind> = diff.lines().iter().map(|line| line.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiffLineKind::FileHeader,
                DiffLineKind::FileHeader,
                DiffLineKind::HunkHeader,
                DiffLineKind::Removed,
                DiffLineKind::Added,
                DiffLineKind::Context,
            ]
        );
    }

    #[test]
    fn a_plus_plus_plus_header_is_a_file_header_not_an_added_line() {
        let diff = UnifiedDiff::parse("+++ b/pack.rs\n");
        assert_eq!(diff.lines()[0].kind, DiffLineKind::FileHeader);
    }

    #[test]
    fn added_and_removed_counts_match_the_sample() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        assert_eq!(diff.added_count(), 1);
        assert_eq!(diff.removed_count(), 1);
    }

    #[test]
    fn an_empty_diff_is_empty() {
        assert!(UnifiedDiff::parse("").is_empty());
    }

    #[test]
    fn text_that_is_not_a_diff_still_parses_without_failing() {
        // "Never show a summary": a confirmation always has something to
        // show, even when a tool could not produce a real diff.
        let diff = UnifiedDiff::parse("plain text, not a diff at all");
        assert_eq!(diff.lines().len(), 1);
        assert_eq!(diff.lines()[0].kind, DiffLineKind::Context);
    }

    #[test]
    fn added_lines_use_the_ok_token_and_removed_lines_use_danger() {
        let theme = theme();
        assert_eq!(
            style_for(DiffLineKind::Added, &theme).fg,
            Some(theme.palette().ok)
        );
        assert_eq!(
            style_for(DiffLineKind::Removed, &theme).fg,
            Some(theme.palette().danger)
        );
    }

    #[test]
    fn render_lines_preserves_the_exact_diff_text() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let theme = theme();
        let rendered = render_lines(&diff, &theme);
        let plain: Vec<String> = rendered
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let expected: Vec<&str> = SAMPLE_DIFF.lines().collect();
        assert_eq!(plain, expected);
    }

    fn render_widget(widget: impl Widget, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("a TestBackend always builds a terminal");
        terminal
            .draw(|frame| frame.render_widget(widget, frame.area()))
            .expect("render must not fail against a TestBackend");
        terminal.backend().buffer().clone()
    }

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

    #[test]
    fn diff_view_shows_the_path_and_the_diff_text() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let theme = theme();
        let path = Path::new("crates/dark-lexicon/pack.rs");
        let view = DiffView::new(&diff, &theme).path(path);
        let text = buffer_text(&render_widget(view, 60, 10));
        assert!(text.contains("crates/dark-lexicon/pack.rs"));
        assert!(text.contains("fn stale"));
    }

    #[test]
    fn diff_view_says_so_when_there_is_nothing_to_show() {
        let diff = UnifiedDiff::parse("");
        let theme = theme();
        let view = DiffView::new(&diff, &theme);
        let text = buffer_text(&render_widget(view, 40, 5));
        assert!(text.contains("No changes."));
    }

    #[test]
    fn diff_view_never_panics_on_a_tiny_area() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let theme = theme();
        let view = DiffView::new(&diff, &theme);
        let _ = render_widget(view, 1, 1);
    }

    #[test]
    fn the_rendered_buffer_actually_carries_distinct_colours_for_added_and_removed_cells() {
        // `render_lines` sets a `Line`'s own style rather than its spans';
        // this checks the property that actually matters — what colour
        // lands on the terminal buffer once `DiffView` renders — the same
        // way `colour_degradation.rs` checks the rest of the shell, rather
        // than trusting an intermediate `Line`/`Span` structure.
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let theme = theme();
        let view = DiffView::new(&diff, &theme);
        let buffer = render_widget(view, 60, 10);

        let removed_row = 3; // "-fn stale(&self) -> bool {"
        let added_row = 4; // "+fn stale(&self, now: Instant) -> bool {"
        let removed_fg = buffer.cell((0, removed_row)).unwrap().fg;
        let added_fg = buffer.cell((0, added_row)).unwrap().fg;
        assert_eq!(removed_fg, theme.palette().danger);
        assert_eq!(added_fg, theme.palette().ok);
        assert_ne!(removed_fg, added_fg);
    }

    #[test]
    fn confirm_detail_from_a_write_prompt_carries_the_exact_diff() {
        let prompt = ConfirmPrompt::Write {
            path: PathBuf::from("pack.rs"),
            diff: SAMPLE_DIFF.to_owned(),
        };
        let detail = ConfirmDetail::from_prompt(&prompt);
        let ConfirmDetail::Write { path, diff } = &detail else {
            panic!("expected ConfirmDetail::Write");
        };
        assert_eq!(path, Path::new("pack.rs"));
        assert_eq!(diff.lines().len(), SAMPLE_DIFF.lines().count());
    }

    #[test]
    fn confirm_detail_from_an_exec_prompt_carries_the_exact_command() {
        let prompt = ConfirmPrompt::Exec {
            command: "cargo nextest run -p dark-lexicon".to_owned(),
            cwd: PathBuf::from("/home/dan/myrepo"),
            shell: false,
        };
        let theme = theme();
        let detail = ConfirmDetail::from_prompt(&prompt);
        let lines = detail.lines(&theme);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("cargo nextest run -p dark-lexicon"));
        assert!(text.contains("/home/dan/myrepo"));
    }

    #[test]
    fn confirm_detail_never_truncates_the_other_variant() {
        let prompt = ConfirmPrompt::Other {
            summary: "restart the resident set".to_owned(),
            detail: "line one\nline two\nline three".to_owned(),
        };
        let theme = theme();
        let detail = ConfirmDetail::from_prompt(&prompt);
        let lines = detail.lines(&theme);
        // The summary heading plus every line of the full detail.
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn confirm_modal_clears_what_was_behind_it() {
        let prompt = ConfirmPrompt::Exec {
            command: "rm -rf /tmp/scratch".to_owned(),
            cwd: PathBuf::from("/home/dan/myrepo"),
            shell: true,
        };
        let theme = theme();
        let detail = ConfirmDetail::from_prompt(&prompt);
        let modal = ConfirmModal::new(&detail, &theme);
        let buffer = render_widget(modal, 50, 10);
        let text = buffer_text(&buffer);
        assert!(text.contains("CONFIRM"));
        assert!(text.contains("rm -rf /tmp/scratch"));
    }

    #[test]
    fn a_no_colour_theme_still_shows_every_diff_line() {
        let diff = UnifiedDiff::parse(SAMPLE_DIFF);
        let theme = Theme::new(crate::theme::ColorLevel::None);
        let rendered = render_lines(&diff, &theme);
        // `render_lines` builds each `Line` with `Line::styled`, which sets
        // the *line's* style — ratatui applies that to every cell the line
        // covers when it renders (see `colour_degradation.rs`), so this
        // checks `line.style`, not a span's.
        for line in &rendered {
            assert_eq!(line.style.fg, Some(Color::Reset));
        }
        assert_eq!(rendered.len(), diff.lines().len());
    }

    #[test]
    fn confirm_modal_never_panics_on_a_tiny_area() {
        let prompt = ConfirmPrompt::Other {
            summary: "s".to_owned(),
            detail: "d".to_owned(),
        };
        let theme = theme();
        let detail = ConfirmDetail::from_prompt(&prompt);
        let modal = ConfirmModal::new(&detail, &theme);
        let _ = render_widget(modal, 1, 1);
        let _ = render_widget(ConfirmModal::new(&detail, &theme), 0, 0);
    }
}
