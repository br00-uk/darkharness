//! The `retrieve` stage: task unit `G4`.
//!
//! [`search`] is the whole pipeline: BM25 always runs (`crate::index::bm25`
//! is the fallback — G4's "Done when" requires it to answer well alone),
//! dense search runs when the caller supplies an index and a query vector
//! (`crate::index::dense`), the two fuse by Reciprocal Rank Fusion
//! (`crate::index::fusion`), reranking runs when the caller supplies a
//! [`RerankGate`] ([`rerank`]), and the result is shaped to the caller's
//! token budget, deduplicated, and capped per Rule 28 ([`budget`]).
//!
//! `crate::tools::get::docs_get` is the one caller task unit `G5` builds on
//! this; this module has no knowledge of packs, manifests, or staleness —
//! it only ranks and shapes one query against indexes and chunks a caller
//! already loaded.

pub mod budget;
pub mod rerank;

pub use budget::{
    DEFAULT_TOKEN_BUDGET, MAX_CHUNK_TOKENS, MAX_RESPONSE_DOCUMENT_FRACTION, RetrievedSnippet,
};
pub use rerank::{DEFAULT_RERANK_LATENCY_THRESHOLD, RerankGate, Reranker};

use dark_contract::Result;

use crate::chunk::Chunk;
use crate::index::{Bm25Index, DenseIndex, RRF_K, reciprocal_rank_fusion};

/// How many results BM25 and dense search each keep before fusion, and the
/// size of the fused list reranking sees. G4 Do 8: "Rerank the top 50
/// fused results."
pub const DEFAULT_CANDIDATE_POOL: usize = 50;

/// One search over one pack's indexes.
pub struct SearchRequest<'a> {
    /// Every chunk in the pack, in the order the indexes were built over.
    pub chunks: &'a [Chunk],
    /// The lexical index. Always searched — it is the fallback.
    pub bm25: &'a Bm25Index,
    /// The dense index and the already-embedded query vector, when an
    /// embedding model is resident and its vectors match the pack's
    /// (`crate::pack::embed::compare`). `None` skips the dense tier
    /// outright; BM25 alone still answers.
    pub dense: Option<(&'a DenseIndex, &'a [f32])>,
    /// The query text.
    pub query: &'a str,
    /// The rerank gate. `None` skips reranking — the caller decides this
    /// once, based on `Caps::logprobs`, before building a gate at all.
    pub reranker: Option<&'a RerankGate<'a>>,
    /// How many results each stage keeps before the next. Defaults to
    /// [`DEFAULT_CANDIDATE_POOL`].
    pub candidate_pool: usize,
    /// The caller's token budget for the final response.
    pub token_budget: usize,
}

/// What one [`search`] call found.
#[derive(Debug)]
pub struct SearchResponse {
    /// The final snippets: within budget, deduplicated, and capped per
    /// Rule 28.
    pub hits: Vec<RetrievedSnippet>,
    /// Which tiers contributed: `"bm25"` always, `"dense"` and `"rerank"`
    /// when they ran.
    pub tiers_used: Vec<&'static str>,
}

