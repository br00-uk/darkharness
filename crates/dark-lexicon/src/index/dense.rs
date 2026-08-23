//! The dense index: int8-quantised embedding vectors, scanned by brute
//! force.
//!
//! Task unit `G4`, Do 5 to 6: quantise each vector to int8 with an f32
//! scale, and scan by brute force rather than build an approximate index —
//! "a 50000-chunk pack at 1024 dimensions is about 51 MB. A full scan takes
//! tens of milliseconds." [`DenseIndex`] is that scan. [`Embedder`] is the
//! seam that supplies the vectors in the first place.
//!
//! ## Why `Embedder`, not `&dyn dark_contract::Engine`
//!
//! `Engine::embed` is `async`. Calling it needs nothing beyond core Rust
//! (`async`/`.await` are language features, not a crate), but *driving* an
//! `async fn` to completion needs an executor, and this crate has no tokio
//! dependency to provide one (Rule 16) — the same wall `crate::chunk`'s
//! module docs describe for `Engine::stream`. [`Embedder`] is a small,
//! synchronous, object-safe trait on the same pattern
//! `crate::ingest::fetch::Fetcher` already establishes for the same reason:
//! a caller that already depends on `dark-contract`'s `Engine` and runs
//! inside a tokio runtime (`dark-core`, `dark-cli`) implements `Embedder`
//! over `&dyn Engine` by blocking on the call — `Fetcher`'s module docs
//! name `tokio::runtime::Handle::block_on` as the exact tool for that —
//! and `dark-lexicon` never needs to.

use dark_contract::{EmbedPurpose, ErrCode, Error, Result};

use crate::index::RankedHit;

/// Produces embedding vectors for text.
///
/// See the module docs for why this trait exists rather than every caller
/// holding `&dyn dark_contract::Engine` directly.
pub trait Embedder: Send + Sync {
    /// Produces one vector for each of `texts`, in order.
    ///
    /// # Errors
    ///
    /// Returns whatever the underlying embedding call returns — typically
    /// `E_ENGINE_UNSUPPORTED` when no embedding model is resident.
    fn embed(&self, texts: &[String], purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>>;
}

/// The 4-byte tag at the start of an encoded [`DenseIndex`].
const MAGIC: &[u8; 4] = b"DVEC";
/// The encoding version. Bump this when the byte layout changes.
const FORMAT_VERSION: u8 = 1;

/// One int8-quantised vector with the f32 scale that recovers it.
#[derive(Debug, Clone, PartialEq)]
struct QuantizedVector {
    /// `original[i] ≈ values[i] as f32 * scale`.
    scale: f32,
    values: Vec<i8>,
}

/// Quantises `vector` to int8 by symmetric scaling: the scale is the
/// vector's own maximum absolute value divided by 127, so the largest
/// component always uses the full int8 range.
fn quantize(vector: &[f32]) -> QuantizedVector {
    let max_abs = vector.iter().fold(0.0_f32, |acc, &v| acc.max(v.abs()));
    let scale = if max_abs == 0.0 { 1.0 } else { max_abs / 127.0 };
    let values = vector
        .iter()
        .map(|&v| (v / scale).round().clamp(-127.0, 127.0) as i8)
        .collect();
    QuantizedVector { scale, values }
}

/// Reconstructs an approximate f32 vector from its quantised form.
fn dequantize(vector: &QuantizedVector) -> Vec<f32> {
    vector.values.iter().map(|&v| f32::from(v) * vector.scale).collect()
}

/// The Euclidean norm of `v`.
fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// A dense (embedding) index over one pack's chunks.
///
/// One quantised vector per chunk, in chunk order. G4 Do 6 rules out an
/// approximate index by name; [`DenseIndex::search`] is a full scan, not a
/// shortcut around one.
#[derive(Debug, Clone, PartialEq)]
pub struct DenseIndex {
    dim: usize,
    vectors: Vec<QuantizedVector>,
}

impl DenseIndex {
    /// Builds a dense index from one f32 vector per chunk, in chunk order.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when the vectors are not all the same
    /// width.
    pub fn build(vectors: &[Vec<f32>]) -> Result<Self> {
        let dim = vectors.first().map_or(0, Vec::len);
        for v in vectors {
            if v.len() != dim {
                return Err(Error::new(
                    ErrCode::ToolFailed,
                    format!("embedding vectors are not uniform width: expected {dim}, found {}", v.len()),
                ));
            }
        }
        Ok(Self { dim, vectors: vectors.iter().map(|v| quantize(v)).collect() })
    }

