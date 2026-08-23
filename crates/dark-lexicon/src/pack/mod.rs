//! The `pack` stage: task unit `G1`.
//!
//! A pack holds one library's documentation in a portable, verifiable
//! directory:
//!
//! ```text
//! packs/tokio@1.47.0/
//! ├── pack.toml
//! ├── chunks.jsonl
//! ├── bm25.idx
//! ├── dense.vec
//! ├── graph.json
//! └── LICENSE
//! ```
//!
//! [`manifest`] defines `pack.toml`. [`hash`] computes and verifies the
//! pack hash that guards every other file (`pack.hash`, written next to
//! `pack.toml`). [`darkpack`] packs and unpacks the single-file
//! `.darkpack` form: a zstd-compressed tar of the whole directory.
//! [`embed`] detects an embedding model change by comparing the
//! manifest's `[embed]` block against the harness's live configuration.
//!
//! ## A note on the pack hash and the error taxonomy
//!
//! Task unit `G1` asks the harness to "verify the pack hash before use".
//! The error taxonomy in `dark-contract` (owned by task unit `Z1`, which
//! this task unit does not touch) gives the `Pack` domain three codes:
//! [`dark_contract::ErrCode::PackNoLicence`],
//! [`dark_contract::ErrCode::PackDimMismatch`], and
//! [`dark_contract::ErrCode::PackNotFound`]. None of the three names a
//! corrupted or hand-edited pack. [`hash::verify`] reports that case as
//! [`dark_contract::ErrCode::ToolFailed`] — the domain-agnostic code the
//! taxonomy documents as covering "a reason that no other code covers" —
//! with a specific message and remedy attached. A future task unit that
//! revisits `dark-contract` could add a dedicated `E_PACK_CORRUPT` code;
//! until then this is the closest honest fit.

pub mod darkpack;
pub mod embed;
pub mod hash;
pub mod manifest;

pub use darkpack::{export as export_darkpack, import as import_darkpack};
pub use embed::{EmbedConfig, EmbedStatus, compare as compare_embed};
pub use hash::{HASH_FILE_NAME, PackHash};
pub use manifest::{
    EmbedBlock, Ingest, License, MANIFEST_FILE_NAME, PackId, PackManifest, Source, Staleness,
};

/// The file name of a pack directory's licence file.
pub const LICENSE_FILE_NAME: &str = "LICENSE";

/// The file name of the lexical index.
pub const BM25_INDEX_FILE_NAME: &str = "bm25.idx";

/// The file name of the dense vector store.
pub const DENSE_VECTORS_FILE_NAME: &str = "dense.vec";

/// The file name of the retrieval graph.
pub const GRAPH_FILE_NAME: &str = "graph.json";

/// The file name of the chunk store.
pub const CHUNKS_FILE_NAME: &str = "chunks.jsonl";

/// The file extension of the single-file pack form.
pub const DARKPACK_EXTENSION: &str = "darkpack";
