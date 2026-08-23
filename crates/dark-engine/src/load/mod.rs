//! Loads a model from disk, downloading it first when it is not there yet
//! (task unit `B2`).
//!
//! Three formats, preferred in [`format::ModelFormat::preference_order`]:
//! UQFF (fastest, the default for every configured profile), GGUF (files
//! the user already holds), and Hugging Face weights quantised in place
//! (slowest, first use only). [`format`] and [`manifest`] are pure logic
//! over file names and TOML text, fully tested against fake files on disk.
//! [`download`] holds the progress-reporting loop, tested against a fake
//! slow source with no network. [`materialize`] is the one seam that calls
//! into mistral.rs to actually load weights — see its module documentation
//! for why that part is compile-true but not exercised by a test here.

pub mod download;
pub mod format;
pub mod manifest;
pub mod materialize;

use std::path::{Path, PathBuf};

use dark_contract::{Chunk, ErrCode, Error, Result};

pub use download::{ByteSource, DownloadOutcome};
pub use format::ModelFormat;
pub use manifest::{Manifest, sha256_of_file};
pub use materialize::LoadSpec;

/// Returns the directory a model's files live in under `$DARK_HOME/models`.
///
/// `/` in a repository name would create a nested directory the harness
/// did not ask for, so this joins the owner and the name with `__`
/// instead — the same convention `dark doctor`'s model-manifest check
/// already assumes (see `crates/dark-cli/src/doctor.rs`).
#[must_use]
pub fn model_dir(dark_home: &Path, repository: &str) -> PathBuf {
    dark_home.join("models").join(repository.replace('/', "__"))
}

/// Returns the Hugging Face URL for one file in one repository revision.
#[must_use]
pub fn hf_file_url(repository: &str, revision: &str, filename: &str) -> String {
    format!("https://huggingface.co/{repository}/resolve/{revision}/{filename}")
}

/// A quantisation request parsed from a CLI flag such as `uqff-q4k`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantRequest {
    /// The format the prefix named, when it named one.
    pub format: Option<ModelFormat>,
    /// The quantisation name, with the format prefix removed.
    pub quant: String,
}

/// Parses a `--quant` flag such as `uqff-q4k`, `gguf-q4k`, or a bare
/// `q4k` (which leaves the format for [`format::select_from_dir`] to
/// decide from what is already on disk).
#[must_use]
pub fn parse_quant_flag(flag: &str) -> QuantRequest {
    if let Some(rest) = flag.strip_prefix("uqff-") {
        return QuantRequest {
            format: Some(ModelFormat::Uqff),
            quant: rest.to_owned(),
        };
    }
    if let Some(rest) = flag.strip_prefix("gguf-") {
        return QuantRequest {
            format: Some(ModelFormat::Gguf),
            quant: rest.to_owned(),
        };
    }
    QuantRequest {
        format: None,
        quant: flag.to_owned(),
    }
}

/// One file to fetch as part of a pull.
#[derive(Debug, Clone)]
pub struct PullFile {
    /// The file name inside the repository.
    pub filename: String,
    /// Whether this is the primary weight file: the one
    /// [`manifest::sha256_of_file`] hashes for the manifest.
    pub primary: bool,
}

/// What to download and record for one model (task unit `B2`, steps 4 to
/// 6).
#[derive(Debug, Clone)]
pub struct PullRequest {
    /// The Hugging Face repository, for example `Qwen/Qwen3-4B`.
    pub repository: String,
    /// The revision to fetch.
    pub revision: String,
    /// The quantisation this pull records in the manifest.
    pub quantisation: String,
    /// The format this pull records in the manifest.
    pub format: ModelFormat,
    /// The files to fetch, in order.
    pub files: Vec<PullFile>,
    /// Where to place the files: [`model_dir`]'s return value.
    pub dest_dir: PathBuf,
}

/// Downloads every file in `req` through `client`, hashes the primary file,
/// and writes the manifest to `req.dest_dir`. Emits
/// [`Chunk::ModelLoading`] through `on_chunk` at 2 Hz or faster while a
/// file is in flight (Rule from task unit `B2`, step 6; see
/// [`download::drain_to_file`] for where the gate lives).
///
/// The `measured_memory_bytes` field of the returned manifest is `0`: this
/// function downloads files, it does not load them, so it has not measured
/// anything yet. A caller that goes on to call [`materialize::materialize`]
/// and inspect the loaded model's residency should overwrite the manifest
/// with the measured figure.
///
/// # Errors
///
/// Returns `E_POLICY_DARK` when dark mode blocks a download. Returns
/// `E_ENGINE_LOAD` when a download or the manifest write fails.
pub async fn pull(
    client: &dark_airlock::Client,
    req: &PullRequest,
    mut on_chunk: impl FnMut(Chunk),
) -> Result<Manifest> {
    std::fs::create_dir_all(&req.dest_dir).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not create {}: {source}", req.dest_dir.display()),
        )
    })?;

    let mut primary_path = None;
    for file in &req.files {
        let url = hf_file_url(&req.repository, &req.revision, &file.filename);
        let dest = req.dest_dir.join(&file.filename);
        download::download_via_airlock(client, &url, &dest, &req.repository, &mut on_chunk).await?;
        if file.primary {
            primary_path = Some(dest);
        }
    }

    let primary_path = primary_path.ok_or_else(|| {
        Error::new(
            ErrCode::EngineLoad,
            format!(
                "{}: no file in the pull request is marked primary",
                req.repository
            ),
        )
    })?;
    let sha256 = sha256_of_file(&primary_path)?;

    let manifest = Manifest {
        repository: req.repository.clone(),
        revision: req.revision.clone(),
        quantisation: req.quantisation.clone(),
        sha256,
        measured_memory_bytes: 0,
        format: req.format,
    };
    manifest.write_to(&req.dest_dir.join("manifest.toml"))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dir_replaces_the_repository_slash() {
        let dir = model_dir(Path::new("/home/user/.darkharness"), "Qwen/Qwen3-4B");
        assert_eq!(
            dir,
            Path::new("/home/user/.darkharness/models/Qwen__Qwen3-4B")
        );
    }

    #[test]
    fn hf_file_url_builds_the_resolve_url() {
        assert_eq!(
            hf_file_url("Qwen/Qwen3-4B", "main", "config.json"),
            "https://huggingface.co/Qwen/Qwen3-4B/resolve/main/config.json"
        );
    }

    #[test]
    fn parse_quant_flag_reads_the_uqff_prefix() {
        let parsed = parse_quant_flag("uqff-q4k");
        assert_eq!(parsed.format, Some(ModelFormat::Uqff));
        assert_eq!(parsed.quant, "q4k");
    }

    #[test]
    fn parse_quant_flag_reads_the_gguf_prefix() {
        let parsed = parse_quant_flag("gguf-q8_0");
        assert_eq!(parsed.format, Some(ModelFormat::Gguf));
        assert_eq!(parsed.quant, "q8_0");
    }

    #[test]
    fn parse_quant_flag_leaves_the_format_unset_with_no_prefix() {
        let parsed = parse_quant_flag("q4k");
        assert_eq!(parsed.format, None);
        assert_eq!(parsed.quant, "q4k");
    }
}
