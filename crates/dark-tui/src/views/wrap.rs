//! Span-aware line wrapping.
//!
//! [`ratatui::widgets::Paragraph`] can wrap for itself, but it wraps at
//! draw time and reports nothing back. Two things this crate needs are
//! impossible on those terms:
//!
//! - **A gutter on every visual line.** The transcript marks who is
//!   speaking with a coloured bar down the left of a block (see
//!   [`crate::views::transcript`]). If a paragraph wraps after the gutter
//!   is attached, only the first visual line of each block carries one and
//!   the rest hang loose.
//! - **Anchoring to the bottom.** A transcript shows the newest output, so
//!   the caller must know how many visual lines the content occupies before
//!   it can decide which of them fit.
//!
//! So this module wraps first and draws second. [`wrap_lines`] takes styled
//! [`Line`]s and returns styled [`Line`]s, each already no wider than the
//! width it was given, and the caller then counts, slices, and prefixes
//! them freely.
//!
//! Wrapping breaks at a space where one is available and mid-word only when
//! a single word is itself wider than the target, which keeps a long path
//! or a hash visible rather than clipped. Every span keeps its own style
//! across a break.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Wraps `lines` so that no returned line is wider than `width` columns.
///
/// A returned line holds the same spans, in the same order, with the same
/// styles; only the break points are new. `width` of `0` returns `lines`
/// unchanged, because no break point can help at that width.
#[must_use]
pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        wrap_one(line, width, &mut out);
    }
    out
}

/// Wraps one line into `out`.
///
/// A line that already fits is moved across whole, so the common case
/// allocates nothing beyond the push.
fn wrap_one(line: Line<'static>, width: usize, out: &mut Vec<Line<'static>>) {
    if line_width(&line) <= width {
        out.push(line);
        return;
    }

    // A `Line` carries a style of its own, separate from its spans'.
    // `crate::views::diff::render_lines` uses exactly that — it colours the
    // line, not the spans — so every line this produces must carry it
    // forward or a wrapped diff loses its colour.
    let line_style = line.style;
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;

    for span in line.spans {
        let style = span.style;
        let mut rest = span.content.into_owned();

        while !rest.is_empty() {
            let room = width.saturating_sub(used);
            if room == 0 {
                out.push(Line::from(std::mem::take(&mut current)).style(line_style));
                used = 0;
                continue;
            }
            let (head, tail) = split_at_width(&rest, room, used == 0);
            if head.is_empty() {
                // Nothing fits in what is left of this line, but the line
                // already holds something: break and try again on a fresh
                // one, where `room` is the full width.
                out.push(Line::from(std::mem::take(&mut current)).style(line_style));
                used = 0;
                continue;
            }
            used += head.width();
            current.push(Span::styled(head, style));
            rest = tail;
            if !rest.is_empty() {
                out.push(Line::from(std::mem::take(&mut current)).style(line_style));
                used = 0;
                rest = rest.trim_start().to_owned();
            }
        }
    }

    if !current.is_empty() {
        out.push(Line::from(current).style(line_style));
    }
}

/// Splits `text` into a head no wider than `room` and the remainder.
///
/// Prefers to break after the last space that fits. Breaks mid-word when no
/// space fits and `at_line_start` is true, which is the only way to make
/// progress on a word wider than the whole line; otherwise it returns an
/// empty head so the caller ends the current line and retries with the full
/// width available.
fn split_at_width(text: &str, room: usize, at_line_start: bool) -> (String, String) {
    if text.width() <= room {
        return (text.to_owned(), String::new());
    }

    let mut last_space: Option<usize> = None;
    let mut used = 0usize;
    let mut cut = 0usize;

    for (index, ch) in text.char_indices() {
        let ch_width = char_width(ch);
        if used + ch_width > room {
            break;
        }
        if ch == ' ' {
            last_space = Some(index);
        }
        used += ch_width;
        cut = index + ch.len_utf8();
    }

    if let Some(space) = last_space {
        return (text[..space].to_owned(), text[space + 1..].to_owned());
    }
    if at_line_start && cut > 0 {
        return (text[..cut].to_owned(), text[cut..].to_owned());
    }
    (String::new(), text.to_owned())
}

/// Returns the display width of one character, treating a control
/// character as zero-width rather than as a missing value.
fn char_width(ch: char) -> usize {
    let mut buf = [0u8; 4];
    ch.encode_utf8(&mut buf).width()
}

