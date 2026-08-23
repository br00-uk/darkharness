//! The `llms-txt` adapter: `llms.txt` and `llms-full.txt`.
//!
//! This is the preferred adapter (G2 says so directly): the
//! [llms.txt convention](https://llmstxt.org) shapes a page for an agent
//! already, an H1 title, an optional summary, then H2 sections of Markdown
//! links, so this adapter does the least work of the seven. `llms-full.txt`
//! inlines the full content of every linked page under its own heading
//! instead of just linking it; both variants parse the same way here,
//! because both are Markdown text with a heading outline, and the chunker
//! (`crate::chunk`) is what actually cares about that outline, not this
//! adapter.

use crate::ingest::document::Document;
use crate::ingest::markdown::extract_headings;

/// Parses `llms.txt` or `llms-full.txt` text into one [`Document`].
///
/// The whole file becomes one document: its title is the text of the first
/// H1, or `path` when the file has none, and its body is the file
/// unchanged. Splitting into smaller pieces is the chunker's job, not this
/// adapter's; `crate::chunk` already knows how to split on headings and
/// respects the same fence rules that would matter here.
#[must_use]
pub fn parse(path: &str, url: Option<&str>, text: &str) -> Document {
    let headings = extract_headings(text);
    let title = headings
        .iter()
        .find(|h| h.level == 1)
        .map_or_else(|| path.to_owned(), |h| h.text.clone());

    let mut doc = Document::new(path, title, text).with_headings(headings);
    if let Some(url) = url {
        doc = doc.with_url(url);
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../tests/fixtures/llms_txt/llms.txt");

    #[test]
    fn parses_the_fixture_into_one_document() {
        let doc = parse("llms.txt", Some("https://example.com/llms.txt"), FIXTURE);
        assert_eq!(doc.title, "ExampleLib");
        assert_eq!(doc.url.as_deref(), Some("https://example.com/llms.txt"));
        assert!(doc.headings.iter().any(|h| h.text == "Docs"));
        assert!(doc.body.contains("Quick reference"));
    }

    #[test]
    fn falls_back_to_the_path_when_there_is_no_h1() {
        let doc = parse("a/llms.txt", None, "## Docs\n- [x](y)\n");
        assert_eq!(doc.title, "a/llms.txt");
    }
}
