//! A small, fixed hash for turning an identifier into a deterministic number.
//!
//! [`std::collections::hash_map::DefaultHasher`] is deliberately avoided
//! here. Its output is stable within one running process but the standard
//! library documents it as unspecified between Rust versions, and the fog
//! map's determinism requirement (task unit `H3`) holds across a rebuild,
//! not only within one process — "the same map must look identical every
//! time" names no compiler version. FNV-1a is a fixed, published algorithm
//! with no such caveat, so [`stable_hash`] gives the same number for the
//! same text on any platform and any build of this crate, for as long as
//! this file exists.

/// The FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// The FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

/// Hashes `text` with FNV-1a, 64 bits wide.
///
/// The fog map (task unit `H3`) uses this to turn a ticket identifier into
/// the angle it sits at: the same identifier always lands at the same
/// angle, so the map layout never shuffles between two runs over the same
/// data.
#[must_use]
pub fn stable_hash(text: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_text_always_hashes_the_same() {
        assert_eq!(stable_hash("T-018"), stable_hash("T-018"));
    }

    #[test]
    fn different_text_usually_hashes_differently() {
        assert_ne!(stable_hash("T-018"), stable_hash("T-019"));
    }

    #[test]
    fn the_empty_string_hashes_to_the_offset_basis() {
        // No byte ever runs through the loop, so the basis passes through
        // unchanged. This pins the constant itself, not just the algorithm.
        assert_eq!(stable_hash(""), FNV_OFFSET_BASIS);
    }

    #[test]
    fn a_one_byte_difference_changes_the_hash() {
        assert_ne!(stable_hash("a"), stable_hash("b"));
    }
}
