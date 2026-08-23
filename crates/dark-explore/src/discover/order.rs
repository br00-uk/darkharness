//! The byte comparator that Rule 30 requires.
//!
//! [`compare_paths`] orders two paths by the raw bytes of their string form.
//! It never asks the platform for locale collation rules, so the order is
//! the same on every machine and in every process locale. Use it to sort any
//! list of paths that later feeds a hash. A locale-aware sort would change
//! the hash from machine to machine, and [`Path`]'s own [`Ord`] impl sorts by
//! path *component*, not by raw byte, which disagrees with a byte comparator
//! whenever a path segment holds a byte that sorts before the path
//! separator. See Rule 30 in `PRD.md` section 4.9.

use std::cmp::Ordering;
use std::path::Path;

/// Compares two paths by the raw bytes of their string form.
///
/// This is the byte comparator that Rule 30 requires: it compares
/// [`Path::as_os_str`] byte-for-byte and applies no locale collation. Two
/// equal-length paths that differ in one byte order the same way that
/// `<[u8]>::cmp` would order their encoded bytes.
///
/// Do not sort paths with [`Path`]'s own [`Ord`] impl instead of this
/// function. `Path` compares component by component, which silently
/// disagrees with a byte comparator whenever one path segment holds a byte
/// that sorts before the platform path separator: for example `"a-b"` sorts
/// before `"a/b"` under this function (`-` is `0x2D`, `/` is `0x2F`), but
/// `Path`'s component-wise `Ord` sorts `"a/b"` first, because it compares
/// the first *component* (`"a"` against `"a-b"`) rather than the raw bytes.
#[must_use]
pub fn compare_paths(a: &Path, b: &Path) -> Ordering {
    a.as_os_str()
        .as_encoded_bytes()
        .cmp(b.as_os_str().as_encoded_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn orders_plain_paths_lexicographically_by_byte() {
        assert_eq!(
            compare_paths(Path::new("a.rs"), Path::new("b.rs")),
            Ordering::Less
        );
        assert_eq!(
            compare_paths(Path::new("b.rs"), Path::new("a.rs")),
            Ordering::Greater
        );
        assert_eq!(
            compare_paths(Path::new("a.rs"), Path::new("a.rs")),
            Ordering::Equal
        );
    }

    /// Pins Rule 30: the byte comparator disagrees with `Path`'s own `Ord`
    /// on this pair, because `Path::cmp` compares components, not bytes.
    #[test]
    fn disagrees_with_path_ord_on_a_hyphen_before_a_slash() {
        let dash = PathBuf::from("a-b");
        let slash = PathBuf::from("a/b");

        // The byte comparator: '-' (0x2D) sorts before '/' (0x2F).
        assert_eq!(compare_paths(&dash, &slash), Ordering::Less);

        // `Path`'s component-wise `Ord` disagrees: the first component of
        // "a/b" is "a", which is a byte-wise prefix of "a-b", so "a/b"
        // sorts first under `Path::cmp`.
        assert_eq!(dash.cmp(&slash), Ordering::Greater);
    }

    #[test]
    fn sorting_a_list_matches_a_direct_byte_sort_of_the_strings() {
        let mut paths: Vec<PathBuf> = [
            "src/lib.rs",
            "src-old/lib.rs",
            "src/a/b.rs",
            "README.md",
            "a",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        paths.sort_by(|a, b| compare_paths(a, b));

        let mut strings: Vec<&str> = vec![
            "src/lib.rs",
            "src-old/lib.rs",
            "src/a/b.rs",
            "README.md",
            "a",
        ];
        strings.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));

        let got: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(got, strings);
    }
}
