//! Error types for the library.
//!
//! The library returns typed errors so callers can match on failure modes. The
//! binary layer converts these into `anyhow::Error` for human-readable
//! reporting, which is why `anyhow` is not used here.

/// Errors produced by the `darkharness` library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A [`Config`](crate::Config) field failed validation.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// An underlying I/O operation failed.
    #[error("i/o operation failed")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for results returned by this crate.
pub type Result<T> = std::result::Result<T, Error>;
