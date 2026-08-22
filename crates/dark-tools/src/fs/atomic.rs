//! Atomic file writes.
//!
//! Every mutating file tool writes through [`write`]: write the new content
//! to a temporary file in the same directory as the target, then rename the
//! temporary file over the target. A rename within one directory is atomic
//! on every platform the harness builds for, so a reader never observes a
//! half-written file, and a crash mid-write never corrupts the target.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

/// Writes `bytes` to `target` atomically, preserving `mode` when given.
///
/// `mode` is the target's permissions from before the write, captured by the
/// caller. Pass `None` when `target` did not exist before this call; the new
/// file then gets the platform default permissions.
///
/// The write and the rename happen on a blocking thread, because
/// [`tempfile`] has no asynchronous API.
///
/// # Errors
///
/// Returns [`ErrCode::ToolFailed`] when the temporary file cannot be
/// created, written, or renamed into place.
pub(crate) async fn write(
    target: PathBuf,
    bytes: Vec<u8>,
    mode: Option<std::fs::Permissions>,
) -> Result<()> {
    tokio::task::spawn_blocking(move || write_blocking(&target, &bytes, mode))
        .await
        .map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("the write task did not finish: {err}"),
            )
        })?
}

fn write_blocking(target: &Path, bytes: &[u8], mode: Option<std::fs::Permissions>) -> Result<()> {
    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    if !dir.as_os_str().is_empty() {
        std::fs::create_dir_all(dir).map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot create the directory {}: {err}", dir.display()),
            )
        })?;
    }

    let mut tmp = tempfile::Builder::new()
        .prefix(".darkharness-tmp-")
        .tempfile_in(dir)
        .map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot create a temporary file in {}: {err}", dir.display()),
            )
        })?;

    tmp.write_all(bytes).map_err(|err| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write the temporary file: {err}"),
        )
    })?;
    tmp.flush().map_err(|err| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot flush the temporary file: {err}"),
        )
    })?;

    if let Some(mode) = mode {
        tmp.as_file().set_permissions(mode).map_err(|err| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot set permissions on the temporary file: {err}"),
            )
        })?;
    }

    tmp.persist(target).map_err(|err| {
        Error::new(
            ErrCode::ToolFailed,
            format!(
                "cannot rename the temporary file to {}: {}",
                target.display(),
                err.error
            ),
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_creates_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("new.txt");
        write(target.clone(), b"hello".to_vec(), None)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[tokio::test]
    async fn write_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("existing.txt");
        std::fs::write(&target, "old").unwrap();
        write(target.clone(), b"new".to_vec(), None).await.unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
    }

    #[tokio::test]
    async fn write_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/b/c.txt");
        write(target.clone(), b"nested".to_vec(), None)
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "nested");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_preserves_the_given_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("script.sh");
        std::fs::write(&target, "old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions();

        write(target.clone(), b"new".to_vec(), Some(mode))
            .await
            .unwrap();

        let after = std::fs::metadata(&target).unwrap().permissions();
        assert_eq!(after.mode() & 0o777, 0o755);
    }
}