    /// The vector width every entry in this index shares.
    #[must_use]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// The number of vectors in this index.
    #[must_use]
    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    /// Returns `true` when this index holds no vectors.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Scores every chunk against `query` by cosine similarity, scanned by
    /// brute force (G4 Do 6), and returns the `top_k` highest, best first.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when `query`'s width does not match this
    /// index's.
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<RankedHit>> {
        if !self.vectors.is_empty() && query.len() != self.dim {
            return Err(Error::new(
                ErrCode::ToolFailed,
                format!("query vector has {} dimensions, the index has {}", query.len(), self.dim),
            ));
        }
        let query_norm = norm(query);
        let mut scored: Vec<RankedHit> = self
            .vectors
            .iter()
            .enumerate()
            .map(|(i, quantized)| {
                let dequantized = dequantize(quantized);
                let dot: f32 = dequantized.iter().zip(query).map(|(a, b)| a * b).sum();
                let denom = norm(&dequantized) * query_norm;
                let cosine = if denom == 0.0 { 0.0 } else { dot / denom };
                RankedHit { chunk_index: i, score: cosine }
            })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.chunk_index.cmp(&b.chunk_index)));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Encodes this index as bytes, ready to write to
    /// `crate::pack::DENSE_VECTORS_FILE_NAME`.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(FORMAT_VERSION);
        out.extend_from_slice(&(self.dim as u32).to_le_bytes());
        out.extend_from_slice(&(self.vectors.len() as u32).to_le_bytes());
        for vector in &self.vectors {
            out.extend_from_slice(&vector.scale.to_le_bytes());
            for &value in &vector.values {
                out.push(value as u8);
            }
        }
        out
    }

    /// Decodes an index that [`DenseIndex::to_bytes`] produced.
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
            return Err(Error::new(ErrCode::ToolFailed, "not a dense index: bad magic bytes"));
        }
        pos += 4;
        let version = *bytes.get(pos).ok_or_else(too_short)?;
        pos += 1;
        if version != FORMAT_VERSION {
            return Err(Error::new(
                ErrCode::ToolFailed,
                format!("dense index format version {version} is not supported (expected {FORMAT_VERSION})"),
            ));
        }
        let dim = read_u32(bytes, &mut pos)? as usize;
        let count = read_u32(bytes, &mut pos)? as usize;

        let mut vectors = Vec::with_capacity(count);
        for _ in 0..count {
            let scale = read_f32(bytes, &mut pos)?;
            let raw = bytes.get(pos..pos + dim).ok_or_else(too_short)?;
            let values: Vec<i8> = raw.iter().map(|&b| b as i8).collect();
            pos += dim;
            vectors.push(QuantizedVector { scale, values });
        }

        Ok(Self { dim, vectors })
    }
}

fn too_short() -> Error {
    Error::new(ErrCode::ToolFailed, "dense index bytes end before the format expects")
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32> {
    let slice: [u8; 4] = bytes.get(*pos..*pos + 4).ok_or_else(too_short)?.try_into().unwrap();
    *pos += 4;
    Ok(u32::from_le_bytes(slice))
}

fn read_f32(bytes: &[u8], pos: &mut usize) -> Result<f32> {
    let slice: [u8; 4] = bytes.get(*pos..*pos + 4).ok_or_else(too_short)?.try_into().unwrap();
    *pos += 4;
    Ok(f32::from_le_bytes(slice))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_then_dequantize_stays_close_to_the_original() {
        let original = vec![0.5_f32, -0.25, 1.0, -1.0, 0.0];
        let q = quantize(&original);
        let back = dequantize(&q);
        for (a, b) in original.iter().zip(&back) {
            assert!((a - b).abs() < 0.02, "{a} vs {b}");
        }
    }

    #[test]
    fn an_all_zero_vector_quantises_without_division_by_zero() {
        let q = quantize(&[0.0, 0.0, 0.0]);
        assert_eq!(q.values, vec![0, 0, 0]);
    }

    #[test]
    fn build_rejects_vectors_of_different_widths() {
        let err = DenseIndex::build(&[vec![1.0, 2.0], vec![1.0]]).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn search_ranks_the_nearest_vector_first() {
        let vectors = vec![vec![1.0, 0.0], vec![0.0, 1.0], vec![0.9, 0.1]];
        let index = DenseIndex::build(&vectors).unwrap();
        let hits = index.search(&[1.0, 0.0], 3).unwrap();
        assert_eq!(hits[0].chunk_index, 0);
        assert_eq!(hits[1].chunk_index, 2);
        assert_eq!(hits[2].chunk_index, 1);
    }

    #[test]
    fn search_rejects_a_query_of_the_wrong_width() {
        let index = DenseIndex::build(&[vec![1.0, 0.0]]).unwrap();
        let err = index.search(&[1.0, 0.0, 0.0], 5).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn top_k_caps_the_result_count() {
        let vectors: Vec<Vec<f32>> = (0..10).map(|i| vec![i as f32, 1.0]).collect();
        let index = DenseIndex::build(&vectors).unwrap();
        assert_eq!(index.search(&[1.0, 1.0], 4).unwrap().len(), 4);
    }

    #[test]
    fn bytes_round_trip_preserves_search_results() {
        let vectors = vec![vec![1.0, 0.0, 0.2], vec![0.0, 1.0, 0.1], vec![0.8, 0.1, 0.9]];
        let index = DenseIndex::build(&vectors).unwrap();
        let before = index.search(&[1.0, 0.0, 0.0], 3).unwrap();

        let bytes = index.to_bytes();
        let restored = DenseIndex::from_bytes(&bytes).unwrap();
        let after = restored.search(&[1.0, 0.0, 0.0], 3).unwrap();

        assert_eq!(index, restored);
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(a.chunk_index, b.chunk_index);
            assert!((a.score - b.score).abs() < 1e-6);
        }
    }

    #[test]
    fn from_bytes_rejects_bad_magic() {
        let err = DenseIndex::from_bytes(b"nope").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn from_bytes_rejects_truncated_bytes() {
        let index = DenseIndex::build(&[vec![1.0, 2.0, 3.0]]).unwrap();
        let mut bytes = index.to_bytes();
        bytes.truncate(bytes.len() - 2);
        assert!(DenseIndex::from_bytes(&bytes).is_err());
    }

    #[test]
    fn an_empty_index_is_empty_and_searches_without_panicking() {
        let index = DenseIndex::build(&[]).unwrap();
        assert!(index.is_empty());
        assert_eq!(index.dim(), 0);
        assert!(index.search(&[], 5).unwrap().is_empty());
    }
}
