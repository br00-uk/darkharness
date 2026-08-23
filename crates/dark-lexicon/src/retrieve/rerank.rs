//! Reranking: gated on `Caps::logprobs`, and disabled once it proves slow.
//!
//! Task unit `G4`, Do 8: "Rerank the top 50 fused results when
//! `Caps::logprobs` is true. Measure the latency. Disable reranking by
//! default when the latency exceeds 400 milliseconds."
//!
//! [`Reranker`] is the same seam shape as `crate::index::dense::Embedder`
//! and `crate::ingest::fetch::Fetcher`: `dark_contract::Engine::rerank` is
//! `async`, and this crate has no executor to drive one (Rule 16), so a
//! caller that already runs inside a tokio runtime implements `Reranker`
//! over `&dyn Engine`, blocking on the call.
//!
//! [`RerankGate`] carries the "measure the latency… disable by default"
//! half of Do 8. `Caps::logprobs` is the caller's decision, made once
//! before it ever builds a gate — pass no [`RerankGate`] to
//! [`crate::retrieve::search`] when `Caps::logprobs` is false, and
//! reranking never runs. The gate itself decides only the timing question:
//! a caller builds one gate per session and reuses it across every query,
//! so a single slow call disables reranking for the rest of that session,
//! not just for the query that measured it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dark_contract::{Result, Scored};

use crate::chunk::Chunk;
use crate::index::RankedHit;

/// Scores documents against a query.
///
/// See the module docs for why this trait exists rather than every caller
/// holding `&dyn dark_contract::Engine` directly.
pub trait Reranker: Send + Sync {
    /// Scores each of `docs` against `query`. Higher is more relevant.
    ///
    /// # Errors
    ///
    /// Returns whatever the underlying call returns — typically
    /// `E_ENGINE_UNSUPPORTED` when `Caps::logprobs` is false for the
    /// loaded model.
    fn rerank(&self, query: &str, docs: &[String]) -> Result<Vec<Scored>>;
}

/// G4 Do 8: "Disable reranking by default when the latency exceeds 400
/// milliseconds."
pub const DEFAULT_RERANK_LATENCY_THRESHOLD: Duration = Duration::from_millis(400);

/// Wraps a [`Reranker`], timing every call and disabling further use once
/// one call runs over its threshold.
///
/// A caller creates one gate per session (or per turn; anywhere its
/// lifetime outlives more than one query) and passes it to every
/// [`crate::retrieve::search`] call, so the latency measurement in G4 Do 8
/// applies across the session rather than resetting on every query.
pub struct RerankGate<'a> {
    reranker: &'a dyn Reranker,
    threshold: Duration,
    disabled: AtomicBool,
}

impl<'a> RerankGate<'a> {
    /// Creates a gate at the default 400-millisecond threshold.
    #[must_use]
    pub fn new(reranker: &'a dyn Reranker) -> Self {
        Self::with_threshold(reranker, DEFAULT_RERANK_LATENCY_THRESHOLD)
    }

    /// Creates a gate at a caller-chosen threshold. Mainly for tests that
    /// need a threshold a fake reranker can reliably cross or stay under.
    #[must_use]
    pub fn with_threshold(reranker: &'a dyn Reranker, threshold: Duration) -> Self {
        Self {
            reranker,
            threshold,
            disabled: AtomicBool::new(false),
        }
    }

