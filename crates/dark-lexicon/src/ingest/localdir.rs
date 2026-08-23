//! The `localdir` adapter: a local directory of documentation.
//!
//! G2: "Use this for private documentation." No network, no licence
//! ambiguity to resolve remotely — a caller still runs the licence gate
//! (`crate::ingest::licence`) over the same directory before trusting the
//! result, since a private directory can be unlicensed too.

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};

use crate::ingest::document::Document;
use crate::ingest::markdown::extract_headings;

/// The file extensions that this adapter reads as documentation.
const DOCUMENT_EXTENSIONS: &[&str] = &["md", "markdown", "txt"];

/// Walks `root` and produces one [`Document`] for each Markdown or plain
/// text file it finds.
///
/// Files are visited in a deterministic, byte-comparator order on their
/// path relative to `root` (Rule 30), independent of the order the
/// filesystem returns directory entries in. `path` on each document is
/// that same relative path, with forward slashes on every platform.
///
/// # Errors
///
/// Returns `E_TOOL_NOT_FOUND` when `root` does not exist. Returns
/// `E_TOOL_FAILED` when a directory cannot be listed or a file cannot be
/// read.
pub fn ingest(root: &Path) -> Result<Vec<Document>> {
    if !root.exists() {
        return Err(Error::new(
            ErrCode::ToolNotFound,
            format!("{} does not exist", root.display()),
        ));
    }

    let mut relative_paths = list_relative_files(root, root)?;
    relative_paths.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let mut documents = Vec::with_capacity(relative_paths.len());
    for relative in relative_paths {
        let full_path = root.join(&relative);
        let body = std::fs::read_to_string(&full_path).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {}: {source}", full_path.display()),
            )
        })?;
        let headings = extract_headings(&body);
        let title = headings.first().map_or_else(
            || {
                std::path::Path::new(&relative)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&relative)
                    .to_owned()
            },
            |h| h.text.clone(),
        );
        documents.push(Document::new(relative, title, body).with_headings(headings));
    }
    Ok(documents)
}

/// Lists every file under `dir` whose extension is in
/// [`DOCUMENT_EXTENSIONS`], as paths relative to `root`.
fn list_relative_files(root: &Path, dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot list {}: {source}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!(
                    "cannot read a directory entry under {}: {source}",
                    dir.display()
                ),
            )
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot stat {}: {source}", path.display()),
            )
        })?;
        if file_type.is_dir() {
            out.extend(list_relative_files(root, &path)?);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let has_document_extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| DOCUMENT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
        if !has_document_extension {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let Some(relative) = relative.to_str() else {
            continue;
        };
        out.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingests_markdown_files_sorted_by_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("b.md"), "# B Title\nbody b\n").unwrap();
        std::fs::write(dir.path().join("a.md"), "# A Title\nbody a\n").unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/c.md"), "# C Title\nbody c\n").unwrap();

        let docs = ingest(dir.path()).unwrap();
        let paths: Vec<&str> = docs.iter().map(|d| d.path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "b.md", "sub/c.md"]);
        assert_eq!(docs[0].title, "A Title");
    }

    #[test]
    fn ignores_files_with_other_extensions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "# Notes\n").unwrap();
        std::fs::write(dir.path().join("image.png"), [0u8, 1, 2]).unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();
        let docs = ingest(dir.path()).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "notes.md");
    }

    #[test]
    fn falls_back_to_the_file_stem_when_there_is_no_heading() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plain.txt"), "no headings here").unwrap();
        let docs = ingest(dir.path()).unwrap();
        assert_eq!(docs[0].title, "plain");
    }

    #[test]
    fn reports_tool_not_found_for_a_missing_root() {
        let err = ingest(Path::new("/does/not/exist/anywhere")).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolNotFound);
    }
}
