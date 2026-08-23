//! The byte comparator that Rule 30 requires.
//!
//! [`compare_paths`] orders two paths by the bytes of their components
//! joined with `/`, whatever separator the platform walks with. It never
//! asks the platform for locale collation rules, so the order is the same
//! on every machine and in every process locale. Use it to sort any list of
//! paths that later feeds a hash.
//!
//! # Why the separator is normalised
//!
//! Windows walks paths with `\` (byte `0x5C`) where Unix uses `/` (byte
//! `0x2F`). Comparing native bytes therefore orders the same repository
//! differently on the two platforms whenever a name's next byte falls
//! between them, and everything numbered from that order — graph nodes,
//! Louvain's visit order, the tree hash — diverges with it. Comparing the
//! `/`-joined component bytes gives one order everywhere, and on Unix it is
//! the same bytes the native form already had. See Rule 30 and Rule 32 in
//! `PRD.md` section 4.9.
//!
//! One consequence to know: the comparison sees *components*, so a
//! redundant `./` segment does not participate. Discovery never produces
//! such a path — every snapshot path is a clean, repository-relative one —
//! and a test pins the equivalence so the property is chosen, not
//! accidental.

use std::cmp::Ordering;
use std::path::Path;

/// Compares two paths by the bytes of their `/`-joined component form.
///
/// This is the byte comparator that Rule 30 requires: it compares the
/// bytes [`slash_bytes`] yields and applies no locale collation. On Unix
/// those are exactly the native bytes; on Windows the `\` separator is
/// read as `/`, so both platforms produce one order.
///
/// Do not sort paths with [`Path`]'s own [`Ord`] impl instead of this
/// function. `Path` compares component by component, which silently
/// disagrees with a byte comparator whenever one path segment holds a byte
/// that sorts before `/`: for example `"a-b"` sorts before `"a/b"` under
/// this function (`-` is `0x2D`, `/` is `0x2F`), but `Path`'s
/// component-wise `Ord` sorts `"a/b"` first, because it compares the first
/// *component* (`"a"` against `"a-b"`) rather than the bytes.
#[must_use]
pub fn compare_paths(a: &Path, b: &Path) -> Ordering {
    slash_bytes(a).cmp(slash_bytes(b))
}

/// The bytes of `path`'s components joined with `/`, without allocating.
///
/// This is the one byte form every sort and every hash in this crate reads,
/// so the platform's own separator never reaches an order or a digest.
pub(crate) fn slash_bytes(path: &Path) -> impl Iterator<Item = u8> + '_ {
    path.components().enumerate().flat_map(|(index, part)| {
        let separator: &[u8] = if index == 0 { b"" } else { b"/" };
        separator
            .iter()
            .copied()
            .chain(part.as_os_str().as_encoded_bytes().iter().copied())
    })
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

    #[test]
    fn a_path_built_from_components_compares_as_its_slash_joined_bytes() {
        let mut built = PathBuf::from("src");
        built.push("seam");
        built.push("mod.rs");

        assert_eq!(
            compare_paths(&built, Path::new("src/seam/mod.rs")),
            Ordering::Equal,
            "how the path was built must not reach the order"
        );
    }

    /// The comparison sees components, so a redundant `./` segment does not
    /// participate. Discovery never produces such a path; this pins that
    /// the equivalence is chosen rather than accidental.
    #[test]
    fn a_redundant_current_directory_segment_does_not_participate() {
        assert_eq!(
            compare_paths(Path::new("a/./b"), Path::new("a/b")),
            Ordering::Equal
        );
    }
}
