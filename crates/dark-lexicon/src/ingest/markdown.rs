//! Extracting a heading outline from Markdown text.
//!
//! This is deliberately small: it exists so an adapter can fill in
//! [`crate::ingest::Document::headings`] without hand-parsing Markdown
//! itself. The chunker in `crate::chunk` runs its own, more careful
//! heading-and-fence scan when it splits a document's body; the two scans
//! agree on what counts as a heading and what counts as a fence, but they
//! serve different callers and neither depends on the other.

use crate::ingest::document::Heading;

/// Returns `true` when `line`, once its leading spaces are removed, opens
/// or closes a fenced code block.
///
/// `CommonMark` allows up to three leading spaces before a fence marker and
/// requires at least three backticks or three tildes.
fn is_fence_marker(line: &str) -> Option<char> {
    let trimmed = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let mut chars = trimmed.chars();
    let first = chars.next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = 1 + trimmed.chars().skip(1).take_while(|&c| c == first).count();
    if run >= 3 { Some(first) } else { None }
}

/// Scans `body` for ATX headings (`#` through `######`) that fall outside a
/// fenced code block, in document order.
#[must_use]
pub fn extract_headings(body: &str) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut fence_char: Option<char> = None;

    for line in body.lines() {
        if let Some(marker) = is_fence_marker(line) {
            match fence_char {
                Some(open) if open == marker => fence_char = None,
                Some(_) => {}
                None => fence_char = Some(marker),
            }
            continue;
        }
        if fence_char.is_some() {
            continue;
        }
        if let Some(heading) = parse_atx_heading(line) {
            headings.push(heading);
        }
    }
    headings
}

/// Parses one line as an ATX heading, returning `None` when it is not one.
fn parse_atx_heading(line: &str) -> Option<Heading> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    // A `#` heading needs a space (or end of line) after the hashes; `#foo`
    // is a paragraph that starts with a literal hash, not a heading.
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some(Heading::new(
        u8::try_from(level).expect("level is 1..=6, checked above"),
        text,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_headings_of_every_level() {
        let body = "# One\n## Two\n### Three\n";
        let headings = extract_headings(body);
        assert_eq!(
            headings,
            vec![
                Heading::new(1, "One"),
                Heading::new(2, "Two"),
                Heading::new(3, "Three"),
            ]
        );
    }

    #[test]
    fn ignores_a_hash_comment_inside_a_fenced_code_block() {
        let body = "# Real heading\n```rust\n# not a heading\nfn main() {}\n```\n## Also real\n";
        let headings = extract_headings(body);
        assert_eq!(
            headings,
            vec![
                Heading::new(1, "Real heading"),
                Heading::new(2, "Also real")
            ]
        );
    }

    #[test]
    fn ignores_a_line_that_starts_with_hash_but_has_no_space() {
        assert!(extract_headings("#no-space-here\n").is_empty());
    }

    #[test]
    fn strips_a_closing_hash_run() {
        let headings = extract_headings("## Title ##\n");
        assert_eq!(headings, vec![Heading::new(2, "Title")]);
    }

    #[test]
    fn a_tilde_fence_also_hides_a_hash_comment() {
        let body = "~~~\n# not a heading\n~~~\n# real\n";
        assert_eq!(extract_headings(body), vec![Heading::new(1, "real")]);
    }
}
