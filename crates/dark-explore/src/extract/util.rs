//! Small, language-agnostic helpers shared by the `lang` adapters.

use tree_sitter::Node;

/// Returns `true` when `haystack` contains `word` as a whole token: bounded
/// by the start/end of the string or by a byte that is neither
/// alphanumeric nor `_`.
///
/// This is how the `c`, `cpp`, `java`, `csharp`, and `rust` adapters read a
/// modifier keyword (`static`, `public`, `abstract`, `pub`) out of raw
/// source text without depending on a grammar's exact field layout for its
/// modifier nodes.
pub(crate) fn has_word(haystack: &str, word: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|token| token == word)
}

/// Returns the source text from the start of `node` up to the start of
/// `name_node`, or an empty string when the bytes are not valid UTF-8 or
/// `name_node` starts before `node` does.
///
/// A modifier keyword (`pub`, `static`, `public`, `abstract`) sits in this
/// range in every grammar this crate supports: the grammars that carry
/// modifiers give the declaration node itself a span that starts at the
/// first modifier, ahead of the `name` field. See the `rust`, `c`, `cpp`,
/// `java`, and `csharp` grammar adapters.
pub(crate) fn text_before_name<'a>(
    node: &Node<'_>,
    name_node: &Node<'_>,
    source: &'a [u8],
) -> &'a str {
    let end = name_node.start_byte();
    let start = node.start_byte();
    if end < start || end > source.len() {
        return "";
    }
    std::str::from_utf8(&source[start..end]).unwrap_or("")
}

/// Returns the node's own source text, or an empty string when the bytes
/// are not valid UTF-8.
pub(crate) fn node_text<'a>(node: &Node<'_>, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_word_matches_a_whole_token_only() {
        assert!(has_word("public static", "static"));
        assert!(!has_word("static_assert", "static"));
        assert!(!has_word("nonstatic", "static"));
        assert!(has_word("pub fn foo", "pub"));
        assert!(!has_word("pubsub fn foo", "pub"));
    }
}
