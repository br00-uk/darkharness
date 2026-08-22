//! Path resolution and the repository-root security boundary.
//!
//! Every file tool resolves the path it receives through [`resolve`] before
//! it touches the filesystem. Rule 34 of the build specification makes this
//! refusal unconditional: `write_outside_root` is always denied, and no
//! configuration changes that.

use std::path::{Component, Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

/// Resolves `requested` against `root` and enforces the repository-root
/// boundary.
///
/// `requested` is a path that a model supplied. This function always reads
/// it as relative to `root`, even when the underlying platform would parse
/// it as absolute, because a tool never addresses the filesystem outside the
/// repository.
///
/// The check has three layers:
///
/// 1. Reject an absolute path, a Windows drive-letter path, and a Windows
///    UNC path (`\\server\share`, `\\?\C:\...`), by inspecting the raw text.
///    A backslash has no meaning as a separator on a Unix build, so a purely
///    component-based check would miss these on that platform.
/// 2. Reject any `..` component while walking `requested`.
/// 3. Canonicalize the nearest existing ancestor of the resolved path and
///    confirm it stays inside the canonical root. This step catches a
///    symbolic link that leaves the root even when the link target does not
///    yet exist as a full path.
///
/// # Errors
///
/// Returns [`ErrCode::ToolOutsideRoot`] when `requested` fails any layer.
/// Returns [`ErrCode::ToolFailed`] when `root` itself cannot be resolved.
pub(crate) fn resolve(root: &Path, requested: &str) -> Result<PathBuf> {
    reject_absolute_forms(requested)?;

    let mut joined = root.to_path_buf();
    for component in Path::new(requested).components() {
        match component {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(outside_root(requested));
            }
        }
    }

    check_symlink_boundary(root, &joined, requested)?;
    Ok(joined)
}

fn reject_absolute_forms(requested: &str) -> Result<()> {
    let looks_unc_or_unix_absolute = requested.starts_with('\\') || requested.starts_with('/');
    let looks_drive_letter = {
        let bytes = requested.as_bytes();
        bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
    };

    if looks_unc_or_unix_absolute || looks_drive_letter || Path::new(requested).is_absolute() {
        return Err(outside_root(requested));
    }
    Ok(())
}

fn outside_root(requested: &str) -> Error {
    Error::new(
        ErrCode::ToolOutsideRoot,
        format!("the path `{requested}` is outside the repository root"),
    )
}

/// Confirms that `joined` stays inside `root` once every symbolic link
/// resolves, including a link on a segment that does not exist yet.
fn check_symlink_boundary(root: &Path, joined: &Path, requested: &str) -> Result<()> {
    let canonical_root = std::fs::canonicalize(root).map_err(|err| {
        Error::new(
            ErrCode::ToolFailed,
            format!(
                "cannot resolve the repository root {}: {err}",
                root.display()
            ),
        )
    })?;

    // `joined` may not exist yet (write_file can create a new file), so walk
    // up to the nearest existing ancestor, canonicalize that, then rebuild
    // the tail on top of the canonical ancestor. The loop always terminates
    // at `root` at the latest, because `root` canonicalized above.
    let mut probe = joined.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(mut canonical) = std::fs::canonicalize(&probe) {
            for part in tail.iter().rev() {
                canonical.push(part);
            }
            if !canonical.starts_with(&canonical_root) {
                return Err(outside_root(requested));
            }
            return Ok(());
        }

        let Some(file_name) = probe.file_name() else {
            return Err(outside_root(requested));
        };
        tail.push(file_name.to_owned());
        if !probe.pop() {
            return Err(outside_root(requested));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().expect("create a temp root")
    }

    #[test]
    fn a_plain_relative_path_resolves_inside_root() {
        let dir = root();
        let resolved = resolve(dir.path(), "src/main.rs").expect("resolves");
        assert_eq!(resolved, dir.path().join("src/main.rs"));
    }

    #[test]
    fn empty_and_dot_resolve_to_the_root() {
        let dir = root();
        assert_eq!(resolve(dir.path(), "").unwrap(), dir.path());
        assert_eq!(resolve(dir.path(), ".").unwrap(), dir.path());
    }

    #[test]
    fn a_parent_dir_escape_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "../evil.txt").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_buried_parent_dir_escape_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "src/../../evil.txt").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_unix_absolute_path_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "/etc/passwd").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_windows_drive_letter_path_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "C:\\Windows\\System32\\evil.dll").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
        let err = resolve(dir.path(), "C:/Windows/System32/evil.dll").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_windows_unc_share_path_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "\\\\server\\share\\file.txt").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_windows_verbatim_unc_path_is_rejected() {
        let dir = root();
        let err = resolve(dir.path(), "\\\\?\\C:\\Windows\\System32\\evil.dll").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_path_that_resolves_inside_root_survives_a_harmless_dot_segment() {
        let dir = root();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        let resolved = resolve(dir.path(), "./src/./main.rs").expect("resolves");
        assert_eq!(resolved, dir.path().join("src/main.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_that_escapes_root_is_rejected_for_an_existing_target() {
        let dir = root();
        let outside = tempfile::tempdir().expect("create an outside dir");
        std::fs::write(outside.path().join("secret.txt"), b"top secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();

        let err = resolve(dir.path(), "escape/secret.txt").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_that_escapes_root_is_rejected_for_a_new_target() {
        // The escaping segment resolves, but the final component does not
        // exist yet, as it would for a write_file call creating a new file.
        let dir = root();
        let outside = tempfile::tempdir().expect("create an outside dir");
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();

        let err = resolve(dir.path(), "escape/new-file.txt").unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_that_stays_inside_root_is_allowed() {
        let dir = root();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("real/file.txt"), b"hello").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link-inside"))
            .unwrap();

        let resolved = resolve(dir.path(), "link-inside/file.txt").expect("resolves");
        let content = std::fs::read_to_string(&resolved).unwrap();
        assert_eq!(content, "hello");
    }
}
