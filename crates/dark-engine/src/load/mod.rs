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
pub mod shape;

use std::path::{Path, PathBuf};

use dark_contract::{Chunk, ErrCode, Error, Result};

pub use download::{ByteSource, DownloadOutcome};
pub use format::ModelFormat;
pub use manifest::{Manifest, sha256_of_file};
pub use materialize::LoadSpec;
pub use shape::read as read_model_shape;

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

/// Builds the [`LoadSpec`] for the model in `dir`, stored as `format` at
/// `quant`.
///
/// This lives here rather than in the composition root because a
/// [`LoadSpec`] names `mistralrs::IsqType`, and Rule 12 keeps mistral.rs
/// behind this crate. `dark-cli` calls [`crate::RealEngine::install`],
/// which calls this.
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when `dir` cannot be read, or when it
/// holds no file of the format `format` names.
pub fn spec_for(dir: &Path, format: ModelFormat, quant: &str) -> Result<LoadSpec> {
    let file_names: Vec<String> = std::fs::read_dir(dir)
        .map_err(|source| {
            Error::new(
                ErrCode::EngineLoad,
                format!("could not read {}: {source}", dir.display()),
            )
            .with_remedy("Run dark models pull to download the model files again.")
        })?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();

    let missing = |what: &str| {
        Error::new(
            ErrCode::EngineLoad,
            format!("{} holds no {what}", dir.display()),
        )
        .with_remedy("Run dark models pull to download the model files again.")
    };

    let mut spec = LoadSpec {
        format,
        // Always the local directory, never a bare repository id: see
        // `materialize`'s module documentation. This is what keeps a load
        // from reaching the network.
        model_id: dir.display().to_string(),
        uqff_shard: None,
        gguf_files: Vec::new(),
        isq: None,
        hf_cache_path: None,
    };

    match format {
        ModelFormat::Uqff => {
            // The first shard by name. mistral.rs discovers the rest from
            // the same directory.
            let shard = file_names
                .iter()
                .filter(|name| has_extension(name, "uqff"))
                .min()
                .ok_or_else(|| missing(".uqff shard"))?;
            spec.uqff_shard = Some(dir.join(shard));
        }
        ModelFormat::Gguf => {
            let mut gguf: Vec<String> = file_names
                .iter()
                .filter(|name| has_extension(name, "gguf"))
                .cloned()
                .collect();
            if gguf.is_empty() {
                return Err(missing(".gguf file"));
            }
            // Sorted, so a sharded model loads its shards in a fixed
            // order rather than the directory's arbitrary one.
            gguf.sort();
            spec.gguf_files = gguf;
        }
        ModelFormat::HfInSitu => {
            spec.hf_cache_path = Some(dir.to_path_buf());
            spec.isq = isq_for(quant);
        }
    }

    Ok(spec)
}

/// Reports whether `name` ends in `extension`, ignoring letter case.
///
/// A model directory written on a case-insensitive filesystem can hold
/// `Model.UQFF`, and refusing to load it for the case of its name would
/// be a needless failure.
fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
}

/// Maps a quantisation name to mistral.rs's in-situ quantisation type.
///
/// Returns `None` when the name asks for no quantisation, and when
/// mistral.rs has no in-situ type for it. An in-situ load then runs at
/// the precision the weights already hold — which the resident set has
/// budgeted for, because the same name went through
/// [`crate::resident::estimate::bits_per_weight`] when the estimate was
/// made.
fn isq_for(quant: &str) -> Option<mistralrs::IsqType> {
    match quant.to_ascii_lowercase().as_str() {
        "q2k" => Some(mistralrs::IsqType::Q2K),
        "q3k" => Some(mistralrs::IsqType::Q3K),
        "q4k" => Some(mistralrs::IsqType::Q4K),
        "q5k" => Some(mistralrs::IsqType::Q5K),
        "q6k" => Some(mistralrs::IsqType::Q6K),
        "q8_0" => Some(mistralrs::IsqType::Q8_0),
        _ => None,
    }
}

/// Loads the `tokenizer.json` beside a model's weights.
///
/// [`crate::RealEngine`] keeps its own tokenizer handle per model because
/// [`dark_contract::Engine::tokenize`] must answer synchronously while
/// mistral.rs's own tokenizer access is asynchronous. See
/// `docs/adr/0006`.
///
/// # Errors
///
/// Returns [`ErrCode::EngineLoad`] when the file is absent or is not a
/// tokenizer definition.
pub fn tokenizer_in(dir: &Path) -> Result<tokenizers::Tokenizer> {
    let path = dir.join("tokenizer.json");
    tokenizers::Tokenizer::from_file(&path).map_err(|source| {
        Error::new(
            ErrCode::EngineLoad,
            format!("could not read {}: {source}", path.display()),
        )
        .with_remedy("Run dark models pull to fetch tokenizer.json.")
    })
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
