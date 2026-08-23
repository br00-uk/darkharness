//! Selects a model format from what is on disk (task unit `B2`, step 2).
//!
//! | Format | Load speed | Use for |
//! | --- | --- | --- |
//! | UQFF | Fastest | Every configured profile. |
//! | GGUF | Fast | Files the user already holds. |
//! | HF with in-situ quantisation | Slowest | First use only. |
//!
//! [`select`] prefers them in that order. It reads only file names, so a
//! test drives it against an empty temporary directory holding fake,
//! empty files — no real weights are needed to test the selection logic.

use std::path::Path;

use serde::{Deserialize, Serialize};

use dark_contract::{ErrCode, Error, Result};

/// A model format this harness can load, ordered fastest-loading first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    /// mistral.rs's own pre-quantised format. The resident set manager
    /// swaps models, and a slow load makes a swap unusable — UQFF is the
    /// default for every configured profile because of this.
    Uqff,
    /// A GGUF file the user already holds.
    Gguf,
    /// Full-precision Hugging Face weights, quantised in place at load
    /// time. Slowest: first use only.
    HfInSitu,
}

impl ModelFormat {
    /// Returns every format, fastest-loading first — the preference order
    /// [`select`] applies.
    #[must_use]
    pub fn preference_order() -> [Self; 3] {
        [Self::Uqff, Self::Gguf, Self::HfInSitu]
    }
}

impl std::fmt::Display for ModelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Uqff => "uqff",
            Self::Gguf => "gguf",
            Self::HfInSitu => "hf_in_situ",
        };
        f.write_str(name)
    }
}

/// Returns whether `name` names a UQFF shard, for example `q4k-0.uqff`.
fn is_uqff_file(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".uqff")
}

/// Returns whether `name` names a GGUF file.
fn is_gguf_file(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".gguf")
}

/// Returns whether `name` names a safetensors weight shard.
///
/// A UQFF export's own `residual.safetensors` sidecar also matches this,
/// which is why [`select`] checks for a `.uqff` file first: UQFF takes
/// priority over the in-situ format regardless.
fn is_safetensors_file(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".safetensors")
}

/// Selects the fastest format present among `file_names`.
///
/// Returns `None` when none of the three formats are present. HF in-situ
/// needs `config.json`, `tokenizer.json`, and at least one `.safetensors`
/// file; UQFF and GGUF need only their own shard extension, since mistral.rs
/// discovers their sidecar files (`residual.safetensors`, `config.json`,
/// `tokenizer.json`) itself once it has the first shard's name.
#[must_use]
pub fn select(file_names: &[String]) -> Option<ModelFormat> {
    if file_names.iter().any(|name| is_uqff_file(name)) {
        return Some(ModelFormat::Uqff);
    }
    if file_names.iter().any(|name| is_gguf_file(name)) {
        return Some(ModelFormat::Gguf);
    }
    let has_config = file_names.iter().any(|name| name == "config.json");
    let has_tokenizer = file_names.iter().any(|name| name == "tokenizer.json");
    let has_weights = file_names.iter().any(|name| is_safetensors_file(name));
    if has_config && has_tokenizer && has_weights {
        return Some(ModelFormat::HfInSitu);
    }
    None
}

/// Lists the file names directly inside `dir`, sorted, and selects a format
/// from them (see [`select`]).
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `dir` cannot be read.
pub fn select_from_dir(dir: &Path) -> Result<Option<ModelFormat>> {
    let mut names = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not read {}: {source}", dir.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not read an entry of {}: {source}", dir.display()),
            )
        })?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(select(&names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), []).unwrap();
    }

    #[test]
    fn uqff_wins_over_every_other_format() {
        let names = vec![
            "config.json".to_owned(),
            "tokenizer.json".to_owned(),
            "model.safetensors".to_owned(),
            "model.gguf".to_owned(),
            "q4k-0.uqff".to_owned(),
        ];
        assert_eq!(select(&names), Some(ModelFormat::Uqff));
    }

    #[test]
    fn gguf_wins_over_hf_in_situ() {
        let names = vec![
            "config.json".to_owned(),
            "tokenizer.json".to_owned(),
            "model.safetensors".to_owned(),
            "model.gguf".to_owned(),
        ];
        assert_eq!(select(&names), Some(ModelFormat::Gguf));
    }

    #[test]
    fn hf_in_situ_needs_config_tokenizer_and_weights_together() {
        let complete = vec![
            "config.json".to_owned(),
            "tokenizer.json".to_owned(),
            "model-00001-of-00002.safetensors".to_owned(),
        ];
        assert_eq!(select(&complete), Some(ModelFormat::HfInSitu));

        let missing_tokenizer = vec!["config.json".to_owned(), "model.safetensors".to_owned()];
        assert_eq!(select(&missing_tokenizer), None);
    }

    #[test]
    fn an_empty_directory_selects_nothing() {
        assert_eq!(select(&[]), None);
    }

    #[test]
    fn selection_is_case_insensitive_on_the_extension() {
        let names = vec!["Model.GGUF".to_owned()];
        assert_eq!(select(&names), Some(ModelFormat::Gguf));
    }

    #[test]
    fn select_from_dir_reads_fake_empty_files() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "q4k-0.uqff");
        touch(dir.path(), "residual.safetensors");
        touch(dir.path(), "config.json");
        touch(dir.path(), "tokenizer.json");
        assert_eq!(
            select_from_dir(dir.path()).unwrap(),
            Some(ModelFormat::Uqff)
        );
    }

    #[test]
    fn select_from_dir_fails_for_a_missing_directory() {
        let err = select_from_dir(Path::new("/does/not/exist")).unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn preference_order_is_uqff_then_gguf_then_hf_in_situ() {
        assert_eq!(
            ModelFormat::preference_order(),
            [ModelFormat::Uqff, ModelFormat::Gguf, ModelFormat::HfInSitu]
        );
    }

    #[test]
    fn display_matches_the_serde_name() {
        assert_eq!(ModelFormat::HfInSitu.to_string(), "hf_in_situ");
    }
}
