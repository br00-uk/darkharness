//! `docs_resolve`: matching a query against known packs, without deciding
//! between them.
//!
//! Task unit `G5`, Do 2: "Return ambiguity from `docs_resolve`. Do not
//! resolve it. Three candidates are better than one wrong answer."
//! [`docs_resolve`] never commits to a single pack on its own: it scores
//! every pack the caller hands it against the query and returns every
//! candidate over [`CONFIDENCE_FLOOR`], best first — even when one
//! candidate scores far above the rest, since a caller with one candidate
//! in hand should still see the runner-up before treating the match as
//! settled.

use serde::{Deserialize, Serialize};

use crate::pack::PackManifest;

/// One pack that might answer `docs_resolve`'s query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolveCandidate {
    /// The pack identifier, for example `tokio@1.47.0`.
    pub pack_id: String,
    /// The pack name.
    pub name: String,
    /// The pack version.
    pub version: String,
    /// How well this pack matches the query, from 0.0 to 1.0.
    pub confidence: f32,
    /// A short explanation a person or a model can read, for example
    /// "exact match on the pack name".
    pub why: String,
}

/// The lowest confidence that keeps a pack in the returned candidate list.
///
/// Below this, a match is noise, not ambiguity: including it would not
/// help a caller decide, it would only add clutter.
pub const CONFIDENCE_FLOOR: f32 = 0.35;

/// The most candidates that [`docs_resolve`] returns for one query.
pub const MAX_CANDIDATES: usize = 5;

/// Scores `query` against every pack in `packs`, and returns every match
/// over [`CONFIDENCE_FLOOR`], best first, capped at [`MAX_CANDIDATES`].
///
/// This never narrows to one answer on its own, even when one candidate
/// scores far above the rest: G5 Do 2 asks for the ambiguity itself, not a
/// resolution of it.
#[must_use]
pub fn docs_resolve(query: &str, packs: &[PackManifest]) -> Vec<ResolveCandidate> {
    let mut candidates: Vec<ResolveCandidate> = packs
        .iter()
        .filter_map(|manifest| score_pack(query, manifest))
        .filter(|c| c.confidence >= CONFIDENCE_FLOOR)
        .collect();
    candidates.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.name.cmp(&b.name))
    });
    candidates.truncate(MAX_CANDIDATES);
    candidates
}

/// Scores one pack against `query`. Returns `None` when `query` is empty
/// or the pack shares nothing recognisable with it (an edit distance
/// covering the whole word, in practice).
fn score_pack(query: &str, manifest: &PackManifest) -> Option<ResolveCandidate> {
    let query_lower = query.trim().to_lowercase();
    if query_lower.is_empty() {
        return None;
    }
    let name_lower = manifest.pack.name.to_lowercase();

    let (confidence, why) = if name_lower == query_lower {
        (1.0, "exact match on the pack name".to_owned())
    } else if let Some(alias) = manifest
        .pack
        .aliases
        .iter()
        .find(|a| a.to_lowercase() == query_lower)
    {
        (0.95, format!("exact match on the alias '{alias}'"))
    } else if name_lower.contains(&query_lower) || query_lower.contains(&name_lower) {
        #[allow(clippy::cast_precision_loss)]
        // a pack name is far shorter than f32's precision limit
        let shorter = name_lower.len().min(query_lower.len()) as f32;
        #[allow(clippy::cast_precision_loss)]
        let longer = name_lower.len().max(query_lower.len()) as f32;
        (
            0.6 + 0.3 * (shorter / longer),
            "partial match on the pack name".to_owned(),
        )
    } else {
        let distance = levenshtein(&name_lower, &query_lower);
        let longest = name_lower.chars().count().max(query_lower.chars().count());
        if longest == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss)]
        // an edit distance is far shorter than f32's precision limit
        let similarity = 1.0 - (distance as f32 / longest as f32);
        if similarity <= 0.0 {
            return None;
        }
        (
            similarity * 0.7,
            format!(
                "similar to the pack name '{}' (edit distance {distance})",
                manifest.pack.name
            ),
        )
    };

    Some(ResolveCandidate {
        pack_id: manifest.pack.pack_id(),
        name: manifest.pack.name.clone(),
        version: manifest.pack.version.clone(),
        confidence,
        why,
    })
}

