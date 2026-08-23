//! Materialises weights already on disk into a running `mistralrs::Model`.
//!
//! This is the one seam in `dark-engine` where the crate hands a model's
//! files to mistral.rs and asks it to load them. It cannot run in this
//! sandbox: there is no accelerator here, and no model file — [`materialize`]
//! is compile-true against the real mistral.rs 0.8.1 API (see `docs/adr/0006`
//! for the API surface this was built against), but no test in this crate
//! exercises it. `crates/dark-engine/src/resident/mod.rs`'s module
//! documentation lists this alongside the other pieces this task unit
//! defers to a machine with a model and a device.
//!
//! Every builder here points at a local path — [`LoadSpec::model_id`] is
//! the directory `dark-airlock` already downloaded into, or a GGUF/UQFF
//! shard's path within it — never a bare Hugging Face repository id. This
//! is what keeps a load offline once `dark setup` has run once: mistral.rs
//! resolves `model_id` against the local filesystem first, and
//! `from_hf_cache_path` (the HF in-situ path) points it at the same
//! directory `dark-airlock`'s download populated, so neither path calls out
//! to Hugging Face itself. Dark mode blocks the download that populates
//! that directory in the first place (Rule 13); it does not need to block
//! anything here, because nothing here reaches the network.

use std::path::PathBuf;

use dark_contract::{ErrCode, Error, Result};

use super::format::ModelFormat;

/// What to load: the format, the local paths mistral.rs needs, and the
/// quantisation to apply.
#[derive(Debug, Clone)]
pub struct LoadSpec {
    /// Which format to load through.
    pub format: ModelFormat,
    /// The model's local directory (every format) — never a bare
    /// repository id, per the module documentation.
    pub model_id: String,
    /// The first UQFF shard's file name, required when `format` is
    /// [`ModelFormat::Uqff`]. mistral.rs discovers the remaining shards
    /// and sidecar files from the same directory.
    pub uqff_shard: Option<PathBuf>,
    /// The GGUF file names to load, required when `format` is
    /// [`ModelFormat::Gguf`].
    pub gguf_files: Vec<String>,
    /// The in-situ quantisation to apply, when `format` is
    /// [`ModelFormat::HfInSitu`]. `None` loads at full precision.
    pub isq: Option<mistralrs::IsqType>,
    /// The local directory `dark-airlock` downloaded HF in-situ weights
    /// into, required when `format` is [`ModelFormat::HfInSitu`].
    pub hf_cache_path: Option<PathBuf>,
}

/// Builds and loads the model `spec` describes.
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `spec` is missing a field its
/// format needs, or when mistral.rs fails to build the model.
pub async fn materialize(spec: &LoadSpec) -> Result<mistralrs::Model> {
    let result = match spec.format {
        ModelFormat::Uqff => {
            let shard = spec.uqff_shard.clone().ok_or_else(|| {
                missing_field(
                    &spec.model_id,
                    "a UQFF load needs its first shard's file name",
                )
            })?;
            mistralrs::UqffTextModelBuilder::new(&spec.model_id, vec![shard])
                .build()
                .await
        }
        ModelFormat::Gguf => {
            if spec.gguf_files.is_empty() {
                return Err(missing_field(
                    &spec.model_id,
                    "a GGUF load needs at least one file name",
                ));
            }
            mistralrs::GgufModelBuilder::new(&spec.model_id, spec.gguf_files.clone())
                .build()
                .await
        }
        ModelFormat::HfInSitu => {
            let cache_path = spec.hf_cache_path.clone().ok_or_else(|| {
                missing_field(
                    &spec.model_id,
                    "an HF in-situ load needs the local directory dark-airlock downloaded into",
                )
            })?;
            let mut builder =
                mistralrs::TextModelBuilder::new(&spec.model_id).from_hf_cache_path(cache_path);
            if let Some(isq) = spec.isq {
                builder = builder.with_isq(isq);
            }
            builder.build().await
        }
    };
    result.map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("mistral.rs could not load {}: {source}", spec.model_id),
        )
    })
}

/// Builds an `E_ENGINE_LOAD` error for a [`LoadSpec`] missing a field its
/// format requires.
fn missing_field(model_id: &str, message: &str) -> Error {
    Error::new(ErrCode::EngineLoad, format!("{model_id}: {message}"))
}
