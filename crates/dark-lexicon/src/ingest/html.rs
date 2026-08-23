//! A minimal, safe HTML-to-text conversion for the `sitemap` adapter.
//!
//! `dark-lexicon` has no HTML parsing dependency (Rule 16 permits only
//! `dark-contract` and this crate's own storage crates, and adding one
//! would be a new dependency this task unit was not given). This module is
//! a small, tolerant tag scanner: enough to pull a title, a heading
//! outline, and readable text out of a documentation page, and, more
//! importantly, enough to guarantee that fetched HTML is never executed.
//! `<script>` and `<style>` element content is dropped outright, never
//! copied into a document body or evaluated. See Rule 36: fetched HTML is
//! untrusted content, handled as data.

use crate::ingest::document::Heading;

/// The result of converting one HTML page to text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extracted {
    /// The `<title>` element's text, when present.
    pub title: Option<String>,
    /// The `<h1>` through `<h6>` headings, in document order.
    pub headings: Vec<Heading>,
    /// The page's visible text, as Markdown-ish plain text: heading tags
    /// become `#`-prefixed lines, so the chunker's heading scan still
    /// finds the same structure it would in a hand-written Markdown file.
    pub body: String,
}

/// Element names whose content must never appear in the output.
const OPAQUE_ELEMENTS: &[&str] = &["script", "style", "noscript", "template"];

/// One parsed start or end tag.
struct Tag {
    name: String,
    closing: bool,
    /// The bytes of `html` this tag occupied, `<` through `>` inclusive.
    span: std::ops::Range<usize>,
}

/// Finds the next tag in `html` at or after byte offset `from`.
fn next_tag(html: &str, from: usize) -> Option<Tag> {
    let open = html[from..].find('<')? + from;
    let after_open = open + 1;
    let close = html[after_open..]
        .find('>')
        .map_or(html.len(), |off| after_open + off);
    let end = (close + 1).min(html.len());
    let raw = &html[after_open..close.min(html.len())];
    let closing = raw.starts_with('/');
    let name = raw
        .trim_start_matches('/')
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    Some(Tag {
        name,
        closing,
        span: open..end,
    })
}

/// Converts one HTML document to [`Extracted`].
///
/// This never executes anything in `html`: it only ever reads text out of
/// it and copies characters into `body`. `<script>` and `<style>` content,
/// tags included, is skipped entirely, not merely left unrendered.
#[must_use]
pub fn extract(html: &str) -> Extracted {
    let mut out = Extracted::default();
    let mut body = String::new();
    let mut title_text = String::new();
    let mut heading_text = String::new();
    let mut current_heading_level: Option<u8> = None;
    let mut in_title = false;
    let mut skip_until: Option<String> = None;

    let mut cursor = 0usize;
    while cursor < html.len() {
        let Some(tag) = next_tag(html, cursor) else {
            push_text(
                &html[cursor..],
                &mut body,
                &mut title_text,
                &mut heading_text,
                in_title,
                current_heading_level,
                skip_until.is_some(),
            );
            break;
        };

        push_text(
            &html[cursor..tag.span.start],
            &mut body,
            &mut title_text,
            &mut heading_text,
            in_title,
            current_heading_level,
            skip_until.is_some(),
        );
        cursor = tag.span.end;

        if let Some(opaque) = &skip_until {
            if tag.closing && &tag.name == opaque {
                skip_until = None;
            }
            continue;
        }
        if !tag.closing && OPAQUE_ELEMENTS.contains(&tag.name.as_str()) {
            skip_until = Some(tag.name);
            continue;
        }

        match tag.name.as_str() {
            "title" => in_title = !tag.closing,
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                if tag.closing {
                    if let Some(level) = current_heading_level.take() {
                        let text = collapse_whitespace(&heading_text);
                        if !text.is_empty() {
                            out.headings.push(Heading::new(level, text.clone()));
                            body.push_str(&"#".repeat(level as usize));
                            body.push(' ');
                            body.push_str(&text);
                            body.push('\n');
                        }
                        heading_text.clear();
                    }
                } else {
                    current_heading_level = tag.name[1..].parse::<u8>().ok();
                    heading_text.clear();
                }
            }
            "br" | "p" | "div" | "li" => {
                if current_heading_level.is_none() && !in_title {
                    body.push('\n');
                }
            }
            _ => {}
        }
    }

    out.title = {
        let text = collapse_whitespace(&title_text);
        if text.is_empty() { None } else { Some(text) }
    };
    out.body = collapse_blank_lines(&body);
    out
}

/// Routes one run of non-tag text to the buffer that the current parser
/// state selects: the title, the current heading, or the body — or
/// nowhere, when a skip is in progress.
#[allow(clippy::fn_params_excessive_bools)]
fn push_text(
    text: &str,
    body: &mut String,
    title_text: &mut String,
    heading_text: &mut String,
    in_title: bool,
    current_heading_level: Option<u8>,
    skipping: bool,
) {
    if skipping || text.is_empty() {
        return;
    }
    if in_title {
        title_text.push_str(text);
    } else if current_heading_level.is_some() {
        heading_text.push_str(text);
    } else {
        body.push_str(text);
    }
}

/// Collapses runs of ASCII whitespace (including newlines) to one space,
/// and trims the ends.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = true;
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    out.trim().to_owned()
}

/// Collapses three or more consecutive newlines to exactly two, and trims
/// blank lines from each side.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut blank_run = 0;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(line.trim());
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_title_headings_and_body_text() {
        let html = "<html><head><title>Example Page</title></head><body>\
                     <h1>Welcome</h1><p>Hello, world.</p>\
                     <h2>Section</h2><p>More text.</p>\
                     </body></html>";
        let extracted = extract(html);
        assert_eq!(extracted.title.as_deref(), Some("Example Page"));
        assert_eq!(
            extracted.headings,
            vec![Heading::new(1, "Welcome"), Heading::new(2, "Section")]
        );
        assert!(extracted.body.contains("# Welcome"));
        assert!(extracted.body.contains("Hello, world."));
        assert!(extracted.body.contains("## Section"));
    }

    #[test]
    fn never_includes_script_content() {
        let html = "<p>before</p><script>alert('should not appear');</script><p>after</p>";
        let extracted = extract(html);
        assert!(!extracted.body.contains("alert"));
        assert!(!extracted.body.contains("should not appear"));
        assert!(extracted.body.contains("before"));
        assert!(extracted.body.contains("after"));
    }

    #[test]
    fn never_includes_style_content() {
        let html = "<style>body { color: red; }</style><p>visible</p>";
        let extracted = extract(html);
        assert!(!extracted.body.contains("color"));
        assert!(extracted.body.contains("visible"));
    }

    #[test]
    fn a_page_with_no_title_produces_none() {
        let extracted = extract("<body><p>hi</p></body>");
        assert!(extracted.title.is_none());
    }

    #[test]
    fn tolerates_an_unterminated_final_tag() {
        // Must not panic or hang.
        let extracted = extract("<p>text<div");
        assert!(extracted.body.contains("text"));
    }

    #[test]
    fn handles_multibyte_text_correctly() {
        let extracted = extract("<h1>caf\u{e9} \u{2603}</h1>");
        assert_eq!(
            extracted.headings,
            vec![Heading::new(1, "caf\u{e9} \u{2603}")]
        );
    }
}
