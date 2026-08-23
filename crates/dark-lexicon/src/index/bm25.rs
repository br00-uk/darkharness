//! The lexical index: BM25 over a pack's chunks.
//!
//! Task unit `G4`, Do 1 to 4. [`tokenize`] splits text for code and prose in
//! one pass ([`tokenize`]'s own docs cover the rules). [`Bm25Index`] builds
//! the inverted index over [`Chunk::embed_text`](crate::chunk::Chunk) —
//! breadcrumb and body together, the same text a caller embeds, so a query
//! for a term that appears only in a heading (`worker_threads`) still finds
//! the chunk beneath it — and answers a query with Okapi BM25 scores, `k1 =
//! 1.2` and `b = 0.75` by default ([`K1`], [`B`]).
//!
//! `crate::pack::BM25_INDEX_FILE_NAME` (`bm25.idx`) stores the encoded form
//! that [`Bm25Index::to_bytes`] produces and [`Bm25Index::from_bytes`]
//! reads back: delta-encoded, variable-length-integer postings, per Do 4.
//! The lexical index is the fallback path (no embedding model needed), so
//! this module reaches for nothing beyond this crate's existing
//! dependencies — no stemming or tokenizing crate, Rule 16 leaves no room
//! for one.

use std::collections::BTreeMap;

use dark_contract::{ErrCode, Error, Result};

use crate::index::RankedHit;
use crate::index::codec::{read_varint, write_varint};

/// Term-frequency saturation. G4 Do 1 fixes this at 1.2.
pub const K1: f32 = 1.2;
/// Length normalisation. G4 Do 1 fixes this at 0.75.
pub const B: f32 = 0.75;

/// The 4-byte tag at the start of an encoded [`Bm25Index`].
const MAGIC: &[u8; 4] = b"BM25";
/// The encoding version. Bump this when the byte layout changes.
const FORMAT_VERSION: u8 = 1;

/// Splits `text` into lowercase search tokens.
///
/// A run of letters, digits, or underscores is one word. G4 Do 2 to 3 set
/// three rules for turning a word into tokens:
///
/// - **Split camel case and snake case, and keep the original token.**
///   `worker_threads` tokenizes to `worker_threads`, `worker`, and
///   `threads`, so a query for the whole identifier and a query for either
///   half both match. A word counts as an identifier when it mixes case
///   (`workerThreads`) or carries an underscore (`worker_threads`); a plain
///   word like `Runtime` does not split further, it only lowercases.
/// - **Do not stem an identifier.** The pieces above are lowercased, never
///   stemmed: `Builder` stays `builder`, not `build`.
/// - **Apply light stemming to prose.** A plain word — no underscore, no
///   internal case change — loses a short list of common suffixes
///   ([`light_stem`]), so "building" and "builds" both index under
///   "build".
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in split_words(text) {
        push_tokens_for_word(raw, &mut tokens);
    }
    tokens
}

/// Splits `text` on any character that is not a letter, a digit, or an
/// underscore.
fn split_words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
}

/// Returns `true` when `word` looks like a code identifier rather than a
/// prose word: it carries an underscore, or it mixes upper and lower case.
fn is_identifier_like(word: &str) -> bool {
    word.contains('_')
        || (word.chars().any(char::is_lowercase) && word.chars().any(char::is_uppercase))
}

/// Pushes every token that `raw` produces onto `out`.
fn push_tokens_for_word(raw: &str, out: &mut Vec<String>) {
    let lower = raw.to_lowercase();
    if is_identifier_like(raw) {
        out.push(lower.clone());
        for part in split_identifier(raw) {
            let part_lower = part.to_lowercase();
            if part_lower != lower {
                out.push(part_lower);
            }
        }
    } else {
        out.push(light_stem(&lower));
    }
}

/// Splits an identifier-like word on underscore boundaries, then on camel
/// case boundaries within each piece.
fn split_identifier(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in raw.split('_') {
        if piece.is_empty() {
            continue;
        }
        parts.extend(split_camel_case(piece));
    }
    parts
}

/// Splits one underscore-free piece on camel case boundaries: before an
/// upper-case letter that follows a lower-case one (`workerThreads` →
/// `worker`, `Threads`), and before the last of a run of upper-case
/// letters when a lower-case letter follows (`HTTPServer` → `HTTP`,
/// `Server`).
fn split_camel_case(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 {
            let prev = chars[i - 1];
            let next = chars.get(i + 1).copied();
            let lower_to_upper = prev.is_lowercase() && c.is_uppercase();
            let end_of_acronym =
                prev.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase);
            if (lower_to_upper || end_of_acronym) && !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        }
        current.push(c);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// The suffixes that [`light_stem`] strips, longest and most specific
