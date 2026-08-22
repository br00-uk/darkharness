//! Builds the unified diff that every mutating tool attaches to its result.

use similar::TextDiff;

/// Renders a unified diff between `old` and `new`, labelled with `label`.
///
/// Returns `None` when the two are identical, so a caller can leave
/// [`dark_contract::ToolResult::diff`] unset instead of attaching an empty
/// string.
pub(crate) fn render(label: &str, old: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }

    let diff = TextDiff::from_lines(old, new);
    let mut unified = diff.unified_diff();
    let old_label = format!("a/{label}");
    let new_label = format!("b/{label}");
    unified.header(&old_label, &new_label);

    let mut buf = Vec::new();
    unified
        .to_writer(&mut buf)
        .expect("writing a diff to an in-memory buffer cannot fail");
    Some(String::from_utf8(buf).expect("similar emits utf8 output for utf8 input"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_content_produces_no_diff() {
        assert_eq!(render("a.txt", "same\n", "same\n"), None);
    }

    #[test]
    fn a_change_produces_a_header_and_a_hunk() {
        let diff = render("a.txt", "one\ntwo\n", "one\nthree\n").unwrap();
        assert!(diff.contains("--- a/a.txt"));
        assert!(diff.contains("+++ b/a.txt"));
        assert!(diff.contains("-two"));
        assert!(diff.contains("+three"));
    }

    #[test]
    fn a_new_file_shows_every_line_as_an_addition() {
        let diff = render("new.txt", "", "line one\nline two\n").unwrap();
        assert!(diff.contains("+line one"));
        assert!(diff.contains("+line two"));
    }
}