/// Prefixes each line of `body`, wrapping first so that a continuation
/// line is prefixed too.
///
/// `first` goes in front of the first visual line of each input line and
/// `rest` in front of every line it wrapped onto, which is what gives a
/// wrapped tool call or a wrapped path a hanging indent rather than
/// letting it fall back to the left margin. Each input line is wrapped on
/// its own, so one long line never absorbs the line after it.
#[must_use]
pub fn hang(
    body: Vec<Line<'static>>,
    first: &Span<'static>,
    rest: &Span<'static>,
    width: usize,
) -> Vec<Line<'static>> {
    let indent = first.content.width().max(rest.content.width());
    let inner = width.saturating_sub(indent);
    let mut out = Vec::with_capacity(body.len());
    for line in body {
        for (index, wrapped) in wrap_lines(vec![line], inner).into_iter().enumerate() {
            let mut spans = vec![if index == 0 {
                first.clone()
            } else {
                rest.clone()
            }];
            let style = wrapped.style;
            spans.extend(wrapped.spans);
            out.push(Line::from(spans).style(style));
        }
    }
    out
}

/// Returns the display width of a whole line.
#[must_use]
pub fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| span.content.width()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    fn plain(text: &str) -> Line<'static> {
        Line::from(text.to_owned())
    }

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn a_line_that_fits_is_left_alone() {
        let wrapped = wrap_lines(vec![plain("short")], 20);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(text_of(&wrapped[0]), "short");
    }

    #[test]
    fn a_long_line_breaks_at_a_space() {
        let wrapped = wrap_lines(vec![plain("alpha beta gamma")], 11);
        let texts: Vec<String> = wrapped.iter().map(text_of).collect();
        assert_eq!(texts, vec!["alpha beta".to_owned(), "gamma".to_owned()]);
    }

    #[test]
    fn no_wrapped_line_is_wider_than_the_target() {
        let long = "the quick brown fox jumps over the lazy dog and keeps running";
        for width in 5..40 {
            let wrapped = wrap_lines(vec![plain(long)], width);
            for line in &wrapped {
                assert!(
                    line_width(line) <= width,
                    "width {width}: {:?} is {} wide",
                    text_of(line),
                    line_width(line)
                );
            }
        }
    }

    #[test]
    fn a_word_wider_than_the_line_is_broken_rather_than_lost() {
        // A path or a hash must stay readable, so it breaks mid-word.
        let wrapped = wrap_lines(vec![plain("crates/dark-core/src/policy/mod.rs")], 10);
        let joined: String = wrapped.iter().map(text_of).collect();
        assert_eq!(joined, "crates/dark-core/src/policy/mod.rs");
        assert!(wrapped.len() > 1);
    }

    #[test]
    fn a_span_keeps_its_style_across_a_break() {
        let styled = Line::from(vec![
            Span::styled("alpha ".to_owned(), Style::default().fg(Color::Red)),
            Span::styled("beta gamma".to_owned(), Style::default().fg(Color::Blue)),
        ]);
        let wrapped = wrap_lines(vec![styled], 8);
        for line in &wrapped {
            for span in &line.spans {
                assert!(
                    span.style.fg == Some(Color::Red) || span.style.fg == Some(Color::Blue),
                    "a break must not drop a span's style"
                );
            }
        }
    }

    #[test]
    fn a_line_style_survives_a_wrap() {
        // `crate::views::diff::render_lines` styles the line rather than
        // its spans, so dropping the line style here silently uncolours
        // every wrapped diff line.
        let styled = plain("alpha beta gamma delta").style(Style::default().fg(Color::Green));
        let wrapped = wrap_lines(vec![styled], 11);
        assert!(wrapped.len() > 1, "this test needs a line that wraps");
        for line in &wrapped {
            assert_eq!(line.style.fg, Some(Color::Green));
        }
    }

    #[test]
    fn a_line_style_survives_a_hanging_indent() {
        let styled = plain("alpha beta gamma delta").style(Style::default().fg(Color::Green));
        let hung = hang(vec![styled], &Span::raw(""), &Span::raw("  "), 12);
        assert!(hung.len() > 1, "this test needs a line that wraps");
        for line in &hung {
            assert_eq!(line.style.fg, Some(Color::Green));
        }
    }

    #[test]
    fn a_zero_width_target_returns_the_input() {
        let wrapped = wrap_lines(vec![plain("anything")], 0);
        assert_eq!(wrapped.len(), 1);
    }

    #[test]
    fn a_multi_byte_line_never_breaks_inside_a_character() {
        // Every character here is three bytes and two columns wide.
        let wrapped = wrap_lines(vec![plain(&"日".repeat(20))], 7);
        for line in &wrapped {
            assert!(line_width(line) <= 7);
            // Reassembling must produce valid text with no lost characters.
            assert!(text_of(line).chars().all(|c| c == '日'));
        }
    }
}
