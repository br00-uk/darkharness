//! The `manpage` adapter: manual pages.
//!
//! This adapter reads the plain-text rendering of a man page — the output
//! of `man <page> | col -bx`, with backspace-overstrike formatting already
//! stripped — not raw troff/mdoc source. Running `man` is a subprocess
//! call, which belongs to `dark-airlock`'s child-process seam (Rule 13
//! reasoning applies to subprocesses the same way it applies to sockets:
//! `dark-lexicon` is not the crate that launches one), so a caller
//! produces that plain text and hands it to this adapter.
//!
//! A traditional man page marks each section with an unindented, upper
//! case heading — `NAME`, `SYNOPSIS`, `DESCRIPTION`, and so on — followed
//! by indented body text. This adapter turns each such line into a
//! Markdown `##` heading, so `crate::chunk`'s heading-based splitter finds
//! the same structure a person sees in the terminal.

use crate::ingest::document::{Document, Heading};

/// Returns `true` when `line` looks like a man page section heading: no
/// leading whitespace, and at least one upper-case ASCII letter, with no
/// lower-case ASCII letters.
fn is_section_heading(line: &str) -> bool {
    if line.is_empty() || line.starts_with(char::is_whitespace) {
        return false;
    }
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return false;
    }
    let has_upper = trimmed.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = trimmed.chars().any(|c| c.is_ascii_lowercase());
    has_upper && !has_lower
}

/// Parses a rendered man page into one [`Document`].
///
/// `name` becomes the document's `path` and its fallback title; the title
/// prefers the first line of the `NAME` section when one is present, since
/// that line conventionally reads `name - one-line description`.
#[must_use]
pub fn parse(name: &str, rendered_text: &str) -> Document {
    let mut markdown = String::new();
    let mut headings = Vec::new();
    let mut name_section_first_line: Option<String> = None;
    let mut in_name_section = false;

    for line in rendered_text.lines() {
        if is_section_heading(line) {
            let text = line.trim().to_owned();
            headings.push(Heading::new(2, text.clone()));
            markdown.push_str("## ");
            markdown.push_str(&text);
            markdown.push('\n');
            in_name_section = text.eq_ignore_ascii_case("NAME");
            continue;
        }
        if in_name_section && name_section_first_line.is_none() && !line.trim().is_empty() {
            name_section_first_line = Some(line.trim().to_owned());
        }
        markdown.push_str(line);
        markdown.push('\n');
    }

    let title = name_section_first_line.unwrap_or_else(|| name.to_owned());
    Document::new(name, title, markdown).with_headings(headings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/manpage/ls.1.txt");

    #[test]
    fn parses_sections_into_markdown_headings() {
        let doc = parse("ls(1)", FIXTURE);
        assert!(doc.headings.iter().any(|h| h.text == "NAME"));
        assert!(doc.headings.iter().any(|h| h.text == "SYNOPSIS"));
        assert!(doc.headings.iter().any(|h| h.text == "DESCRIPTION"));
        assert!(doc.body.contains("## NAME"));
    }

    #[test]
    fn titles_from_the_name_section_first_line() {
        let doc = parse("ls(1)", FIXTURE);
        assert!(doc.title.starts_with("ls"));
        assert!(doc.title.contains('-'));
    }

    #[test]
    fn falls_back_to_the_given_name_when_there_is_no_name_section() {
        let doc = parse("mystery(1)", "no sections here, just text\n");
        assert_eq!(doc.title, "mystery(1)");
    }

    #[test]
    fn an_indented_all_caps_word_is_not_a_heading() {
        // Indented text inside a section, even if it happens to be
        // upper-case, must not start a new section.
        assert!(!is_section_heading("       ALL CAPS BUT INDENTED"));
    }
}
