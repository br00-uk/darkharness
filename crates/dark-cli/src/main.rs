//! The `dark` binary.
//!
//! This shell parses arguments and delegates. Every command below is a
//! placeholder until its task unit lands. The command surface is fixed now so
//! that later task units add behaviour without changing the interface.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod acp;
mod agents;
mod blast;
mod config;
mod doctor;
mod explore;
mod harness;
mod map;
mod models;
mod pack;
mod replay;
mod run;
mod scrape;
mod session;
mod setup;
mod shell;
mod stats;
mod tune;
mod update;

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
        /// Allow an action that would otherwise need a confirmation.
        ///
        /// A headless run cannot show a prompt, so without this a
        /// `confirm` policy value denies the action and tells the model
        /// why. See task unit `A4`.
        #[arg(long)]
        yes: bool,
    },
    /// Configure the harness and download models.
    Setup {
        /// Print the plan without downloading, converting, or writing
        /// anything.
        #[arg(long)]
        dry_run: bool,
    },
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
    /// Work with other coding agents over the Agent Client Protocol.
    Acp {
        #[command(subcommand)]
        action: AcpAction,
    },
    /// Show usage statistics.
    Stats,
    /// Update the harness.
    Update,
    /// Replay a recorded session through the terminal application.
    Replay {
        /// The session identifier: a ULID, naming
        /// `$DARK_HOME/sessions/<ulid>/transcript.jsonl`.
        session: String,
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
enum AcpAction {
    /// List the agents installed on this machine.
    List,
    /// Run one prompt against another agent.
    Run {
        /// The agent, as `dark acp list` names it.
        agent: String,
        /// The prompt.
        prompt: String,
        /// Block all network egress for this run.
        #[arg(long)]
        dark: bool,
        /// Allow an action that would otherwise need a confirmation.
        #[arg(long)]
        yes: bool,
        /// Send the prompt alone, without this repository's context.
        #[arg(long)]
        bare: bool,
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
        None => shell::run_command(false, None),
        Some(Command::Run { prompt, dark, yes }) => run::run_command(&prompt, dark, yes),
        Some(Command::Setup { dry_run }) => setup::run_command(dry_run),
        Some(Command::Tune) => tune::run_command(),
        Some(Command::Doctor { offline }) => doctor::run_command(offline),
        Some(Command::Models { action }) => models::run_command(action),
        Some(Command::Pack { action }) => pack::run_command(action),
        Some(Command::Map { action }) => map::run_command(action),
        Some(Command::Explore {
            path,
            json,
            refresh,
        }) => explore::run_explore(path, json, refresh),
        Some(Command::Seams { path, top }) => explore::run_seams(path, top),
        Some(Command::Blast { symbol }) => blast::run_command(&symbol),
        Some(Command::Agents { .. }) => agents::run_command(),
        Some(Command::Session { action }) => session::run_command(action),
        Some(Command::Config { action }) => config::run_command(action),
        Some(Command::Acp { action }) => acp::run_command(action),
        Some(Command::Stats) => stats::run_command(),
        Some(Command::Update) => update::run_command(),
        Some(Command::Replay { session }) => replay::run_command(&session),
    }
}

/// Reports that a command exists but its task unit has not landed.
fn not_yet(what: &str, task_unit: &str) -> Result<()> {
    anyhow::bail!("{what} is not implemented yet. It arrives with task unit {task_unit}.")
}

/// Converts a [`dark_contract::Error`] to an [`anyhow::Error`] without
/// losing its remedy.
///
/// [`dark_contract::Error`]'s [`std::fmt::Display`] impl prints only
/// `"{code}: {message}"` — the remedy is a separate field, for a caller
/// that wants to show it apart from the message (see how `doctor::Finding`
/// prints its own `remedy:` line). A plain `anyhow::Error::from(err)`, or a
/// bare `?` on a function returning [`dark_contract::Result`], would carry
/// that `Display` output onward and silently drop the remedy. Every command
/// module in this crate should route a library error through this function
/// instead, so the person who sees it also sees what clears it.
#[allow(
    clippy::needless_pass_by_value,
    reason = "taking Error by value is what lets every call site pass this function directly \
              to Result::map_err, rather than wrapping it in a closure at every one of the \
              many call sites across the command modules"
)]
pub(crate) fn contract_error(err: dark_contract::Error) -> anyhow::Error {
    match &err.remedy {
        Some(remedy) => anyhow::anyhow!("{err}\nremedy: {remedy}"),
        None => anyhow::anyhow!("{err}"),
    }
}

/// Returns `$DARK_HOME` (Section 5.3): the `DARK_HOME` environment
/// variable when it names a non-empty value, otherwise
/// `~/.darkharness`.
///
/// `setup` and `doctor` both need this path, so it lives here, in the
/// composition root, rather than in either of their modules.
///
/// This is a local environment variable and a local filesystem read —
/// never a network lookup. Every command wired against the path this
/// returns (`explore`, `map`, `pack`, `replay`, `agents`) must keep working
/// with the network disconnected (`CLAUDE.md`, "The primary requirement"),
/// so nothing here, or downstream of it, may reach for the network.
fn dark_home() -> PathBuf {
    if let Ok(value) = std::env::var("DARK_HOME")
        && !value.is_empty()
    {
        return PathBuf::from(value);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".darkharness")
}

/// Converts a byte count to gibibytes, for display.
///
/// One copy for the crate: `dark tune`, `dark models list`, and
/// `dark doctor` all report sizes, and three roundings of the same
/// arithmetic drift apart.
#[allow(
    clippy::cast_precision_loss,
    reason = "a size shown to one decimal place in GiB; the precision lost at any real model \
              size is far below what the display shows"
)]
pub(crate) fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Returns the repository root: the nearest ancestor of the current
/// directory that holds a `.git` directory, or the current directory when
/// none does.
///
/// # Errors
///
/// Returns an error when the current directory cannot be read.
fn repo_root() -> Result<PathBuf> {
    let start = std::env::current_dir().context("could not read the current directory")?;
    let mut dir = start.as_path();
    loop {
        if dir.join(".git").exists() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(start),
        }
    }
}
