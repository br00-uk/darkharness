//! The `chunk` stage: task unit `G3`.
//!
//! [`chunk_document`] splits one [`Document`] into [`Chunk`] values with the
//! `heading-v1` algorithm ([`algorithm`]). It targets [`TARGET_TOKENS`]
//! tokens, never emits a chunk over [`MAX_TOKENS`] except a fenced code
//! block that alone exceeds it (marked [`Chunk::oversize`]), and merges a
//! chunk under [`MIN_TOKENS`] into a sibling. [`markdown`] and [`id`] hold
//! the two pieces of machinery `algorithm` builds on: telling headings and
//! fences apart in Markdown text, and computing a chunk's identifier.
//!
//! ## Token counting, `&dyn Engine`, and a second Rule-17 tension
//!
//! Do 10 asks for token counts from [`dark_contract::Engine::tokenize`],
//! and the task brief for this task unit asks for the engine as `&dyn
//! Engine`, matching Rule 17 (every crate but `dark-engine` and `dark-cli`
//! holds the engine as a trait object, never a concrete type). Both are
//! satisfied here: [`chunk_document`] takes `engine: &dyn
//! dark_contract::Engine` and counts through [`EngineCounter`], a thin
//! wrapper that calls `Engine::tokenize`.
//!
//! Testing that path ran into a wall this task unit could not build past
//! without an edit its brief forbade. `Engine::stream` takes a
//! `tokio_util::sync::CancellationToken` — a type `dark-contract` uses in
//! its public signature but never re-exports. Writing any concrete
//! `impl Engine for …`, even a test fixture whose `stream` body never
//! runs, means naming that type, which needs `tokio-util` as a direct
//! dependency of the implementing crate; Rust's extern prelude does not
//! reach through an indirect dependency. `dark-lexicon`'s `Cargo.toml`
//! does not list `tokio-util`, and this task unit's brief says explicitly
//! not to edit any `Cargo.toml` or add a dependency. So no code in this
//! crate, test or otherwise, can implement `Engine` — not without the
//! very edit the brief rules out.
//!
//! [`TokenCounter`] is the seam that resolves this, on the same pattern
//! `crate::ingest::fetch` uses for the Rule 13/16 tension: a small,
//! local, single-method trait that the production path satisfies over the
//! real dependency (here, [`EngineCounter`] over `&dyn Engine`) and a test
//! satisfies with a trivial fixture that needs nothing beyond this crate's
//! existing dependencies. [`chunk_document`]'s public signature still
//! takes `&dyn Engine`, literally, as the task unit asks; the generic
//! `chunk_with_counter` underneath it is what every test in this crate —
//! including `tests/fence_integrity.rs`, which sees only the crate's
//! public API — actually calls.

pub mod algorithm;
pub mod id;
pub mod markdown;

use dark_contract::{Engine, Result, RoleClass};
use serde::{Deserialize, Serialize};

use crate::ingest::Document;

/// The name of this chunking algorithm, recorded in a pack manifest's
/// `[ingest] chunker` field.
pub const ALGORITHM: &str = "heading-v1";

/// The token count `heading-v1` aims for.
pub const TARGET_TOKENS: usize = 512;

/// The token count `heading-v1` never exceeds, except for one oversized
/// block it refuses to split further.
pub const MAX_TOKENS: usize = 900;

/// The token count under which `heading-v1` merges a chunk into a sibling.
pub const MIN_TOKENS: usize = 80;

/// One chunk of a document, ready to embed and index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    /// `blake3(pack_id ‖ breadcrumb ‖ ordinal)`, as lowercase hexadecimal.
    /// See [`id::compute`].
    pub chunk_id: String,
    /// This chunk's position in the document, after every merge, starting
    /// at 0.
    pub ordinal: u32,
    /// The ancestor chain, library name first, for example `tokio ›
    /// runtime › Builder › worker_threads`.
    pub breadcrumb: String,
    /// The source URL with an anchor for the chunk's heading, when the
    /// document has a URL and the chunk has a heading to anchor to.
    pub url: Option<String>,
    /// The chunk's own Markdown text, headings above it excluded.
    pub body: String,
    /// `breadcrumb`, then `body`: the text a caller embeds. Do 8 asks for
    /// the breadcrumb at the start, since it carries most of a
    /// documentation chunk's retrievable signal.
    pub embed_text: String,
    /// The token count of `body`, as [`TokenCounter::count`] measured it.
    pub tokens: usize,
    /// Set when this chunk is a single block — in practice, a fenced code
    /// block — that alone exceeds [`MAX_TOKENS`], which `heading-v1`
    /// leaves whole rather than split.
    pub oversize: bool,
}

/// Counts the tokens in one piece of text.
///
/// Production code counts through [`EngineCounter`], which calls
/// [`dark_contract::Engine::tokenize`]. See the module docs for why this
/// trait exists rather than every caller taking `&dyn Engine` directly.
pub trait TokenCounter {
    /// Returns the token count of `text`.
    ///
    /// # Errors
    ///
    /// Returns whatever the underlying counter returns; for
    /// [`EngineCounter`], whatever [`dark_contract::Engine::tokenize`]
    /// returns.
    fn count(&self, text: &str) -> Result<usize>;
}

/// A [`TokenCounter`] that counts through [`dark_contract::Engine::tokenize`].
pub struct EngineCounter<'a> {
    engine: &'a dyn Engine,
    class: RoleClass,
}

impl<'a> EngineCounter<'a> {
    /// Creates a counter that tokenizes for `class`'s model.
    #[must_use]
    pub fn new(engine: &'a dyn Engine, class: RoleClass) -> Self {
        Self { engine, class }
    }
}

impl TokenCounter for EngineCounter<'_> {
    fn count(&self, text: &str) -> Result<usize> {
        self.engine.tokenize(self.class, text)
    }
}

