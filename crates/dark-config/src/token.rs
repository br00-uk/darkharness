//! A seam for storing the Hugging Face access token outside a
//! configuration file.
//!
//! # A deliberate gap
//!
//! The build specification asks for the token to live in the operating
//! system keyring, using the `keyring` crate. This crate does not add that
//! dependency: on Linux, `keyring` links against D-Bus and secret-service,
//! and getting that native dependency to build cleanly on Windows and
//! macOS too, across three release artefacts (Section 4.5), is a real cost
//! that this task unit was told not to pay.
//!
//! Instead, [`TokenStore`] is the seam. [`InMemoryTokenStore`] and
//! [`EnvTokenStore`] are the two backends this crate ships — an in-memory
//! store and one that reads a supplied environment snapshot. A future
//! `KeyringTokenStore` (in a crate that may depend on `keyring`) only needs
//! to implement this trait; nothing in [`crate::resolve`] or [`crate::Config`]
//! changes.
//!
//! # The rule that always holds
//!
//! Whatever the backend, the harness never writes the token into
//! `config.toml`. [`crate::resolve`] enforces this for the two file
//! layers: a file that sets a key named `token` (at any depth) fails with
//! [`Error::SecretInFile`]. A [`TokenStore`] is where the token belongs
//! instead.

use crate::env::EnvMap;
use crate::error::{Error, Result};
use std::sync::Mutex;

/// Storage for a secret token, kept out of every configuration file.
///
/// Implement this trait to add a backend. See the module docs for the
/// keyring seam this crate leaves open.
pub trait TokenStore: Send + Sync {
    /// Returns the stored token, or `None` when no token is stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend cannot be reached.
    fn get_token(&self) -> Result<Option<String>>;

    /// Stores `token`, replacing any value already stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend refuses the write, including when
    /// the backend does not support writing at all.
    fn set_token(&self, token: &str) -> Result<()>;

    /// Removes the stored token. Does nothing when no token is stored.
    ///
    /// # Errors
    ///
    /// Returns an error when the backend refuses the removal.
    fn clear_token(&self) -> Result<()>;
}

/// Holds the token in process memory only.
///
/// The token does not survive the process exiting. This is a safe default
/// and the backend that tests use; it never touches disk, so it cannot
/// leak a token into a file by accident.
#[derive(Debug, Default)]
pub struct InMemoryTokenStore {
    token: Mutex<Option<String>>,
}

impl InMemoryTokenStore {
    /// Creates a store with no token.
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for InMemoryTokenStore {
    fn get_token(&self) -> Result<Option<String>> {
        Ok(lock(&self.token).clone())
    }

    fn set_token(&self, token: &str) -> Result<()> {
        *lock(&self.token) = Some(token.to_string());
        Ok(())
    }

    fn clear_token(&self) -> Result<()> {
        *lock(&self.token) = None;
        Ok(())
    }
}

/// Locks `mutex`, recovering the guard instead of panicking when a prior
/// holder panicked while it held the lock.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Reads the token from a supplied snapshot of environment variables.
///
/// This backend is read-only: an [`EnvMap`] is a point-in-time copy (see
/// [`crate::env`] for why `dark-config` never reads the real process
/// environment directly), so writing back to it would not persist
/// anywhere. [`TokenStore::set_token`] and [`TokenStore::clear_token`]
/// return [`Error::TokenStoreUnsupported`].
#[derive(Debug, Clone)]
pub struct EnvTokenStore {
    env: EnvMap,
    var_name: String,
}

impl EnvTokenStore {
    /// Creates a store that reads `var_name` out of `env`.
    ///
    /// An empty value counts as unset, matching how a shell treats
    /// `FOO=` and an unset `FOO` the same way in most scripts.
    pub fn new(env: EnvMap, var_name: impl Into<String>) -> Self {
        Self {
            env,
            var_name: var_name.into(),
        }
    }
}

impl TokenStore for EnvTokenStore {
    fn get_token(&self) -> Result<Option<String>> {
        Ok(self
            .env
            .get(&self.var_name)
            .cloned()
            .filter(|value| !value.is_empty()))
    }

    fn set_token(&self, _token: &str) -> Result<()> {
        Err(Error::TokenStoreUnsupported(
            "EnvTokenStore is read-only: set the environment variable and restart the harness",
        ))
    }

    fn clear_token(&self) -> Result<()> {
        Err(Error::TokenStoreUnsupported(
            "EnvTokenStore is read-only: unset the environment variable and restart the harness",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_store_round_trips() {
        let store = InMemoryTokenStore::new();
        assert_eq!(store.get_token().unwrap(), None);
        store.set_token("hf_abc").unwrap();
        assert_eq!(store.get_token().unwrap(), Some("hf_abc".to_string()));
        store.clear_token().unwrap();
        assert_eq!(store.get_token().unwrap(), None);
    }

    #[test]
    fn env_store_reads_only_the_supplied_snapshot() {
        let mut env = EnvMap::new();
        env.insert("DARK_HF_TOKEN".to_string(), "hf_xyz".to_string());
        let store = EnvTokenStore::new(env, "DARK_HF_TOKEN");
        assert_eq!(store.get_token().unwrap(), Some("hf_xyz".to_string()));
    }

    #[test]
    fn env_store_treats_an_empty_value_as_unset() {
        let mut env = EnvMap::new();
        env.insert("DARK_HF_TOKEN".to_string(), String::new());
        let store = EnvTokenStore::new(env, "DARK_HF_TOKEN");
        assert_eq!(store.get_token().unwrap(), None);
    }

    #[test]
    fn env_store_is_read_only() {
        let store = EnvTokenStore::new(EnvMap::new(), "DARK_HF_TOKEN");
        assert!(store.set_token("x").is_err());
        assert!(store.clear_token().is_err());
    }
}
