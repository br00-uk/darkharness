//! `tree_sha`: the commit half of Rule 29's promise.
//!
//! [`config_hash`](super::config_hash) is the configuration half; this is
//! the commit half. It hashes the same `(path, blob_hash)` pairs
//! [`Snapshot::tree_hash`](crate::discover::Snapshot::tree_hash) does, over
//! the same sorted file list, but keyed on [`path_to_string`]'s `/`-joined
//! form rather than [`Path::as_os_str`]'s native bytes.
//!
//! That is a deliberate difference, not a rounding choice. See
//! `output::path`'s module documentation for the full argument; in short,
//! [`Snapshot::tree_hash`] hashes native path bytes, which on Windows use
//! `\` rather than `/`, so two paths whose relative order depends on which
//! separator byte sits between them — a nested path against a sibling whose
//! next byte falls between `/` and `\` — can come out in a different order,
//! and therefore hash differently, than the identical repository checked
//! out and analysed on Linux or macOS. F4's own "done when" requires
//! byte-identical output across all three, so `tree_sha` cannot inherit a
//! hash keyed on a separator that is not itself identical across them; this
//! function recomputes it, over the same [`DiscoveredFile`] list `discover`
//! already produced, normalised the way every other path in this output is
//! normalised.
//!
//! This is a note for whoever next touches
//! `discover::walk::tree_hash` — not a fix to it. `discover` belongs to a
//! different, already-landed task unit; this module works around the gap
//! locally, in the one file this task unit owns that the gap actually
//! reaches, rather than editing a file it does not own.

use crate::discover::DiscoveredFile;

use super::path::path_to_string;

/// Hashes `files` the way [`Snapshot::tree_hash`](crate::discover::Snapshot::tree_hash)
/// does, keyed on the `/`-joined path form. See the module documentation.
///
/// `files` should already be sorted — [`crate::discover::discover`]'s
/// result always is — but this function does not re-sort it: two
/// differently-sorted inputs are two different hashes by design, the same
/// way `Snapshot::tree_hash` behaves.
#[must_use]
pub fn tree_sha(files: &[DiscoveredFile]) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();
    for file in files {
        let path_bytes = path_to_string(&file.path).into_bytes();
        hasher.update(&(path_bytes.len() as u64).to_le_bytes());
        hasher.update(&path_bytes);
        hasher.update(file.blob_hash.as_bytes());
    }
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn file(path: &str, content: &[u8]) -> DiscoveredFile {
        DiscoveredFile {
            path: PathBuf::from(path),
            size: content.len() as u64,
            blob_hash: blake3::hash(content),
        }
    }

    #[test]
    fn the_same_files_hash_the_same_every_time() {
        let files = vec![file("a.rs", b"fn a() {}"), file("src/b.rs", b"fn b() {}")];
        assert_eq!(tree_sha(&files), tree_sha(&files));
    }

    #[test]
    fn a_changed_blob_hash_changes_the_tree_sha() {
        let before = vec![file("a.rs", b"fn a() {}")];
        let after = vec![file("a.rs", b"fn a() { changed() }")];
        assert_ne!(tree_sha(&before), tree_sha(&after));
    }

    #[test]
    fn a_changed_path_changes_the_tree_sha() {
        let a = vec![file("a.rs", b"same content")];
        let b = vec![file("b.rs", b"same content")];
        assert_ne!(tree_sha(&a), tree_sha(&b));
    }

    #[test]
    fn a_nested_path_hashes_by_its_forward_slash_form() {
        // Constructing the PathBuf via `join` mirrors how the walker builds
        // one on every platform, including Windows, where `join` inserts
        // the native separator rather than `/`.
        let joined = PathBuf::from("src").join("lib.rs");
        let files = vec![DiscoveredFile {
            path: joined,
            size: 4,
            blob_hash: blake3::hash(b"code"),
        }];
        let expected = vec![file("src/lib.rs", b"code")];
        assert_eq!(tree_sha(&files), tree_sha(&expected));
    }
}
