//! Identifiers for a resident model and for the turn that leases one.

use serde::{Deserialize, Serialize};

/// Identifies one model in the resident set.
///
/// Two loads of the same repository at the same quantisation share a key, so
/// the resident set manager treats them as the same slot. A different
/// quantisation of the same repository is a different key: it uses a
/// different amount of memory, and the estimator must not confuse the two.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ModelKey {
    /// The Hugging Face repository, for example `Qwen/Qwen3-4B`.
    pub repo: String,
    /// The quantisation name, for example `q4k`. Empty for full precision.
    pub quant: String,
}

impl ModelKey {
    /// Creates a key from a repository and a quantisation name.
    pub fn new(repo: impl Into<String>, quant: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            quant: quant.into(),
        }
    }
}

impl std::fmt::Display for ModelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.quant.is_empty() {
            f.write_str(&self.repo)
        } else {
            write!(f, "{}@{}", self.repo, self.quant)
        }
    }
}

/// Identifies one turn. A lease under this identifier keeps a model resident
/// until the turn calls [`super::ResidentSet::release_turn`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TurnId(pub String);

impl TurnId {
    /// Creates a turn identifier from any string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_the_quantisation_when_present() {
        let key = ModelKey::new("Qwen/Qwen3-4B", "q4k");
        assert_eq!(key.to_string(), "Qwen/Qwen3-4B@q4k");
    }

    #[test]
    fn display_omits_an_empty_quantisation() {
        let key = ModelKey::new("Qwen/Qwen3-4B", "");
        assert_eq!(key.to_string(), "Qwen/Qwen3-4B");
    }

    #[test]
    fn keys_with_different_quantisations_are_not_equal() {
        assert_ne!(
            ModelKey::new("Qwen/Qwen3-4B", "q4k"),
            ModelKey::new("Qwen/Qwen3-4B", "q8k")
        );
    }
}
