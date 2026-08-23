//! The pack manifest: `pack.toml`.
//!
//! [`PackManifest`] mirrors the manifest that task unit `G1` of the build
//! specification gives verbatim. Every field name and section name in this
//! module matches that sample exactly, so a hand-written `pack.toml` parses
//! without translation.

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};
use serde::{Deserialize, Serialize};

/// The file name of the manifest inside a pack directory.
pub const MANIFEST_FILE_NAME: &str = "pack.toml";

/// The whole manifest.
///
/// Serializes to, and deserializes from, the TOML text at `pack.toml`. Each
/// field groups under the same section name that the specification uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    /// The pack identity.
    pub pack: PackId,
    /// Where the documentation came from.
    pub source: Source,
    /// How the pack was built.
    pub ingest: Ingest,
    /// The embedding model that produced `dense.vec`.
    pub embed: EmbedBlock,
    /// The staleness policy.
    pub staleness: Staleness,
    /// The upstream licence. See Rule 26.
    pub license: License,
}

/// The `[pack]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackId {
    /// The library name, for example `tokio`.
    pub name: String,
    /// The version string, for example `1.47.0`.
    pub version: String,
    /// The ecosystem that hosts the library, for example `crates.io`.
    pub ecosystem: String,
    /// Other names that a lookup should also match.
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl PackId {
    /// Returns the pack identifier, for example `tokio@1.47.0`.
    ///
    /// This is the directory name that task unit `G1` specifies:
    /// `packs/tokio@1.47.0/`.
    #[must_use]
    pub fn pack_id(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

/// The `[source]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// Which adapter produced this pack, for example `docsrs`.
    pub kind: String,
    /// The location that the adapter read.
    pub url: String,
    /// The HTTP entity tag at ingest time, when the source served one.
    #[serde(default)]
    pub etag: String,
    /// The commit or tag that the adapter read, when the source is a
    /// repository.
    #[serde(default)]
    pub commit: String,
}

/// The `[ingest]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ingest {
    /// When the ingest ran.
    pub at: toml::value::Datetime,
    /// The version of the tool that ran the ingest.
    pub tool_version: String,
    /// The chunking algorithm name, for example `heading-v1`.
    pub chunker: String,
    /// The number of chunks that the ingest produced.
    pub chunks: u64,
}

/// The `[embed]` section.
///
/// [`crate::pack::embed::compare`] compares this block against the harness's
/// current embedding configuration to detect a model change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbedBlock {
    /// The embedding model identifier.
    pub model: String,
    /// The vector width.
    pub dim: u32,
    /// The quantisation of the stored vectors, for example `int8`.
    pub quant: String,
    /// The prefix that the model expects before a query.
    #[serde(default)]
    pub query_prefix: String,
    /// The prefix that the model expects before a stored document.
    #[serde(default)]
    pub doc_prefix: String,
}

/// The `[staleness]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Staleness {
    /// The staleness policy, for example `90d`.
    pub policy: String,
}

/// The `[license]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct License {
    /// The SPDX identifier, when the adapter could determine one.
    ///
    /// An empty string means a licence exists but its SPDX identifier is
    /// not known. Rule 26 gates on the licence existing, not on this field.
    #[serde(default)]
    pub spdx: String,
    /// The harness must show the attribution notice before it shows a
    /// chunk from this pack. See Rule 27.
    pub notice_required: bool,
}

impl PackManifest {
    /// Parses a manifest from TOML text.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when `text` is not valid TOML, or does not
    /// match the manifest shape.
    pub fn from_toml_str(text: &str) -> Result<Self> {
        toml::from_str(text).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("pack.toml does not parse: {source}"),
            )
        })
    }

    /// Renders the manifest as TOML text.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when a field will not serialize, which does
    /// not happen for a manifest built from valid Rust values.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("pack.toml will not serialize: {source}"),
            )
        })
    }

    /// Reads and parses the manifest at `<dir>/pack.toml`.
    ///
    /// # Errors
    ///
    /// Returns `E_PACK_NOT_FOUND` when the file is absent. Returns
    /// `E_TOOL_FAILED` when the file exists but does not parse.
    pub fn read_from_dir(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE_NAME);
        let text = std::fs::read_to_string(&path).map_err(|source| {
            Error::new(
                ErrCode::PackNotFound,
                format!("cannot read {}: {source}", path.display()),
            )
        })?;
        Self::from_toml_str(&text)
    }

    /// Writes the manifest to `<dir>/pack.toml`.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when the manifest will not serialize, or the
    /// file will not write.
    pub fn write_to_dir(&self, dir: &Path) -> Result<()> {
        let path = dir.join(MANIFEST_FILE_NAME);
        let text = self.to_toml_string()?;
        std::fs::write(&path, text).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot write {}: {source}", path.display()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[pack]
name = "tokio"
version = "1.47.0"
ecosystem = "crates.io"
aliases = ["tokio-rs"]

[source]
kind = "docsrs"
url  = "https://docs.rs/tokio/1.47.0/tokio/"
etag = ""
commit = ""

[ingest]
at = 2026-08-19T11:03:00Z
tool_version = "1.0.0"
chunker = "heading-v1"
chunks = 3104

[embed]
model = "Qwen/Qwen3-Embedding-0.6B"
dim = 1024
quant = "int8"
query_prefix = "Instruct: retrieve documentation\nQuery: "
doc_prefix = ""

[staleness]
policy = "90d"

[license]
spdx = "MIT"
notice_required = true
"#;

    #[test]
    fn the_sample_manifest_from_the_specification_parses() {
        let manifest = PackManifest::from_toml_str(SAMPLE).expect("parses");
        assert_eq!(manifest.pack.name, "tokio");
        assert_eq!(manifest.pack.pack_id(), "tokio@1.47.0");
        assert_eq!(manifest.pack.aliases, vec!["tokio-rs".to_string()]);
        assert_eq!(manifest.source.kind, "docsrs");
        assert_eq!(manifest.ingest.chunker, "heading-v1");
        assert_eq!(manifest.ingest.chunks, 3104);
        assert_eq!(manifest.embed.dim, 1024);
        assert_eq!(manifest.staleness.policy, "90d");
        assert_eq!(manifest.license.spdx, "MIT");
        assert!(manifest.license.notice_required);
    }

    #[test]
    fn a_manifest_round_trips_through_toml_text() {
        let manifest = PackManifest::from_toml_str(SAMPLE).expect("parses");
        let text = manifest.to_toml_string().expect("serializes");
        let reparsed = PackManifest::from_toml_str(&text).expect("reparses");
        assert_eq!(manifest, reparsed);
    }

    #[test]
    fn reading_a_missing_manifest_reports_pack_not_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = PackManifest::read_from_dir(dir.path()).unwrap_err();
        assert_eq!(err.code, ErrCode::PackNotFound);
    }

    #[test]
    fn writing_then_reading_a_manifest_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = PackManifest::from_toml_str(SAMPLE).expect("parses");
        manifest.write_to_dir(dir.path()).expect("writes");
        let reread = PackManifest::read_from_dir(dir.path()).expect("reads");
        assert_eq!(manifest, reread);
    }

    #[test]
    fn malformed_toml_reports_tool_failed() {
        let err = PackManifest::from_toml_str("not = [valid").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
