//! The `tools` stage: task unit `G5`.
//!
//! Two entry points the PRD gives verbatim:
//!
//! ```text
//! docs_resolve(query)
//!   -> [{pack_id, name, version, confidence, why}]
//!
//! docs_get(pack_id, topic, tokens = 4000)
//!   -> { snippets: [{text, breadcrumb, url, chunk_id}],
//!        pack: {name, version, ingested_at, age_days, stale},
//!        tiers_used: ["bm25","dense","rerank"] }
//! ```
//!
//! [`resolve::docs_resolve`] is [`resolve`]. [`get::docs_get`] and
//! [`get::docs_get_from_parts`] are [`get`]; [`staleness`] is the date
//! arithmetic `docs_get`'s `pack.age_days` and `pack.stale` fields need.
//!
//! Neither function here implements `dark_contract::Tool`: doing so needs
//! `async-trait` and `tokio-util` (`ToolCtx` names
//! `tokio_util::sync::CancellationToken` directly), which are direct
//! dependencies of `dark-contract` but not of `dark-lexicon` — this
//! crate's task units may not add either (the same Rule 16 boundary
//! `crate::chunk`'s and `crate::ingest::fetch`'s module docs describe for
//! `Engine` and for an HTTP client). Both functions here are plain,
//! synchronous, and JSON-serialisable, ready for a later change to wrap in
//! a `dark_contract::Tool` impl in a crate that already depends on those
//! two — `dark-tools` or `dark-core`.

pub mod get;
pub mod resolve;
pub mod staleness;

pub use get::{
    DEFAULT_TOKEN_BUDGET, DocsGetDeps, DocsGetResponse, PackSummary, Snippet, docs_get,
    docs_get_from_parts,
};
pub use resolve::{CONFIDENCE_FLOOR, MAX_CANDIDATES, ResolveCandidate, docs_resolve};
