//! Confines the command's working directory to the repository root.
//!
//! See Rule 34: `write_outside_root` is always denied. This module applies
//! the same posture to the working directory that a command runs in.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use dark_contract::{ErrCode, Error, Result};

/// Returns the lexically normalized path components of `path`.
///
/// This never touches the file system. A `..` component pops the previous
/// component. A `.` component is dropped. This is enough to detect an escape
/// attempt built from `..` segments, though it does not see through a
/// symbolic link.
fn lexical_components(path: &Path) -> Vec<OsString> {
    let mut out: Vec<OsString> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part.to_os_string()),
            Component::RootDir | Component::Prefix(_) => {
                out.clear();
                out.push(comp.as_os_str().to_os_string());
            }
        }
    }
    out
}

/// Resolves the working directory for a command.
///
/// `cwd` is `None` for the repository root itself, or `Some` relative or
/// absolute path. The result always lies at or below `root`.
///
/// # Errors
///
/// Returns [`ErrCode::ToolOutsideRoot`] when `cwd` normalizes to a path that
/// is not `root` or a descendant of it.
pub(crate) fn resolve_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf> {
    let requested = match cwd {
        None => return Ok(root.to_path_buf()),
        Some(rel) if rel.trim().is_empty() => return Ok(root.to_path_buf()),
        Some(rel) => rel,
    };

    let candidate = Path::new(requested);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };

    let root_components = lexical_components(root);
    let joined_components = lexical_components(&joined);

    if !joined_components.starts_with(root_components.as_slice()) {
        return Err(Error::new(
            ErrCode::ToolOutsideRoot,
            format!("cwd '{requested}' is outside the repository root"),
        ));
    }

    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::resolve_cwd;
    use dark_contract::ErrCode;
    use std::path::Path;

    fn root() -> &'static Path {
        Path::new("/repo")
    }

    #[test]
    fn none_resolves_to_the_root() {
        assert_eq!(resolve_cwd(root(), None).unwrap(), Path::new("/repo"));
    }

    #[test]
    fn an_empty_string_resolves_to_the_root() {
        assert_eq!(resolve_cwd(root(), Some("")).unwrap(), Path::new("/repo"));
    }

    #[test]
    fn a_relative_path_joins_the_root() {
        assert_eq!(
            resolve_cwd(root(), Some("crates/dark-tools")).unwrap(),
            Path::new("/repo/crates/dark-tools")
        );
    }

    #[test]
    fn a_relative_escape_is_rejected() {
        let err = resolve_cwd(root(), Some("..")).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_deep_relative_escape_is_rejected() {
        let err = resolve_cwd(root(), Some("../../etc")).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn a_wandering_path_that_stays_inside_the_root_is_allowed() {
        assert_eq!(
            resolve_cwd(root(), Some("a/../b")).unwrap(),
            Path::new("/repo/b")
        );
    }

    #[test]
    fn an_absolute_path_inside_the_root_is_allowed() {
        assert_eq!(
            resolve_cwd(root(), Some("/repo/sub")).unwrap(),
            Path::new("/repo/sub")
        );
    }

    #[test]
    fn an_absolute_path_outside_the_root_is_rejected() {
        let err = resolve_cwd(root(), Some("/etc")).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }

    #[test]
    fn an_absolute_path_that_merely_shares_a_prefix_is_rejected() {
        // "/repo2" is not "/repo", even though the text starts the same way.
        let err = resolve_cwd(root(), Some("/repo2")).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolOutsideRoot);
    }
}
