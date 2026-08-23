//! Import-path resolution that needs no disk access.
//!
//! Every function here answers from `RepoPaths::all`, the set of paths
//! [`crate::discover::Snapshot`] already found, rather than by touching the
//! filesystem again: a second, independent read of the tree during
//! extraction could race a working-tree edit and disagree with the
//! `Snapshot` that discovery already committed to. See Rule 29.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

/// The repository-wide context an import resolver needs: which file is
/// doing the importing, and which paths exist at all.
pub(crate) struct RepoPaths<'a> {
    /// The path of the file whose import is being resolved, relative to the
    /// repository root.
    pub file: &'a Path,
    /// Every path [`crate::discover::Snapshot`] found, including files no
    /// supported grammar parses (a `Cargo.toml`, a `LICENSE`).
    pub all: &'a HashSet<PathBuf>,
}

/// Joins `base_dir` with `raw` and normalises `.` and `..` components
/// lexically, with no filesystem access.
///
/// Returns `None` when `raw` is an absolute path (this function is for
/// relative specifiers only) or when a `..` component would climb above the
/// repository root.
fn join_normalized(base_dir: &Path, raw: &str) -> Option<PathBuf> {
    let raw_path = Path::new(raw);
    if raw_path.is_absolute() {
        return None;
    }
    let mut stack: Vec<Component<'_>> = base_dir.components().collect();
    for component in raw_path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                stack.pop()?;
            }
            Component::Normal(_) => stack.push(component),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(stack.iter().collect())
}

