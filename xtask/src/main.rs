//! Build and check tasks.
//!
//! Run these through the cargo alias, for example `cargo xtask check-deps`.

mod check_deps;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    let task = std::env::args().nth(1);
    match task.as_deref() {
        Some("check-deps") => check_deps::run(),
        Some(other) => bail!("unknown task {other:?}. Known tasks: check-deps"),
        None => {
            eprintln!("usage: cargo xtask <task>");
            eprintln!();
            eprintln!("tasks:");
            eprintln!("  check-deps    Assert the dependency rules (Rules 12 to 17).");
            bail!("no task given");
        }
    }
}
