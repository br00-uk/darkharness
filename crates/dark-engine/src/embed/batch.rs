//! Batches embedding input (task unit `B5`, step 2).
//!
//! A pack index contains thousands of chunks; per-call overhead dominates
//! if each one is a separate request. [`chunks`] groups input into batches
//! of a bounded size, so a caller sends one request per batch instead of
//! one per text.

/// The default batch size: how many texts one embedding request carries.
///
/// Chosen to keep one request's prompt well under a typical resident
/// embedding model's context window even for a long chunk, while still
/// amortising per-call overhead across dozens of texts.
pub const DEFAULT_BATCH_SIZE: usize = 32;

/// Splits `texts` into batches of at most `batch_size` items each, in the
/// original order.
///
/// Returns no batches for empty input. Panics if `batch_size` is `0`: a
/// caller asking for zero-sized batches has a bug to fix, not a case to
/// handle silently.
///
/// # Panics
///
/// Panics when `batch_size` is `0`.
#[must_use]
pub fn chunks<T>(texts: &[T], batch_size: usize) -> Vec<&[T]> {
    assert!(batch_size > 0, "batch_size must be greater than zero");
    texts.chunks(batch_size).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_produces_no_batches() {
        let texts: Vec<String> = Vec::new();
        assert!(chunks(&texts, 32).is_empty());
    }

    #[test]
    fn input_smaller_than_the_batch_size_is_one_batch() {
        let texts = vec!["a".to_owned(), "b".to_owned()];
        let batches = chunks(&texts, 32);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
    }

    #[test]
    fn input_splits_evenly_across_batches() {
        let texts: Vec<usize> = (0..64).collect();
        let batches = chunks(&texts, 32);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 32);
        assert_eq!(batches[1].len(), 32);
    }

    #[test]
    fn the_last_batch_carries_the_remainder() {
        let texts: Vec<usize> = (0..70).collect();
        let batches = chunks(&texts, 32);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[2].len(), 6);
    }

    #[test]
    fn batches_preserve_order() {
        let texts: Vec<usize> = (0..10).collect();
        let batches = chunks(&texts, 3);
        let flattened: Vec<usize> = batches.into_iter().flatten().copied().collect();
        assert_eq!(flattened, texts);
    }

    #[test]
    #[should_panic(expected = "batch_size must be greater than zero")]
    fn a_zero_batch_size_panics() {
        let texts = vec![1];
        let _ = chunks(&texts, 0);
    }
}