impl RepoPaths<'_> {
    /// Resolves a relative import specifier (`./foo`, `../foo/bar`) against
    /// the files [`RepoPaths::all`] holds.
    ///
    /// Tries the raw path first, then the raw path with each of
    /// `extensions` appended, then the raw path as a directory holding one
    /// of `index_names`. Returns the first candidate that exists; `None`
    /// when none does.
    pub(crate) fn resolve_relative(
        &self,
        raw: &str,
        extensions: &[&str],
        index_names: &[&str],
    ) -> Option<PathBuf> {
        let base_dir = self.file.parent().unwrap_or_else(|| Path::new(""));
        let joined = join_normalized(base_dir, raw)?;

        if self.all.contains(&joined) {
            return Some(joined);
        }
        for ext in extensions {
            let candidate = with_extension_appended(&joined, ext);
            if self.all.contains(&candidate) {
                return Some(candidate);
            }
        }
        for index in index_names {
            let candidate = joined.join(index);
            if self.all.contains(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Walks from the importing file's directory upward, returning the
    /// first ancestor directory whose join with `marker` names a path in
    /// [`RepoPaths::all`] — the directory holding the nearest `Cargo.toml`,
    /// for example.
    pub(crate) fn nearest_ancestor_with(&self, marker: &str) -> Option<PathBuf> {
        let start = self.file.parent().unwrap_or_else(|| Path::new(""));
        for dir in start.ancestors() {
            let marker_path = if dir.as_os_str().is_empty() {
                PathBuf::from(marker)
            } else {
                dir.join(marker)
            };
            if self.all.contains(&marker_path) {
                return Some(dir.to_path_buf());
            }
        }
        None
    }

    /// Searches every discovered path for exactly one file whose
    /// extension-stripped, `/`-joined component suffix matches `components`
    /// (for example `["a", "b", "c"]` matching `.../a/b/c.py` or
    /// `.../a/b/c/__init__.py`).
    ///
    /// Returns `None` when zero or more than one file matches: an ambiguous
    /// module-path match is not a resolution, per F2's "do not guess" rule.
    pub(crate) fn resolve_unique_suffix(
        &self,
        components: &[&str],
        extensions: &[&str],
        index_names: &[&str],
    ) -> Option<PathBuf> {
        if components.is_empty() {
            return None;
        }
        let mut matches: Vec<&PathBuf> = self
            .all
            .iter()
            .filter(|p| path_matches_suffix(p, components, extensions, index_names))
            .collect();
        matches.sort();
        matches.dedup();
        match matches.len() {
            1 => Some(matches[0].clone()),
            _ => None,
        }
    }
}

pub(crate) fn with_extension_appended(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn path_matches_suffix(
    path: &Path,
    components: &[&str],
    extensions: &[&str],
    index_names: &[&str],
) -> bool {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let file_name = match parts.last() {
        Some(name) => *name,
        None => return false,
    };

    // Case 1: the last `components.len()` parts, with the last one's
    // extension stripped, equal `components` exactly (`a/b/c.py`).
    let stem_matches = |name: &str| {
        extensions
            .iter()
            .any(|ext| name.strip_suffix(&format!(".{ext}")) == components.last().copied())
    };
    if parts.len() >= components.len() {
        let tail = &parts[parts.len() - components.len()..];
        if tail[..tail.len() - 1] == components[..components.len() - 1] && stem_matches(file_name) {
            return true;
        }
    }

    // Case 2: the path is `.../<components>/index_name` (a package
    // directory's own entry point).
    if index_names.contains(&file_name) && parts.len() > components.len() {
        let tail = &parts[parts.len() - 1 - components.len()..parts.len() - 1];
        if tail == components {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn resolves_a_relative_import_with_an_extension_probe() {
        let all = set(&["src/a/foo.ts", "src/a/bar.ts"]);
        let repo = RepoPaths {
            file: Path::new("src/a/bar.ts"),
            all: &all,
        };
        assert_eq!(
            repo.resolve_relative("./foo", &["ts", "tsx"], &["index.ts"]),
            Some(PathBuf::from("src/a/foo.ts"))
        );
    }

    #[test]
    fn resolves_a_relative_import_to_a_directory_index() {
        let all = set(&["src/a/foo/index.ts", "src/a/bar.ts"]);
        let repo = RepoPaths {
            file: Path::new("src/a/bar.ts"),
            all: &all,
        };
        assert_eq!(
            repo.resolve_relative("./foo", &["ts"], &["index.ts"]),
            Some(PathBuf::from("src/a/foo/index.ts"))
        );
    }

    #[test]
    fn a_relative_import_with_no_match_stays_unresolved() {
        let all = set(&["src/a/bar.ts"]);
        let repo = RepoPaths {
            file: Path::new("src/a/bar.ts"),
            all: &all,
        };
        assert_eq!(repo.resolve_relative("./missing", &["ts"], &[]), None);
    }

    #[test]
    fn climbing_above_the_repository_root_does_not_resolve() {
        let all = set(&["outside.ts"]);
        let repo = RepoPaths {
            file: Path::new("a.ts"),
            all: &all,
        };
        assert_eq!(repo.resolve_relative("../../outside", &["ts"], &[]), None);
    }

    #[test]
    fn finds_the_nearest_ancestor_holding_a_marker_file() {
        let all = set(&["crates/foo/Cargo.toml", "crates/foo/src/a/b.rs"]);
        let repo = RepoPaths {
            file: Path::new("crates/foo/src/a/b.rs"),
            all: &all,
        };
        assert_eq!(
            repo.nearest_ancestor_with("Cargo.toml"),
            Some(PathBuf::from("crates/foo"))
        );
    }

    #[test]
    fn a_unique_suffix_match_resolves() {
        let all = set(&["crates/foo/src/a/b.rs", "crates/foo/src/other.rs"]);
        let repo = RepoPaths {
            file: Path::new("crates/foo/src/main.rs"),
            all: &all,
        };
        assert_eq!(
            repo.resolve_unique_suffix(&["a", "b"], &["rs"], &["mod.rs"]),
            Some(PathBuf::from("crates/foo/src/a/b.rs"))
        );
    }

    #[test]
    fn an_ambiguous_suffix_match_does_not_resolve() {
        let all = set(&["crates/foo/src/a/b.rs", "crates/bar/src/a/b.rs"]);
        let repo = RepoPaths {
            file: Path::new("crates/foo/src/main.rs"),
            all: &all,
        };
        assert_eq!(repo.resolve_unique_suffix(&["a", "b"], &["rs"], &[]), None);
    }
}
