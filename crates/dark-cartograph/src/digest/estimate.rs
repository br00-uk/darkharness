//! A token estimate for deciding when the digest needs compression.
//!
//! Task unit `D3`'s budget (1200 tokens) is a real token budget, counted
//! against a real tokenizer, but that tokenizer lives behind
//! [`dark_contract::Engine::tokenize`] — a method that needs a loaded
//! model to answer. This crate cannot hold one: Rule 16 (`CLAUDE.md`)
//! keeps `dark-cartograph` down to `dark-contract` and third-party
//! crates only, so it can never depend on `dark-engine` or
//! `dark-engine-fake` to obtain an [`dark_contract::Engine`] to ask.
//!
//! [`estimate_tokens`] is this module's stand-in: a whitespace word
//! count, matching the counting convention `dark-engine-fake`'s scripted
//! tokenizer already uses elsewhere in this workspace (see
//! `dark-engine-fake::token_count`, `dark-core::context::tokens`'s test
//! `count_tokens_matches_the_engine_tokenizer`). It decides *when this
//! module compresses*, not what the final digest costs against the real
//! tokenizer once it reaches the prefix — that check belongs to whatever
//! assembles the prefix with a real `Engine` in hand (task unit `A3`).
//! See this crate's top-level report for why that split is a real
//! limitation of the dependency graph, not a choice made lightly.
//!
//! A real BPE tokenizer usually produces *more* tokens than a word count
//! for ordinary English prose (it splits words into sub-word pieces), so
//! [`estimate_tokens`] budgets down to three-quarters of the true 1200
//! token target: compressing at an estimated 900 "words" leaves headroom
//! for the real tokenizer to run higher and still land under 1200.

/// The word-count budget this module compresses against.
///
/// Three-quarters of the real 1200-token budget (task unit `D3`), to
/// leave headroom for a real tokenizer producing more tokens per word
/// than this estimate does. See the module documentation.
pub(super) const ESTIMATED_BUDGET: usize = 900;

/// Estimates the token count of `text` by counting whitespace-separated
/// words.
///
/// This is not the real tokenizer. See the module documentation for why
/// this crate cannot reach one, and for how the budget above compensates.
pub(super) fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_whitespace_separated_words() {
        assert_eq!(estimate_tokens("one two three"), 3);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("   "), 0);
    }

    #[test]
    fn is_pure() {
        let text = "the quick brown fox";
        assert_eq!(estimate_tokens(text), estimate_tokens(text));
    }
}
