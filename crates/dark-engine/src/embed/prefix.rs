//! The query/document prefix that an asymmetric embedding model needs
//! (task unit `B5`, step 3).
//!
//! A wrong prefix halves retrieval quality on an asymmetric model. This
//! module holds the one place that decides which prefix a text gets, so
//! every caller — `embed` and the pack indexer alike — applies the same
//! rule.

use dark_contract::EmbedPurpose;
use serde::{Deserialize, Serialize};

/// The prefix text for each [`EmbedPurpose`].
///
/// The default prefixes follow Qwen3-Embedding's documented instruction
/// format; Appendix C notes that exact prompt text in the build
/// specification is an example, to verify against the loaded model's card.
/// A profile that pins a different embedding model overrides these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixConfig {
    /// Prepended to a search query.
    pub query: String,
    /// Prepended to a document being indexed. Empty by default: Qwen3-Embedding
    /// asks for a prefix only on the query side.
    pub document: String,
}

impl Default for PrefixConfig {
    fn default() -> Self {
        Self {
            query: "Instruct: Given a search query, retrieve relevant passages that answer the \
                    query\nQuery:"
                .to_owned(),
            document: String::new(),
        }
    }
}

impl PrefixConfig {
    /// Returns the prefix for `purpose`.
    #[must_use]
    pub fn for_purpose(&self, purpose: EmbedPurpose) -> &str {
        match purpose {
            EmbedPurpose::Query => &self.query,
            EmbedPurpose::Document => &self.document,
        }
    }
}

/// Prepends the correct prefix to `text` for `purpose`.
///
/// A non-empty prefix gets one space before `text`; an empty prefix (the
/// default for [`EmbedPurpose::Document`]) leaves `text` untouched.
#[must_use]
pub fn apply(text: &str, purpose: EmbedPurpose, config: &PrefixConfig) -> String {
    let prefix = config.for_purpose(purpose);
    if prefix.is_empty() {
        text.to_owned()
    } else {
        format!("{prefix} {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_gets_the_query_prefix() {
        let config = PrefixConfig::default();
        let prefixed = apply("how does eviction work", EmbedPurpose::Query, &config);
        assert!(prefixed.starts_with("Instruct:"));
        assert!(prefixed.ends_with("how does eviction work"));
    }

    #[test]
    fn a_document_keeps_the_default_empty_prefix() {
        let config = PrefixConfig::default();
        let prefixed = apply(
            "the resident set evicts by LRU",
            EmbedPurpose::Document,
            &config,
        );
        assert_eq!(prefixed, "the resident set evicts by LRU");
    }

    #[test]
    fn query_and_document_prefixes_differ_for_the_same_text() {
        // The whole point of task unit B5, step 3: a wrong prefix halves
        // retrieval quality, so the two purposes must never collapse to
        // the same text.
        let config = PrefixConfig::default();
        let text = "the same input text";
        assert_ne!(
            apply(text, EmbedPurpose::Query, &config),
            apply(text, EmbedPurpose::Document, &config)
        );
    }

    #[test]
    fn a_custom_document_prefix_is_applied_too() {
        let config = PrefixConfig {
            query: "Query:".to_owned(),
            document: "Passage:".to_owned(),
        };
        assert_eq!(apply("x", EmbedPurpose::Document, &config), "Passage: x");
    }
}