/// first so a word matches at most one of them.
const LIGHT_STEM_SUFFIXES: &[&str] = &[
    "ational", "ization", "ing", "edly", "ies", "ed", "es", "ly", "s",
];

/// Strips one common suffix from `word`, when doing so leaves at least
/// three characters.
///
/// This is deliberately not a full stemmer (no Porter algorithm, Rule 16
/// leaves no room for the crate that would carry one): G4 Do 3 asks for
/// "light" stemming, not a correct one, so a handful of suffix rules that
/// unify the common cases (plurals, `-ing`, `-ed`) is what this provides.
#[must_use]
pub fn light_stem(word: &str) -> String {
    if word.chars().count() <= 3 {
        return word.to_owned();
    }
    for suffix in LIGHT_STEM_SUFFIXES {
        if let Some(stem) = word.strip_suffix(suffix) {
            if stem.chars().count() >= 3 {
                return stem.to_owned();
            }
        }
    }
    word.to_owned()
}

/// The BM25 idf term: `ln(1 + (N - df + 0.5) / (df + 0.5))`.
///
/// The `+ 1` inside the logarithm (the Lucene/Robertson-Walker variant)
/// keeps the term positive even when a token appears in more than half the
/// corpus, which a documentation pack's common words (`the`, `fn`) often
/// do.
fn idf(doc_count: f32, doc_freq: f32) -> f32 {
    (1.0 + (doc_count - doc_freq + 0.5) / (doc_freq + 0.5)).ln()
}

/// One document's occurrence of one term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Posting {
    /// The chunk's position in the corpus that built this index.
    doc_id: u32,
    /// How many times the term appears in that chunk.
    term_freq: u32,
}

/// A lexical (BM25) index over one pack's chunks.
///
/// Build with [`Bm25Index::build`], search with [`Bm25Index::search`],
/// store with [`Bm25Index::to_bytes`], and load with
/// [`Bm25Index::from_bytes`]. This is the fallback index: G4's "Done when"
/// requires it to answer well with no dense index at all, so
/// `dark-lexicon` builds and queries it with nothing beyond a chunk list —
/// no embedding model, no network.
#[derive(Debug, Clone, PartialEq)]
pub struct Bm25Index {
    k1: f32,
    b: f32,
    doc_count: u32,
    avg_doc_len: f32,
    doc_lengths: Vec<u32>,
    /// Sorted ascending, so [`Bm25Index::search`] can binary-search a
    /// query term.
    terms: Vec<String>,
    /// Parallel to `terms`: `postings[i]` is `terms[i]`'s postings list,
    /// sorted ascending by `doc_id`.
    postings: Vec<Vec<Posting>>,
}

impl Bm25Index {
    /// Builds an index over `chunks` at the default `k1` and `b` (G4 Do 1).
    #[must_use]
    pub fn build(chunks: &[crate::chunk::Chunk]) -> Self {
        Self::build_with_params(chunks, K1, B)
    }

    /// Builds an index over `chunks` at a caller-chosen `k1` and `b`,
    /// mainly for tests that probe the scoring formula directly.
    #[must_use]
    pub fn build_with_params(chunks: &[crate::chunk::Chunk], k1: f32, b: f32) -> Self {
        let mut doc_lengths = Vec::with_capacity(chunks.len());
        let mut term_map: BTreeMap<String, Vec<Posting>> = BTreeMap::new();

        for (index, chunk) in chunks.iter().enumerate() {
            let doc_id = u32::try_from(index).unwrap_or(u32::MAX);
            let tokens = tokenize(&chunk.embed_text);
            doc_lengths.push(u32::try_from(tokens.len()).unwrap_or(u32::MAX));

            let mut freqs: BTreeMap<String, u32> = BTreeMap::new();
            for token in tokens {
                *freqs.entry(token).or_insert(0) += 1;
            }
            for (term, term_freq) in freqs {
                term_map
                    .entry(term)
                    .or_default()
                    .push(Posting { doc_id, term_freq });
            }
        }

        let doc_count = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
        #[allow(clippy::cast_possible_truncation)]
        // an average document length has no need of f64 precision
        let avg_doc_len = if doc_count == 0 {
            0.0
        } else {
            (doc_lengths.iter().map(|&len| f64::from(len)).sum::<f64>() / f64::from(doc_count))
                as f32
        };

        let (terms, postings) = term_map.into_iter().unzip();

        Self {
            k1,
            b,
            doc_count,
            avg_doc_len,
            doc_lengths,
            terms,
            postings,
        }
    }

