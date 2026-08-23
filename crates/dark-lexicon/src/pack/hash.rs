//! The pack hash.
//!
//! Task unit `G1` asks the harness to "verify the pack hash before use".
//! [`compute_dir`] walks a pack directory and folds every file into one
//! BLAKE3 digest. [`write`] stores that digest in the pack, next to the
//! files it covers. [`verify`] recomputes the digest and checks it against
//! the stored value, so a caller never opens a pack whose files changed
//! since the pack was built, whether by disk corruption or by hand-editing.

use std::path::{Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

/// The file name that holds the stored hash, at the pack root.
///
/// This file sits beside `pack.toml`. Walking the directory for
/// [`compute_dir`] skips it, so the hash never covers itself.
pub const HASH_FILE_NAME: &str = "pack.hash";

/// A BLAKE3 digest over every file in a pack directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackHash([u8; 32]);

impl PackHash {
    /// Renders the hash as lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        blake3::Hash::from(self.0).to_hex().to_string()
    }

    /// Parses a hash from the hexadecimal form that [`Self::to_hex`]
    /// produces.
    ///
    /// # Errors
    ///
    /// Returns `E_TOOL_FAILED` when `text` is not 64 lowercase hexadecimal
    /// characters.
    pub fn from_hex(text: &str) -> Result<Self> {
        let hash = blake3::Hash::from_hex(text.trim()).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("'{text}' is not a BLAKE3 hash: {source}"),
            )
        })?;
        Ok(Self(*hash.as_bytes()))
    }
}

impl std::fmt::Display for PackHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Lists every regular file under `dir`, as paths relative to `dir`, using
/// forward slashes on every platform.
///
/// The result is not sorted; callers that need a deterministic order sort
/// it themselves with a byte comparator, per Rule 30.
pub(crate) fn list_files_relative(dir: &Path) -> Result<Vec<String>> {
    fn walk(root: &Path, current: &Path, out: &mut Vec<String>) -> Result<()> {
        let entries = std::fs::read_dir(current).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot list {}: {source}", current.display()),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!(
                        "cannot read a directory entry under {}: {source}",
                        current.display()
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
                walk(root, &path, out)?;
                continue;
            }
            if !file_type.is_file() {
                // A symlink or other special entry. A pack ships plain
                // files only.
                continue;
            }
            let relative = path.strip_prefix(root).unwrap_or(&path);
            let relative = relative.to_str().ok_or_else(|| {
                Error::new(
                    ErrCode::ToolFailed,
                    format!("{} is not valid UTF-8", relative.display()),
                )
            })?;
            out.push(relative.replace(std::path::MAIN_SEPARATOR, "/"));
        }
        Ok(())
    }

    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Computes the pack hash over every file in `dir`, excluding
/// [`HASH_FILE_NAME`] itself.
///
/// The digest folds in each file's relative path and its content, over
/// files sorted by a byte comparator on the relative path (Rule 30), so the
/// result does not depend on the order that the filesystem returns entries
/// in, and depends on both a file's name and its bytes.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `dir` cannot be listed, when a file cannot
/// be read, or when a relative path is not valid UTF-8.
pub fn compute_dir(dir: &Path) -> Result<PackHash> {
    let mut relative_paths = list_files_relative(dir)?;
    relative_paths.retain(|path| path != HASH_FILE_NAME);
    relative_paths.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

    let mut hasher = blake3::Hasher::new();
    for relative in &relative_paths {
        let bytes = std::fs::read(dir.join(relative)).map_err(|source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot read {relative}: {source}"),
            )
        })?;
        hasher.update(relative.as_bytes());
        hasher.update(&[0u8]);
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(PackHash(*hasher.finalize().as_bytes()))
}

/// Computes the pack hash for `dir` and writes it to `<dir>/pack.hash`.
///
/// Call this once, after every other file in the pack is final. A later
/// change to any file invalidates the stored hash, which [`verify`] then
/// reports.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when the directory cannot be hashed or the hash
/// file cannot be written.
pub fn write(dir: &Path) -> Result<PackHash> {
    let hash = compute_dir(dir)?;
    let path = dir.join(HASH_FILE_NAME);
    std::fs::write(&path, hash.to_hex()).map_err(|source| {
        Error::new(
            ErrCode::ToolFailed,
            format!("cannot write {}: {source}", path.display()),
        )
    })?;
    Ok(hash)
}

