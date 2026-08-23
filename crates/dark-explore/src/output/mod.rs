//! The `output` stage: write the report and lock it. Task unit `F4`.
//!
//! Stages 1 to 5 of `/explore` — discovery, extraction, graph building,
//! metrics, and seam scoring — carry Rule 29's promise as far as an
//! in-memory value. This module is where that promise becomes bytes on
//! disk: [`document::build`] turns a [`document::Sources`] value into the
//! [`document::Document`] task unit `F4`'s "Do" item 1 describes, and
//! [`write::write`] serialises it — pretty-printed JSON, `\n` line endings
//! on every platform, no timestamp anywhere in either file (Rule 31) — to
//! `.dark/explore/<tree-sha>.json`, alongside the `.lock` file
//! [`lock::Lock`] describes.
//!
//! # Reading order, for whoever reads this module next
//!
//! 1. `path` — every path this stage writes goes through
//!    [`path::path_to_string`] first. Read this one first: the reasoning
//!    here is why `tree`, `config_hash`, and `document` all sort by string
//!    rather than by [`crate::discover::compare_paths`] on a raw [`Path`].
//! 2. `tree` — `tree_sha`, the commit half of Rule 29's promise.
//! 3. `config_hash` — the configuration half.
//! 4. `document` — the report shape itself, the field mappings from the
//!    PRD's example, and the rounding and sorting rules.
//! 5. `lock` — the `.lock` file.
//! 6. `write` — turning the two into bytes on disk.
//!
//! [`Path`]: std::path::Path

mod config_hash;
mod document;
mod lock;
mod path;
mod tree;
mod write;

pub use document::{
    Bridge, Document, HOTSPOT_CA_WEIGHT, HOTSPOT_CHURN_WEIGHT, HOTSPOT_D_WEIGHT, Hotspot,
    MAX_REPORTED_HOTSPOTS, MAX_REPORTED_SEAMS, Module, ROUND_DECIMALS, Seam, Sources, Stats,
    VERSION, build,
};
pub use lock::{Lock, grammar_versions};
pub use path::path_to_string;
pub use tree::tree_sha;
pub use write::{WrittenPaths, build_lock, document_bytes, lock_bytes, write};
