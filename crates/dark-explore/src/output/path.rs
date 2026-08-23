//! Path formatting for the output stage.
//!
//! Every path in `.dark/explore/<tree-sha>.json` must read the same on
//! every platform: [`path_to_string`] renders a repository-relative
//! [`Path`] with `/` between components, regardless of the host's own
//! separator.
//!
//! # Why this cannot lean on `discover::compare_paths`
//!
//! [`discover::compare_paths`](crate::discover::compare_paths) is Rule 30's
//! byte comparator, and it is exactly right for what F1 uses it for: sorting
//! [`Path`] values by the raw bytes of [`Path::as_os_str`]. On Windows,
//! though, a [`PathBuf`] the walker built by joining directory entries
//! carries `\` (0x5C) as its separator byte, not `/` (0x2F) — `Path` does
//! not normalise one to the other. A byte comparator over those bytes and a
//! byte comparator over this module's `/`-joined string therefore agree on
//! Unix, where the native separator already is `/`, and can disagree on
//! Windows: whenever one path continues into a subdirectory at a point
//! where a sibling path's next byte falls between `/` (0x2F) and `\`
//! (0x5C) — a digit, an uppercase letter, or one of `:;<=>?@[` — comparing
//! against `\` places that sibling on the other side of the continuing path
//! than comparing against `/` would. `"a/y"` (a directory `a` holding a
//! file `y`) sorts before `"aZx"` under a `/`-keyed comparator, and after it
//! under a `\`-keyed one.
//!
//! Every path this stage sorts or hashes is therefore normalised to its
//! `/`-joined string *first*, with [`path_to_string`], and only then
//! compared or hashed, using the plain byte order of that string
//! ([`compare_path_strings`]). That order is what [`discover::compare_paths`]
//! itself computes on a Unix host, and it is what F4's own "done when" —
//! identical bytes on Linux, macOS, and Windows — needs on every host. See
//! also [`super::tree`], which hashes the discovered file list the same
//! way rather than reusing [`crate::discover::Snapshot::tree_hash`]'s
//! native-separator bytes.
//!
//! This module does not change [`discover::compare_paths`] itself: F1 owns
//! that function, and the gap above is a note for whoever next touches it,
//! not a fix folded into this task unit's own files.

use std::cmp::Ordering;
use std::path::Path;

/// Renders `path` with `/` between components, regardless of the host's
/// native separator.
///
/// A path component that is not valid Unicode is lossily converted (the
/// same policy [`Path::to_string_lossy`] uses), because the output is JSON
/// text and JSON text is Unicode. Discovery already excludes binary files
/// by content, not by name, so a non-Unicode *path* is the rare case this
/// function still has to have an answer for.
#[must_use]
pub fn path_to_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Compares two already-`/`-joined path strings by raw byte, the way
/// [`discover::compare_paths`](crate::discover::compare_paths) compares raw
/// [`Path`] bytes. See the module documentation for why this stage sorts
/// the normalised string rather than calling that function on the
/// [`Path`] values directly.
#[must_use]
pub(super) fn compare_path_strings(a: &str, b: &str) -> Ordering {
    a.as_bytes().cmp(b.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn joins_components_with_a_forward_slash() {
        let path = PathBuf::from("crates").join("dark-explore").join("src");
        assert_eq!(path_to_string(&path), "crates/dark-explore/src");
    }

    #[test]
    fn a_single_component_has_no_slash() {
        assert_eq!(path_to_string(Path::new("Cargo.toml")), "Cargo.toml");
    }

    #[test]
    fn an_empty_path_renders_as_an_empty_string() {
        assert_eq!(path_to_string(Path::new("")), "");
    }

    #[test]
    fn joining_by_hand_matches_joining_via_path_components() {
        // `PathBuf::from("a/b/c")` parses "a/b/c" into components on every
        // platform (the forward slash is always a separator, even on
        // Windows, which merely does not *emit* it); re-joining those
        // components must reproduce the original string.
        let path = PathBuf::from("a/b/c");
        assert_eq!(path_to_string(&path), "a/b/c");
    }

    #[test]
    fn compare_path_strings_orders_by_raw_byte() {
        assert_eq!(compare_path_strings("a.rs", "b.rs"), Ordering::Less);
        assert_eq!(compare_path_strings("b.rs", "a.rs"), Ordering::Greater);
        assert_eq!(compare_path_strings("a.rs", "a.rs"), Ordering::Equal);
    }

    /// Pins the module documentation's own example: a directory continuing
    /// past `/` must sort before a sibling whose next byte sits between the
    /// two platforms' separators, using the `/`-keyed order this module
    /// always compares with.
    #[test]
    fn a_nested_path_sorts_before_a_sibling_with_an_uppercase_next_byte() {
        assert_eq!(compare_path_strings("a/y", "aZx"), Ordering::Less);
    }
}
