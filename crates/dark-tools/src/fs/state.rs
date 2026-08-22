//! Session-scoped tracking of which files a session has read.
//!
//! `write_file` and `edit_file` refuse to touch a file that this session has
//! not read, and refuse a file that changed on disk since that read. Both
//! rules map to [`ErrCode::ToolStale`], because both clear the same way:
//! read the file again.
//!
//! One [`ReadState`] belongs to one session. [`super::file_tools`] builds a
//! single instance and shares it, through an [`std::sync::Arc`], across
//! every tool it constructs, because the rule is session-scoped rather than
//! per-tool-call.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use dark_contract::{ErrCode, Error, Result};

/// A cheap, in-process fingerprint of file content.
///
/// This only needs to compare equal for identical bytes observed by the same
/// process during one session. It never leaves the process, so it does not
/// need to be a cryptographic hash or stable across runs.
type Fingerprint = u64;

fn fingerprint(bytes: &[u8]) -> Fingerprint {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Tracks, for one session, the last content this session observed at each
/// path it has read or written.
#[derive(Debug, Default)]
pub struct ReadState {
    seen: Mutex<HashMap<PathBuf, Fingerprint>>,
}

impl ReadState {
    /// Creates a tracker that has not observed any file yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that the session just observed `bytes` at `path`.
    ///
    /// A successful `read_file`, `write_file`, `edit_file`, or `apply_patch`
    /// call this, so the tool after it sees the file as fresh.
    pub fn record(&self, path: &Path, bytes: &[u8]) {
        let mut seen = self.lock();
        seen.insert(path.to_path_buf(), fingerprint(bytes));
    }

    /// Removes the record for `path`.
    ///
    /// A successful delete (a patch that removes a file) calls this, so a
    /// later write to the same path is treated as creating a new file.
    pub fn forget(&self, path: &Path) {
        let mut seen = self.lock();
        seen.remove(path);
    }

    /// Confirms that the session read `path` and that `current` still
    /// matches what it read.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::ToolStale`] when the session never read `path`, or
    /// when `current` no longer matches the content from that read.
    pub fn check_fresh(&self, path: &Path, current: &[u8]) -> Result<()> {
        let seen = self.lock();
        match seen.get(path) {
            None => Err(Error::new(
                ErrCode::ToolStale,
                format!(
                    "{} has not been read in this session; read it before you change it",
                    path.display()
                ),
            )),
            Some(&recorded) if recorded == fingerprint(current) => Ok(()),
            Some(_) => Err(Error::new(
                ErrCode::ToolStale,
                format!(
                    "{} changed on disk since this session last read it",
                    path.display()
                ),
            )),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Fingerprint>> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_never_read_is_stale() {
        let state = ReadState::new();
        let err = state
            .check_fresh(Path::new("a.txt"), b"anything")
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
    }

    #[test]
    fn a_file_read_and_unchanged_is_fresh() {
        let state = ReadState::new();
        state.record(Path::new("a.txt"), b"hello");
        assert!(state.check_fresh(Path::new("a.txt"), b"hello").is_ok());
    }

    #[test]
    fn a_file_changed_since_the_read_is_stale() {
        let state = ReadState::new();
        state.record(Path::new("a.txt"), b"hello");
        let err = state
            .check_fresh(Path::new("a.txt"), b"hello, world")
            .unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
    }

    #[test]
    fn forgetting_a_path_makes_it_stale_again() {
        let state = ReadState::new();
        state.record(Path::new("a.txt"), b"hello");
        state.forget(Path::new("a.txt"));
        let err = state.check_fresh(Path::new("a.txt"), b"hello").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolStale);
    }
}
