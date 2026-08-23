//! The section 4.1 memory estimation formula.
//!
//! ```text
//! weights   = parameters × bits_per_weight / 8
//! kv_cache  = 2 × layers × kv_heads × head_dim × context_length × bytes_per_element
//! total     = weights + kv_cache + 10% headroom
//! ```
//!
//! Rule 4 requires the resident set manager to estimate memory before a load
//! and refuse one that does not fit. Every function here is pure arithmetic
//! over values that come from the model configuration file (layer count,
//! key-value head count, head dimension) or from the quantisation name, so a
//! test drives it with no model file and no accelerator. [`tests::five_published_models`]
//! pins the formula against five published Qwen3 configurations, which is
//! the honest substitute this module can offer today for the build
//! specification's "within 10% of measured memory" acceptance criterion:
//! that criterion needs a measurement on real hardware with real weights on
//! disk, which this sandbox has neither of. Measuring the five models on a
//! real machine and checking the estimate against that measurement is
//! deferred to that hardware.

use serde::{Deserialize, Serialize};

use dark_contract::{ErrCode, Error, Result};

/// The bytes that one element of the key-value cache uses.
///
/// mistral.rs keeps the key-value cache in half precision by default
/// regardless of the compute dtype the weights load in, so this is a
/// constant rather than a per-model field. See `docs/adr/0006`.
pub const KV_CACHE_BYTES_PER_ELEMENT: u64 = 2;

/// The shape values that section 4.1's formula reads from a model's
/// `config.json`.
///
/// `params` is the total parameter count, not the non-embedding count: the
/// weights that the resident set manager loads include the embedding table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// The total parameter count.
    pub params: u64,
    /// The number of transformer layers.
    pub layers: u64,
    /// The number of key-value heads (the GQA group count).
    pub kv_heads: u64,
    /// The dimension of one attention head.
    pub head_dim: u64,
}

/// Returns the bits per weight for a quantisation name, for example `4.0`
/// for `q4k`.
///
/// # Errors
///
/// Returns [`ErrCode::EngineUnsupported`] when `quant` names no
/// quantisation this harness recognises.
pub fn bits_per_weight(quant: &str) -> Result<f64> {
    let bits = match quant.to_ascii_lowercase().as_str() {
        "q2k" => 2.0,
        "q3k" => 3.0,
        "q4_0" | "q4_1" | "q4k" | "afq4" | "hqq4" => 4.0,
        "q5_0" | "q5_1" | "q5k" => 5.0,
        "q6k" | "afq6" => 6.0,
        "q8_0" | "q8_1" | "q8k" | "afq8" | "f8e4m3" | "f8q8" => 8.0,
        // An absent or automatic quantisation means unquantised weights,
        // which cost the same as f16.
        "f16" | "bf16" | "" | "none" | "auto" => 16.0,
        "f32" => 32.0,
        other => {
            return Err(Error::new(
                ErrCode::EngineUnsupported,
                format!("'{other}' names no quantisation this harness recognises"),
            )
            .with_remedy("Use a supported quantisation, for example q4k, q8_0, or f16."));
        }
    };
    Ok(bits)
}

/// Returns the weight bytes for `params` parameters at `bits_per_weight`
/// bits each.
#[must_use]
// A parameter count is far below the 2^53 where f64 loses integers, the
// product of positive inputs cannot be negative, and ceil() keeps it whole,
// so each cast is exact for every realistic model.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn weights_bytes(params: u64, bits_per_weight: f64) -> u64 {
    let bytes = (params as f64 * bits_per_weight / 8.0).ceil();
    bytes as u64
}

/// Returns the key-value cache bytes for `context_length` tokens of a model
/// with `cfg`'s shape.
#[must_use]
pub fn kv_cache_bytes(cfg: ModelConfig, context_length: u64) -> u64 {
    2 * cfg.layers * cfg.kv_heads * cfg.head_dim * context_length * KV_CACHE_BYTES_PER_ELEMENT
}

/// Adds section 4.1's 10% headroom to `bytes`.
#[must_use]
pub fn with_headroom(bytes: u64) -> u64 {
    bytes.saturating_add(bytes / 10)
}

/// Returns the total memory that a model needs: weights, key-value cache,
/// and 10% headroom.
#[must_use]
pub fn total_bytes(cfg: ModelConfig, context_length: u64, quant_bits: f64) -> u64 {
    let subtotal = weights_bytes(cfg.params, quant_bits) + kv_cache_bytes(cfg, context_length);
    with_headroom(subtotal)
}

