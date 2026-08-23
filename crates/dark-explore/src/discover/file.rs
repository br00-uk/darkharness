//! The record that discovery produces for one file.

use std::path::PathBuf;

/// One file that survived discovery's filters.
///
/// The path is relative to the repository root, so a [`Snapshot`](super::Snapshot)
/// stays byte-identical across machines that check out the same commit to a
/// different absolute location. See Rule 29.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    /// The file path, relative to the repository root.
    pub path: PathBuf,
    /// The file size, in bytes, at the time discovery read it.
    pub size: u64,
    /// The `BLAKE3` hash of the file's full byte content.
    ///
    /// The syntax stage sub-caches a parsed tree by this hash. See F1,
    /// "Do" item 6.
    pub blob_hash: blake3::Hash,
}
