//! Sampling defaults and versioned system prompt fragments for Qwen models.
//!
//! Sets the two default sampling rows the build specification gives for
//! thinking and non-thinking generation, guards against greedy decoding
//! while thinking, and adds a presence penalty for a heavily quantised
//! model that repeats. Also embeds the three versioned prompt fragments and
//! assembles the system prompt for a model's parameter count. See task unit
//! `I4`.
//!
//! # Verify the sampling values against the model card
//!
//! The table below is the default for the exact checkpoint this crate was
//! written against. Qwen releases have changed these values between
//! versions before. Check them against the model card for the loaded
//! checkpoint and `dark tune` before trusting them for a new release.

use dark_contract::{Caps, ErrCode, Error, Result, Sampling};

/// The sampling row for thinking generation.
///
/// | Mode | Temperature | Top-p | Top-k | Min-p |
/// | --- | --- | --- | --- | --- |
/// | Thinking | 0.6 | 0.95 | 20 | 0 |
///
/// See task unit `I4`, step 1.
#[must_use]
pub fn thinking_defaults() -> Sampling {
    Sampling {
        temperature: Some(0.6),
        top_p: Some(0.95),
        top_k: Some(20),
        min_p: Some(0.0),
        presence_penalty: None,
        repetition_penalty: None,
        seed: None,
    }
}

/// The sampling row for non-thinking generation.
///
/// | Mode | Temperature | Top-p | Top-k | Min-p |
/// | --- | --- | --- | --- | --- |
/// | Not thinking | 0.7 | 0.8 | 20 | 0 |
///
/// See task unit `I4`, step 1.
#[must_use]
pub fn not_thinking_defaults() -> Sampling {
    Sampling {
        temperature: Some(0.7),
        top_p: Some(0.8),
        top_k: Some(20),
        min_p: Some(0.0),
        presence_penalty: None,
        repetition_penalty: None,
        seed: None,
    }
}

/// The presence penalty this crate applies to a heavily quantised model
/// that repeats.
///
/// The build specification gives a range of 0.5 to 1.0. This sits in the
/// middle of it. See task unit `I4`, step 3.
const HEAVY_QUANT_PRESENCE_PENALTY: f32 = 0.7;

/// Checks whether `quant` names a quantisation heavy enough to need the
/// repeat penalty.
///
/// Two bits per weight and three bits per weight both lose enough precision
/// that a model falls into repetition loops more often. Four bits and
/// above need no adjustment by default.
fn is_heavily_quantised(quant: &str) -> bool {
    let quant = quant.to_ascii_lowercase();
    ["q2", "q3", "iq2", "iq3"]
        .iter()
        .any(|prefix| quant.starts_with(prefix))
}

/// Checks whether `sampling` would decode greedily: always the single most
/// likely token.
///
/// A temperature of zero and a top-k of one both collapse sampling to a
/// pure argmax, regardless of what the other fields say.
#[must_use]
pub fn is_greedy(sampling: &Sampling) -> bool {
    matches!(sampling.temperature, Some(t) if t <= 0.0) || matches!(sampling.top_k, Some(1))
}

/// Rejects a greedy sampling row for a turn that thinks.
///
/// Greedy decoding causes repetition, and it is worst in a long thinking
/// block. See task unit `I4`, step 2.
///
/// # Errors
///
/// Returns [`ErrCode::EngineUnsupported`] when `thinking` is true and
/// `sampling` decodes greedily.
pub fn guard_against_greedy_thinking(sampling: &Sampling, thinking: bool) -> Result<()> {
    if thinking && is_greedy(sampling) {
        return Err(Error::new(
            ErrCode::EngineUnsupported,
            "greedy decoding is not valid in thinking mode; it causes repetition",
        )
        .with_remedy("Raise the temperature above 0, or raise top_k above 1."));
    }
    Ok(())
}

/// Builds the sampling row for `caps`, given whether the turn thinks.
///
/// Starts from [`thinking_defaults`] or [`not_thinking_defaults`], then
/// adds [`HEAVY_QUANT_PRESENCE_PENALTY`] when [`Caps::quant`] names a
/// heavily quantised model. See task unit `I4`, steps 1 to 3.
#[must_use]
pub fn for_model(caps: &Caps, thinking: bool) -> Sampling {
    let mut sampling = if thinking {
        thinking_defaults()
    } else {
        not_thinking_defaults()
    };
    if is_heavily_quantised(&caps.quant) {
        sampling.presence_penalty = Some(HEAVY_QUANT_PRESENCE_PENALTY);
    }
    sampling
}