/// The Levenshtein (edit) distance between `a` and `b`.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, &ac) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, &bc) in b.iter().enumerate() {
            let cost = usize::from(ac != bc);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{EmbedBlock, Ingest, License, PackId, Source, Staleness};

    fn manifest(name: &str, version: &str, aliases: &[&str]) -> PackManifest {
        PackManifest {
            pack: PackId {
                name: name.to_owned(),
                version: version.to_owned(),
                ecosystem: "crates.io".to_owned(),
                aliases: aliases.iter().map(|a| (*a).to_owned()).collect(),
            },
            source: Source {
                kind: "localdir".to_owned(),
                url: ".".to_owned(),
                etag: String::new(),
                commit: String::new(),
            },
            ingest: Ingest {
                at: "2026-08-19T11:03:00Z".parse().unwrap(),
                tool_version: "1.0.0".to_owned(),
                chunker: "heading-v1".to_owned(),
                chunks: 1,
            },
            embed: EmbedBlock {
                model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
                dim: 4,
                quant: "int8".to_owned(),
                query_prefix: String::new(),
                doc_prefix: String::new(),
            },
            staleness: Staleness {
                policy: "90d".to_owned(),
            },
            license: License {
                spdx: "MIT".to_owned(),
                notice_required: true,
            },
        }
    }

    #[test]
    fn an_exact_name_match_scores_highest() {
        let packs = vec![manifest("tokio", "1.47.0", &[])];
        let candidates = docs_resolve("tokio", &packs);
        assert_eq!(candidates.len(), 1);
        assert!((candidates[0].confidence - 1.0).abs() < f32::EPSILON);
        assert_eq!(candidates[0].pack_id, "tokio@1.47.0");
    }

    #[test]
    fn matching_is_case_insensitive() {
        let packs = vec![manifest("tokio", "1.47.0", &[])];
        let candidates = docs_resolve("TOKIO", &packs);
        assert!((candidates[0].confidence - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn an_alias_match_scores_just_below_an_exact_name_match() {
        let packs = vec![manifest("tokio", "1.47.0", &["tokio-rs"])];
        let candidates = docs_resolve("tokio-rs", &packs);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].confidence < 1.0);
        assert!(candidates[0].confidence >= CONFIDENCE_FLOOR);
        assert!(candidates[0].why.contains("alias"));
    }

    #[test]
    fn ambiguous_queries_return_more_than_one_candidate() {
        // "tokio" is an exact match for one pack and a strong partial match
        // for another: G5 Do 2 says return both, not just the exact one.
        let packs = vec![
            manifest("tokio", "1.47.0", &[]),
            manifest("tokio-util", "0.7.0", &[]),
        ];
        let candidates = docs_resolve("tokio", &packs);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "tokio");
        assert!(candidates[0].confidence > candidates[1].confidence);
    }

    #[test]
    fn an_unrelated_query_returns_no_candidates() {
        let packs = vec![manifest("tokio", "1.47.0", &[])];
        assert!(docs_resolve("kubernetes", &packs).is_empty());
    }

    #[test]
    fn an_empty_query_returns_no_candidates() {
        let packs = vec![manifest("tokio", "1.47.0", &[])];
        assert!(docs_resolve("", &packs).is_empty());
        assert!(docs_resolve("   ", &packs).is_empty());
    }

    #[test]
    fn results_are_capped_at_max_candidates() {
        let packs: Vec<PackManifest> = (0..10)
            .map(|i| manifest(&format!("tokio-ext-{i}"), "1.0.0", &[]))
            .collect();
        let candidates = docs_resolve("tokio", &packs);
        assert!(candidates.len() <= MAX_CANDIDATES);
    }

    #[test]
    fn a_slight_misspelling_still_matches_by_edit_distance() {
        let packs = vec![manifest("tokio", "1.47.0", &[])];
        let candidates = docs_resolve("tokoi", &packs);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].why.contains("edit distance"));
    }

    #[test]
    fn levenshtein_distance_is_symmetric_and_zero_for_equal_strings() {
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(
            levenshtein("kitten", "sitting"),
            levenshtein("sitting", "kitten")
        );
    }
}