    /// The number of chunks this index was built over.
    #[must_use]
    pub fn doc_count(&self) -> usize {
        self.doc_count as usize
    }

    /// Scores every chunk that shares at least one term with `query`, and
    /// returns the `top_k` highest, best first.
    ///
    /// A query term absent from the index (never seen at build time)
    /// contributes nothing — not an error, since a query is free text a
    /// person or a model wrote, not a validated identifier.
    #[must_use]
    pub fn search(&self, query: &str, top_k: usize) -> Vec<RankedHit> {
        let mut scores: BTreeMap<u32, f32> = BTreeMap::new();
        let mut seen_terms: std::collections::HashSet<String> = std::collections::HashSet::new();

        for term in tokenize(query) {
            if !seen_terms.insert(term.clone()) {
                continue; // Do not double-count a repeated query term's idf.
            }
            let Ok(term_index) = self.terms.binary_search(&term) else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)]
            // a postings-list length never nears f32's precision limit
            let doc_freq = self.postings[term_index].len() as f32;
            #[allow(clippy::cast_precision_loss)] // a chunk count never nears f32's precision limit
            let term_idf = idf(self.doc_count as f32, doc_freq);
            for posting in &self.postings[term_index] {
                let doc_len = f32::from(
                    u16::try_from(self.doc_lengths[posting.doc_id as usize]).unwrap_or(u16::MAX),
                );
                #[allow(clippy::cast_precision_loss)]
                // a term frequency within one chunk never nears f32's precision limit
                let tf = posting.term_freq as f32;
                let denom =
                    tf + self.k1 * (1.0 - self.b + self.b * doc_len / self.avg_doc_len.max(1.0));
                let score = term_idf * (tf * (self.k1 + 1.0)) / denom.max(f32::EPSILON);
                *scores.entry(posting.doc_id).or_insert(0.0) += score;
            }
        }

        let mut ranked: Vec<RankedHit> = scores
            .into_iter()
            .map(|(doc_id, score)| RankedHit {
                chunk_index: doc_id as usize,
                score,
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.chunk_index.cmp(&b.chunk_index))
        });
        ranked.truncate(top_k);
        ranked
    }

    /// Encodes this index as bytes: G4 Do 4's delta-encoded,
    /// variable-length-integer postings, ready to write to
    /// `crate::pack::BM25_INDEX_FILE_NAME`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&self.k1.to_le_bytes());
        out.extend_from_slice(&self.b.to_le_bytes());
        write_varint(&mut out, u64::from(self.doc_count));
        out.extend_from_slice(&self.avg_doc_len.to_le_bytes());
        for &len in &self.doc_lengths {
            write_varint(&mut out, u64::from(len));
        }
        write_varint(&mut out, self.terms.len() as u64);
        for (term, postings) in self.terms.iter().zip(&self.postings) {
            let term_bytes = term.as_bytes();
            write_varint(&mut out, term_bytes.len() as u64);
            out.extend_from_slice(term_bytes);
            write_varint(&mut out, postings.len() as u64);
            let mut previous_doc_id: u32 = 0;
            for (i, posting) in postings.iter().enumerate() {
                let delta = if i == 0 {
                    posting.doc_id
                } else {
                    posting.doc_id - previous_doc_id
                };
                write_varint(&mut out, u64::from(delta));
                write_varint(&mut out, u64::from(posting.term_freq));
                previous_doc_id = posting.doc_id;
            }
        }
        out
    }

    /// Decodes an index that [`Bm25Index::to_bytes`] produced.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when `bytes` does not start with the
    /// expected magic and version, or ends before the format says it
    /// should.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut pos = 0usize;
        let magic = bytes.get(0..4).ok_or_else(too_short)?;
        if magic != MAGIC {
            return Err(Error::new(
                ErrCode::ToolFailed,
                "not a BM25 index: bad magic bytes",
            ));
        }
        pos += 4;
        let version = *bytes.get(pos).ok_or_else(too_short)?;
        pos += 1;
        if version != FORMAT_VERSION {
            return Err(Error::new(
                ErrCode::ToolFailed,
                format!(
                    "BM25 index format version {version} is not supported (expected {FORMAT_VERSION})"
                ),
            ));
        }
        let k1 = read_f32(bytes, &mut pos)?;
        let b = read_f32(bytes, &mut pos)?;
        let doc_count = u32::try_from(read_varint(bytes, &mut pos)?)
            .map_err(|_| Error::new(ErrCode::ToolFailed, "BM25 index doc_count overflows u32"))?;
        let avg_doc_len = read_f32(bytes, &mut pos)?;

        let mut doc_lengths = Vec::with_capacity(doc_count as usize);
        for _ in 0..doc_count {
            let len = u32::try_from(read_varint(bytes, &mut pos)?).map_err(|_| {
                Error::new(ErrCode::ToolFailed, "BM25 index doc length overflows u32")
            })?;
            doc_lengths.push(len);
        }

        let term_count = to_usize(read_varint(bytes, &mut pos)?)?;
        let mut terms = Vec::with_capacity(term_count);
        let mut postings = Vec::with_capacity(term_count);
        for _ in 0..term_count {
            let term_len = to_usize(read_varint(bytes, &mut pos)?)?;
            let term_bytes = bytes.get(pos..pos + term_len).ok_or_else(too_short)?;
            pos += term_len;
            let term = String::from_utf8(term_bytes.to_vec()).map_err(|source| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!("BM25 index term is not UTF-8: {source}"),
                )
            })?;

            let posting_count = read_varint(bytes, &mut pos)?;
            let mut term_postings = Vec::with_capacity(to_usize(posting_count)?);
            let mut previous_doc_id: u32 = 0;
            for i in 0..posting_count {
                let delta = read_varint(bytes, &mut pos)?;
                let doc_id = if i == 0 {
                    u32::try_from(delta).map_err(|_| {
                        Error::new(ErrCode::ToolFailed, "BM25 index doc id overflows u32")
                    })?
                } else {
                    previous_doc_id
                        + u32::try_from(delta).map_err(|_| {
                            Error::new(ErrCode::ToolFailed, "BM25 index delta overflows u32")
                        })?
                };
                let term_freq = u32::try_from(read_varint(bytes, &mut pos)?).map_err(|_| {
                    Error::new(
                        ErrCode::ToolFailed,
                        "BM25 index term frequency overflows u32",
                    )
                })?;
                term_postings.push(Posting { doc_id, term_freq });
                previous_doc_id = doc_id;
            }

            terms.push(term);
            postings.push(term_postings);
        }

        Ok(Self {
            k1,
            b,
            doc_count,
            avg_doc_len,
            doc_lengths,
            terms,
            postings,
        })
    }
}

