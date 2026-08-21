//! Validated configuration for a [`Harness`](crate::Harness) run.

use crate::{Error, Result};

/// Upper bound on [`Config::workers`], guarding against absurd inputs.
pub const MAX_WORKERS: usize = 1024;

/// Settings that control a single run.
///
/// Construct with [`Config::new`], which validates its inputs; the fields are
/// read-only afterwards so a `Config` value is always valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    name: String,
    workers: usize,
}

impl Config {
    /// Creates a validated configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidConfig`] if `name` is empty or entirely
    /// whitespace, or if `workers` is zero or exceeds [`MAX_WORKERS`].
    pub fn new(name: impl Into<String>, workers: usize) -> Result<Self> {
        let name = name.into();

        if name.trim().is_empty() {
            return Err(Error::InvalidConfig("name must not be empty".to_owned()));
        }

        if workers == 0 {
            return Err(Error::InvalidConfig(
                "workers must be at least 1".to_owned(),
            ));
        }

        if workers > MAX_WORKERS {
            return Err(Error::InvalidConfig(format!(
                "workers must not exceed {MAX_WORKERS}, got {workers}"
            )));
        }

        Ok(Self { name, workers })
    }

    /// The name identifying this run.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How many tasks the run executes.
    pub fn workers(&self) -> usize {
        self.workers
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            workers: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_input() {
        let config = Config::new("run-1", 4).expect("valid config");
        assert_eq!(config.name(), "run-1");
        assert_eq!(config.workers(), 4);
    }

    #[test]
    fn rejects_blank_name() {
        let err = Config::new("   ", 1).expect_err("blank name must be rejected");
        assert!(matches!(err, Error::InvalidConfig(_)));
    }

    #[test]
    fn rejects_zero_workers() {
        let err = Config::new("run-1", 0).expect_err("zero workers must be rejected");
        assert!(matches!(err, Error::InvalidConfig(_)));
    }

    #[test]
    fn rejects_too_many_workers() {
        let err =
            Config::new("run-1", MAX_WORKERS + 1).expect_err("oversized worker count is rejected");
        assert!(matches!(err, Error::InvalidConfig(_)));
    }

    #[test]
    fn default_is_valid() {
        let config = Config::default();
        assert_eq!(config.workers(), 1);
        assert!(!config.name().is_empty());
    }
}