/// Returns the largest context length that fits in `budget_bytes`, given
/// `weights_bytes` already committed to holding the model's weights.
///
/// Returns `None` when the weights alone, with headroom, do not fit —
/// this is a stronger claim than "zero context fits": no positive amount
/// of memory freed up elsewhere would help, because the weights alone are
/// already over budget. Returns `Some(0)` in the different case where the
/// weights do fit but nothing is left over for a key-value cache. Never
/// returns more than `max_context`, the length the model itself supports.
///
/// This is the computation behind `Caps::granted_context` (Rule 4): a
/// caller budgets against the value this function returns, never against
/// the model's raw `max_context`.
#[must_use]
pub fn granted_context(
    cfg: ModelConfig,
    weights_bytes: u64,
    budget_bytes: u64,
    max_context: u64,
) -> Option<u64> {
    // total = (weights + kv) * 1.1 <= budget
    // kv <= budget / 1.1 - weights
    //
    // Integer division here rounds the allowance down, so the context this
    // returns never lets the true total exceed `budget_bytes`: see
    // `granted_context_never_lets_the_true_total_exceed_the_budget` for the
    // property test that pins this down.
    let allowance = budget_bytes.saturating_mul(10) / 11;
    let kv_budget = allowance.checked_sub(weights_bytes)?;
    let per_token = 2 * cfg.layers * cfg.kv_heads * cfg.head_dim * KV_CACHE_BYTES_PER_ELEMENT;
    if per_token == 0 {
        return Some(max_context);
    }
    Some((kv_budget / per_token).min(max_context))
}