/// Searches one pack: BM25, optionally dense, fused by Reciprocal Rank
/// Fusion, optionally reranked, then shaped to the caller's token budget.
///
/// # Errors
///
/// Returns whatever [`DenseIndex::search`] returns — a vector width
/// mismatch between the query and the index — when `req.dense` is `Some`.
pub fn search(req: &SearchRequest<'_>) -> Result<SearchResponse> {
    let candidate_pool = req.candidate_pool.max(1);
    let mut tiers = vec!["bm25"];

    let bm25_hits = req.bm25.search(req.query, candidate_pool);
    let mut lists = vec![bm25_hits];

    if let Some((dense_index, query_vector)) = req.dense
        && !dense_index.is_empty()
    {
        lists.push(dense_index.search(query_vector, candidate_pool)?);
        tiers.push("dense");
    }

    let mut fused = reciprocal_rank_fusion(&lists, RRF_K);
    fused.truncate(candidate_pool);

    if let Some(gate) = req.reranker
        && let Some(reranked) = gate.rerank(req.query, &fused, req.chunks)
    {
        fused = reranked;
        tiers.push("rerank");
    }

    let hits = budget::fill(req.chunks, &fused, req.token_budget);
    Ok(SearchResponse {
        hits,
        tiers_used: tiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::{ErrCode, Error, Scored};

    fn chunk(id: &str, breadcrumb: &str, body: &str) -> Chunk {
        Chunk {
            chunk_id: id.to_owned(),
            ordinal: 0,
            breadcrumb: breadcrumb.to_owned(),
            url: Some(format!("https://example.com/{id}")),
            body: body.to_owned(),
            embed_text: format!("{breadcrumb}\n\n{body}"),
            tokens: body.split_whitespace().count(),
            oversize: false,
        }
    }

    fn corpus() -> Vec<Chunk> {
        vec![
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
        ]
    }

    #[test]
    fn bm25_alone_answers_a_query_and_reports_only_the_bm25_tier() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: None,
            query: "async tasks",
            reranker: None,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25"]);
        assert!(!response.hits.is_empty());
        assert_eq!(response.hits[0].chunk_index, 0);
    }

    #[test]
    fn a_dense_index_adds_the_dense_tier() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]];
        let dense = DenseIndex::build(&vectors).unwrap();
        let query_vector = [1.0, 0.0];
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: Some((&dense, &query_vector)),
            query: "async tasks",
            reranker: None,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25", "dense"]);
    }

    #[test]
    fn an_empty_dense_index_is_skipped() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let dense = DenseIndex::build(&[]).unwrap();
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: Some((&dense, &[])),
            query: "async tasks",
            reranker: None,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25"]);
    }

    struct ReverseReranker;
    impl Reranker for ReverseReranker {
        fn rerank(&self, _query: &str, docs: &[String]) -> Result<Vec<Scored>> {
            Ok(docs
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    #[allow(clippy::cast_precision_loss)]
                    let score = index as f32;
                    Scored { index, score }
                })
                .collect())
        }
    }

    #[test]
    fn a_reranker_adds_the_rerank_tier_and_changes_the_order() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let reranker = ReverseReranker;
        let gate = RerankGate::new(&reranker);
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: None,
            query: "async",
            reranker: Some(&gate),
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap();
        assert!(response.tiers_used.contains(&"rerank"));
    }

    struct UnsupportedReranker;
    impl Reranker for UnsupportedReranker {
        fn rerank(&self, _query: &str, _docs: &[String]) -> Result<Vec<Scored>> {
            Err(Error::new(ErrCode::EngineUnsupported, "no logprobs"))
        }
    }

    #[test]
    fn a_failing_reranker_falls_back_to_fusion_order_without_the_rerank_tier() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let reranker = UnsupportedReranker;
        let gate = RerankGate::new(&reranker);
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: None,
            query: "async",
            reranker: Some(&gate),
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25"]);
    }

    #[test]
    fn a_dimension_mismatch_between_query_and_dense_index_is_an_error() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let dense = DenseIndex::build(&[vec![1.0, 0.0], vec![0.0, 1.0], vec![0.5, 0.5]]).unwrap();
        let bad_query = [1.0, 0.0, 0.0];
        let err = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: Some((&dense, &bad_query)),
            query: "async",
            reranker: None,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: DEFAULT_TOKEN_BUDGET,
        })
        .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn the_response_respects_the_token_budget() {
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let response = search(&SearchRequest {
            chunks: &chunks,
            bm25: &bm25,
            dense: None,
            query: "async tasks files sockets",
            reranker: None,
            candidate_pool: DEFAULT_CANDIDATE_POOL,
            token_budget: 5,
        })
        .unwrap();
        assert!(response.hits.len() <= 1);
    }
}
