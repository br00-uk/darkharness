//! `dark models`: downloads and inspects model files (task unit `B2`).
//!
//! `pull` is the only action this module makes real today; `list`,
//! `quantize`, `rm`, and `verify` need the model manifest directory layout
//! and quantisation tooling that later task units add, so they stay
//! [`crate::not_yet`] for now, exactly as `models` itself was before this
//! task unit.

use std::io::Write as _;

use anyhow::{Context, Result};

use dark_airlock::Client;
use dark_engine::load::{self, ModelFormat, PullFile, PullRequest};

use crate::ModelsAction;

/// Runs the `dark models` subcommand named by `action`.
pub(crate) fn run_command(action: ModelsAction) -> Result<()> {
    match action {
        ModelsAction::Pull { repo, quant } => pull(&repo, quant.as_deref()),
        ModelsAction::List => crate::not_yet(
            "dark models list",
            "G-series pack tooling reused for models",
        ),
        ModelsAction::Quantize { .. } => crate::not_yet("dark models quantize", "B2 follow-up"),
        ModelsAction::Rm { .. } => crate::not_yet("dark models rm", "B2 follow-up"),
        ModelsAction::Verify => crate::not_yet("dark models verify", "B2 follow-up"),
    }
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
