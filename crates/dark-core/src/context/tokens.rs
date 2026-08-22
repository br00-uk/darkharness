//! Token counting.
//!
//! Do step 8 says to count tokens with [`dark_contract::Engine::tokenize`].
//! Do not says the same thing from the other side: do not estimate tokens by
//! character count. The two functions here are the only place this module
//! counts tokens, so every other function in `context/` goes through them.

use dark_contract::{Engine, Message, Result, RoleClass};

/// Counts the tokens in `text` for the model that serves `class`.
///
/// This calls [`dark_contract::Engine::tokenize`] and returns its count
/// unchanged. Do not replace this call with a character-count or
/// word-count estimate: the token boundaries a real tokenizer produces do
/// not line up with either, and the budgets in Appendix B are token
/// budgets, not character budgets.
///
/// # Errors
///
/// Returns an error when [`dark_contract::Engine::tokenize`] fails, for
/// example when no tokenizer is loaded for `class`.
pub fn count_tokens(engine: &dyn Engine, class: RoleClass, text: &str) -> Result<usize> {
    engine.tokenize(class, text)
}

/// Counts the combined tokens in every message's text content.
///
/// # Errors
///
/// Returns an error when [`count_tokens`] fails for any message.
pub fn count_message_tokens(
    engine: &dyn Engine,
    class: RoleClass,
    messages: &[Message],
) -> Result<usize> {
    let mut total = 0_usize;
    for message in messages {
        total += count_tokens(engine, class, &message.text_content())?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use dark_contract::Role;
    use dark_engine_fake::FakeEngine;

    use super::*;

    #[test]
    fn count_tokens_matches_the_engine_tokenizer() {
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let count = count_tokens(&engine, RoleClass::Worker, "one two three").unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn count_message_tokens_sums_every_message() {
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let messages = vec![
            Message::text(Role::User, "one two"),
            Message::text(Role::Assistant, "three four five"),
        ];
        let count = count_message_tokens(&engine, RoleClass::Worker, &messages).unwrap();
        assert_eq!(count, 5);
    }

    /// A character-count estimator disagrees with the real tokenizer. This
    /// is not a test of a type this module ships; it exists to demonstrate,
    /// in code, why Do says not to estimate tokens by character count.
    #[test]
    fn a_character_count_estimator_would_be_wrong() {
        let engine = FakeEngine::with_replies(Vec::<String>::new());
        let text = "internationalisation refactor";

        let real = count_tokens(&engine, RoleClass::Worker, text).unwrap();
        let char_estimate = text.chars().count();

        assert_ne!(
            real, char_estimate,
            "a character count happened to match the token count for {text:?}; \
             pick a different text so this test still proves the point"
        );
    }
}