/// Splits `doc` into chunks with the `heading-v1` algorithm, counting
/// tokens through `engine`.
///
/// `pack_id` is the pack this document belongs to, for example
/// `tokio@1.47.0`; see [`id::compute`] and the module docs for how it
/// enters the breadcrumb and the chunk identifier. `class` selects which
/// model's tokenizer counts the tokens — ordinarily
/// [`RoleClass::Embed`](dark_contract::RoleClass::Embed), the model that
/// will actually embed these chunks.
///
/// # Errors
///
/// Returns whatever `engine.tokenize` returns, typically because no
/// tokenizer is loaded for `class`.
pub fn chunk_document(
    engine: &dyn Engine,
    class: RoleClass,
    pack_id: &str,
    doc: &Document,
) -> Result<Vec<Chunk>> {
    let counter = EngineCounter::new(engine, class);
    chunk_with_counter(&counter, pack_id, doc)
}

/// Splits `doc` into chunks with the `heading-v1` algorithm, counting
/// tokens through `counter`.
///
/// This is what [`chunk_document`] calls after wrapping `&dyn Engine` in
/// an [`EngineCounter`]. It is `pub` so a test — including one in
/// `tests/`, which sees only this crate's public API — can exercise the
/// algorithm with a trivial fixture counter, without building a concrete
/// [`dark_contract::Engine`]. See the module docs.
///
/// # Errors
///
/// Returns whatever `counter.count` returns.
pub fn chunk_with_counter(
    counter: &dyn TokenCounter,
    pack_id: &str,
    doc: &Document,
) -> Result<Vec<Chunk>> {
    algorithm::run(counter, pack_id, doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Heading;

    /// Counts tokens as whitespace-separated words. `heading-v1`'s own
    /// logic does not care what a "token" is, only that the count is
    /// deterministic and monotonic in text length, both of which this
    /// gives without needing a real tokenizer.
    struct WordCounter;
    impl TokenCounter for WordCounter {
        fn count(&self, text: &str) -> Result<usize> {
            Ok(text.split_whitespace().count())
        }
    }

    fn doc(title: &str, body: &str) -> Document {
        Document::new("path.md", title, body)
    }

    #[test]
    fn a_short_document_becomes_one_chunk_with_the_breadcrumb_prefixed() {
        let document = doc("ExampleLib", "Just a little text, nothing fancy.");
        let chunks = chunk_with_counter(&WordCounter, "examplelib@1.0.0", &document).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].breadcrumb, "examplelib › ExampleLib");
        assert!(chunks[0].embed_text.starts_with("examplelib › ExampleLib"));
        assert!(chunks[0].embed_text.contains("Just a little text"));
        assert_eq!(chunks[0].ordinal, 0);
        assert!(!chunks[0].oversize);
    }

    #[test]
    fn headings_produce_one_chunk_per_leaf_section_with_a_nested_breadcrumb() {
        let body = "# Runtime\nintro text that is not tiny at all, several words long here\n\
                     ## Builder\nbuilder text that is also not tiny, several words long here\n";
        let document = doc("tokio", body);
        let chunks = chunk_with_counter(&WordCounter, "tokio@1.47.0", &document).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].breadcrumb, "tokio › tokio › Runtime");
        assert_eq!(chunks[1].breadcrumb, "tokio › tokio › Runtime › Builder");
    }

    #[test]
    fn a_chunk_below_the_minimum_merges_into_its_next_sibling() {
        let long_para = "word ".repeat(90);
        let body = format!("# Section\n## One\ntiny\n## Two\n{long_para}\n");
        let document = doc("Lib", &body);
        let chunks = chunk_with_counter(&WordCounter, "lib@1.0.0", &document).unwrap();
        // "One" (4 tokens) is below MIN_TOKENS and has no content of its
        // own worth keeping separate; it merges into "Two".
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].body.contains("tiny"));
        assert!(chunks[0].tokens >= MIN_TOKENS);
    }

    #[test]
    fn a_fenced_code_block_over_the_maximum_becomes_one_oversize_chunk() {
        let big_code = format!("```text\n{}\n```\n", "line\n".repeat(950));
        let body = format!("# Section\nintro\n{big_code}");
        let document = doc("Lib", &body);
        let chunks = chunk_with_counter(&WordCounter, "lib@1.0.0", &document).unwrap();
        let oversize_chunk = chunks
            .iter()
            .find(|c| c.oversize)
            .expect("an oversize chunk");
        assert!(oversize_chunk.body.contains("```"));
        assert!(oversize_chunk.tokens > MAX_TOKENS);
    }

    #[test]
    fn the_same_document_always_produces_the_same_chunk_ids() {
        let document = doc(
            "ExampleLib",
            "# A\nfirst section text here\n## B\nsecond section text here\n",
        );
        let a = chunk_with_counter(&WordCounter, "examplelib@1.0.0", &document).unwrap();
        let b = chunk_with_counter(&WordCounter, "examplelib@1.0.0", &document).unwrap();
        let ids_a: Vec<&str> = a.iter().map(|c| c.chunk_id.as_str()).collect();
        let ids_b: Vec<&str> = b.iter().map(|c| c.chunk_id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
    }

    #[test]
    fn a_document_with_a_url_gets_an_anchored_chunk_url() {
        let document = Document::new("path.md", "Lib", "# Heading One\ntext here\n")
            .with_headings(vec![Heading::new(1, "Heading One")])
            .with_url("https://example.com/docs");
        let chunks = chunk_with_counter(&WordCounter, "lib@1.0.0", &document).unwrap();
        assert_eq!(
            chunks[0].url.as_deref(),
            Some("https://example.com/docs#heading-one")
        );
    }
}
