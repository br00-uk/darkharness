//! Serialising a [`Document`] and its [`Lock`], and writing both to
//! `.dark/explore/`.
//!
//! # Why pretty-printed
//!
//! `serde_json`'s pretty printer always writes `\n` for a line break, on
//! every platform — it does not consult the host's own newline convention —
//! so choosing it over compact JSON costs nothing for Rule 29's byte
//! identity and gains a file a person can read directly.
//!
//! # `ErrCode`
//!
//! `dark-contract`'s taxonomy has no dedicated "wrote a file, and it
//! failed" code for the `Explore` domain — only [`ErrCode::ExploreDirty`]
//! and [`ErrCode::ExploreParse`] exist, and neither names an I/O failure
//! precisely. `crate::seam::cochange` already reaches for
//! [`ErrCode::ExploreParse`] for a git failure that is not a parse failure
//! either (see that module's `CoChange::read`); this module follows the
//! same precedent for the same reason, rather than widen `dark-contract`'s
//! taxonomy for one call site.

use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

use super::document::Document;
use super::lock::{Lock, grammar_versions};

/// Where [`write`] put the two files it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenPaths {
    /// `.dark/explore/<tree-sha>.json`.
    pub json: PathBuf,
    /// `.dark/explore/<tree-sha>.lock`.
    pub lock: PathBuf,
}

/// Serialises `document` to its canonical bytes: pretty-printed JSON with a
/// trailing newline.
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when serialisation fails. Every field
/// `document::build` produces is already-rounded, finite data, so this is
/// not expected to happen in practice; the `Result` exists because
/// `serde_json` itself makes no infallibility guarantee.
pub fn document_bytes(document: &Document) -> Result<Vec<u8>> {
    to_bytes(document, "the explore report")
}

/// Serialises `lock` to its canonical bytes. See [`document_bytes`].
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when serialisation fails.
pub fn lock_bytes(lock: &Lock) -> Result<Vec<u8>> {
    to_bytes(lock, "the explore lock file")
}

fn to_bytes<T: serde::Serialize>(value: &T, what: &str) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|err| {
        Error::new(
            ErrCode::ExploreParse,
            format!("cannot serialise {what}: {err}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Builds the [`Lock`] that belongs beside `document`, from the exact JSON
/// bytes [`document_bytes`] produced for it.
#[must_use]
pub fn build_lock(document: &Document, document_json: &[u8]) -> Lock {
    Lock {
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        config_hash: document.config_hash.clone(),
        grammar_versions: grammar_versions(),
        output_blake3: blake3::hash(document_json).to_string(),
    }
}

/// Writes `document` and its [`Lock`] to `.dark/explore/<tree-sha>.json`
/// and `.dark/explore/<tree-sha>.lock` under `root`, creating the
/// directory if it does not exist yet.
///
/// # Errors
///
/// Returns [`ErrCode::ExploreParse`] when serialisation fails, when the
/// `.dark/explore` directory cannot be created, or when either file cannot
/// be written.
pub fn write(root: &Path, document: &Document) -> Result<(WrittenPaths, Lock)> {
    let document_json = document_bytes(document)?;
    let lock = build_lock(document, &document_json);
    let lock_json = lock_bytes(&lock)?;

    let dir = root.join(".dark").join("explore");
    std::fs::create_dir_all(&dir).map_err(|source| io_error(&dir, &source))?;

    let json_path = dir.join(format!("{}.json", document.tree_sha));
    let lock_path = dir.join(format!("{}.lock", document.tree_sha));

    std::fs::write(&json_path, &document_json).map_err(|source| io_error(&json_path, &source))?;
    std::fs::write(&lock_path, &lock_json).map_err(|source| io_error(&lock_path, &source))?;

    Ok((
        WrittenPaths {
            json: json_path,
            lock: lock_path,
        },
        lock,
    ))
}

fn io_error(path: &Path, source: &std::io::Error) -> Error {
    Error::new(
        ErrCode::ExploreParse,
        format!("cannot write {}: {source}", path.display()),
    )
    .with_remedy("Check that the repository root is writable.")
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::discover::DiscoverOptions;
    use crate::output::document::{self, Sources};
    use crate::seam::{CoChange, Weights};

    fn empty_document() -> Document {
        let graphs = crate::graph::build(&[]);
        let analysis =
            crate::seam::analyse(&graphs, &CoChange::default(), &Weights::default()).unwrap();
        let discover_options = DiscoverOptions::default();
        let weights = Weights::default();
        document::build(&Sources {
            files: &[],
            graphs: &graphs,
            analysis: &analysis,
            cochange: &CoChange::default(),
            discover_options: &discover_options,
            weights: &weights,
            tree_sha: blake3::hash(b"fixture"),
        })
    }

    #[test]
    fn document_bytes_end_with_a_single_trailing_newline() {
        let bytes = document_bytes(&empty_document()).unwrap();
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn write_creates_the_dark_explore_directory_and_both_files() {
        let dir = TempDir::new().unwrap();
        let document = empty_document();

        let (paths, lock) = write(dir.path(), &document).unwrap();

        assert!(paths.json.is_file());
        assert!(paths.lock.is_file());
        assert!(
            paths
                .json
                .starts_with(dir.path().join(".dark").join("explore"))
        );
        assert_eq!(lock.config_hash, document.config_hash);

        let on_disk = std::fs::read(&paths.json).unwrap();
        assert_eq!(on_disk, document_bytes(&document).unwrap());
    }

    #[test]
    fn the_lock_file_names_the_hash_of_the_exact_json_bytes_written() {
        let dir = TempDir::new().unwrap();
        let document = empty_document();

        let (paths, lock) = write(dir.path(), &document).unwrap();
        let on_disk = std::fs::read(&paths.json).unwrap();
        assert_eq!(lock.output_blake3, blake3::hash(&on_disk).to_string());
    }

    #[test]
    fn writing_twice_produces_identical_bytes() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let document = empty_document();

        let (paths_a, lock_a) = write(dir_a.path(), &document).unwrap();
        let (paths_b, lock_b) = write(dir_b.path(), &document).unwrap();

        assert_eq!(
            std::fs::read(paths_a.json).unwrap(),
            std::fs::read(paths_b.json).unwrap()
        );
        assert_eq!(lock_a, lock_b);
    }
}
