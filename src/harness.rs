//! The run entry point.
//!
//! [`Harness::run`] is the seam where real work belongs. Today it performs a
//! trivial, fully deterministic pass so that the build, the test suite, and CI
//! all have something meaningful to exercise from the first commit.

use crate::{Config, Result};

/// Outcome of a completed [`Harness::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Number of tasks that executed.
    pub tasks_run: usize,
    /// The name of the configuration that produced this report.
    pub name: String,
}

/// Executes a run described by a [`Config`].
#[derive(Debug, Clone)]
pub struct Harness {
    config: Config,
}

impl Harness {
    /// Creates a harness for `config`.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// The configuration this harness was built with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Executes the run and returns a [`Report`].
    ///
    /// # Errors
    ///
    /// Currently infallible, but returns [`Result`] so that adding fallible
    /// work here does not become a breaking change for callers.
    pub fn run(&self) -> Result<Report> {
        let name = self.config.name();
        let workers = self.config.workers();

        tracing::info!(name, workers, "starting run");

        for task in 1..=workers {
            tracing::debug!(task, "executing task");
        }

        tracing::info!(name, tasks_run = workers, "run complete");

        Ok(Report {
            tasks_run: workers,
            name: name.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_one_task_per_worker() {
        let config = Config::new("run-1", 3).expect("valid config");
        let report = Harness::new(config).run().expect("run succeeds");

        assert_eq!(report.tasks_run, 3);
        assert_eq!(report.name, "run-1");
    }

    #[test]
    fn exposes_its_config() {
        let config = Config::new("run-2", 7).expect("valid config");
        let harness = Harness::new(config);

        assert_eq!(harness.config().workers(), 7);
    }
}
