//! The error type for configuration resolution.
//!
//! `dark-contract` defines the workspace error taxonomy (see
//! [`dark_contract::error`]), with prefixes from `E_ENGINE_` through
//! `E_SESSION_`. That frozen list has no `E_CONFIG_` domain, and task unit
//! `Z1` owns the list, not this crate. Reusing an unrelated code (for
//! example `E_TOOL_INVALID_ARGS` for a bad TOML file) would mislead a
//! caller that matches on [`dark_contract::ErrDomain`]. This crate defines
//! its own small error type instead.

use std::path::PathBuf;

/// An error from loading or resolving configuration.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A configuration file exists but the harness could not read it.
    Io {
        /// The file that the harness tried to read.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A configuration file held text that is not valid TOML.
    Parse {
        /// The file that failed to parse. Built-in defaults report this as
        /// `<built-in defaults>`, since they come from code, not a file.
        path: PathBuf,
        /// The underlying error.
        source: toml::de::Error,
    },
    /// A configuration file set a key that looks like a secret.
    ///
    /// The harness never stores a token in a configuration file. See the
    /// crate-level docs on [`crate::token`] for where a secret belongs
    /// instead.
    SecretInFile {
        /// The dotted key that looked like a secret, for example
        /// `huggingface.token`.
        key: String,
        /// The file that set it.
        path: PathBuf,
    },
    /// A resolved section did not match the shape that the caller requested.
    Section {
        /// The dotted prefix that the caller asked to deserialize.
        prefix: String,
        /// The underlying error.
        source: toml::de::Error,
    },
    /// A [`crate::TokenStore`] does not support the requested operation.
    TokenStoreUnsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "{} is not valid TOML: {source}", path.display())
            }
            Self::SecretInFile { key, path } => write!(
                f,
                "{} sets '{key}', and a configuration file must not hold a secret; \
                 use a TokenStore instead",
                path.display()
            ),
            Self::Section { prefix, source } => {
                write!(
                    f,
                    "section '{prefix}' does not match the requested type: {source}"
                )
            }
            Self::TokenStoreUnsupported(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } | Self::Section { source, .. } => Some(source),
            Self::SecretInFile { .. } | Self::TokenStoreUnsupported(_) => None,
        }
    }
}

/// The result type for this crate.
pub type Result<T> = std::result::Result<T, Error>;
