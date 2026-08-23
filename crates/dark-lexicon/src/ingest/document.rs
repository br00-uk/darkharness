//! The common document type that every adapter produces.

use serde::{Deserialize, Serialize};

/// One heading found in a document's body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heading {
    /// The heading level: 1 for an H1, up to 6 for an H6.
    pub level: u8,
    /// The heading text, with Markdown emphasis markers left in place.
    pub text: String,
}

impl Heading {
    /// Creates a heading.
    #[must_use]
    pub fn new(level: u8, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
        }
    }
}

/// One document from a source, in the shape that every adapter produces.
///
/// An adapter converts one source-specific unit (an `llms.txt` file, a
/// rustdoc JSON item, a fetched HTML page, an `OpenAPI` operation, a man
/// page) into one or more `Document` values. The chunker in `crate::chunk`
/// then splits `body` into retrievable pieces; it does not care which
/// adapter produced the document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    /// A stable, source-relative path, for example `runtime/builder.md`.
    ///
    /// Two documents from the same source never share a path. A path uses
    /// forward slashes on every platform.
    pub path: String,
    /// The document title.
    pub title: String,
    /// The headings that appear in `body`, in document order.
    pub headings: Vec<Heading>,
    /// The document body, as Markdown text.
    ///
    /// This is untrusted content: it comes from a fetched file or a parsed
    /// source, not from a person typing into the harness. See Rule 36. A
    /// caller that stores or displays `body` treats it as data, never as
    /// instructions, and never executes anything found inside it.
    pub body: String,
    /// The source URL for this document, when the source is addressable by
    /// URL. A local source, for example `localdir`, leaves this `None`.
    pub url: Option<String>,
}

impl Document {
    /// Creates a document with no headings.
    ///
    /// Most adapters extract headings from `body` after building it; this
    /// constructor exists so a call site can build the common fields first
    /// and fill in headings with [`Self::with_headings`] or by assigning
    /// the field directly.
    #[must_use]
    pub fn new(path: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            title: title.into(),
            headings: Vec::new(),
            body: body.into(),
            url: None,
        }
    }

    /// Sets the headings.
    #[must_use]
    pub fn with_headings(mut self, headings: Vec<Heading>) -> Self {
        self.headings = headings;
        self
    }

    /// Sets the source URL.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_methods_set_the_expected_fields() {
        let doc = Document::new("a.md", "A", "body text")
            .with_headings(vec![Heading::new(1, "A")])
            .with_url("https://example.com/a");
        assert_eq!(doc.path, "a.md");
        assert_eq!(doc.title, "A");
        assert_eq!(doc.headings, vec![Heading::new(1, "A")]);
        assert_eq!(doc.body, "body text");
        assert_eq!(doc.url.as_deref(), Some("https://example.com/a"));
    }

    #[test]
    fn a_fresh_document_has_no_url_and_no_headings() {
        let doc = Document::new("a.md", "A", "body");
        assert!(doc.url.is_none());
        assert!(doc.headings.is_empty());
    }
}