#[cfg(test)]
// The tests read byte counts as approximate gibibytes for range asserts,
// where the last-bit precision the lint protects does not matter.
#[allow(clippy::cast_precision_loss)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    /// Five real Qwen3 dense model configurations, read from each model's
    /// published `config.json` and model card on Hugging Face on 2026-08-23:
    /// `Qwen/Qwen3-0.6B`, `Qwen/Qwen3-4B`, `Qwen/Qwen3-8B`, `Qwen/Qwen3-14B`,
    /// and `Qwen/Qwen3-32B`. Every one of the five uses `head_dim = 128` and
    /// `num_key_value_heads = 8`; `layers` and `params` (the card's "Total
    /// Parameters" figure) vary by model. This is the fixture the module
    /// doc promises in place of a measurement on real hardware.
    fn published_qwen3_models() -> [(&'static str, ModelConfig); 5] {
        [
            (
                "qwen3-0.6b",
                ModelConfig {
                    params: 600_000_000,
                    layers: 28,
                    kv_heads: 8,
                    head_dim: 128,
                },
            ),
            (
                "qwen3-4b",
                ModelConfig {
                    params: 4_000_000_000,
                    layers: 36,
                    kv_heads: 8,
                    head_dim: 128,
                },
            ),
            (
                "qwen3-8b",
                ModelConfig {
                    params: 8_200_000_000,
                    layers: 36,
                    kv_heads: 8,
                    head_dim: 128,
                },
            ),
            (
                "qwen3-14b",
                ModelConfig {
                    params: 14_800_000_000,
                    layers: 40,
                    kv_heads: 8,
                    head_dim: 128,
                },
            ),
            (
                "qwen3-32b",
                ModelConfig {
                    params: 32_800_000_000,
                    layers: 64,
                    kv_heads: 8,
                    head_dim: 128,
                },
            ),
        ]
    }

    /// Hand-computed `weights + kv_cache + 10%` at `q4k` (4 bits) and an
    /// 8192-token context, for each model in [`published_qwen3_models`], in
    /// the same order. Computed independently of `total_bytes` (by a short
    /// Python script applying section 4.1's formula verbatim) so this test
    /// cannot pass merely because both sides share a bug.
    const EXPECTED_TOTAL_BYTES_AT_Q4K_8K_CTX: [u64; 5] = [
        1_363_476_505,  // qwen3-0.6b
        3_528_755_507,  // qwen3-4b
        5_838_755_507,  // qwen3-8b
        9_616_395_008,  // qwen3-14b
        20_402_232_012, // qwen3-32b
    ];

    #[test]
    fn five_published_models_match_the_hand_computed_total() {
        let bits = bits_per_weight("q4k").unwrap();
        for ((name, cfg), expected) in published_qwen3_models()
            .into_iter()
            .zip(EXPECTED_TOTAL_BYTES_AT_Q4K_8K_CTX)
        {
            let got = total_bytes(cfg, 8192, bits);
            assert_eq!(got, expected, "{name}: total_bytes mismatch");
        }
    }

    #[test]
    fn qwen3_32b_weights_alone_are_about_16_gib_at_4_bit() {
        // Section 4.1: "A 30B model at 4 bits needs approximately 17 GB for
        // weights." Qwen3-32B is the nearest published model to that
        // example; 32.8B params at 4 bits is 15.27 GiB (16.4 GB in decimal
        // gigabytes), within the range the prose calls "approximately".
        let bytes = weights_bytes(32_800_000_000, 4.0);
        let gib = bytes as f64 / GIB as f64;
        assert!(
            (14.5..=18.0).contains(&gib),
            "expected roughly 14.5-18 GiB, got {gib:.2} GiB"
        );
    }

    #[test]
    fn a_32k_kv_cache_is_a_few_gib_on_published_models() {
        // Section 4.1: "A 32k KV cache adds 1 GB to 4 GB." That figure is
        // an example (Appendix C), not a bound this formula must hit for
        // every model: with head_dim = 128 and 8 key-value heads fixed
        // across Qwen3's whole family (verified against each model's
        // config.json), a deeper model's KV cache scales with its layer
        // count alone, so the 64-layer 32B model lands well past the
        // 4 GB the example gives for whichever model it had in mind. This
        // checks the formula stays in the right order of magnitude — low
        // single-digit gibibytes, not tens of gigabytes or megabytes —
        // rather than pinning the prose's exact range.
        for (name, cfg) in published_qwen3_models() {
            let bytes = kv_cache_bytes(cfg, 32_768);
            let gib = bytes as f64 / GIB as f64;
            assert!(
                (0.9..=10.0).contains(&gib),
                "{name}: expected a few GiB at a 32k context, got {gib:.2} GiB"
            );
        }
    }

    // Whole-number bit widths are exactly representable, so an exact
    // comparison is the correct one here, not an approximation.
    #[allow(clippy::float_cmp)]
    #[test]
    fn bits_per_weight_covers_the_named_quantisations() {
        assert_eq!(bits_per_weight("q4k").unwrap(), 4.0);
        assert_eq!(bits_per_weight("Q4K").unwrap(), 4.0, "case-insensitive");
        assert_eq!(bits_per_weight("q8_0").unwrap(), 8.0);
        assert_eq!(bits_per_weight("f16").unwrap(), 16.0);
    }

    #[test]
    fn bits_per_weight_rejects_an_unknown_quantisation() {
        let err = bits_per_weight("q999").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineUnsupported);
    }

    #[test]
    fn with_headroom_adds_ten_percent() {
        assert_eq!(with_headroom(1000), 1100);
        assert_eq!(with_headroom(0), 0);
    }

    #[test]
    fn granted_context_never_lets_the_true_total_exceed_the_budget() {
        let cfg = ModelConfig {
            params: 4_000_000_000,
            layers: 36,
            kv_heads: 8,
            head_dim: 128,
        };
        let weights = weights_bytes(cfg.params, 4.0);
        // The weights alone need ~2 GB; below that a fit is impossible at
        // any context (see `granted_context_is_none_when_weights_alone_do_not_fit`),
        // so this property only holds — and is only checked — once the
        // budget clears that floor.
        for budget_gib in [4u64, 8, 16, 32, 64] {
            let budget = budget_gib * GIB;
            let ctx = granted_context(cfg, weights, budget, 131_072)
                .expect("budget comfortably covers the weights");
            let actual_total = total_bytes(cfg, ctx, 4.0);
            assert!(
                actual_total <= budget,
                "budget {budget_gib} GiB: granted context {ctx} costs {actual_total} bytes, \
                 over budget {budget} bytes"
            );
        }
    }

    #[test]
    fn granted_context_is_none_when_weights_alone_do_not_fit() {
        let cfg = ModelConfig {
            params: 32_800_000_000,
            layers: 64,
            kv_heads: 8,
            head_dim: 128,
        };
        let weights = weights_bytes(cfg.params, 4.0);
        // One gibibyte is far short of the ~16 GiB the weights alone need:
        // no context, not even zero, is a safe answer here.
        let ctx = granted_context(cfg, weights, GIB, 131_072);
        assert_eq!(ctx, None);
    }

    #[test]
    fn granted_context_is_some_zero_when_weights_fit_with_nothing_left_over() {
        let cfg = ModelConfig {
            params: 4_000_000_000,
            layers: 36,
            kv_heads: 8,
            head_dim: 128,
        };
        let weights = weights_bytes(cfg.params, 4.0);
        // Just over the ~2.2 GiB the weights need with headroom, and far
        // too little left over for even one token of key-value cache.
        let budget = weights + weights / 10 + 1;
        assert_eq!(granted_context(cfg, weights, budget, 131_072), Some(0));
    }

    #[test]
    fn granted_context_is_capped_at_max_context() {
        let cfg = ModelConfig {
            params: 600_000_000,
            layers: 28,
            kv_heads: 8,
            head_dim: 128,
        };
        let weights = weights_bytes(cfg.params, 4.0);
        // A generous budget on a small model should saturate at the model's
        // own maximum, not grow without bound.
        let ctx = granted_context(cfg, weights, 512 * GIB, 32_768);
        assert_eq!(ctx, Some(32_768));
    }

    #[test]
    fn weights_bytes_rounds_up_a_fractional_byte() {
        // 3 params at 4 bits is 1.5 bytes; the estimator must not under-count.
        assert_eq!(weights_bytes(3, 4.0), 2);
    }
}
