//! Command-line entry point.
//!
//! This binary stays deliberately thin: parse arguments, initialise logging,
//! delegate to the library, and format the result. Logic that deserves a test
//! belongs in `src/lib.rs` and its modules instead.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use darkharness::{Config, Harness};
use tracing_subscriber::EnvFilter;

/// Command-line interface definition.
#[derive(Debug, Parser)]
#[command(name = "darkharness", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity (repeat for more detail, e.g. -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

/// Supported subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Execute a run.
    Run {
        /// Name identifying this run.
        #[arg(short, long, default_value = "default")]
        name: String,

        /// Number of tasks to execute.
        #[arg(short, long, default_value_t = 1)]
        workers: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Run { name, workers } => {
            let config = Config::new(&name, workers)
                .with_context(|| format!("building configuration for run {name:?}"))?;

            let report = Harness::new(config).run().context("executing run")?;

            println!(
                "run {:?} completed {} task(s)",
                report.name, report.tasks_run
            );
        }
    }

    Ok(())
}

/// Initialises tracing.
///
/// `RUST_LOG` wins when set, so operators keep full control; otherwise the
/// `-v` count selects the level.
fn init_tracing(verbosity: u8) {
    let default_level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("darkharness={default_level}")));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
