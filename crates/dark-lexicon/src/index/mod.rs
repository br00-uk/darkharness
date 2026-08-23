//! The `index` stage: task unit `G4`.
//!
//! Two indexes over one pack's chunks, and the fusion that combines their
//! results without ever comparing their scores directly:
//!
//! - [`bm25`]: the lexical index. `k1 = 1.2`, `b = 0.75`, a tokenizer built
//!   for code identifiers as much as prose (split camel case and snake
//!   case, keep the original token, stem prose lightly and identifiers not
//!   at all), postings delta-encoded with variable-length integers. This
//!   is the fallback: it must answer well with no embedding model resident
//!   at all, which is why `crate::retrieve` always searches it, dense
//!   index or not.
//! - [`dense`]: the embedding index. Int8-quantised vectors, an f32 scale
//!   per vector, scanned by brute force — "a 50000-chunk pack at 1024
//!   dimensions is about 51 MB. A full scan takes tens of milliseconds," so
//!   an approximate index buys nothing here and G4 rules one out by name.
//! - [`fusion`]: Reciprocal Rank Fusion, `1 / (60 + rank)` summed across
//!   whichever of the two lists ran, reading only rank, never either
//!   list's own score.
//!
//! [`codec`] holds the variable-length integer encoding both [`bm25`] and
//! [`dense`] use for their on-disk form.

pub mod bm25;
pub mod codec;
pub mod dense;
pub mod fusion;

pub use bm25::{B, Bm25Index, K1};
pub use dense::{DenseIndex, Embedder};
pub use fusion::{RRF_K, reciprocal_rank_fusion};

/// One ranked chunk from a search stage: BM25, dense, or fused.
///
/// `score` is meaningful only within the list that produced it — a BM25
/// score and a cosine score are not on the same scale, which is exactly
/// why [`reciprocal_rank_fusion`] never reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankedHit {
    /// This chunk's position in the slice the index was built over.
    pub chunk_index: usize,
    /// Higher is more relevant, within the list this hit came from.
    pub score: f32,
}
