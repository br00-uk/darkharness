//! Reads a model's shape from the files beside its weights.
//!
//! [`crate::resident::estimate`] needs a [`ModelConfig`] — parameter
//! count, layers, key-value heads, head dimension — before it can decide
//! whether a model fits (Rule 1: estimate before loading, never discover a
//! limit by allocation failure). This module builds that value from the
//! model directory alone, so the estimate happens with no network and
//! before mistral.rs allocates anything.
//!
//! # Where each field comes from
//!
//! Three of the four fields are stated in the Hugging Face `config.json`
//! that every one of the three supported formats ships beside its weights
//! (see [`super::format`], which requires the file to recognise a
//! directory at all):
//!
//! - `layers` is `num_hidden_layers`.
//! - `kv_heads` is `num_key_value_heads`, which a model with no grouped
//!   query attention omits; there every attention head carries its own
//!   key-value pair, so `num_attention_heads` is the right count.
//! - `head_dim` is the field of that name when the model states it, as
//!   Qwen3 does, and `hidden_size / num_attention_heads` otherwise.
//!
//! The fourth, `params`, is **not** in `config.json`. Rather than carry a
//! table of parameter counts per repository — which would be wrong the
//! first time a model this harness has never seen is pulled — this module
//! measures it: the weight files on disk hold one quantised weight per
//! parameter, so dividing their total size by the bits per weight
//! recovers the count. [`params_from_weight_bytes`] is that arithmetic,
//! and it reads a real directory rather than a published figure.
//!
//! That estimate is deliberately slightly high: a weight file also holds
//! its own header and, for some formats, an unquantised embedding table.
//! Overestimating the parameter count overestimates the memory a load
//! needs, which fails a load early rather than discovering the limit by
//! allocation failure. Rule 1 wants the error in exactly that direction.

use std::path::Path;

use dark_contract::{ErrCode, Error, Result};
use serde::Deserialize;

use crate::resident::estimate::{ModelConfig, bits_per_weight};

/// The `config.json` fields this module reads.
///
/// Every field is optional here even when a real model always states it,
/// so that a malformed or unfamiliar file produces a remedied
/// [`ErrCode::EngineLoad`] naming the missing field, rather than a serde
/// error naming a line and column that means nothing to the person who
/// ran the command.
#[derive(Debug, Clone, Default, Deserialize)]
struct RawConfig {
    /// The transformer layer count.
    num_hidden_layers: Option<u64>,
    /// The attention head count.
    num_attention_heads: Option<u64>,
    /// The key-value head count. Absent on a model with no grouped query
    /// attention.
    num_key_value_heads: Option<u64>,
    /// The model dimension.
    hidden_size: Option<u64>,
    /// The per-head dimension, when the model states it directly.
    head_dim: Option<u64>,
}

/// The file names whose bytes count towards the parameter measurement.
///
/// A model directory also holds a tokenizer, a manifest, and sidecar JSON.
/// Counting those towards the weight total would inflate the parameter
/// count by a few megabytes' worth — harmless at this scale, but this
/// module would rather measure the thing it names.
const WEIGHT_EXTENSIONS: [&str; 3] = ["safetensors", "gguf", "uqff"];

/// Returns whether `path` names a weight file this module measures.
fn is_weight_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| WEIGHT_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Sums the size of every weight file directly inside `dir`.
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `dir` cannot be read, or when it
/// holds no weight file at all.
pub fn weight_bytes_in(dir: &Path) -> Result<u64> {
    let entries = std::fs::read_dir(dir).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not read {}: {source}", dir.display()),
        )
        .with_remedy("Run dark setup, or dark models pull, to install a model.")
    })?;

    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if is_weight_file(&path) {
            total = total.saturating_add(entry.metadata().map(|meta| meta.len()).unwrap_or(0));
        }
    }

    if total == 0 {
        return Err(Error::new(
            ErrCode::EngineLoad,
            format!("{} holds no weight file", dir.display()),
        )
        .with_remedy("Run dark models pull to download the weights again."));
    }
    Ok(total)
}

/// Recovers a parameter count from the size of the weight files.
///
/// One parameter occupies `bits` bits on disk, so `bytes * 8 / bits` is
/// the count. See this module's documentation for why the result is
/// deliberately a slight overestimate.
#[must_use]
pub fn params_from_weight_bytes(bytes: u64, bits: f64) -> u64 {
    if bits <= 0.0 {
        return 0;
    }
    // The cast is the point of the function: a parameter count is a whole
    // number, and this arithmetic is an estimate to within a few million
    // either way, so the fractional part carries no information.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a parameter count is a whole number and this is an estimate; see the module \
                  documentation"
    )]
    let params = ((bytes as f64) * 8.0 / bits) as u64;
    params
}

