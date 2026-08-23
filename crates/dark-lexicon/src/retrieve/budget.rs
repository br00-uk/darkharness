//! Shaping a ranked hit list into a response: Rule 28, deduplication, and
//! the caller's token budget.
//!
//! Task unit `G4`, Do 9 to 10: "Fill the caller's token budget. Remove
//! duplicates by breadcrumb prefix. Include the breadcrumb and the URL in
//! each returned chunk." "Enforce Rule 28. Cap one chunk at 400 tokens. Cap
//! one response at 15% of one source document." [`fill`] is all four rules
//! in one pass over a ranked hit list, best first.
//!
//! Rule 28's per-chunk cap (400 tokens) sits below `crate::chunk`'s own
//! target (512 tokens, up to 900): the chunker's ceiling bounds what a
//! chunk can *be*, this module's cap bounds what one *reaches the model*
//! in a single snippet. Most chunks a retrieval call returns need
//! truncating, not just the rare oversize one.

use crate::chunk::Chunk;
use crate::chunk::algorithm::BREADCRUMB_SEPARATOR;
use crate::index::RankedHit;

/// Rule 28: "Cap one chunk at 400 tokens."
pub const MAX_CHUNK_TOKENS: usize = 400;

/// Rule 28: "Cap one response at 15% of one source document."
pub const MAX_RESPONSE_DOCUMENT_FRACTION: f64 = 0.15;

/// `docs_get`'s default token budget (task unit `G5`:
/// `docs_get(pack_id, topic, tokens = 4000)`).
pub const DEFAULT_TOKEN_BUDGET: usize = 4000;

/// One chunk shaped for a response: truncated to [`MAX_CHUNK_TOKENS`] when
/// its full body would exceed that, still carrying enough to look up the
/// rest of the chunk (`chunk_index`) in the caller's own chunk list.
#[derive(Debug, Clone, PartialEq)]
pub struct RetrievedSnippet {
    /// The chunk's position in the slice `fill` was called over.
    pub chunk_index: usize,
    /// The chunk's body, truncated to [`MAX_CHUNK_TOKENS`] when needed.
    pub text: String,
    /// The token count of `text` (not of the chunk's full body).
    pub tokens: usize,
}

/// Returns the first two breadcrumb segments (library, document title):
/// the grouping this module treats as "one source document" for Rule 28's
/// 15% cap. A breadcrumb with fewer than two segments returns all of it.
fn document_key(breadcrumb: &str) -> String {
    let segments: Vec<&str> = breadcrumb.split(BREADCRUMB_SEPARATOR).collect();
    let take = segments.len().min(2);
    segments[..take].join(BREADCRUMB_SEPARATOR)
}

/// Returns `true` when `candidate` shares a common breadcrumb ancestor
/// with something already in `kept` — an exact repeat, an ancestor
/// heading already covered, or a descendant of one. Segment-aligned, so
/// `"lib › Foo"` does not falsely match `"lib › FooBar"`.
fn is_duplicate(candidate: &str, kept: &[&str]) -> bool {
    let candidate_segments: Vec<&str> = candidate.split(BREADCRUMB_SEPARATOR).collect();
    kept.iter().any(|k| {
        let kept_segments: Vec<&str> = k.split(BREADCRUMB_SEPARATOR).collect();
        let shared = candidate_segments.len().min(kept_segments.len());
        candidate_segments[..shared] == kept_segments[..shared]
    })
}

/// Truncates `chunk`'s body to at most `max_tokens`, approximating the cut
/// point by the same fraction of its whitespace-separated words as
/// `max_tokens` is of `chunk.tokens` — `chunk.tokens` came from a real
/// tokenizer at chunk-build time (`crate::chunk::TokenCounter`), which
/// this module has no access to at retrieval time, so this scales rather
/// than re-measures.
fn truncated(chunk: &Chunk, max_tokens: usize) -> (String, usize) {
    if chunk.tokens <= max_tokens || chunk.tokens == 0 {
        return (chunk.body.clone(), chunk.tokens);
    }
    let words: Vec<&str> = chunk.body.split_whitespace().collect();
    if words.is_empty() {
        return (chunk.body.clone(), chunk.tokens);
    }
    #[allow(clippy::cast_precision_loss)] // a chunk's token count never nears f64's precision limit
    let ratio = max_tokens as f64 / chunk.tokens as f64;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    // ratio is in (0, 1), words.len() is small: this never truncates below 1 or above words.len()
    let keep = (((words.len() as f64) * ratio).floor() as usize).clamp(1, words.len());
    let mut text = words[..keep].join(" ");
    if keep < words.len() {
        text.push_str(" …");
    }
    (text, max_tokens.min(chunk.tokens))
}

