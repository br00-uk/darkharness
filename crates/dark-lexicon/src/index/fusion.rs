//! Reciprocal Rank Fusion.
//!
//! Task unit `G4`, Do 7:
//!
//! ```text
//! score = sum over lists of 1 / (60 + rank)
//! ```
//!
//! "Fusion needs no score calibration. BM25 scores and cosine scores are
//! not comparable." [`reciprocal_rank_fusion`] never reads an input list's
//! own score — only each hit's rank (its 1-based position) feeds the
//! formula — which is exactly what makes it safe to combine a lexical list
//! and a dense list whose score scales share nothing.

use std::collections::BTreeMap;

use crate::index::RankedHit;

/// The RRF constant. G4 Do 7 fixes it at 60.
pub const RRF_K: f32 = 60.0;

/// Fuses several ranked lists into one, by Reciprocal Rank Fusion.
///
/// Each input list is assumed already sorted best first; a hit's rank is
/// its 1-based position within its own list. A chunk that appears in more
/// than one list sums a `1 / (k + rank)` term for each appearance; a chunk
/// that appears in only one list still scores, just lower than one backed
/// by every list. The result is sorted by the fused score, best first,
/// ties broken by chunk index for a deterministic order.
#[must_use]
pub fn reciprocal_rank_fusion(lists: &[Vec<RankedHit>], k: f32) -> Vec<RankedHit> {
    let mut totals: BTreeMap<usize, f32> = BTreeMap::new();
    for list in lists {
        for (position, hit) in list.iter().enumerate() {
            let rank = (position + 1) as f32;
            *totals.entry(hit.chunk_index).or_insert(0.0) += 1.0 / (k + rank);
        }
    }
    let mut fused: Vec<RankedHit> =
        totals.into_iter().map(|(chunk_index, score)| RankedHit { chunk_index, score }).collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.chunk_index.cmp(&b.chunk_index)));
    fused
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(order: &[usize]) -> Vec<RankedHit> {
        // Scores here are deliberately not RRF-comparable across lists —
        // one list uses BM25-shaped scores, the other cosine-shaped ones —
        // to prove fusion never reads them.
        order.iter().enumerate().map(|(i, &chunk_index)| RankedHit { chunk_index, score: 100.0 - i as f32 }).collect()
    }

    #[test]
    fn a_hit_at_the_top_of_every_list_wins() {
        let bm25 = hits(&[3, 1, 2]);
        let dense = vec![RankedHit { chunk_index: 3, score: 0.01 }, RankedHit { chunk_index: 5, score: 0.99 }];
        let fused = reciprocal_rank_fusion(&[bm25, dense], RRF_K);
        assert_eq!(fused[0].chunk_index, 3);
    }

    #[test]
    fn the_fused_score_matches_the_formula() {
        let list_a = hits(&[7]);
        let list_b = hits(&[7]);
        let fused = reciprocal_rank_fusion(&[list_a, list_b], 60.0);
        let expected = 2.0 / 61.0;
        assert!((fused[0].score - expected).abs() < 1e-6);
    }

    #[test]
    fn a_hit_in_only_one_list_still_appears() {
        let list_a = hits(&[1, 2]);
        let list_b = hits(&[9]);
        let fused = reciprocal_rank_fusion(&[list_a, list_b], RRF_K);
        assert!(fused.iter().any(|h| h.chunk_index == 9));
    }

    #[test]
    fn a_single_list_preserves_its_own_order() {
        let list = hits(&[5, 1, 9]);
        let fused = reciprocal_rank_fusion(&[list], RRF_K);
        let order: Vec<usize> = fused.iter().map(|h| h.chunk_index).collect();
        assert_eq!(order, vec![5, 1, 9]);
    }

    #[test]
    fn no_lists_produces_no_hits() {
        let fused: Vec<RankedHit> = reciprocal_rank_fusion(&[], RRF_K);
        assert!(fused.is_empty());
    }

    #[test]
    fn ties_break_by_chunk_index_for_a_deterministic_order() {
        let list_a = vec![RankedHit { chunk_index: 5, score: 1.0 }];
        let list_b = vec![RankedHit { chunk_index: 2, score: 1.0 }];
        // Both chunk_index 5 and 2 are rank 1 in their own list, so their
        // fused scores tie exactly.
        let fused = reciprocal_rank_fusion(&[list_a, list_b], RRF_K);
        assert_eq!(fused[0].chunk_index, 2);
        assert_eq!(fused[1].chunk_index, 5);
    }
}
