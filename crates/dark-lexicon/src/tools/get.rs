//! `docs_get`: search one pack and return snippets within budget.
//!
//! Task unit `G5`, Do 1:
//!
//! ```text
//! docs_get(pack_id, topic, tokens = 4000)
//!   -> { snippets: [{text, breadcrumb, url, chunk_id}],
//!        pack: {name, version, ingested_at, age_days, stale},
//!        tiers_used: ["bm25","dense","rerank"] }
//! ```
//!
//! [`docs_get_from_parts`] is that shape over already-loaded data —
//! `crate::chunk::Chunk` values, an already-built `crate::index::Bm25Index`
//! and (optionally) `crate::index::DenseIndex`, and a
//! [`DocsGetDeps`] — with no filesystem access at all, so a test drives it
//! directly with hand-built fixtures. [`docs_get`] is the thin filesystem
//! layer around it: it reads a pack directory (verifying the pack hash
//! first, per task unit `G1`'s "verify the pack hash before use"), decodes
//! its indexes, and calls the pure function. This mirrors the pattern
//! `crate::chunk`'s module docs describe for `chunk_document` over
//! `chunk_with_counter`.
//!
//! Do 3: "Put the staleness warning in the returned text. Do not put it
//! only in the metadata. The model reads the text." Every snippet in a
//! stale pack's response carries the warning at its own start — not just
//! the first one — so it survives however a caller trims or reorders the
//! list before a model sees it.

use std::path::Path;

use dark_contract::{EmbedPurpose, ErrCode, Error, Result};
use serde::{Deserialize, Serialize};

use crate::chunk::Chunk;
use crate::index::{Bm25Index, DenseIndex, Embedder};
use crate::pack::{self, EmbedConfig, PackManifest};
use crate::retrieve::{self, DEFAULT_CANDIDATE_POOL, RerankGate, SearchRequest};

use super::staleness;

/// `docs_get`'s default token budget: `docs_get(pack_id, topic, tokens =
/// 4000)`.
pub const DEFAULT_TOKEN_BUDGET: usize = retrieve::DEFAULT_TOKEN_BUDGET;

/// One snippet in a [`DocsGetResponse`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snippet {
    /// The chunk's (possibly truncated, per Rule 28) text, with a
    /// staleness banner prepended when the pack is stale.
    pub text: String,
    /// The chunk's breadcrumb.
    pub breadcrumb: String,
    /// The chunk's source URL, when it has one.
    pub url: Option<String>,
    /// The chunk's identifier.
    pub chunk_id: String,
}

/// The pack summary in a [`DocsGetResponse`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackSummary {
    /// The pack name.
    pub name: String,
    /// The pack version.
    pub version: String,
    /// When the pack was ingested, rendered as its TOML text.
    pub ingested_at: String,
    /// How many days old the ingest is.
    pub age_days: u32,
    /// Whether `age_days` exceeds the pack's own staleness policy.
    pub stale: bool,
}

/// What `docs_get` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocsGetResponse {
    /// The retrieved snippets, best first.
    pub snippets: Vec<Snippet>,
    /// The pack this response came from.
    pub pack: PackSummary,
    /// Which retrieval tiers contributed: `"bm25"`, `"dense"`, `"rerank"`.
    pub tiers_used: Vec<String>,
}

/// What a [`docs_get_from_parts`] call needs beyond the pack itself.
pub struct DocsGetDeps<'a> {
    /// Embeds the query for the dense tier. `None` skips the dense tier
    /// outright — the lexical index still answers alone.
    pub embedder: Option<&'a dyn Embedder>,
    /// The harness's live embedding configuration, compared against the
    /// pack's `[embed]` block (`crate::pack::embed::compare`). Required,
    /// alongside `embedder` and a non-empty dense index, to use the dense
    /// tier at all — a mismatch means "serve lexical results only", the
    /// same fallback `crate::pack::embed`'s module docs describe.
    pub current_embed: Option<&'a EmbedConfig>,
    /// The rerank gate. `None` skips reranking; the caller decides this
    /// once, based on `Caps::logprobs`, before ever building a gate.
    pub reranker: Option<&'a RerankGate<'a>>,
}

