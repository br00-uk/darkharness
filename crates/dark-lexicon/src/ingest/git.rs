//! The `git` adapter: a repository at a tag.
//!
//! Checking out the repository is `dark-airlock`'s job (`dark_airlock::git`
//! already owns running `git`, for the same Rule 13 reason
//! `crate::ingest::fetch` cannot build an HTTP client here). This adapter
//! starts from a worktree that is already on disk, at the tag or commit
//! the caller checked out, and walks it the same way
//! `crate::ingest::localdir` walks a private directory. The one thing this
//! adapter adds over `localdir` is a source URL: given a template for the
//! host's "view this file at this ref" page, each document gets a `url`
//! that points at the exact line the caller checked out, not at whatever
//! the repository's default branch shows today.

use std::path::Path;

use dark_contract::Result;

use crate::ingest::document::Document;
use crate::ingest::localdir;

/// Walks a checked-out repository at `worktree_root` and produces one
/// [`Document`] per Markdown or plain text file, the same set `localdir`
/// would produce for the same directory.
///
/// When `url_template` is given, it must contain the literal substring
/// `{path}`, which this function replaces with each document's relative
/// path to build that document's `url` — for example
/// `https://github.com/tokio-rs/tokio/blob/tokio-1.47.0/{path}`. Without a
/// template, documents carry no URL, the same as `localdir`.
///
/// # Errors
///
/// Returns the same errors [`localdir::ingest`] returns for the same
/// `worktree_root`.
pub fn ingest(worktree_root: &Path, url_template: Option<&str>) -> Result<Vec<Document>> {
    let mut documents = localdir::ingest(worktree_root)?;
    if let Some(template) = url_template {
        for document in &mut documents {
            let url = template.replace("{path}", &document.path);
            document.url = Some(url);
        }
    }
    Ok(documents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attaches_a_url_built_from_the_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Readme\nhello\n").unwrap();

        let docs = ingest(
            dir.path(),
            Some("https://github.com/tokio-rs/tokio/blob/tokio-1.47.0/{path}"),
        )
        .unwrap();
        assert_eq!(
            docs[0].url.as_deref(),
            Some("https://github.com/tokio-rs/tokio/blob/tokio-1.47.0/README.md")
        );
    }

    #[test]
    fn leaves_url_unset_with_no_template() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();
        let docs = ingest(dir.path(), None).unwrap();
        assert!(docs[0].url.is_none());
    }
}
