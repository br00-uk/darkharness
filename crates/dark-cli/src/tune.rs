//! `dark tune`: measures the machine and recommends a profile (task unit
//! `B6`).
//!
//! This runs [`dark_engine::tune::run`] with no engine to measure a live
//! generation rate against: `dark-cli` has no wired path yet from a
//! `--quant` flag to a registered [`dark_engine::RealEngine`] model (that
//! is `dark run`'s job, and `dark run` still reports "not implemented"
//! pending the rest of this build), so a measured tokens-per-second figure
//! is not available here. The device, the memory, and the recommended
//! profile are all real regardless: none of them need a loaded model.
//!
//! This prints the `[hardware]` section rather than writing it into
//! `$DARK_HOME/config.toml` directly. `dark-config` (task unit `J2`) owns
//! that file's merge and write path, and is not part of this task unit;
//! printing the section, ready to paste in, is the boundary that respects
//! that ownership rather than reaching into a file this task unit does
//! not own.

use anyhow::{Context, Result};

use dark_engine::tune::{self, device};

/// Runs `dark tune`.
pub(crate) fn run_command() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .context("could not start the tuning runtime")?;
    let report = runtime
        .block_on(tune::run(None))
        .map_err(crate::contract_error)?;

    println!("device:          {}", device::device_name(&report.device));
    println!("hardware class:  {}", report.class.label());
    println!(
        "memory:          {:.1} GiB total, {:.1} GiB budget",
        crate::bytes_to_gib(report.memory.total_bytes),
        crate::bytes_to_gib(report.memory.budget_bytes()),
    );
    match report.measured_tok_s {
        Some(rate) => println!("measured rate:   {rate:.1} tok/s"),
        None => println!("measured rate:   not measured (no model loaded)"),
    }
    println!();

    let rec = &report.recommendation;
    println!("recommended profile:");
    println!("  model:                          {}", rec.model);
    println!("  quantisation:                   {}", rec.quant);
    println!("  context:                        {}", rec.context);
    println!(
        "  role classes share one model:   {}",
        rec.share_role_classes
    );
    println!("  thinking:                       {}", rec.thinking);
    println!(
        "  round-trip limit:                {}",
        rec.round_trip_limit
    );

    let label = format!(
        "{}-{}",
        rec.model.to_ascii_lowercase().replace('/', "-"),
        rec.quant
    );
    let section = report.to_hardware_section(&label);
    let toml = section
        .to_toml()
        .map_err(|err| anyhow::anyhow!("could not render the [hardware] section: {err}"))?;

    println!();
    println!("[hardware] section (paste into $DARK_HOME/config.toml):");
    print!("{toml}");

    Ok(())
}