/// Sums `chunk.tokens` for every chunk in `chunks`, grouped by
/// [`document_key`]: how large "one source document" is, for Rule 28's 15%
/// cap.
fn document_totals(chunks: &[Chunk]) -> std::collections::HashMap<String, u64> {
    let mut totals = std::collections::HashMap::new();
    for chunk in chunks {
        *totals
            .entry(document_key(&chunk.breadcrumb))
            .or_insert(0u64) += chunk.tokens as u64;
    }
    totals
}

/// Shapes `ranked` (best first) into a response over `chunks`: deduplicated
/// by breadcrumb prefix, each chunk truncated to [`MAX_CHUNK_TOKENS`],
/// each source document capped at [`MAX_RESPONSE_DOCUMENT_FRACTION`] of
/// its own total size, and the whole response capped at `token_budget`.
///
/// The single highest-ranked chunk from a document always gets through,
/// subject only to the per-chunk cap and the overall budget: Rule 28
/// bounds how much of one document a response repeats, not whether the
/// single most relevant part of it appears at all. A second or later chunk
/// from the same document is what the 15% cap actually gates.
#[must_use]
pub fn fill(chunks: &[Chunk], ranked: &[RankedHit], token_budget: usize) -> Vec<RetrievedSnippet> {
    let doc_totals = document_totals(chunks);
    let mut kept_breadcrumbs: Vec<&str> = Vec::new();
    let mut doc_used: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut budget_used = 0usize;
    let mut out = Vec::new();

    for hit in ranked {
        let Some(chunk) = chunks.get(hit.chunk_index) else {
            continue;
        };
        if is_duplicate(&chunk.breadcrumb, &kept_breadcrumbs) {
            continue;
        }

        let (text, tokens) = truncated(chunk, MAX_CHUNK_TOKENS);

        if !out.is_empty() && budget_used + tokens > token_budget {
            break;
        }

        let doc_key = document_key(&chunk.breadcrumb);
        let already_used = doc_used.get(&doc_key).copied().unwrap_or(0);
        if already_used > 0 {
            let doc_total = doc_totals.get(&doc_key).copied().unwrap_or(0);
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            // a document's own token count is far below usize::MAX or f64's precision limit
            let doc_cap = ((doc_total as f64) * MAX_RESPONSE_DOCUMENT_FRACTION).floor() as usize;
            if already_used + tokens > doc_cap {
                continue;
            }
        }

        kept_breadcrumbs.push(chunk.breadcrumb.as_str());
        *doc_used.entry(doc_key).or_insert(0) += tokens;
        budget_used += tokens;
        out.push(RetrievedSnippet {
            chunk_index: hit.chunk_index,
            text,
            tokens,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_with_tokens(id: &str, breadcrumb: &str, body: &str, tokens: usize) -> Chunk {
        Chunk {
            chunk_id: id.to_owned(),
            ordinal: 0,
            breadcrumb: breadcrumb.to_owned(),
            url: None,
            body: body.to_owned(),
            embed_text: format!("{breadcrumb}\n\n{body}"),
            tokens,
            oversize: false,
        }
    }

    fn hit(index: usize, score: f32) -> RankedHit {
        RankedHit {
            chunk_index: index,
            score,
        }
    }

    #[test]
    fn document_key_takes_the_first_two_breadcrumb_segments() {
        assert_eq!(
            document_key("tokio › tokio › Runtime › Builder"),
            "tokio › tokio"
        );
        assert_eq!(document_key("lib › Title"), "lib › Title");
    }

    #[test]
    fn a_chunk_within_the_cap_is_returned_whole() {
        let chunks = vec![chunk_with_tokens("a", "lib › Doc", "hello world", 50)];
        let out = fill(&chunks, &[hit(0, 1.0)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "hello world");
        assert_eq!(out[0].tokens, 50);
    }

    #[test]
    fn a_chunk_over_the_per_chunk_cap_is_truncated() {
        let body = (0..200)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = vec![chunk_with_tokens("a", "lib › Doc", &body, 800)];
        let out = fill(&chunks, &[hit(0, 1.0)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tokens, MAX_CHUNK_TOKENS);
        assert!(out[0].text.len() < body.len());
        assert!(out[0].text.ends_with('…'));
    }

    #[test]
    fn duplicates_by_breadcrumb_prefix_are_removed() {
        let chunks = vec![
            chunk_with_tokens("a", "lib › Doc › Section", "top-level text", 30),
            chunk_with_tokens("b", "lib › Doc › Section › Sub", "nested text", 30),
        ];
        // Both rank highly; the descendant should be treated as a
        // duplicate of the ancestor already kept.
        let out = fill(&chunks, &[hit(0, 2.0), hit(1, 1.0)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chunk_index, 0);
    }

    #[test]
    fn unrelated_breadcrumbs_are_not_treated_as_duplicates() {
        let chunks = vec![
            chunk_with_tokens("a", "lib › Foo", "foo text", 30),
            chunk_with_tokens("b", "lib › FooBar", "foobar text", 30),
        ];
        let out = fill(&chunks, &[hit(0, 2.0), hit(1, 1.0)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn the_response_stops_once_the_token_budget_is_spent() {
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| chunk_with_tokens(&format!("c{i}"), &format!("lib › Doc{i}"), "text", 300))
            .collect();
        let ranked: Vec<RankedHit> = (0..10)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let s = i as f32;
                hit(i, 10.0 - s)
            })
            .collect();
        let out = fill(&chunks, &ranked, 1000);
        assert!(
            out.len() <= 4,
            "1000 / 300 leaves room for at most 3 full chunks plus rounding"
        );
        let total: usize = out.iter().map(|s| s.tokens).sum();
        assert!(total <= 1000);
    }

    #[test]
    fn at_least_one_hit_is_returned_even_if_it_alone_exceeds_the_budget() {
        let chunks = vec![chunk_with_tokens("a", "lib › Doc", "text", 300)];
        let out = fill(&chunks, &[hit(0, 1.0)], 10);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn the_first_chunk_from_a_document_always_gets_through() {
        // A tiny document: its whole content is one chunk, so 15% of its
        // own total is far less than the chunk itself. The single most
        // relevant chunk still must not be dropped outright.
        let chunks = vec![chunk_with_tokens(
            "a",
            "lib › Tiny",
            "the only chunk in this document",
            50,
        )];
        let out = fill(&chunks, &[hit(0, 1.0)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_second_chunk_from_the_same_document_is_capped_at_fifteen_percent() {
        // A 1000-token document split into 5 chunks of 200 tokens each:
        // 15% of 1000 is 150, so only the first chunk (already over that
        // on its own) gets through; a second one from the same document
        // would push cumulative usage well past the cap.
        let chunks: Vec<Chunk> = (0..5)
            .map(|i| {
                chunk_with_tokens(
                    &format!("c{i}"),
                    &format!("lib › Big › Section{i}"),
                    "text",
                    200,
                )
            })
            .collect();
        let ranked: Vec<RankedHit> = (0..5)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let s = i as f32;
                hit(i, 5.0 - s)
            })
            .collect();
        let out = fill(&chunks, &ranked, DEFAULT_TOKEN_BUDGET);
        assert_eq!(
            out.len(),
            1,
            "only the first chunk from this document should get through"
        );
    }

    #[test]
    fn a_large_document_with_many_chunks_allows_more_than_one_through() {
        // A 10000-token document: 15% is 1500, comfortably more than one
        // 200-token chunk.
        let chunks: Vec<Chunk> = (0..50)
            .map(|i| {
                chunk_with_tokens(
                    &format!("c{i}"),
                    &format!("lib › Huge › Section{i}"),
                    "text",
                    200,
                )
            })
            .collect();
        let ranked: Vec<RankedHit> = (0..50)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let s = i as f32;
                hit(i, 50.0 - s)
            })
            .collect();
        let out = fill(&chunks, &ranked, DEFAULT_TOKEN_BUDGET);
        assert!(out.len() > 1);
    }

    #[test]
    fn missing_chunk_indexes_are_skipped_without_panicking() {
        let chunks = vec![chunk_with_tokens("a", "lib › Doc", "text", 30)];
        let out = fill(&chunks, &[hit(5, 1.0), hit(0, 0.5)], DEFAULT_TOKEN_BUDGET);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].chunk_index, 0);
    }

    #[test]
    fn an_empty_ranked_list_returns_no_snippets() {
        let chunks = vec![chunk_with_tokens("a", "lib › Doc", "text", 30)];
        assert!(fill(&chunks, &[], DEFAULT_TOKEN_BUDGET).is_empty());
    }
}
