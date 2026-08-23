//! Detecting an embedding model change.
//!
//! `dense.vec` holds vectors from one embedding model. When the harness
//! switches models, those vectors no longer mean anything: a new model's
//! vector for "spawn a task" does not sit near the old model's vector for
//! the same phrase. Task unit `G1` asks the pack format to detect this by
//! comparing the manifest's `[embed]` block against the harness's current
//! configuration, and to keep serving lexical results until the pack is
//! indexed again.

use serde::{Deserialize, Serialize};

use crate::pack::manifest::EmbedBlock;

/// The embedding configuration that the harness runs now.
///
/// A caller builds this from live configuration, not from a pack, and
/// passes it to [`compare`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedConfig {
    /// The embedding model identifier that is loaded now.
    pub model: String,
    /// The vector width that the loaded model produces.
    pub dim: u32,
    /// The quantisation that the harness stores vectors at.
    pub quant: String,
}

/// The result of comparing a pack's `[embed]` block against the current
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedStatus {
    /// The pack's dense vectors match the current model. The dense index is
    /// safe to query.
    Match,
    /// The pack's dense vectors came from a different model. Only lexical
    /// (BM25) retrieval is safe until `dark pack reindex` runs.
    ///
    /// `field` names the first `[embed]` field that differs, for a message
    /// that a person can act on.
    Mismatch {
        /// The field that differs, for example `model` or `dim`.
        field: &'static str,
        /// The value that the pack was built with.
        pack_value: String,
        /// The value that the harness runs now.
        current_value: String,
    },
}

impl EmbedStatus {
    /// Returns `true` when the dense index is safe to query.
    #[must_use]
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Match)
    }

    /// Renders a message for a person to read.
    ///
    /// Rule 27 asks the harness to show attribution on a retrieved chunk;
    /// this message is the analogous notice for a stale dense index. It
    /// names the field that changed and states the fallback in one
    /// sentence, so the reader gets the remedy without a lookup.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Match => "the dense index matches the current embedding model".to_owned(),
            Self::Mismatch {
                field,
                pack_value,
                current_value,
            } => format!(
                "the pack's {field} is '{pack_value}'; the harness runs '{current_value}'. \
                 Serve lexical results only. Run dark pack reindex."
            ),
        }
    }
}

/// Compares a pack's `[embed]` block against the harness's current
/// configuration.
///
/// The comparison checks `model`, then `dim`, then `quant`, in that order,
/// and reports the first field that differs. A caller does not need to
/// check more than one field: a model change on its own already means the
/// dense index is unusable.
#[must_use]
pub fn compare(pack: &EmbedBlock, current: &EmbedConfig) -> EmbedStatus {
    if pack.model != current.model {
        return EmbedStatus::Mismatch {
            field: "model",
            pack_value: pack.model.clone(),
            current_value: current.model.clone(),
        };
    }
    if pack.dim != current.dim {
        return EmbedStatus::Mismatch {
            field: "dim",
            pack_value: pack.dim.to_string(),
            current_value: current.dim.to_string(),
        };
    }
    if pack.quant != current.quant {
        return EmbedStatus::Mismatch {
            field: "quant",
            pack_value: pack.quant.clone(),
            current_value: current.quant.clone(),
        };
    }
    EmbedStatus::Match
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed_block() -> EmbedBlock {
        EmbedBlock {
            model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
            dim: 1024,
            quant: "int8".to_owned(),
            query_prefix: String::new(),
            doc_prefix: String::new(),
        }
    }

    fn matching_config() -> EmbedConfig {
        EmbedConfig {
            model: "Qwen/Qwen3-Embedding-0.6B".to_owned(),
            dim: 1024,
            quant: "int8".to_owned(),
        }
    }

    #[test]
    fn identical_blocks_match() {
        let status = compare(&embed_block(), &matching_config());
        assert_eq!(status, EmbedStatus::Match);
        assert!(status.is_match());
    }

    #[test]
    fn a_different_model_is_a_mismatch_on_the_model_field() {
        let mut current = matching_config();
        current.model = "Qwen/Qwen3-Embedding-4B".to_owned();
        let status = compare(&embed_block(), &current);
        assert!(!status.is_match());
        match status {
            EmbedStatus::Mismatch { field, .. } => assert_eq!(field, "model"),
            EmbedStatus::Match => panic!("expected a mismatch"),
        }
    }

    #[test]
    fn a_different_dimension_is_a_mismatch_on_the_dim_field() {
        let mut current = matching_config();
        current.dim = 768;
        let status = compare(&embed_block(), &current);
        match status {
            EmbedStatus::Mismatch { field, .. } => assert_eq!(field, "dim"),
            EmbedStatus::Match => panic!("expected a mismatch"),
        }
    }

    #[test]
    fn a_different_quantisation_is_a_mismatch_on_the_quant_field() {
        let mut current = matching_config();
        current.quant = "f16".to_owned();
        let status = compare(&embed_block(), &current);
        match status {
            EmbedStatus::Mismatch { field, .. } => assert_eq!(field, "quant"),
            EmbedStatus::Match => panic!("expected a mismatch"),
        }
    }

    #[test]
    fn the_model_field_is_reported_first_even_when_several_fields_differ() {
        let current = EmbedConfig {
            model: "different-model".to_owned(),
            dim: 42,
            quant: "different-quant".to_owned(),
        };
        let status = compare(&embed_block(), &current);
        match status {
            EmbedStatus::Mismatch { field, .. } => assert_eq!(field, "model"),
            EmbedStatus::Match => panic!("expected a mismatch"),
        }
    }

    #[test]
    fn describe_names_the_field_and_both_values() {
        let mut current = matching_config();
        current.dim = 768;
        let message = compare(&embed_block(), &current).describe();
        assert!(message.contains("dim"));
        assert!(message.contains("1024"));
        assert!(message.contains("768"));
    }
}
