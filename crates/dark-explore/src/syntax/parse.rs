//! One parsed file.

use std::path::PathBuf;
use std::sync::Arc;

use super::language::Language;

/// A file that the syntax stage parsed.
///
/// `tree` always exists, even over source that does not parse cleanly:
/// `tree-sitter` recovers from a syntax error by inserting an `ERROR` node
/// and continuing, rather than by failing outright. Call
/// [`tree_sitter::Node::has_error`] on the root node to check for one.
#[derive(Clone)]
pub struct ParsedFile {
    /// The file path, relative to the repository root.
    pub path: PathBuf,
    /// The language that `tree` was parsed with.
    pub language: Language,
    /// The `BLAKE3` hash of `source`. The cache keys this file's entry by
    /// this value.
    pub blob_hash: blake3::Hash,
    /// The full source text that produced `tree`.
    pub source: Arc<[u8]>,
    /// The parsed syntax tree.
    pub tree: tree_sitter::Tree,
}

impl std::fmt::Debug for ParsedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedFile")
            .field("path", &self.path)
            .field("language", &self.language.name())
            .field("blob_hash", &self.blob_hash)
            .field("source_len", &self.source.len())
            .field("has_error", &self.tree.root_node().has_error())
            .finish()
    }
}