/// A static `YaRN` context extension, applied only when the engine loads a
/// model.
///
/// `YaRN` trades short-context quality for a longer context window. Loading
/// it costs a fresh model load, which the harness never does mid-turn: see
/// PRD Rule 5. An extended model is therefore its own [`crate::profile::Profile`],
/// selected at load time, never a runtime toggle. See task unit `I4`, step 6.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YarnExtension {
    /// The multiple to scale the trained context length by.
    pub factor: f32,
    /// The context length the model trained on, before this extension.
    pub original_max_context: usize,
}

impl YarnExtension {
    /// The quality warning to show whenever a profile enables this
    /// extension.
    pub const WARNING: &'static str = "Static YaRN extension reduces quality on a short prompt. Use the extended profile only for a document that needs the longer context.";

    /// Returns the context length this extension grants, before the
    /// resident set manager applies any further budget.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation
    )]
    pub fn extended_context(&self) -> usize {
        ((self.original_max_context as f32) * self.factor) as usize
    }
}

/// One versioned system prompt fragment, embedded at compile time.
///
/// Version the file name, not the field: `base.v1.txt` becomes `base.v2.txt`
/// on the next change, and the old file stays available for a rollback.
/// See task unit `I4`, step 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptFragment {
    /// The fragment's name, for example `base`.
    pub name: &'static str,
    /// The version encoded in the fragment's file name.
    pub version: u32,
    /// The fragment text.
    pub text: &'static str,
}

/// Identity, repository context, tool discipline, and the names-not-identifiers
/// rule. Every profile includes this fragment. See task unit `I4`, step 4.
pub const BASE_PROMPT: PromptFragment = PromptFragment {
    name: "base",
    version: 1,
    text: include_str!("../prompts/base.v1.txt"),
};

/// Short imperative instructions for a model below 8B parameters: one
/// instruction per line, no nested conditions. See task unit `I4`, step 4.
pub const COMPACT_PROMPT: PromptFragment = PromptFragment {
    name: "compact",
    version: 1,
    text: include_str!("../prompts/compact.v1.txt"),
};

/// Wayfinder discipline, the plan-do-not-do statement, the seam terms, and
/// the fog test, for a model of 14B parameters and above. See task unit
/// `I4`, step 4.
pub const FULL_PROMPT: PromptFragment = PromptFragment {
    name: "full",
    version: 1,
    text: include_str!("../prompts/full.v1.txt"),
};

/// The parameter count, in billions, below which a model gets
/// [`COMPACT_PROMPT`] instead of [`FULL_PROMPT`].
///
/// The build specification gives "below 8B" for the compact fragment and
/// "14B and above" for the full one, leaving the 8B-to-14B band unnamed.
/// This constant resolves that gap at 8B: an 8B model gets the full
/// fragment. See this crate's completion report for the ambiguity.
const COMPACT_BELOW_PARAMS_B: f32 = 8.0;

