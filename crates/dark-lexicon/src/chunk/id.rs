//! Computing a chunk identifier.
//!
//! Task unit `G3`, Do 9: `chunk_id = blake3(pack_id ‖ breadcrumb ‖
//! ordinal)`. This concatenates the three inputs with a NUL separator
//! between each — a fixed byte, never a value any of the three inputs can
//! itself contain unnoticed, since a breadcrumb built by
//! [`super::algorithm::format_breadcrumb`] never carries one — and hashes
//! the result with BLAKE3.
//!
//! The ordinal is encoded as 4 little-endian bytes rather than its decimal
//! text: a fixed-width binary encoding cannot collide between, say,
//! ordinal `1` followed by breadcrumb text starting with a digit and
//! ordinal `12` with a different split. This keeps the identifier
//! deterministic across platforms: BLAKE3 and little-endian integer
//! encoding are both platform-independent.

/// Computes the chunk identifier for one chunk.
///
/// Returns the identifier as lowercase hexadecimal, the same rendering
/// [`crate::pack::PackHash::to_hex`] uses, so both hash-shaped values in
/// this crate look the same to a person reading a log or a `chunks.jsonl`
/// line.
#[must_use]
pub fn compute(pack_id: &str, breadcrumb: &str, ordinal: u32) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(pack_id.as_bytes());
    hasher.update(&[0u8]);
    hasher.update(breadcrumb.as_bytes());
    hasher.update(&[0u8]);
    hasher.update(&ordinal.to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_inputs_always_produce_the_same_id() {
        let a = compute("tokio@1.47.0", "tokio › runtime › Builder", 3);
        let b = compute("tokio@1.47.0", "tokio › runtime › Builder", 3);
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_ordinal_changes_the_id() {
        let a = compute("tokio@1.47.0", "tokio › runtime › Builder", 3);
        let b = compute("tokio@1.47.0", "tokio › runtime › Builder", 4);
        assert_ne!(a, b);
    }

    #[test]
    fn a_different_breadcrumb_changes_the_id() {
        let a = compute("tokio@1.47.0", "tokio › runtime › Builder", 0);
        let b = compute("tokio@1.47.0", "tokio › runtime › Handle", 0);
        assert_ne!(a, b);
    }

    #[test]
    fn a_different_pack_id_changes_the_id() {
        let a = compute("tokio@1.47.0", "tokio › runtime", 0);
        let b = compute("tokio@1.48.0", "tokio › runtime", 0);
        assert_ne!(a, b);
    }

    #[test]
    fn the_id_is_sixty_four_lowercase_hex_characters() {
        let id = compute("p", "b", 0);
        assert_eq!(id.len(), 64);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
