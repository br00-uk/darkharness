//! Reranking as single-token scoring (task unit `B5`, step 4).
//!
//! `rerank` is not a second embedding pass. It asks the model a yes/no
//! question about one document against one query, with `max_tokens` set
//! to 1, and reads the log probability the model assigned to the
//! affirmative token. [`affirmative_probability`] is the pure half of
//! that: given the [`mistralrs::Logprobs`] a completion returned, find the
//! affirmative token's probability, wherever in the response it appears.
//!
//! # A note on the log base
//!
//! mistral.rs's `logprob` fields hold **log base 10** of the probability,
//! not the natural log a name like "logprob" usually implies elsewhere.
//! [`affirmative_probability`] converts with `10f32.powf`, not `f32::exp`,
//! for exactly that reason — using the wrong base silently produces a
//! score that is monotonic but not comparable to a true probability. See
//! `docs/adr/0006`.

use dark_contract::{Caps, ErrCode, Error, Result};

/// The token this harness treats as the affirmative answer.
///
/// mistral.rs decodes a candidate token to its surface text in
/// [`mistralrs::TopLogprob::bytes`]; that text often carries the leading
/// space the tokenizer attached (`" yes"`, not `"yes"`), so every
/// comparison in this module trims and lower-cases before matching.
pub const AFFIRMATIVE_TOKEN: &str = "yes";

/// Builds the fixed prompt `rerank` sends the model for one query/document
/// pair.
///
/// The instruction asks for exactly one word, matching `max_tokens = 1`:
/// a longer answer would just be truncated, and asking for one word keeps
/// the model from spending its one token on something other than the
/// judgement itself.
#[must_use]
pub fn prompt(query: &str, document: &str) -> String {
    format!(
        "Query: {query}\nDocument: {document}\nIs the document relevant to the query? Answer \
         yes or no with exactly one word."
    )
}

/// Finds the affirmative token's probability in `logprobs`, when it
/// appears at all.
///
/// Checks the token the model actually generated first (the exact
/// probability mistral.rs reported for it), then falls back to searching
/// `top_logprobs` for the same text among the alternatives the model
/// considered instead. Returns `None` when neither carries it — this
/// happens when the model was confident enough in some other answer that
/// the affirmative token's probability did not make the top-K list mistral.rs
/// returned; a caller then treats it as a very low but unobserved score,
/// never as zero (silence is not the same claim as "definitely not").
#[must_use]
pub fn affirmative_probability(logprobs: &mistralrs::Logprobs) -> Option<f32> {
    let entry = logprobs.content.as_ref()?.first()?;
    if is_affirmative(&entry.token) {
        return Some(to_probability(entry.logprob));
    }
    entry
        .top_logprobs
        .iter()
        .find(|candidate| candidate.bytes.as_deref().is_some_and(is_affirmative))
        .map(|candidate| to_probability(candidate.logprob))
}

/// Reports whether `token` is the affirmative token, ignoring surrounding
/// whitespace and case.
fn is_affirmative(token: &str) -> bool {
    token.trim().eq_ignore_ascii_case(AFFIRMATIVE_TOKEN)
}

/// Converts a mistral.rs log10 probability to a plain probability in
/// `[0.0, 1.0]`.
fn to_probability(log10_probability: f32) -> f32 {
    10f32.powf(log10_probability).clamp(0.0, 1.0)
}

/// Gates `rerank` on [`Caps::logprobs`] (task unit `B5`, step 5).
///
/// # Errors
///
/// Returns [`ErrCode::EngineUnsupported`] when `caps.logprobs` is `false`.
pub fn require_logprobs(caps: &Caps) -> Result<()> {
    if caps.logprobs {
        Ok(())
    } else {
        Err(Error::new(
            ErrCode::EngineUnsupported,
            format!(
                "{} returns no log probabilities; rerank needs them",
                caps.model_id
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::Device;
    use mistralrs::core::ResponseLogprob;
    use mistralrs::{Logprobs, TopLogprob};

    fn caps(logprobs: bool) -> Caps {
        Caps {
            model_id: "fake/qwen3-4b".to_owned(),
            max_context: 32_768,
            granted_context: 32_768,
            native_tools: false,
            thinking: false,
            grammar: false,
            vision: false,
            logprobs,
            params_b: 4.0,
            quant: "q4k".to_owned(),
            device: Device::Cpu,
            measured_tok_s: None,
        }
    }

    fn logprobs_with(
        generated_token: &str,
        generated_log10: f32,
        top: Vec<(u32, &str, f32)>,
    ) -> Logprobs {
        Logprobs {
            content: Some(vec![ResponseLogprob {
                token: generated_token.to_owned(),
                logprob: generated_log10,
                bytes: None,
                top_logprobs: top
                    .into_iter()
                    .map(|(token, bytes, logprob)| TopLogprob {
                        token,
                        logprob,
                        bytes: Some(bytes.to_owned()),
                    })
                    .collect(),
            }]),
        }
    }

    #[test]
    fn uses_the_generated_tokens_own_probability_when_it_is_affirmative() {
        // log10(1.0) = 0.0, so this is a certain "yes".
        let logprobs = logprobs_with("yes", 0.0, vec![]);
        let score = affirmative_probability(&logprobs).unwrap();
        assert!((score - 1.0).abs() < 1e-6, "got {score}");
    }

    #[test]
    fn trims_and_lowercases_the_generated_token_before_matching() {
        let logprobs = logprobs_with(" Yes", -0.1, vec![]);
        assert!(affirmative_probability(&logprobs).is_some());
    }

    #[test]
    fn falls_back_to_top_logprobs_when_the_generated_token_is_no() {
        // The model said "no" with log10(0.9), but "yes" is still visible
        // among the alternatives at log10(0.1).
        let logprobs = logprobs_with("no", 0.9f32.log10(), vec![(42, " yes", 0.1f32.log10())]);
        let score = affirmative_probability(&logprobs).unwrap();
        assert!((score - 0.1).abs() < 1e-4, "got {score}");
    }

    #[test]
    fn returns_none_when_the_affirmative_token_appears_nowhere() {
        let logprobs = logprobs_with("no", 0.9f32.log10(), vec![(7, "maybe", 0.05f32.log10())]);
        assert_eq!(affirmative_probability(&logprobs), None);
    }

    #[test]
    fn returns_none_for_an_empty_response() {
        let logprobs = Logprobs { content: None };
        assert_eq!(affirmative_probability(&logprobs), None);
    }

    #[test]
    fn require_logprobs_passes_when_capable() {
        assert!(require_logprobs(&caps(true)).is_ok());
    }

    #[test]
    fn require_logprobs_fails_with_engine_unsupported() {
        let err = require_logprobs(&caps(false)).unwrap_err();
        assert_eq!(err.code, ErrCode::EngineUnsupported);
    }

    #[test]
    fn prompt_asks_for_exactly_one_word_matching_max_tokens_one() {
        let text = prompt("a query", "a document");
        assert!(text.contains("a query"));
        assert!(text.contains("a document"));
        assert!(text.contains("one word"));
    }

    #[test]
    fn a_higher_probability_alternative_still_loses_to_the_generated_answer() {
        // Sanity: the function must not accidentally always prefer
        // top_logprobs over the generated token.
        let logprobs = logprobs_with("yes", 0.99f32.log10(), vec![(1, "no", 0.5f32.log10())]);
        let score = affirmative_probability(&logprobs).unwrap();
        assert!((score - 0.99).abs() < 1e-4);
    }
}
