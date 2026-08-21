//! Core library for the `darkharness` application.
//!
//! All real logic lives here rather than in `main.rs`, so it can be unit tested
//! without spawning a process. The binary in `src/main.rs` is a thin shell that
//! parses arguments and calls into this crate.
//!
//! ```
//! use darkharness::{Config, Harness};
//!
//! let config = Config::new("demo", 2).unwrap();
//! let report = Harness::new(config).run().unwrap();
//! assert_eq!(report.tasks_run, 2);
//! ```

pub mod config;
pub mod error;
pub mod harness;

pub use config::Config;
pub use error::{Error, Result};
pub use harness::{Harness, Report};