fn too_short() -> Error {
    Error::new(
        ErrCode::ToolFailed,
        "BM25 index bytes end before the format expects",
    )
}

/// Converts a decoded count to `usize`, refusing one that would not fit —
/// possible on a 32-bit target even though the encoder never writes one
/// that large.
fn to_usize(value: u64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| Error::new(ErrCode::ToolFailed, "BM25 index count overflows usize"))
}

fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32> {
    let slice: [u8; 4] = bytes
        .get(*pos..*pos + 4)
        .ok_or_else(too_short)?
        .try_into()
        .unwrap();
    *pos += 4;
    Ok(f32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;

    fn chunk(id: &str, breadcrumb: &str, body: &str) -> Chunk {
        Chunk {
            chunk_id: id.to_owned(),
            ordinal: 0,
            breadcrumb: breadcrumb.to_owned(),
            url: None,
            body: body.to_owned(),
            embed_text: format!("{breadcrumb}\n\n{body}"),
            tokens: body.split_whitespace().count(),
            oversize: false,
        }
    }

    #[test]
    fn tokenize_splits_snake_case_and_keeps_the_original() {
        let tokens = tokenize("worker_threads");
        assert!(tokens.contains(&"worker_threads".to_owned()));
        assert!(tokens.contains(&"worker".to_owned()));
        assert!(tokens.contains(&"threads".to_owned()));
    }

    #[test]
    fn tokenize_splits_camel_case_and_keeps_the_original() {
        let tokens = tokenize("workerThreads");
        assert!(tokens.contains(&"workerthreads".to_owned()));
        assert!(tokens.contains(&"worker".to_owned()));
        assert!(tokens.contains(&"threads".to_owned()));
    }

    #[test]
    fn tokenize_splits_acronym_camel_case() {
        let tokens = tokenize("HTTPServer");
        assert!(tokens.contains(&"http".to_owned()));
        assert!(tokens.contains(&"server".to_owned()));
    }

    #[test]
    fn a_plain_word_does_not_split() {
        let tokens = tokenize("Runtime");
        assert_eq!(tokens, vec!["runtime".to_owned()]);
    }

    #[test]
    fn identifiers_are_not_stemmed() {
        // "builder" as an identifier keeps its "er"; light_stem alone would
        // not touch it either (no suffix matches), but push_tokens_for_word
        // must route identifiers around stemming entirely.
        let tokens = tokenize("my_builder_config");
        assert!(tokens.contains(&"builder".to_owned()));
        assert!(tokens.contains(&"config".to_owned()));
    }

    #[test]
    fn prose_gets_light_stemmed() {
        assert_eq!(light_stem("building"), "build");
        assert_eq!(light_stem("threads"), "thread");
        assert_eq!(light_stem("cats"), "cat");
    }

    #[test]
    fn a_short_word_is_left_alone_by_light_stemming() {
        assert_eq!(light_stem("run"), "run");
        assert_eq!(light_stem("is"), "is");
    }

    #[test]
    fn worker_threads_matches_both_the_identifier_and_the_split_words() {
        let chunks = vec![chunk(
            "a",
            "tokio › runtime › Builder",
            "Configures the worker_threads setting for the runtime.",
        )];
        let index = Bm25Index::build(&chunks);
        assert!(!index.search("worker_threads", 5).is_empty());
        assert!(!index.search("worker threads", 5).is_empty());
    }

    #[test]
    fn search_ranks_the_more_relevant_chunk_first() {
        let chunks = vec![
            chunk(
                "a",
                "tokio › runtime",
                "The runtime schedules async tasks efficiently.",
            ),
            chunk("b", "tokio › fs", "Reads and writes files on disk."),
        ];
        let index = Bm25Index::build(&chunks);
        let hits = index.search("async runtime tasks", 5);
        assert_eq!(hits[0].chunk_index, 0);
    }

    #[test]
    fn a_term_absent_from_the_corpus_returns_no_hits() {
        let chunks = vec![chunk(
            "a",
            "tokio › runtime",
            "The runtime schedules tasks.",
        )];
        let index = Bm25Index::build(&chunks);
        assert!(index.search("xylophone", 5).is_empty());
    }

    #[test]
    fn top_k_caps_the_result_count() {
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| {
                chunk(
                    &format!("c{i}"),
                    "tokio › runtime",
                    "runtime tasks scheduler",
                )
            })
            .collect();
        let index = Bm25Index::build(&chunks);
        assert_eq!(index.search("runtime", 3).len(), 3);
    }

    #[test]
    fn bytes_round_trip_preserves_search_results() {
        let chunks = vec![
            chunk(
                "a",
                "tokio › runtime",
                "The runtime schedules async tasks efficiently.",
            ),
            chunk("b", "tokio › fs", "Reads and writes files on disk."),
            chunk(
                "c",
                "tokio › net",
                "TCP and UDP sockets for async networking.",
            ),
        ];
        let index = Bm25Index::build(&chunks);
        let before = index.search("async tasks", 5);

        let bytes = index.to_bytes();
        let restored = Bm25Index::from_bytes(&bytes).unwrap();
        let after = restored.search("async tasks", 5);

        assert_eq!(before, after);
        assert_eq!(index, restored);
    }

    #[test]
    fn from_bytes_rejects_bad_magic() {
        let err = Bm25Index::from_bytes(b"nope").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn from_bytes_rejects_truncated_bytes() {
        let chunks = vec![chunk("a", "tokio › runtime", "runtime tasks")];
        let index = Bm25Index::build(&chunks);
        let mut bytes = index.to_bytes();
        bytes.truncate(bytes.len() - 2);
        assert!(Bm25Index::from_bytes(&bytes).is_err());
    }

    #[test]
    fn an_empty_index_builds_and_searches_without_panicking() {
        let index = Bm25Index::build(&[]);
        assert_eq!(index.doc_count(), 0);
        assert!(index.search("anything", 5).is_empty());
    }
}
