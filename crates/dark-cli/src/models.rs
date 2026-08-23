//! `dark models`: downloads and inspects model files (task unit `B2`).
//!
//! `pull` is the one action here that uses the network. `list`, `rm`, and
//! `verify` read and write only what is already under
//! `$DARK_HOME/models`, through the same manifest scan the composition
//! root uses to choose a model ([`crate::harness::installed`]), so what
//! this command shows is exactly what a turn would load.
//!
//! `quantize` is the one action still open: converting weights from one
//! quantisation to another needs mistral.rs's own conversion path, which
//! `dark-engine` does not expose yet.

use std::io::Write as _;

use anyhow::{Context, Result};

use dark_airlock::Client;
use dark_engine::load::{self, ModelFormat, PullFile, PullRequest};

use crate::ModelsAction;

/// Runs the `dark models` subcommand named by `action`.
pub(crate) fn run_command(action: ModelsAction) -> Result<()> {
    match action {
        ModelsAction::Pull { repo, quant } => pull(&repo, quant.as_deref()),
        ModelsAction::List => list(),
        ModelsAction::Quantize { .. } => crate::not_yet("dark models quantize", "B2 follow-up"),
        ModelsAction::Rm { repo } => remove(&repo),
        ModelsAction::Verify => verify(),
    }
}

/// Runs `dark models list`.
///
/// Prints one line per installed model: the repository, the format, the
/// quantisation, and the size of its weight files on disk.
fn list() -> Result<()> {
    let installed = crate::harness::installed(&crate::dark_home())?;
    if installed.is_empty() {
        println!("no model is installed. Run dark setup, or dark models pull <repository>.");
        return Ok(());
    }

    for model in &installed {
        let manifest = &model.manifest;
        let size = load::shape::weight_bytes_in(&model.dir).unwrap_or(0);
        let quant = if manifest.quantisation.is_empty() {
            "unquantised"
        } else {
            &manifest.quantisation
        };
        println!(
            "{}  {}  {}  {:.1} GiB",
            manifest.repository,
            manifest.format,
            quant,
            crate::bytes_to_gib(size),
        );
    }
    Ok(())
}

/// Runs `dark models rm <repo>`.
///
/// Removes the model's whole directory. A model that is not installed is
/// an error rather than a silent success: a person who misspells a
/// repository name should be told, not left believing a removal happened.
fn remove(repo: &str) -> Result<()> {
    let dir = load::model_dir(&crate::dark_home(), repo);
    if !dir.exists() {
        anyhow::bail!("{repo} is not installed. Run dark models list to see what is.");
    }

    std::fs::remove_dir_all(&dir).with_context(|| format!("could not remove {}", dir.display()))?;
    println!("removed {repo} from {}", dir.display());
    Ok(())
}

/// Runs `dark models verify`.
///
/// Hashes each installed model's primary weight file and compares it with
/// the hash its manifest recorded at pull time. Reports every model, and
/// fails when any hash does not match — a silent mismatch would mean a
/// turn loading weights that are not the ones that were downloaded.
fn verify() -> Result<()> {
    let installed = crate::harness::installed(&crate::dark_home())?;
    if installed.is_empty() {
        println!("no model is installed. Run dark setup, or dark models pull <repository>.");
        return Ok(());
    }

    let mut mismatched = Vec::new();
    for model in &installed {
        let repo = &model.manifest.repository;
        match primary_hash(&model.dir, model.manifest.format) {
            Ok(actual) if actual == model.manifest.sha256 => {
                println!("{repo}: ok");
            }
            Ok(actual) => {
                println!(
                    "{repo}: MISMATCH\n  manifest: {}\n  on disk:  {actual}",
                    model.manifest.sha256
                );
                mismatched.push(repo.clone());
            }
            Err(err) => {
                println!("{repo}: could not hash: {err}");
                mismatched.push(repo.clone());
            }
        }
    }

    if !mismatched.is_empty() {
        anyhow::bail!(
            "{} model(s) do not match their manifest: {}. Run dark models pull <repository> to \
             download them again.",
            mismatched.len(),
            mismatched.join(", "),
        );
    }
    Ok(())
}

