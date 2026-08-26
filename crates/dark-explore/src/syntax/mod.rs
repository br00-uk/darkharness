//! The `syntax` stage.
//!
//! The syntax stage parses each file that [`discover`](crate::discover)
//! kept, using the `tree-sitter` grammar for one of thirteen supported
//! languages (F1, "Do" item 5). It parses files in parallel with `rayon`,
//! and it caches a parsed tree by the file's blob hash so that an
//! incremental run re-parses only the files that changed (F1, "Do" items 6
//! and 7). See task unit `F1`.

mod cache;
mod language;
mod parse;

pub use cache::Cache;
pub use cache::parse as parse_snapshot;
pub use language::{Language, MAX_SUPPORTED_ABI, MIN_SUPPORTED_ABI};
pub use parse::ParsedFile;
