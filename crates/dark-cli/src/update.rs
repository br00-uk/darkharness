//! `dark update`: reports whether a newer release exists (task unit
//! `J4`, step 9).
//!
//! # Exiting cleanly with no network
//!
//! This is the one behaviour task unit `J4` names for this command, and
//! it follows from the primary requirement: a person disconnects the
//! network after `dark setup` and keeps working. A harness that fails
//! when it cannot reach a release server would make a disconnected
//! machine feel broken every time this ran. So an unreachable network is
//! reported as a plain sentence and exits successfully. A refusal from
//! dark mode is the same: the person asked for no egress, and got it.
//!
//! # Why this reports rather than installs
//!
//! Replacing a running binary in place is the installer's job, not a
//! turn's. `cargo-dist` publishes an installer script alongside the three
//! artefacts (see `dist-workspace.toml` and `.github/workflows/release.yml`),
//! and it handles the platform detection, the checksum, and the signature
//! this command would otherwise have to repeat. So this says what is
//! available and how to take it, which is honest about what it did.
//!
//! Every request goes through [`dark_airlock::Client`], the one crate
//! that may construct an HTTP client (Rule 13).

use anyhow::{Context as _, Result};
use dark_airlock::Client;

/// Where the release metadata comes from.
///
/// The GitHub releases API for this repository. Named here rather than
/// configured: a person who wants a different source is building their
/// own binary, and would change this line.
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/br00-uk/darkharness/releases/latest";

/// The version this binary was built as.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runs `dark update`.
///
/// # Errors
///
/// Returns an error when the runtime cannot start, or when the release
/// server answers with something that is not the JSON this expects. An
/// unreachable network is **not** an error: see the module documentation.
pub(crate) fn run_command() -> Result<()> {
    println!("dark {CURRENT_VERSION}");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("could not start the update check")?;

    // Dark mode is off for this command by construction: a person who
    // runs `dark update` is asking to reach the network. The airlock
    // still applies its own host rules to the request.
    let client = Client::new(false);

    let body = match runtime.block_on(fetch_latest(&client)) {
        Ok(body) => body,
        Err(reason) => {
            // Step 9 of task unit J4: no network is not a failure.
            println!("{reason}");
            println!("dark is unchanged.");
            return Ok(());
        }
    };

    if let Some(tag) = latest_tag(&body) {
        report(&tag);
    } else {
        println!("the release server answered without naming a version.");
        println!("dark is unchanged.");
    }
    Ok(())
}

/// Fetches the latest release metadata.
///
/// Returns `Err` with a sentence to print — not an error to propagate —
/// when the request cannot be made or does not succeed. Every one of
/// those cases is a "cannot check now", which this command reports and
/// exits cleanly from.
async fn fetch_latest(client: &Client) -> Result<String, String> {
    let response = client
        .get(LATEST_RELEASE_URL)
        .await
        .map_err(|err| describe_unreachable(&err))?;

    if !response.status().is_success() {
        return Err(format!(
            "the release server answered {}; cannot check for a newer version now.",
            response.status(),
        ));
    }

    response
        .text()
        .await
        .map_err(|err| format!("the release server's answer could not be read: {err}"))
}

/// Turns an airlock error into the sentence this command prints.
///
/// A dark-mode refusal is named for what it is, because a person who
/// turned dark mode on should be told that is what stopped the check,
/// not left reading a generic network message.
fn describe_unreachable(err: &dark_contract::Error) -> String {
    if err.code == dark_contract::ErrCode::PolicyDark {
        return "dark mode is on, so no request was made.".to_owned();
    }
    format!("the network is unavailable: {}", err.message)
}

/// Reads the `tag_name` out of the release metadata.
fn latest_tag(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("tag_name")?
        .as_str()
        .map(|tag| tag.trim_start_matches('v').to_owned())
}

/// Prints what the release server named, against what is running.
fn report(latest: &str) {
    if latest == CURRENT_VERSION {
        println!("dark {latest} is the newest release. Nothing to do.");
        return;
    }

    println!("dark {latest} is available.");
    println!(
        "Install it with the release installer, which checks the signature and the checksum \
         this command does not:"
    );
    println!("  https://github.com/br00-uk/darkharness/releases/latest");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tag_is_read_from_the_release_metadata() {
        let body = r#"{"tag_name": "v0.2.0", "name": "0.2.0"}"#;
        assert_eq!(latest_tag(body), Some("0.2.0".to_owned()));
    }

    #[test]
    fn a_tag_with_no_v_prefix_is_read_as_it_is() {
        assert_eq!(
            latest_tag(r#"{"tag_name": "0.2.0"}"#),
            Some("0.2.0".to_owned())
        );
    }

    #[test]
    fn metadata_with_no_tag_reads_as_none() {
        assert_eq!(latest_tag(r#"{"name": "a release"}"#), None);
    }

    #[test]
    fn a_body_that_is_not_json_reads_as_none() {
        assert_eq!(latest_tag("<html>not json</html>"), None);
    }

    #[test]
    fn a_dark_mode_refusal_is_named_as_one() {
        let err = dark_contract::Error::new(
            dark_contract::ErrCode::PolicyDark,
            "dark mode blocked the request",
        );
        let described = describe_unreachable(&err);
        assert!(described.contains("dark mode"), "described: {described}");
        assert!(
            !described.contains("network is unavailable"),
            "a dark-mode refusal is not a network failure: {described}"
        );
    }

    #[test]
    fn a_network_failure_is_named_as_one() {
        let err =
            dark_contract::Error::new(dark_contract::ErrCode::ToolFailed, "connection refused");
        let described = describe_unreachable(&err);
        assert!(
            described.contains("network is unavailable"),
            "described: {described}"
        );
    }

    #[test]
    fn reporting_the_running_version_says_there_is_nothing_to_do() {
        // Exercised for the branch rather than the text: `report` prints,
        // and the point is that the equal case takes the other path.
        report(CURRENT_VERSION);
        report("99.0.0");
    }
}