/// Hashes the weight file a manifest's `sha256` refers to.
///
/// `dark models pull` marks one file primary and hashes that one (see
/// [`load::pull`]), so this picks the same file: the first by name of the
/// format's own extension, which is the order `pull_files` builds.
fn primary_hash(dir: &std::path::Path, format: ModelFormat) -> Result<String> {
    let extension = match format {
        ModelFormat::Uqff => "uqff",
        ModelFormat::Gguf => "gguf",
        ModelFormat::HfInSitu => "safetensors",
    };

    let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("could not read {}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        })
        .collect();
    candidates.sort();

    let primary = candidates
        .first()
        .with_context(|| format!("{} holds no .{extension} file", dir.display()))?;
    load::sha256_of_file(primary).map_err(crate::contract_error)
}

/// The revision `dark models pull` fetches when the caller does not name
/// one. Every repository this harness targets serves it.
const DEFAULT_REVISION: &str = "main";

/// Runs `dark models pull <repo> [--quant <flag>]`.
fn pull(repo: &str, quant_flag: Option<&str>) -> Result<()> {
    let parsed = load::parse_quant_flag(quant_flag.unwrap_or("uqff-q4k"));
    // UQFF is the default format (task unit B2, step 3: a slow load makes
    // a swap unusable, and UQFF is the fastest of the three), so a bare
    // quantisation name with no `uqff-`/`gguf-` prefix is read as UQFF.
    let format = parsed.format.unwrap_or(ModelFormat::Uqff);

    let dark_home = crate::dark_home();
    let dest_dir = load::model_dir(&dark_home, repo);

    let request = PullRequest {
        repository: repo.to_owned(),
        revision: DEFAULT_REVISION.to_owned(),
        quantisation: parsed.quant.clone(),
        format,
        files: pull_files(format, &parsed.quant),
        dest_dir: dest_dir.clone(),
    };

    println!(
        "pulling {repo} ({format}, {}) into {}",
        parsed.quant,
        dest_dir.display()
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .enable_io()
        .build()
        .context("could not start the download runtime")?;

    // This crate has no wired source for the harness's persistent dark
    // mode setting (`dark-config`, task unit J2) yet, so this reads the
    // same `DARK_OFFLINE` environment variable `dark-airlock::child`
    // already propagates to spawned tools, rather than silently assuming
    // the network is open.
    let dark_mode = std::env::var(dark_airlock::DARK_OFFLINE_ENV).is_ok_and(|v| !v.is_empty());
    let client = Client::new(dark_mode);

    let mut last_progress = -1.0f32;
    let manifest = runtime
        .block_on(load::pull(&client, &request, |chunk| {
            if let dark_contract::Chunk::ModelLoading { progress, .. } = chunk {
                // Print at whole-percentage-point granularity: a real
                // terminal gets a readable trickle of lines, not one line
                // per 2 Hz-or-faster tick.
                if progress - last_progress >= 0.01 || progress >= 1.0 {
                    print!("\r  {:>5.1}%", progress * 100.0);
                    let _ = std::io::stdout().flush();
                    last_progress = progress;
                }
            }
        }))
        .map_err(crate::contract_error)?;
    println!();

    println!("done: sha256 {}", manifest.sha256);
    println!("manifest: {}", dest_dir.join("manifest.toml").display());
    Ok(())
}

/// Returns the files to fetch for one pull request.
///
/// This assumes a single-file (or single-shard) layout: `{quant}-0.uqff`
/// plus its UQFF sidecars, a bare `.gguf` file, or the smallest possible
/// HF in-situ set. A repository sharded across many files needs its file
/// list read from the Hugging Face API first — that lookup is not wired
/// yet, so a sharded repository fails partway through today with a plain
/// download error naming the missing file, not a clean pre-flight check.
/// This is a named gap, not a silent one.
fn pull_files(format: ModelFormat, quant: &str) -> Vec<PullFile> {
    match format {
        ModelFormat::Uqff => vec![
            PullFile {
                filename: format!("{quant}-0.uqff"),
                primary: true,
            },
            PullFile {
                filename: "residual.safetensors".to_owned(),
                primary: false,
            },
            PullFile {
                filename: "config.json".to_owned(),
                primary: false,
            },
            PullFile {
                filename: "tokenizer.json".to_owned(),
                primary: false,
            },
        ],
        ModelFormat::Gguf => vec![PullFile {
            filename: format!("{quant}.gguf"),
            primary: true,
        }],
        ModelFormat::HfInSitu => vec![
            PullFile {
                filename: "config.json".to_owned(),
                primary: false,
            },
            PullFile {
                filename: "tokenizer.json".to_owned(),
                primary: false,
            },
            PullFile {
                filename: "model.safetensors".to_owned(),
                primary: true,
            },
        ],
    }
}
