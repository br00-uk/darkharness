//! Asserts the workspace dependency rules.
//!
//! These rules keep the build fast and make the airlock auditable. A reader
//! can confirm that only one crate can reach the network by reading one
//! manifest. This task turns that claim into a check.
//!
//! The task reads `cargo metadata` and inspects **direct** dependencies. A
//! transitive dependency that arrives through an allowed crate is expected:
//! `dark-cli` reaches mistral.rs through `dark-engine`, and that is the point
//! of the layering.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Rule 12. Only this crate may depend on the inference engine.
const ENGINE_OWNER: &str = "dark-engine";

/// Rule 12. Crates that pull in mistral.rs.
const ENGINE_CRATES: &[&str] = &["mistralrs", "mistralrs-core", "mistralrs-quant"];

/// Rule 13. Only this crate may construct an HTTP client.
const HTTP_OWNER: &str = "dark-airlock";

/// Rule 13. Crates that can open a socket to a remote host.
const HTTP_CRATES: &[&str] = &["reqwest", "hyper", "ureq", "isahc", "curl", "attohttpc"];

/// Rule 15. What `dark-contract` may depend on.
///
/// The specification names serde, thiserror, ulid, bytes, and tokio. The trait
/// signatures that the same specification mandates also need the async
/// plumbing below. See `docs/adr/0001-dark-contract-dependencies.md`.
const CONTRACT_ALLOWED: &[&str] = &[
    "async-trait",
    "bytes",
    "futures-core",
    "serde",
    "serde_json",
    "thiserror",
    "tokio",
    "tokio-util",
    "ulid",
];

/// Rule 14. `dark-tui` may depend on this workspace crate and no other.
const TUI_ALLOWED_WORKSPACE: &[&str] = &["dark-contract"];

/// Rule 16. These crates may depend on `dark-contract` and their own storage
/// crates only. They must not reach for another workspace crate.
///
/// `dark-acp` is held to the same rule for the same reason: it speaks one
/// protocol to one subprocess and holds no session state of its own, so
/// `dark-cli` composes it with the policy and the event bus exactly as it
/// composes the others. A dependency from here on `dark-core` would put
/// this harness's turn loop inside a crate that has no turns.
/// See `docs/adr/0007-agent-client-protocol.md`.
const STORAGE_CRATES: &[&str] = &[
    "dark-acp",
    "dark-explore",
    "dark-lexicon",
    "dark-cartograph",
];

/// Rule 17. Only these crates may take a normal dependency on `dark-engine`.
///
/// `dark-cli` is the composition root: it builds the real engine and hands it
/// to `dark-core` as `dyn Engine`. Every other crate develops and tests
/// against `dark-engine-fake`.
const ENGINE_DEPENDENTS: &[&str] = &["dark-engine", "dark-cli"];

/// One workspace package and its direct dependencies.
struct Package {
    name: String,
    /// Dependencies of kind `normal`. Dev and build dependencies are excluded.
    normal_deps: BTreeSet<String>,
}

/// Runs the check.
pub(crate) fn run() -> Result<()> {
    let packages = workspace_packages()?;
    let workspace_names: BTreeSet<&str> = packages.iter().map(|p| p.name.as_str()).collect();

    let mut failures = Vec::new();

    for pkg in &packages {
        check_exclusive(pkg, ENGINE_CRATES, ENGINE_OWNER, 12, &mut failures);
        check_exclusive(pkg, HTTP_CRATES, HTTP_OWNER, 13, &mut failures);

        if pkg.name == "dark-tui" {
            check_workspace_subset(
                pkg,
                TUI_ALLOWED_WORKSPACE,
                &workspace_names,
                14,
                &mut failures,
            );
        }

        if pkg.name == "dark-contract" {
            for dep in &pkg.normal_deps {
                if !CONTRACT_ALLOWED.contains(&dep.as_str()) {
                    failures.push(format!(
                        "Rule 15: dark-contract must not depend on {dep}. \
                         Allowed: {}.",
                        CONTRACT_ALLOWED.join(", ")
                    ));
                }
            }
        }

        if STORAGE_CRATES.contains(&pkg.name.as_str()) {
            check_workspace_subset(pkg, &["dark-contract"], &workspace_names, 16, &mut failures);
        }

        if !ENGINE_DEPENDENTS.contains(&pkg.name.as_str())
            && pkg.normal_deps.contains("dark-engine")
        {
            failures.push(format!(
                "Rule 17: {} must not depend on dark-engine. Hold the engine as \
                 `dyn Engine` and develop against dark-engine-fake.",
                pkg.name
            ));
        }
    }

    if failures.is_empty() {
        println!(
            "check-deps: {} workspace crates, all rules pass",
            packages.len()
        );
        return Ok(());
    }

    eprintln!("check-deps found {} violation(s):", failures.len());
    for failure in &failures {
        eprintln!("  - {failure}");
    }
    bail!("dependency rules violated");
}

/// Asserts that only `owner` depends on any crate in `banned`.
fn check_exclusive(
    pkg: &Package,
    banned: &[&str],
    owner: &str,
    rule: u8,
    failures: &mut Vec<String>,
) {
    if pkg.name == owner {
        return;
    }
    for dep in &pkg.normal_deps {
        if banned.contains(&dep.as_str()) {
            failures.push(format!(
                "Rule {rule}: {} must not depend on {dep}. Only {owner} may.",
                pkg.name
            ));
        }
    }
}

/// Asserts that the workspace dependencies of `pkg` are a subset of `allowed`.
fn check_workspace_subset(
    pkg: &Package,
    allowed: &[&str],
    workspace_names: &BTreeSet<&str>,
    rule: u8,
    failures: &mut Vec<String>,
) {
    for dep in &pkg.normal_deps {
        if workspace_names.contains(dep.as_str()) && !allowed.contains(&dep.as_str()) {
            failures.push(format!(
                "Rule {rule}: {} must not depend on {dep}. Allowed workspace crates: {}.",
                pkg.name,
                allowed.join(", ")
            ));
        }
    }
}

/// Reads the workspace members and their direct dependencies.
fn workspace_packages() -> Result<Vec<Package>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = std::process::Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("running cargo metadata")?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let metadata: Value =
        serde_json::from_slice(&output.stdout).context("parsing cargo metadata output")?;

    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .context("cargo metadata has no packages array")?;

    let mut result = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .context("a package has no name")?
            .to_owned();

        let mut normal_deps = BTreeSet::new();
        if let Some(deps) = package.get("dependencies").and_then(Value::as_array) {
            for dep in deps {
                // `kind` is null for a normal dependency, "dev" or "build"
                // otherwise. Only normal dependencies carry the rules: a dev
                // dependency on dark-engine-fake is exactly what Rule 17 asks
                // for.
                let kind = dep.get("kind").and_then(Value::as_str);
                if kind.is_none()
                    && let Some(dep_name) = dep.get("name").and_then(Value::as_str)
                {
                    normal_deps.insert(dep_name.to_owned());
                }
            }
        }
        result.push(Package { name, normal_deps });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
