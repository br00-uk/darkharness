//! Layered configuration resolution.
//!
//! darkharness reads configuration from five sources; a later source
//! overrides an earlier one (Section 6, task unit `J2`):
//!
//! 1. Built-in defaults.
//! 2. `$DARK_HOME/config.toml`.
//! 3. `<repo>/.dark/config.toml`.
//! 4. Environment variables with the `DARK_` prefix.
//! 5. Command-line flags.
//!
//! The headline feature is provenance, not just the merged value: `dark
//! config explain <key>` must show which of the five sources set the
//! value the harness is using. A plain merge of nested structs loses that
//! information the moment two sources touch the same field, and it cannot
//! be added back afterwards. [`resolve`] avoids that by working over one
//! flat map of dotted key to `(value, source)` pairs — see [`Config`] and
//! [`Source`] — instead of merging typed structs.
//!
//! This crate defines the resolution machinery only. It carries no
//! built-in knowledge of any configuration section: `[policy]` (task unit
//! `A4`), `[hardware]` (`B6`), `[agents_md]` (`K1`), `[plan.axes]` (`E2`),
//! and `[qwen.profile]` (`I1`) all arrive later, from other crates, without
//! changing a line here. A section owner contributes its own default TOML
//! text (see [`Sources::defaults`]) and reads its typed value back with
//! [`Config::section`].
//!
//! # Example
//!
//! ```
//! use dark_config::{resolve, EnvMap, Source, Sources};
//!
//! let dark_home = tempfile::tempdir().unwrap();
//! let repo_root = tempfile::tempdir().unwrap();
//! std::fs::create_dir_all(repo_root.path().join(".dark")).unwrap();
//! std::fs::write(
//!     repo_root.path().join(".dark/config.toml"),
//!     "[policy]\nwrite = \"deny\"\n",
//! )
//! .unwrap();
//!
//! let env = EnvMap::new();
//! let flags: Vec<(String, String)> = Vec::new();
//! let sources = Sources {
//!     defaults: "[policy]\nwrite = \"confirm\"\n",
//!     dark_home: dark_home.path(),
//!     repo_root: repo_root.path(),
//!     env: &env,
//!     flags: &flags,
//! };
//!
//! let config = resolve(&sources).unwrap();
//! let resolved = config.explain("policy.write").unwrap();
//! assert_eq!(resolved.value.as_str(), Some("deny"));
//! assert!(matches!(resolved.source, Source::ProjectFile(_)));
//! ```
//!
//! # The Hugging Face token
//!
//! A configuration file must never hold a secret. [`resolve`] enforces
//! this for the two file layers (see [`Error::SecretInFile`]); the token
//! itself lives behind [`TokenStore`] instead. See the [`token`] module
//! docs for that seam, including the gap this crate leaves open around the
//! OS keyring.

pub mod env;
mod error;
mod resolver;
mod source;
pub mod token;
mod value;

pub use env::EnvMap;
pub use error::{Error, Result};
pub use resolver::{Config, ResolvedValue, Sources, resolve};
pub use source::Source;
pub use token::{EnvTokenStore, InMemoryTokenStore, TokenStore};
