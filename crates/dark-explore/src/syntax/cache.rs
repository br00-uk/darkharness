//! The parse cache: one entry per tree hash, sub-cached by blob hash.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use dark_contract::{ErrCode, Error, Result};
use rayon::prelude::*;

use super::language::Language;
use super::parse::ParsedFile;
use crate::discover::{DiscoveredFile, Snapshot};

/// The parse results from a previous run, kept to make the next run
/// incremental.
///
/// A [`Cache`] sub-caches each file by its blob hash (F1, "Do" item 6): a
/// file whose content did not change since the last run is not re-parsed,
/// no matter how the rest of the tree changed. It also remembers the
/// [`Snapshot::tree_hash`] of the run that built it, so a run over an
/// unchanged tree can skip straight to returning the cached files.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    tree_hash: Option<blake3::Hash>,
    by_blob: HashMap<blake3::Hash, Arc<ParsedFile>>,
}

impl Cache {
    /// Returns an empty cache. The first run against it parses every file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of parsed files that this cache holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_blob.len()
    }

    /// Returns `true` when this cache holds no parsed files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_blob.is_empty()
    }
}

/// Parses every file in `snapshot` that has a supported language, reusing
/// `cache` for any file whose blob hash it already holds.
///
/// `root` is the repository root that `snapshot`'s paths are relative to.
/// A file whose extension matches no supported grammar (see
/// [`Language::from_path`]) is silently skipped: it is not an error for a
/// repository to hold files outside the thirteen supported languages.
///
/// The returned [`Vec`] holds one entry per parsed file, in `snapshot`'s
/// order — which is `snapshot.files`' sort order, the byte comparator order
/// from Rule 30. Running this function twice against the same `snapshot`
/// and the same file content on disk, regardless of how the parallel
/// parsing step interleaves, produces trees whose
/// [`tree_sitter::Node::to_sexp`] strings are identical file for file. See
/// Rule 32.
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when a file cannot be read from disk,
/// when its grammar fails to load, or when `tree-sitter` returns no tree
/// (which happens only on cancellation; this function sets no cancellation
/// flag, so this indicates a bug in `tree-sitter` itself).
pub fn parse(
    cache: &Cache,
    root: &Path,
    snapshot: &Snapshot,
) -> Result<(Vec<Arc<ParsedFile>>, Cache)> {
    if cache.tree_hash == Some(snapshot.tree_hash) {
        let files: Vec<Arc<ParsedFile>> = snapshot
            .files
            .iter()
            .filter_map(|file| cache.by_blob.get(&file.blob_hash).cloned())
            .collect();
        return Ok((files, cache.clone()));
    }

    let parsed: Vec<Option<Arc<ParsedFile>>> = snapshot
        .files
        .par_iter()
        .map(|file| match cache.by_blob.get(&file.blob_hash) {
            Some(hit) => Ok(Some(Arc::clone(hit))),
            None => parse_one(root, file),
        })
        .collect::<Result<Vec<_>>>()?;

    let mut by_blob = HashMap::with_capacity(parsed.len());
    let mut files = Vec::with_capacity(parsed.len());
    for (file, parsed) in snapshot.files.iter().zip(parsed) {
        if let Some(parsed) = parsed {
            by_blob.insert(file.blob_hash, Arc::clone(&parsed));
            files.push(parsed);
        }
    }

    Ok((
        files,
        Cache {
            tree_hash: Some(snapshot.tree_hash),
            by_blob,
        },
    ))
}

