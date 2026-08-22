//! Caps captured command output at a fixed size.
//!
//! A test failure often prints its error near the end. A plain truncation at
//! the end keeps the boring setup output and drops the error. This module
//! keeps the head and the tail instead, and marks the cut with an elision
//! marker.

/// The number of characters that the harness keeps from a command's output.
///
/// The count applies to the original text that survives, not to the marker
/// text that this module adds. See [`cap_output`].
pub(crate) const OUTPUT_CAP: usize = 30_000;

/// The share of [`OUTPUT_CAP`] that the head keeps.
///
/// The tail keeps the rest. The tail is larger, because the tail usually
/// holds the error.
const HEAD_NUMERATOR: usize = 2;
/// The divisor that pairs with [`HEAD_NUMERATOR`].
const HEAD_DENOMINATOR: usize = 5;

/// Caps `text` at [`OUTPUT_CAP`] characters.
///
/// Returns `text` unchanged when it already fits. Otherwise keeps the head
/// and the tail, and joins them with an elision marker that states how many
/// characters the harness dropped.
pub(crate) fn cap_output(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= OUTPUT_CAP {
        return text.to_owned();
    }

    let head_len = OUTPUT_CAP * HEAD_NUMERATOR / HEAD_DENOMINATOR;
    let tail_len = OUTPUT_CAP - head_len;
    let elided = chars.len() - head_len - tail_len;

    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();

    format!("{head}\n… [{elided} characters elided; the head and the tail remain] …\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::{OUTPUT_CAP, cap_output};

    #[test]
    fn short_text_passes_through_unchanged() {
        assert_eq!(cap_output("hello"), "hello");
    }

    #[test]
    fn text_at_exactly_the_cap_passes_through_unchanged() {
        let text: String = "a".repeat(OUTPUT_CAP);
        assert_eq!(cap_output(&text), text);
    }

    #[test]
    fn long_text_keeps_the_head_and_the_tail() {
        // Build "0123456789" repeated so the head and the tail are distinctive.
        let body: String = (0..OUTPUT_CAP + 10_000)
            .map(|i| char::from(b'0' + u8::try_from(i % 10).unwrap()))
            .collect();
        let capped = cap_output(&body);

        assert!(capped.starts_with(&body[..100]));
        assert!(capped.ends_with(&body[body.len() - 100..]));
        assert!(capped.contains("elided"));
        assert!(capped.len() < body.len());
    }

    #[test]
    fn the_elided_count_matches_the_dropped_characters() {
        let total = OUTPUT_CAP + 4_321;
        let body: String = "x".repeat(total);
        let capped = cap_output(&body);
        let expected = format!("{}", total - OUTPUT_CAP);
        assert!(
            capped.contains(&format!("[{expected} characters elided")),
            "capped output did not report {expected} elided characters: {capped}"
        );
    }

    #[test]
    fn the_tail_is_larger_than_the_head_so_errors_survive() {
        // The last few hundred characters of a long stream are what a failing
        // test usually prints. A plain truncation at the end drops exactly
        // this, so the tail must be the larger half.
        let body: String = "y".repeat(OUTPUT_CAP * 2);
        let capped = cap_output(&body);
        let marker_index = capped.find('…').unwrap();
        let head_len = marker_index;
        let tail_len = capped.chars().rev().take_while(|&c| c == 'y').count();
        assert!(tail_len > head_len);
    }

    #[test]
    fn unicode_text_is_capped_on_a_character_boundary() {
        let body: String = "é".repeat(OUTPUT_CAP + 1_000);
        // Must not panic on a multi-byte boundary.
        let capped = cap_output(&body);
        assert!(capped.contains('é'));
    }
}