/// Verifies the pack at `dir` against its stored hash.
///
/// # Errors
///
/// Returns `E_PACK_NOT_FOUND` when `<dir>/pack.hash` is absent. Returns
/// `E_TOOL_FAILED` when the stored hash does not parse, or when the
/// recomputed hash does not match it — the pack changed since it was
/// hashed. There is no dedicated code for a hash mismatch in the taxonomy
/// that task unit `Z1` owns, so this uses `E_TOOL_FAILED` with a specific
/// remedy; see the module docs of `pack` for why.
pub fn verify(dir: &Path) -> Result<()> {
    let path: PathBuf = dir.join(HASH_FILE_NAME);
    let stored_text = std::fs::read_to_string(&path).map_err(|source| {
        Error::new(
            ErrCode::PackNotFound,
            format!("cannot read {}: {source}", path.display()),
        )
    })?;
    let stored = PackHash::from_hex(&stored_text)?;
    let recomputed = compute_dir(dir)?;
    if stored == recomputed {
        return Ok(());
    }
    Err(Error::new(
        ErrCode::ToolFailed,
        format!(
            "pack hash mismatch: pack.hash records {stored}, the files on disk hash to \
             {recomputed}"
        ),
    )
    .with_remedy("Re-export the pack from a trusted source. Do not edit a pack's files by hand."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pack_files(dir: &Path) {
        std::fs::write(dir.join("pack.toml"), b"pack = true").expect("write pack.toml");
        std::fs::write(dir.join("chunks.jsonl"), b"{}\n").expect("write chunks.jsonl");
        std::fs::create_dir_all(dir.join("nested")).expect("mkdir nested");
        std::fs::write(dir.join("nested/graph.json"), b"{}").expect("write nested file");
    }

    #[test]
    fn the_same_files_hash_the_same_way_regardless_of_creation_order() {
        let dir_a = tempfile::tempdir().expect("tempdir");
        let dir_b = tempfile::tempdir().expect("tempdir");

        std::fs::write(dir_a.path().join("a.txt"), b"one").unwrap();
        std::fs::write(dir_a.path().join("b.txt"), b"two").unwrap();

        std::fs::write(dir_b.path().join("b.txt"), b"two").unwrap();
        std::fs::write(dir_b.path().join("a.txt"), b"one").unwrap();

        assert_eq!(
            compute_dir(dir_a.path()).unwrap(),
            compute_dir(dir_b.path()).unwrap()
        );
    }

    #[test]
    fn changing_a_byte_changes_the_hash() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pack_files(dir.path());
        let before = compute_dir(dir.path()).unwrap();
        std::fs::write(dir.path().join("chunks.jsonl"), b"{}\n{}\n").unwrap();
        let after = compute_dir(dir.path()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn renaming_a_file_changes_the_hash_even_with_identical_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"same content").unwrap();
        let a_hash = compute_dir(dir.path()).unwrap();

        let dir2 = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir2.path().join("z.txt"), b"same content").unwrap();
        let z_hash = compute_dir(dir2.path()).unwrap();

        assert_ne!(a_hash, z_hash);
    }

    #[test]
    fn write_then_verify_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pack_files(dir.path());
        write(dir.path()).expect("write hash");
        verify(dir.path()).expect("verify");
    }

    #[test]
    fn verify_fails_after_the_pack_changes_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pack_files(dir.path());
        write(dir.path()).expect("write hash");
        std::fs::write(dir.path().join("chunks.jsonl"), b"tampered\n").unwrap();
        let err = verify(dir.path()).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn verify_reports_pack_not_found_when_the_hash_file_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pack_files(dir.path());
        let err = verify(dir.path()).unwrap_err();
        assert_eq!(err.code, ErrCode::PackNotFound);
    }

    #[test]
    fn hash_round_trips_through_hex() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_pack_files(dir.path());
        let hash = compute_dir(dir.path()).unwrap();
        let parsed = PackHash::from_hex(&hash.to_hex()).unwrap();
        assert_eq!(hash, parsed);
    }
}