/// Reads and parses one file, or returns `Ok(None)` when its extension
/// names no supported language.
fn parse_one(root: &Path, file: &DiscoveredFile) -> Result<Option<Arc<ParsedFile>>> {
    let Some(language) = Language::from_path(&file.path) else {
        return Ok(None);
    };

    let abs_path = root.join(&file.path);
    let source = fs::read(&abs_path).map_err(|source_err| {
        Error::new(
            ErrCode::ExploreParse,
            format!("failed to read {}: {source_err}", abs_path.display()),
        )
    })?;

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language.grammar())
        .map_err(|grammar_err| {
            Error::new(
                ErrCode::ExploreParse,
                format!("{}: grammar failed to load: {grammar_err}", language.name()),
            )
        })?;

    let tree = parser.parse(&source, None).ok_or_else(|| {
        Error::new(
            ErrCode::ExploreParse,
            format!("{}: the parser returned no tree", abs_path.display()),
        )
    })?;

    Ok(Some(Arc::new(ParsedFile {
        path: file.path.clone(),
        language,
        blob_hash: file.blob_hash,
        source: Arc::from(source.into_boxed_slice()),
        tree,
    })))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::discover::{self, DiscoverOptions};

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn sexp_map(files: &[Arc<ParsedFile>]) -> BTreeMap<String, String> {
        files
            .iter()
            .map(|f| {
                (
                    f.path.to_string_lossy().into_owned(),
                    f.tree.root_node().to_sexp(),
                )
            })
            .collect()
    }

    #[test]
    fn parses_every_supported_file_and_skips_the_rest() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", "fn a() {}");
        write(dir.path(), "b.py", "def b():\n    pass\n");
        write(dir.path(), "notes.json", "{}");

        let snapshot = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (files, cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();

        let paths: Vec<&str> = files.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert_eq!(paths, vec!["a.rs", "b.py"]);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn a_parse_error_in_source_does_not_fail_the_stage() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "broken.rs", "fn (((( not valid");

        let snapshot = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (files, _cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();

        assert_eq!(files.len(), 1);
        assert!(files[0].tree.root_node().has_error());
    }

    #[test]
    fn an_unchanged_blob_hash_is_reused_from_the_cache_without_reparsing() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", "fn a() {}");
        write(dir.path(), "b.rs", "fn b() {}");

        let snapshot = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (first_run, cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();

        // Change only b.rs. a.rs keeps its blob hash, so its cached tree
        // must be the very same allocation on the second run.
        write(dir.path(), "b.rs", "fn b() { changed() }");
        let snapshot2 = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (second_run, _cache2) = parse(&cache, dir.path(), &snapshot2).unwrap();

        let a_first = first_run
            .iter()
            .find(|f| f.path.to_str() == Some("a.rs"))
            .unwrap();
        let a_second = second_run
            .iter()
            .find(|f| f.path.to_str() == Some("a.rs"))
            .unwrap();
        assert!(Arc::ptr_eq(a_first, a_second));
    }

    #[test]
    fn an_unchanged_tree_hash_returns_the_cache_without_touching_disk() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "a.rs", "fn a() {}");

        let snapshot = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (_first, cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();

        // Delete the file on disk. If the fast path re-read it, this would
        // fail with an ExploreParse error instead of returning happily.
        fs::remove_file(dir.path().join("a.rs")).unwrap();

        let (second, _cache2) = parse(&cache, dir.path(), &snapshot).unwrap();
        assert_eq!(second.len(), 1);
    }

    /// Pins Rule 32 for the syntax stage: parsing the same files through
    /// `rayon` in a different arrival order must not change any file's
    /// parsed `S`-expression.
    #[test]
    fn parallel_parsing_is_order_independent() {
        let dir = TempDir::new().unwrap();
        for i in 0..40 {
            write(
                dir.path(),
                &format!("f_{i:03}.rs"),
                &format!("fn f{i}() {{ let x = {i}; }}"),
            );
        }
        for i in 0..40 {
            write(
                dir.path(),
                &format!("g_{i:03}.py"),
                &format!("def g{i}():\n    return {i}\n"),
            );
        }

        let mut snapshot = discover::discover(dir.path(), &DiscoverOptions::default()).unwrap();
        let (baseline, _cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();
        let baseline_sexp = sexp_map(&baseline);

        // Reverse, then interleave odd/even indices: two different arrival
        // orders, neither of which is the sorted order.
        snapshot.files.reverse();
        let (reversed, _cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();
        assert_eq!(sexp_map(&reversed), baseline_sexp);

        let odds: Vec<_> = snapshot.files.iter().step_by(2).cloned().collect();
        let evens: Vec<_> = snapshot.files.iter().skip(1).step_by(2).cloned().collect();
        snapshot.files = odds.into_iter().chain(evens).collect();
        let (interleaved, _cache) = parse(&Cache::new(), dir.path(), &snapshot).unwrap();
        assert_eq!(sexp_map(&interleaved), baseline_sexp);
    }
}
