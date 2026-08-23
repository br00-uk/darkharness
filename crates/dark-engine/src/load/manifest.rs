//! The manifest that [`super::load`] writes for each loaded model
//! (task unit `B2`, step 5).
//!
//! A manifest records the repository, the revision, the quantisation, the
//! SHA-256 hash, and the measured memory use. It is TOML, matching the
//! configuration file's own `[hardware]` section (task unit `B6`) and
//! `dark-config`'s format elsewhere in the harness.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use dark_contract::{ErrCode, Error, Result};

use super::format::ModelFormat;

/// A record of one loaded model: what it is, and how it was verified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The Hugging Face repository, for example `Qwen/Qwen3-4B`.
    pub repository: String,
    /// The revision (commit or tag) that was loaded.
    pub revision: String,
    /// The quantisation name, for example `q4k`. Empty for full precision.
    pub quantisation: String,
    /// The lowercase hex SHA-256 hash of the primary weight file.
    pub sha256: String,
    /// The memory this load measured, in bytes.
    pub measured_memory_bytes: u64,
    /// The format the weights loaded from.
    pub format: ModelFormat,
}

impl Manifest {
    /// Serialises this manifest as TOML and writes it to `path`.
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when serialisation or the write
    /// fails.
    pub fn write_to(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!(
                    "could not serialise the manifest for {}: {source}",
                    self.repository
                ),
            )
        })?;
        std::fs::write(path, text).map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not write {}: {source}", path.display()),
            )
        })
    }

    /// Reads a manifest previously written by [`Self::write_to`].
    ///
    /// # Errors
    ///
    /// Returns [`ErrCode::EngineLoad`] when `path` cannot be read or does
    /// not parse.
    pub fn read_from(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not read {}: {source}", path.display()),
            )
        })?;
        toml::from_str(&text).map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not parse {}: {source}", path.display()),
            )
        })
    }
}

/// Hashes `path` with SHA-256 and returns the lowercase hex digest.
///
/// Reads the file in fixed-size chunks, so this does not load a
/// multi-gigabyte weight file into memory at once.
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `path` cannot be read.
pub fn sha256_of_file(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not open {}: {source}", path.display()),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not read {}: {source}", path.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample() -> Manifest {
        Manifest {
            repository: "Qwen/Qwen3-4B".to_owned(),
            revision: "main".to_owned(),
            quantisation: "q4k".to_owned(),
            sha256: "abc123".to_owned(),
            measured_memory_bytes: 3_500_000_000,
            format: ModelFormat::Uqff,
        }
    }

    #[test]
    fn a_manifest_round_trips_through_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");
        let manifest = sample();
        manifest.write_to(&path).unwrap();
        let read_back = Manifest::read_from(&path).unwrap();
        assert_eq!(read_back, manifest);
    }

    #[test]
    fn the_written_file_is_readable_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");
        sample().write_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("repository"));
        assert!(text.contains("Qwen/Qwen3-4B"));
        let _: toml::Table = text.parse().expect("must be valid TOML");
    }

    #[test]
    fn read_from_fails_for_a_missing_file() {
        let err = Manifest::read_from(Path::new("/does/not/exist.toml")).unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn read_from_fails_for_malformed_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        let err = Manifest::read_from(&path).unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"abc").unwrap();
        // The standard SHA-256 test vector for the ASCII string "abc".
        assert_eq!(
            sha256_of_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn sha256_is_stable_across_a_file_larger_than_one_read_buffer() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("large.bin");
        // Larger than the 64 KiB read buffer, so this exercises the loop.
        let data = vec![0x5Au8; 200 * 1024];
        std::fs::write(&path, &data).unwrap();
        let direct = format!("{:x}", Sha256::digest(&data));
        assert_eq!(sha256_of_file(&path).unwrap(), direct);
    }

    #[test]
    fn sha256_fails_for_a_missing_file() {
        let err = sha256_of_file(Path::new("/does/not/exist")).unwrap_err();
        assert_eq!(err.code, ErrCode::EngineLoad);
    }
}
