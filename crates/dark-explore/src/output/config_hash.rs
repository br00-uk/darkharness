//! `config_hash`: a digest over everything that changes the analysis
//! without changing the commit.
//!
//! Rule 29 promises identical bytes "for the same commit and the same
//! configuration." [`tree_sha`](super::tree_sha) is the commit half of that
//! promise; this module is the configuration half. Three things change what
//! [`crate::seam::analyse`] and [`crate::discover::discover`] produce
//! without touching a single byte of the repository itself, and all three
//! feed [`compute`]:
//!
//! - [`Weights`], the five seam-score weights (F3, Do step 7: "Read the
//!   weights from the configuration. Include them in the configuration
//!   hash.");
//! - [`Window`], the co-change commit window (F3, Do step 6: "Include the
//!   window in the configuration hash.");
//! - [`DiscoverOptions`], the file-size limit and the vendored directory
//!   list (F1, Do step 2) — a file discovery would otherwise have excluded
//!   or kept changes which files ever reach the analysis at all.
//!
//! Nothing else feeds it. The set of thirteen grammars is fixed at compile
//! time, not read from a configuration, so it has no business here; the
//! [`Lock::grammar_versions`](super::Lock::grammar_versions) field records
//! it in the `.lock` file instead, where a *tool* upgrade shows up rather
//! than a *configuration* change. The two constants this module deliberately
//! excludes for the same reason — a fixed algorithm, not a configuration —
//! are documented at their own definitions:
//! [`MAX_REPORTED_SEAMS`](super::document::MAX_REPORTED_SEAMS) and
//! [`MAX_REPORTED_HOTSPOTS`](super::document::MAX_REPORTED_HOTSPOTS), plus
//! the hotspot weights beside them. Changing one of those changes the
//! report's *shape*, not the *analysis* Rule 29 is about, and it ships as a
//! `tool_version` change instead.
//!
//! # Canonical form
//!
//! [`compute`] hashes a fixed sequence of little-endian integers and IEEE
//! 754 bit patterns, each length-prefixed where its length can vary (the
//! vendor directory list), so no field can be crafted to collide with its
//! neighbour across a boundary. `vendor_dirs` is sorted with
//! [`compare_path_strings`] before hashing: two configurations naming the
//! same directories in a different order exclude the same files, so they
//! must hash the same.
//!
//! Hashing the raw bits of a weight rather than a formatted decimal string
//! is safe here because nothing computes with these values before they are
//! hashed — they are the configuration's own input, stored exactly as
//! given, and IEEE 754 arithmetic already has to be bit-reproducible across
//! this workspace's supported platforms for [`crate::seam`]'s own
//! determinism guarantees (Rules 29 to 32) to hold at all.

use std::path::Path;

use crate::discover::DiscoverOptions;
use crate::seam::{Weights, Window};

use super::path::{compare_path_strings, path_to_string};

/// Computes the configuration hash. See the module documentation for
/// exactly what feeds it and why.
#[must_use]
pub(super) fn compute(
    weights: &Weights,
    window: Window,
    discover_options: &DiscoverOptions,
) -> blake3::Hash {
    let mut hasher = blake3::Hasher::new();

    for weight in [
        weights.betweenness,
        weights.crosses_community,
        weights.abstractness,
        weights.inverse_cochange,
        weights.tested,
    ] {
        hasher.update(&weight.to_le_bytes());
    }

    hasher.update(&(window.commits as u64).to_le_bytes());

    hasher.update(&discover_options.max_file_size.to_le_bytes());

    let mut vendor_dirs: Vec<String> = discover_options
        .vendor_dirs
        .iter()
        .map(|dir| path_to_string(Path::new(dir)))
        .collect();
    vendor_dirs.sort_by(|a, b| compare_path_strings(a, b));
    hasher.update(&(vendor_dirs.len() as u64).to_le_bytes());
    for dir in &vendor_dirs {
        let bytes = dir.as_bytes();
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }

    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(vendor_dirs: &[&str]) -> DiscoverOptions {
        DiscoverOptions {
            max_file_size: 1_048_576,
            vendor_dirs: vendor_dirs.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn the_same_configuration_hashes_the_same_every_time() {
        let weights = Weights::default();
        let window = Window::default();
        let discover = options(&["vendor", "node_modules"]);

        let first = compute(&weights, window, &discover);
        let second = compute(&weights, window, &discover);
        assert_eq!(first, second);
    }

    #[test]
    fn a_different_weight_changes_the_hash() {
        let window = Window::default();
        let discover = options(&["vendor"]);

        let base = compute(&Weights::default(), window, &discover);
        let changed = compute(
            &Weights {
                betweenness: 0.30,
                ..Weights::default()
            },
            window,
            &discover,
        );
        assert_ne!(base, changed);
    }

    #[test]
    fn a_different_window_changes_the_hash() {
        let weights = Weights::default();
        let discover = options(&["vendor"]);

        let base = compute(&weights, Window { commits: 500 }, &discover);
        let changed = compute(&weights, Window { commits: 1000 }, &discover);
        assert_ne!(base, changed);
    }

    #[test]
    fn a_different_max_file_size_changes_the_hash() {
        let weights = Weights::default();
        let window = Window::default();

        let base = compute(&weights, window, &options(&["vendor"]));
        let changed = compute(
            &weights,
            window,
            &DiscoverOptions {
                max_file_size: 2_048,
                vendor_dirs: vec!["vendor".to_owned()],
            },
        );
        assert_ne!(base, changed);
    }

    #[test]
    fn a_different_vendor_list_changes_the_hash() {
        let weights = Weights::default();
        let window = Window::default();

        let base = compute(&weights, window, &options(&["vendor"]));
        let changed = compute(&weights, window, &options(&["vendor", "third_party"]));
        assert_ne!(base, changed);
    }

    #[test]
    fn the_same_vendor_dirs_in_a_different_order_hash_the_same() {
        let weights = Weights::default();
        let window = Window::default();

        let forward = compute(
            &weights,
            window,
            &options(&["node_modules", "third_party", "vendor"]),
        );
        let reversed = compute(
            &weights,
            window,
            &options(&["vendor", "third_party", "node_modules"]),
        );
        assert_eq!(forward, reversed);
    }
}
