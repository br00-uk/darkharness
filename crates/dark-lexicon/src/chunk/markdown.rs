//! Scanning Markdown into heading-and-fence-aware blocks.
//!
//! This is the parsing core that [`crate::chunk::algorithm`] builds on. It
//! keeps two responsibilities separate from the algorithm proper: telling
//! headings apart from a `#` inside a fenced code block, and telling a
//! whole fenced code block apart from an ordinary paragraph, so the
//! algorithm never has to look at raw lines itself.

/// One line's classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Line<'a> {
    /// An ATX heading (`#` through `######`) outside any fence.
    Heading { level: u8, text: &'a str },
    /// Any other line, fence delimiters included.
    Text(&'a str),
}

/// Returns the fence character (`` ` `` or `~`) that `line` opens or
/// closes, when `line` is a fence marker.
///
/// `CommonMark` allows up to three leading spaces before a fence marker and
/// requires a run of at least three identical fence characters.
pub(crate) fn fence_marker(line: &str) -> Option<char> {
    let mut indent = 0;
    let bytes = line.as_bytes();
    while indent < 3 && bytes.get(indent) == Some(&b' ') {
        indent += 1;
    }
    let rest = &line[indent..];
    let first = rest.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = rest.chars().take_while(|&c| c == first).count();
    if run >= 3 { Some(first) } else { None }
}

/// Parses one line as an ATX heading, when it is not inside a fence.
fn atx_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some((
        u8::try_from(level).expect("level is 1..=6, checked above"),
        text,
    ))
}

/// Classifies every line of `body`, tracking fence state so a `#` inside a
/// fenced code block never classifies as [`Line::Heading`].
///
/// The returned slice has one entry per line of `body` (split on `\n`,
/// matching [`str::lines`]), in order.
pub(crate) fn classify_lines(body: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut fence_char: Option<char> = None;
    for line in body.lines() {
        if let Some(marker) = fence_marker(line) {
            match fence_char {
                Some(open) if open == marker => fence_char = None,
                Some(_) => {}
                None => fence_char = Some(marker),
            }
            out.push(Line::Text(line));
            continue;
        }
        if fence_char.is_some() {
            out.push(Line::Text(line));
            continue;
        }
        match atx_heading(line) {
            Some((level, text)) => out.push(Line::Heading { level, text }),
            None => out.push(Line::Text(line)),
        }
    }
    out
}

/// Returns `true` when `text` contains no unbalanced fenced code block:
/// every opened fence closes before the text ends.
///
/// `fence_integrity.rs` uses this to check every chunk the algorithm
/// produces.
#[must_use]
pub fn has_balanced_fences(text: &str) -> bool {
    let mut fence_char: Option<char> = None;
    for line in text.lines() {
        if let Some(marker) = fence_marker(line) {
            match fence_char {
                Some(open) if open == marker => fence_char = None,
                Some(_) => {}
                None => fence_char = Some(marker),
            }
        }
    }
    fence_char.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_headings_outside_fences() {
        let lines = classify_lines("# One\ntext\n## Two\n");
        assert_eq!(
            lines,
            vec![
                Line::Heading {
                    level: 1,
                    text: "One"
                },
                Line::Text("text"),
                Line::Heading {
                    level: 2,
                    text: "Two"
                },
            ]
        );
    }

    #[test]
    fn a_hash_inside_a_fence_is_never_a_heading() {
        let lines = classify_lines("```\n# not a heading\n```\n# real\n");
        let headings: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heading { text, .. } => Some(*text),
                Line::Text(_) => None,
            })
            .collect();
        assert_eq!(headings, vec!["real"]);
    }

    #[test]
    fn balanced_fences_report_balanced() {
        assert!(has_balanced_fences("text\n```\ncode\n```\nmore text\n"));
    }

    #[test]
    fn an_unclosed_fence_reports_unbalanced() {
        assert!(!has_balanced_fences("```\ncode with no closing fence\n"));
    }

    #[test]
    fn text_with_no_fence_at_all_is_balanced() {
        assert!(has_balanced_fences("just plain text\n"));
    }

    #[test]
    fn mismatched_fence_characters_do_not_close_each_other() {
        // A backtick fence is not closed by a tilde fence of the same
        // length; the backtick fence stays open.
        assert!(!has_balanced_fences("```\ncode\n~~~\n"));
    }
}
