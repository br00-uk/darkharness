//! The `dark` binary.
//!
//! This shell parses arguments and delegates. Every command below is a
//! placeholder until its task unit lands. The command surface is fixed now so
//! that later task units add behaviour without changing the interface.

use anyhow::Result;
use clap::{Parser, Subcommand};

/// darkharness: a local coding harness that keeps working with no network.
#[derive(Debug, Parser)]
#[command(name = "dark", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity. Repeat for more detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run one turn and show no interface.
    Run {
        /// The prompt.
        prompt: String,
        /// Block all network egress for this run.
        #[arg(long)]
        dark: bool,
    },
    /// Configure the harness and download models.
    Setup,
    /// Measure the hardware and write the profile.
    Tune,
    /// Check the installation.
    Doctor {
        /// Check the offline path only.
        #[arg(long)]
        offline: bool,
    },
    /// Manage models.
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
    /// Manage documentation packs.
    Pack {
        #[command(subcommand)]
        action: PackAction,
    },
    /// Manage maps.
    Map {
        #[command(subcommand)]
        action: MapAction,
    },
    /// Analyse the repository.
    Explore {
        /// The path to analyse. Defaults to the repository root.
        path: Option<std::path::PathBuf>,
        /// Write the report as JSON.
        #[arg(long)]
        json: bool,
        /// Ignore the cache.
        #[arg(long)]
        refresh: bool,
    },
    /// Show the seam report.
    Seams {
        /// The path to analyse.
        path: Option<std::path::PathBuf>,
        /// How many seams to show.
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// Show what a change to a symbol can affect.
    Blast {
        /// The symbol.
        symbol: String,
    },
    /// Inspect the agent instruction chain.
    Agents {
        #[command(subcommand)]
        action: AgentsAction,
    },
    /// Manage sessions.
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Read and write configuration.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Show usage statistics.
    Stats,
    /// Update the harness.
    Update,
    /// Replay a recorded session through the terminal application.
    Replay {
        /// The session directory.
        session: std::path::PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ModelsAction {
    /// List installed models.
    List,
    /// Download a model.
    Pull {
        /// The model repository, for example `Qwen/Qwen3-4B`.
        repo: String,
        /// The quantisation to produce.
        #[arg(long)]
        quant: Option<String>,
    },
    /// Quantise a model that is already on disk.
    Quantize {
        /// The model repository.
        repo: String,
        /// The quantisation to produce.
        #[arg(long)]
        quant: String,
    },
    /// Remove a model.
    Rm {
        /// The model repository.
        repo: String,
    },
    /// Verify model hashes.
    Verify,
}

#[derive(Debug, Subcommand)]
enum PackAction {
    /// Add a pack.
    Add {
        /// A library name or a local directory.
        source: String,
        /// Where to fetch the documentation from.
        #[arg(long)]
        source_kind: Option<String>,
        /// The pack name.
        #[arg(long)]
        name: Option<String>,
        /// The version to record.
        #[arg(long)]
        version: Option<String>,
    },
    /// List packs.
    List,
    /// Fetch packs again.
    Refresh {
        /// Refresh every pack.
        #[arg(long)]
        all: bool,
    },
    /// Remove a pack.
    Rm {
        /// The pack identifier.
        pack: String,
    },
    /// Write a pack to one file.
    Export {
        /// The pack identifier.
        pack: String,
        /// The output file.
        #[arg(short, long)]
        output: std::path::PathBuf,
    },
    /// Read a pack from one file.
    Import {
        /// The pack file.
        file: std::path::PathBuf,
    },
    /// Build the indexes again.
    Reindex {
        /// Reindex every pack.
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
enum MapAction {
    /// List maps.
    List,
    /// Show one map.
    Show {
        /// The map identifier.
        map: String,
    },
    /// Write a map to another tracker.
    Export {
        /// The map identifier.
        map: String,
        /// The output format.
        #[arg(long, default_value = "markdown")]
        format: String,
    },
    /// Rebuild the database from the journal.
    Rebuild,
    /// Report ticket sizing quality.
    Health {
        /// The map identifier.
        #[arg(long)]
        map: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum AgentsAction {
    /// Show the resolved instruction chain.
    Explain,
}

#[derive(Debug, Subcommand)]
enum SessionAction {
    /// List sessions.
    List,
    /// Replay a session.
    Replay {
        /// The session identifier.
        session: String,
    },
    /// Continue a session.
    Resume {
        /// The session identifier.
        session: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigAction {
    /// Show one value.
    Get {
        /// The key.
        key: String,
    },
    /// Set one value.
    Set {
        /// The key.
        key: String,
        /// The value.
        value: String,
    },
    /// Show one value and the source that set it.
    Explain {
        /// The key.
        key: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        // `dark` with no subcommand starts the terminal application.
        None => not_yet("the terminal application", "H1"),
        Some(Command::Run { .. }) => not_yet("dark run", "A2"),
        Some(Command::Setup) => not_yet("dark setup", "J3"),
        Some(Command::Tune) => not_yet("dark tune", "B6"),
        Some(Command::Doctor { .. }) => not_yet("dark doctor", "J3"),
        Some(Command::Models { .. }) => not_yet("dark models", "B2"),
        Some(Command::Pack { .. }) => not_yet("dark pack", "G5"),
        Some(Command::Map { .. }) => not_yet("dark map", "D5"),
        Some(Command::Explore { .. }) => not_yet("dark explore", "F1"),
        Some(Command::Seams { .. }) => not_yet("dark seams", "F3"),
        Some(Command::Blast { .. }) => not_yet("dark blast", "F3"),
        Some(Command::Agents { .. }) => not_yet("dark agents explain", "K3"),
        Some(Command::Session { .. }) => not_yet("dark session", "A1"),
        Some(Command::Config { .. }) => not_yet("dark config", "J2"),
        Some(Command::Stats) => not_yet("dark stats", "J6"),
        Some(Command::Update) => not_yet("dark update", "J4"),
        Some(Command::Replay { .. }) => not_yet("dark replay", "H5"),
    }
}

/// Reports that a command exists but its task unit has not landed.
fn not_yet(what: &str, task_unit: &str) -> Result<()> {
    anyhow::bail!("{what} is not implemented yet. It arrives with task unit {task_unit}.")
}