/// Renders the warning that Do 3 asks every snippet in a stale pack's
/// response to carry, in the text itself, not only in `pack.stale`.
fn staleness_banner(manifest: &PackManifest, age_days: u32) -> String {
    format!(
        "[stale documentation] this pack ({} {}) was ingested {age_days} day(s) ago, beyond \
         its {} staleness policy. Treat details as possibly outdated; run `dark pack refresh` \
         to update it.",
        manifest.pack.name, manifest.pack.version, manifest.staleness.policy
    )
}

/// Answers `docs_get`'s question against already-loaded data: no
/// filesystem access, so a test drives this directly. See the module
/// docs for why this exists alongside [`docs_get`].
///
/// # Errors
///
/// Returns whatever [`retrieve::search`] returns. Returns `E_TOOL_FAILED`
/// when `manifest`'s staleness policy or ingest date does not parse.
pub fn docs_get_from_parts(
    manifest: &PackManifest,
    chunks: &[Chunk],
    bm25: &Bm25Index,
    dense: Option<&DenseIndex>,
    topic: &str,
    tokens: usize,
    deps: &DocsGetDeps<'_>,
) -> Result<DocsGetResponse> {
    let mut query_vector: Option<Vec<f32>> = None;
    if let (Some(dense_index), Some(embedder), Some(current)) =
        (dense, deps.embedder, deps.current_embed)
    {
        if !dense_index.is_empty() && pack::compare_embed(&manifest.embed, current).is_match() {
            let vectors = embedder.embed(&[topic.to_owned()], EmbedPurpose::Query)?;
            query_vector = vectors.into_iter().next();
        }
    }
    let dense_pair = dense.zip(query_vector.as_deref());

    let response = retrieve::search(&SearchRequest {
        chunks,
        bm25,
        dense: dense_pair,
        query: topic,
        reranker: deps.reranker,
        candidate_pool: DEFAULT_CANDIDATE_POOL,
        token_budget: tokens,
    })?;

    let (age_days, stale) = staleness::evaluate(manifest)?;
    let banner = stale.then(|| staleness_banner(manifest, age_days));

    let snippets = response
        .hits
        .into_iter()
        .filter_map(|hit| {
            chunks.get(hit.chunk_index).map(|chunk| {
                let text = match &banner {
                    Some(banner) => format!("{banner}\n\n{}", hit.text),
                    None => hit.text,
                };
                Snippet {
                    text,
                    breadcrumb: chunk.breadcrumb.clone(),
                    url: chunk.url.clone(),
                    chunk_id: chunk.chunk_id.clone(),
                }
            })
        })
        .collect();

    Ok(DocsGetResponse {
        snippets,
        pack: PackSummary {
            name: manifest.pack.name.clone(),
            version: manifest.pack.version.clone(),
            ingested_at: manifest.ingest.at.to_string(),
            age_days,
            stale,
        },
        tiers_used: response.tiers_used.into_iter().map(str::to_owned).collect(),
    })
}

/// Reads every chunk from `<dir>/chunks.jsonl`, one JSON object per line.
///
/// # Errors
///
/// Returns `E_PACK_NOT_FOUND` when the file is absent. Returns
/// `E_TOOL_FAILED` when a line does not parse.
pub(crate) fn read_chunks(dir: &Path) -> Result<Vec<Chunk>> {
    let path = dir.join(pack::CHUNKS_FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|source| {
        Error::new(
            ErrCode::PackNotFound,
            format!("cannot read {}: {source}", path.display()),
        )
    })?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<Chunk>(line).map_err(|source| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!("{} does not parse: {source}", path.display()),
                )
            })
        })
        .collect()
}