/// Assembles the system prompt for a model with `params_b` billion
/// parameters.
///
/// Always includes [`BASE_PROMPT`]. Adds [`COMPACT_PROMPT`] below
/// [`COMPACT_BELOW_PARAMS_B`] and [`FULL_PROMPT`] at or above it.
#[must_use]
pub fn system_prompt_for(params_b: f32) -> String {
    let size_fragment = if params_b < COMPACT_BELOW_PARAMS_B {
        COMPACT_PROMPT
    } else {
        FULL_PROMPT
    };
    format!(
        "{}\n\n{}",
        BASE_PROMPT.text.trim_end(),
        size_fragment.text.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_caps(quant: &str) -> Caps {
        Caps {
            model_id: "fake/qwen3-14b".to_owned(),
            max_context: 32_768,
            granted_context: 32_768,
            native_tools: false,
            thinking: true,
            grammar: true,
            vision: false,
            logprobs: false,
            params_b: 14.0,
            quant: quant.to_owned(),
            device: dark_contract::Device::Cpu,
            measured_tok_s: None,
        }
    }

    #[test]
    fn thinking_defaults_match_the_build_specification() {
        let sampling = thinking_defaults();
        assert_eq!(sampling.temperature, Some(0.6));
        assert_eq!(sampling.top_p, Some(0.95));
        assert_eq!(sampling.top_k, Some(20));
        assert_eq!(sampling.min_p, Some(0.0));
    }

    #[test]
    fn not_thinking_defaults_match_the_build_specification() {
        let sampling = not_thinking_defaults();
        assert_eq!(sampling.temperature, Some(0.7));
        assert_eq!(sampling.top_p, Some(0.8));
        assert_eq!(sampling.top_k, Some(20));
        assert_eq!(sampling.min_p, Some(0.0));
    }

    #[test]
    fn greedy_decoding_is_detected_by_temperature_or_top_k() {
        let mut sampling = thinking_defaults();
        assert!(!is_greedy(&sampling));

        sampling.temperature = Some(0.0);
        assert!(is_greedy(&sampling));

        sampling.temperature = Some(0.6);
        sampling.top_k = Some(1);
        assert!(is_greedy(&sampling));
    }

    #[test]
    fn greedy_decoding_is_refused_only_while_thinking() {
        let mut greedy = thinking_defaults();
        greedy.temperature = Some(0.0);

        let err = guard_against_greedy_thinking(&greedy, true).expect_err("must refuse");
        assert_eq!(err.code, ErrCode::EngineUnsupported);

        guard_against_greedy_thinking(&greedy, false).expect("greedy is fine without thinking");
    }

    #[test]
    fn a_heavily_quantised_model_gets_a_presence_penalty_in_range() {
        for quant in ["q2k", "Q2_K_S", "iq3_xxs", "q3k"] {
            let sampling = for_model(&fake_caps(quant), true);
            let penalty = sampling
                .presence_penalty
                .unwrap_or_else(|| panic!("{quant} should carry a presence penalty"));
            assert!((0.5..=1.0).contains(&penalty), "{quant}: {penalty}");
        }
    }

    #[test]
    fn a_normally_quantised_model_gets_no_extra_presence_penalty() {
        let sampling = for_model(&fake_caps("q4k"), true);
        assert_eq!(sampling.presence_penalty, None);
    }

    #[test]
    fn for_model_selects_the_row_by_thinking_flag() {
        let thinking = for_model(&fake_caps("q4k"), true);
        let not_thinking = for_model(&fake_caps("q4k"), false);
        assert_eq!(thinking.temperature, Some(0.6));
        assert_eq!(not_thinking.temperature, Some(0.7));
    }

    #[test]
    fn yarn_extension_scales_the_context_and_always_carries_the_warning() {
        let extension = YarnExtension {
            factor: 4.0,
            original_max_context: 32_768,
        };
        assert_eq!(extension.extended_context(), 131_072);
        assert!(YarnExtension::WARNING.contains("quality"));
    }

    #[test]
    fn prompt_fragments_are_versioned_and_non_empty() {
        for fragment in [BASE_PROMPT, COMPACT_PROMPT, FULL_PROMPT] {
            assert_eq!(fragment.version, 1);
            assert!(
                !fragment.text.trim().is_empty(),
                "{} is empty",
                fragment.name
            );
        }
    }

    #[test]
    fn below_8b_gets_the_compact_fragment() {
        let prompt = system_prompt_for(4.0);
        assert!(prompt.contains(COMPACT_PROMPT.text.trim_end()));
        assert!(!prompt.contains("wayfinder"));
    }

    #[test]
    fn fourteen_b_and_above_gets_the_full_fragment() {
        let prompt = system_prompt_for(14.0);
        assert!(prompt.contains(FULL_PROMPT.text.trim_end()));

        let prompt32 = system_prompt_for(32.0);
        assert!(prompt32.contains(FULL_PROMPT.text.trim_end()));
    }

    #[test]
    fn every_system_prompt_carries_the_base_fragment() {
        assert!(system_prompt_for(4.0).contains(BASE_PROMPT.text.trim_end()));
        assert!(system_prompt_for(32.0).contains(BASE_PROMPT.text.trim_end()));
    }
}