/// Reads `config.json` from `dir` and returns the shape it states.
///
/// `quant` names the quantisation the weights are stored at, which sets
/// the bits per weight that [`params_from_weight_bytes`] divides by. Pass
/// the value from the model's [`super::Manifest`].
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `config.json` is absent,
/// unreadable, not valid JSON, or missing a field this function needs,
/// and when `dir` holds no weight file. Returns
/// [`ErrCode::EngineUnsupported`] when `quant` names no quantisation this
/// harness recognises.
pub fn read(dir: &Path, quant: &str) -> Result<ModelConfig> {
    let path = dir.join("config.json");
    let text = std::fs::read_to_string(&path).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not read {}: {source}", path.display()),
        )
        .with_remedy("Run dark models pull to download the model files again.")
    })?;

    let raw: RawConfig = serde_json::from_str(&text).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("{} is not valid JSON: {source}", path.display()),
        )
        .with_remedy("Run dark models pull to download the model files again.")
    })?;

    let missing = |field: &str| {
        Error::new(
            ErrCode::EngineLoad,
            format!("{} does not state {field}", path.display()),
        )
        .with_remedy("Use a model whose config.json states its shape.")
    };

    let layers = raw
        .num_hidden_layers
        .ok_or_else(|| missing("num_hidden_layers"))?;
    let attention_heads = raw
        .num_attention_heads
        .ok_or_else(|| missing("num_attention_heads"))?;
    // A model with no grouped query attention omits num_key_value_heads:
    // every attention head carries its own key-value pair.
    let kv_heads = raw.num_key_value_heads.unwrap_or(attention_heads);

    let head_dim = if let Some(dim) = raw.head_dim {
        dim
    } else {
        let hidden = raw.hidden_size.ok_or_else(|| missing("hidden_size"))?;
        if attention_heads == 0 {
            return Err(missing("a non-zero num_attention_heads"));
        }
        hidden / attention_heads
    };

    let bits = bits_per_weight(quant)?;
    let params = params_from_weight_bytes(weight_bytes_in(dir)?, bits);

    Ok(ModelConfig {
        params,
        layers,
        kv_heads,
        head_dim,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Writes a `config.json` and one weight file of `weight_bytes` bytes.
    fn model_dir(config_json: &str, weight_bytes: usize) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("config.json"), config_json).unwrap();
        fs::write(
            dir.path().join("model.safetensors"),
            vec![0u8; weight_bytes],
        )
        .unwrap();
        dir
    }

    #[test]
    fn reads_a_qwen3_shaped_config() {
        let dir = model_dir(
            r#"{
                "num_hidden_layers": 36,
                "num_attention_heads": 32,
                "num_key_value_heads": 8,
                "hidden_size": 4096,
                "head_dim": 128
            }"#,
            1024,
        );

        let cfg = read(dir.path(), "q4k").unwrap();
        assert_eq!(cfg.layers, 36);
        assert_eq!(cfg.kv_heads, 8);
        assert_eq!(cfg.head_dim, 128);
        // 1024 bytes at 4 bits per weight is 2048 parameters.
        assert_eq!(cfg.params, 2048);
    }

    #[test]
    fn derives_the_head_dimension_when_the_config_omits_it() {
        let dir = model_dir(
            r#"{
                "num_hidden_layers": 28,
                "num_attention_heads": 16,
                "num_key_value_heads": 8,
                "hidden_size": 2048
            }"#,
            512,
        );

        let cfg = read(dir.path(), "q8_0").unwrap();
        assert_eq!(cfg.head_dim, 2048 / 16);
    }

    #[test]
    fn a_model_with_no_grouped_query_attention_uses_its_attention_head_count() {
        let dir = model_dir(
            r#"{
                "num_hidden_layers": 12,
                "num_attention_heads": 12,
                "hidden_size": 768
            }"#,
            256,
        );

        let cfg = read(dir.path(), "f16").unwrap();
        assert_eq!(
            cfg.kv_heads, 12,
            "no num_key_value_heads means one per head"
        );
    }

    #[test]
    fn a_missing_field_names_itself_and_carries_a_remedy() {
        let dir = model_dir(r#"{"num_attention_heads": 12, "hidden_size": 768}"#, 256);

        let err = read(dir.path(), "q4k").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(
            err.message.contains("num_hidden_layers"),
            "the message names the missing field: {}",
            err.message
        );
        assert!(err.remedy.is_some(), "every error carries a remedy");
    }

    #[test]
    fn an_absent_config_reports_engine_load_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let err = read(dir.path(), "q4k").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(err.remedy.is_some());
    }

    #[test]
    fn a_directory_with_no_weight_file_says_so() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("config.json"),
            r#"{"num_hidden_layers": 1, "num_attention_heads": 1, "hidden_size": 8}"#,
        )
        .unwrap();

        let err = read(dir.path(), "q4k").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
        assert!(
            err.message.contains("no weight file"),
            "message: {}",
            err.message
        );
    }

    #[test]
    fn an_unknown_quantisation_reports_engine_unsupported() {
        let dir = model_dir(
            r#"{"num_hidden_layers": 1, "num_attention_heads": 1, "hidden_size": 8}"#,
            64,
        );
        let err = read(dir.path(), "q9z").unwrap_err();
        assert_eq!(err.code, ErrCode::EngineUnsupported);
    }

    #[test]
    fn weight_bytes_sums_every_shard_and_ignores_sidecar_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("model-00001.safetensors"), vec![0u8; 100]).unwrap();
        fs::write(dir.path().join("model-00002.safetensors"), vec![0u8; 200]).unwrap();
        fs::write(dir.path().join("tokenizer.json"), vec![0u8; 5000]).unwrap();
        fs::write(dir.path().join("manifest.toml"), vec![0u8; 5000]).unwrap();

        assert_eq!(weight_bytes_in(dir.path()).unwrap(), 300);
    }

    #[test]
    fn params_from_weight_bytes_divides_by_the_bits_per_weight() {
        assert_eq!(params_from_weight_bytes(1000, 4.0), 2000);
        assert_eq!(params_from_weight_bytes(1000, 8.0), 1000);
        assert_eq!(params_from_weight_bytes(1000, 16.0), 500);
        assert_eq!(
            params_from_weight_bytes(1000, 0.0),
            0,
            "no division by zero"
        );
    }
}