    /// Returns `true` once a call has exceeded this gate's threshold.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Relaxed)
    }

    /// Reranks `fused` against `chunks`, unless this gate is disabled.
    ///
    /// Returns `None` — meaning "the caller falls back to `fused`'s own
    /// order" — when the gate is already disabled, `fused` is empty, a
    /// chunk index in `fused` is out of range, or the underlying call
    /// fails. A failed call does not disable the gate on its own: only
    /// slowness does: `Caps::logprobs` failing is a capability question
    /// the caller already answered by choosing whether to pass a gate at
    /// all, not a latency question this method decides.
    pub(crate) fn rerank(
        &self,
        query: &str,
        fused: &[RankedHit],
        chunks: &[Chunk],
    ) -> Option<Vec<RankedHit>> {
        if self.is_disabled() || fused.is_empty() {
            return None;
        }
        let docs: Vec<String> = fused
            .iter()
            .filter_map(|hit| chunks.get(hit.chunk_index))
            .map(|c| c.embed_text.clone())
            .collect();
        if docs.len() != fused.len() {
            return None;
        }

        let start = Instant::now();
        let outcome = self.reranker.rerank(query, &docs);
        if start.elapsed() > self.threshold {
            self.disabled.store(true, Ordering::Relaxed);
        }

        let scored = outcome.ok()?;
        let mut reranked: Vec<RankedHit> = scored
            .into_iter()
            .filter_map(|s| {
                fused.get(s.index).map(|hit| RankedHit {
                    chunk_index: hit.chunk_index,
                    score: s.score,
                })
            })
            .collect();
        reranked.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then(a.chunk_index.cmp(&b.chunk_index))
        });
        Some(reranked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    struct FastReranker;
    impl Reranker for FastReranker {
        fn rerank(&self, _query: &str, docs: &[String]) -> Result<Vec<Scored>> {
            // Scores the last document highest, so sorting by score
            // afterward exactly reverses the input order: a test can then
            // tell the reranked order apart from the fused order.
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

    struct SlowReranker {
        sleep: Duration,
    }
    impl Reranker for SlowReranker {
        fn rerank(&self, _query: &str, docs: &[String]) -> Result<Vec<Scored>> {
            std::thread::sleep(self.sleep);
            Ok(docs
                .iter()
                .enumerate()
                .map(|(index, _)| Scored { index, score: 1.0 })
                .collect())
        }
    }

    struct FailingReranker;
    impl Reranker for FailingReranker {
        fn rerank(&self, _query: &str, _docs: &[String]) -> Result<Vec<Scored>> {
            Err(dark_contract::Error::new(
                dark_contract::ErrCode::EngineUnsupported,
                "no logprobs",
            ))
        }
    }

    fn chunks() -> Vec<Chunk> {
        vec![
            chunk("a", "lib › A", "first"),
            chunk("b", "lib › B", "second"),
            chunk("c", "lib › C", "third"),
        ]
    }

    fn fused() -> Vec<RankedHit> {
        vec![
            RankedHit {
                chunk_index: 0,
                score: 3.0,
            },
            RankedHit {
                chunk_index: 1,
                score: 2.0,
            },
            RankedHit {
                chunk_index: 2,
                score: 1.0,
            },
        ]
    }

    #[test]
    fn a_fast_reranker_reorders_the_fused_list() {
        let scorer = FastReranker;
        let gate = RerankGate::new(&scorer);
        let reranked = gate.rerank("q", &fused(), &chunks()).unwrap();
        assert_eq!(reranked[0].chunk_index, 2);
        assert_eq!(reranked[2].chunk_index, 0);
    }

    #[test]
    fn a_slow_call_disables_the_gate_for_later_calls() {
        let reranker = SlowReranker {
            sleep: Duration::from_millis(20),
        };
        let gate = RerankGate::with_threshold(&reranker, Duration::from_millis(5));
        assert!(!gate.is_disabled());
        assert!(
            gate.rerank("q", &fused(), &chunks()).is_some(),
            "the slow call itself still returns its result"
        );
        assert!(gate.is_disabled());
        assert!(
            gate.rerank("q", &fused(), &chunks()).is_none(),
            "a disabled gate skips the call entirely"
        );
    }

    #[test]
    fn a_call_under_the_threshold_leaves_the_gate_enabled() {
        let reranker = FastReranker;
        let gate = RerankGate::with_threshold(&reranker, Duration::from_secs(1));
        gate.rerank("q", &fused(), &chunks());
        assert!(!gate.is_disabled());
    }

    #[test]
    fn a_failed_call_falls_back_without_disabling_the_gate() {
        let reranker = FailingReranker;
        let gate = RerankGate::new(&reranker);
        assert!(gate.rerank("q", &fused(), &chunks()).is_none());
        assert!(
            !gate.is_disabled(),
            "a capability failure is not a latency failure"
        );
    }

    #[test]
    fn an_empty_fused_list_skips_the_call() {
        let reranker = FastReranker;
        let gate = RerankGate::new(&reranker);
        assert!(gate.rerank("q", &[], &chunks()).is_none());
    }
}