/// Loads a pack from `packs_root` and answers `docs_get`'s question
/// against it.
///
/// This is the thin filesystem-and-hash-verification layer around
/// [`docs_get_from_parts`] — production code calls this; a test that wants
/// to skip real files calls the pure function directly.
///
/// # Errors
///
/// Returns `E_PACK_NOT_FOUND` when the pack directory, its manifest, its
/// chunk store, or its lexical index is absent. Returns `E_TOOL_FAILED`
/// when the pack hash does not verify, a stored file does not parse, or
/// the dense index will not decode. See [`docs_get_from_parts`] for
/// retrieval errors.
pub fn docs_get(
    packs_root: &Path,
    pack_id: &str,
    topic: &str,
    tokens: usize,
    deps: &DocsGetDeps<'_>,
) -> Result<DocsGetResponse> {
    let dir = packs_root.join(pack_id);
    pack::hash::verify(&dir)?;
    let manifest = PackManifest::read_from_dir(&dir)?;
    let chunks = read_chunks(&dir)?;

    let bm25_path = dir.join(pack::BM25_INDEX_FILE_NAME);
    let bm25_bytes = std::fs::read(&bm25_path).map_err(|source| {
        Error::new(
            ErrCode::PackNotFound,
            format!("cannot read {}: {source}", bm25_path.display()),
        )
    })?;
    let bm25 = Bm25Index::from_bytes(&bm25_bytes)?;

    let dense_path = dir.join(pack::DENSE_VECTORS_FILE_NAME);
    let dense = if dense_path.is_file() {
        let bytes = std::fs::read(&dense_path).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {}: {source}", dense_path.display()),
            )
        })?;
        Some(DenseIndex::from_bytes(&bytes)?)
    } else {
        None
    };

    docs_get_from_parts(
        &manifest,
        &chunks,
        &bm25,
        dense.as_ref(),
        topic,
        tokens,
        deps,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{EmbedBlock, Ingest, License, PackId, Source, Staleness};

    fn chunk(id: &str, breadcrumb: &str, body: &str, url: Option<&str>) -> Chunk {
        Chunk {
            chunk_id: id.to_owned(),
            ordinal: 0,
            breadcrumb: breadcrumb.to_owned(),
            url: url.map(ToOwned::to_owned),
            body: body.to_owned(),
            embed_text: format!("{breadcrumb}\n\n{body}"),
            tokens: body.split_whitespace().count(),
            oversize: false,
        }
    }

    fn manifest(policy: &str, ingested_at: &str) -> PackManifest {
        PackManifest {
            pack: PackId {
                name: "tokio".to_owned(),
                version: "1.47.0".to_owned(),
                ecosystem: "crates.io".to_owned(),
                aliases: vec![],
            },
            source: Source {
                kind: "docsrs".to_owned(),
                url: "https://docs.rs/tokio/1.47.0/tokio/".to_owned(),
                etag: String::new(),
                commit: String::new(),
            },
            ingest: Ingest {
                at: ingested_at.parse().unwrap(),
                tool_version: "1.0.0".to_owned(),
                chunker: "heading-v1".to_owned(),
                chunks: 1,
            },
            embed: EmbedBlock {
                model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
                dim: 2,
                quant: "int8".to_owned(),
                query_prefix: String::new(),
                doc_prefix: String::new(),
            },
            staleness: Staleness {
                policy: policy.to_owned(),
            },
            license: License {
                spdx: "MIT".to_owned(),
                notice_required: true,
            },
        }
    }

    fn recent_manifest() -> PackManifest {
        let (year, month, day) = staleness::civil_from_days(staleness::today_epoch_day().unwrap());
        manifest("90d", &format!("{year:04}-{month:02}-{day:02}T00:00:00Z"))
    }

    fn corpus() -> Vec<Chunk> {
        vec![
            chunk(
                "a",
                "tokio › runtime",
                "The runtime schedules async tasks.",
                Some("https://docs.rs/tokio#runtime"),
            ),
            chunk("b", "tokio › fs", "Reads and writes files on disk.", None),
        ]
    }

    #[test]
    fn a_fresh_pack_returns_snippets_with_no_staleness_banner() {
        let manifest = recent_manifest();
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let deps = DocsGetDeps {
            embedder: None,
            current_embed: None,
            reranker: None,
        };

        let response = docs_get_from_parts(
            &manifest,
            &chunks,
            &bm25,
            None,
            "async tasks",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();

        assert!(!response.pack.stale);
        assert!(!response.snippets.is_empty());
        assert!(!response.snippets[0].text.contains("stale"));
        assert_eq!(response.snippets[0].breadcrumb, "tokio › runtime");
        assert_eq!(
            response.snippets[0].url.as_deref(),
            Some("https://docs.rs/tokio#runtime")
        );
        assert_eq!(response.snippets[0].chunk_id, "a");
        assert_eq!(response.tiers_used, vec!["bm25".to_owned()]);
    }

    #[test]
    fn a_stale_pack_puts_the_warning_in_every_snippets_text() {
        let manifest = manifest("90d", "2000-01-01T00:00:00Z");
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let deps = DocsGetDeps {
            embedder: None,
            current_embed: None,
            reranker: None,
        };

        let response = docs_get_from_parts(
            &manifest,
            &chunks,
            &bm25,
            None,
            "runtime files",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();

        assert!(response.pack.stale);
        assert!(response.pack.age_days > 90);
        assert!(!response.snippets.is_empty());
        for snippet in &response.snippets {
            assert!(
                snippet.text.contains("stale"),
                "every snippet's text must carry the warning, not just the metadata"
            );
            assert!(snippet.text.contains("dark pack refresh"));
        }
    }

    struct FixedEmbedder;
    impl Embedder for FixedEmbedder {
        fn embed(&self, texts: &[String], _purpose: EmbedPurpose) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn a_matching_embed_config_enables_the_dense_tier() {
        let manifest = recent_manifest();
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let dense = DenseIndex::build(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        let embedder = FixedEmbedder;
        let current = EmbedConfig {
            model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
            dim: 2,
            quant: "int8".to_owned(),
        };
        let deps = DocsGetDeps {
            embedder: Some(&embedder),
            current_embed: Some(&current),
            reranker: None,
        };

        let response = docs_get_from_parts(
            &manifest,
            &chunks,
            &bm25,
            Some(&dense),
            "async tasks",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();
        assert!(response.tiers_used.contains(&"dense".to_owned()));
    }

    #[test]
    fn a_mismatched_embed_config_falls_back_to_lexical_only() {
        let manifest = recent_manifest();
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let dense = DenseIndex::build(&[vec![1.0, 0.0], vec![0.0, 1.0]]).unwrap();
        let embedder = FixedEmbedder;
        let current = EmbedConfig {
            model: "a-different-model".to_owned(),
            dim: 2,
            quant: "int8".to_owned(),
        };
        let deps = DocsGetDeps {
            embedder: Some(&embedder),
            current_embed: Some(&current),
            reranker: None,
        };

        let response = docs_get_from_parts(
            &manifest,
            &chunks,
            &bm25,
            Some(&dense),
            "async tasks",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25".to_owned()]);
    }

    #[test]
    fn no_dense_index_at_all_still_answers_from_bm25() {
        let manifest = recent_manifest();
        let chunks = corpus();
        let bm25 = Bm25Index::build(&chunks);
        let deps = DocsGetDeps {
            embedder: None,
            current_embed: None,
            reranker: None,
        };
        let response = docs_get_from_parts(
            &manifest,
            &chunks,
            &bm25,
            None,
            "async tasks",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();
        assert_eq!(response.tiers_used, vec!["bm25".to_owned()]);
        assert!(!response.snippets.is_empty());
    }

    #[test]
    fn docs_get_from_disk_reads_a_written_pack() {
        let dir = tempfile::tempdir().unwrap();
        let pack_dir = dir.path().join("tokio@1.47.0");
        std::fs::create_dir_all(&pack_dir).unwrap();

        let manifest = recent_manifest();
        manifest.write_to_dir(&pack_dir).unwrap();

        let chunks = corpus();
        let mut jsonl = String::new();
        for chunk in &chunks {
            jsonl.push_str(&serde_json::to_string(chunk).unwrap());
            jsonl.push('\n');
        }
        std::fs::write(pack_dir.join(pack::CHUNKS_FILE_NAME), jsonl).unwrap();

        let bm25 = Bm25Index::build(&chunks);
        std::fs::write(pack_dir.join(pack::BM25_INDEX_FILE_NAME), bm25.to_bytes()).unwrap();
        std::fs::write(pack_dir.join(pack::GRAPH_FILE_NAME), b"{}").unwrap();
        std::fs::write(pack_dir.join(pack::LICENSE_FILE_NAME), b"MIT License").unwrap();

        pack::hash::write(&pack_dir).unwrap();

        let deps = DocsGetDeps {
            embedder: None,
            current_embed: None,
            reranker: None,
        };
        let response = docs_get(
            dir.path(),
            "tokio@1.47.0",
            "async tasks",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap();
        assert!(!response.snippets.is_empty());
        assert_eq!(response.pack.name, "tokio");
    }

    #[test]
    fn docs_get_reports_pack_not_found_for_a_missing_pack() {
        let dir = tempfile::tempdir().unwrap();
        let deps = DocsGetDeps {
            embedder: None,
            current_embed: None,
            reranker: None,
        };
        let err = docs_get(
            dir.path(),
            "nonexistent@0.0.0",
            "anything",
            DEFAULT_TOKEN_BUDGET,
            &deps,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrCode::PackNotFound);
    }
}
