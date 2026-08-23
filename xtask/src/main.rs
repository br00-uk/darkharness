//! Build and check tasks.
//!
//! Run these through the cargo alias, for example `cargo xtask check-deps`.

mod airgap;
mod check_deps;
mod explore_fixture;
mod release;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("check-deps") => check_deps::run(),
        Some("check-binary-size") => release::check_binary_size(),
        Some("check-reproducible") => release::check_reproducible(),
        Some("airgap") => airgap::run(),
        Some("explore-fixture") => {
            let assert_hash = std::env::args().any(|arg| arg == "--assert-hash");
            explore_fixture::run(assert_hash)
        }
        // Not a documented task: `airgap::run` re-executes this binary
        // with this argument, inside `unshare --net`, to run the
        // network probe on the far side of the namespace it is proving.
        // See `airgap::PROBE_TASK`.
        Some(airgap::PROBE_TASK) => airgap::probe_network_subcommand(),
        Some(other) => bail!(
            "unknown task {other:?}. Known tasks: check-deps, check-binary-size, \
             check-reproducible, airgap, explore-fixture"
        ),
        None => {
            eprintln!("usage: cargo xtask <task>");
            eprintln!();
            eprintln!("tasks:");
            eprintln!("  check-deps          Assert the dependency rules (Rules 12 to 17).");
            eprintln!("  check-binary-size   Assert the dark-cpu artefact stays under 80 MiB.");
            eprintln!("  check-reproducible  Assert two dark-cpu builds are byte-identical.");
            eprintln!("  airgap              Run the air-gap test (task unit J5).");
            eprintln!("  explore-fixture     Run the explore report on a fixture (task unit F4).");
            eprintln!(
                "                      Add --assert-hash to check it against \
                 testdata/explore-fixture.hash."
            );
            bail!("no task given");
        }
    }
}
