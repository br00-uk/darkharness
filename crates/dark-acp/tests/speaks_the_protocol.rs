//! Drives the real client against a real agent over a real subprocess.
//!
//! `docs/adr/0007` recorded `session::connect` as compile-true and never
//! run, because the agents that speak this protocol are other people's
//! programs needing their own credentials. These tests close that: the
//! agent here is `dark-acp-echo-agent`, the other side of the same
//! protocol built with the same crate, which answers from a script.
//!
//! Everything under test is the shipping path — the same
//! [`dark_acp::run_prompt`] that `dark acp run` calls, spawning a real
//! process and speaking real JSON-RPC over its standard input and
//! output. Only the thing at the far end is cheap.
//!
//! No network connection is opened and no credential is read, so these
//! run anywhere, including with the network unplugged.

use std::sync::{Arc, Mutex};

use dark_acp::discover::{Agent, Launch};
use dark_acp::{Decide, PermissionAsk, Report};
use dark_contract::Allow;

/// The fixture agent, as an [`Agent`] this harness would run.
///
/// `CARGO_BIN_EXE_` names the binary cargo built for this test, so the
/// path is right whatever the profile or target directory.
fn echo_agent() -> Agent {
    Agent {
        name: "echo".to_owned(),
        launch: Launch {
            program: env!("CARGO_BIN_EXE_dark-acp-echo-agent").to_owned(),
            args: Vec::new(),
            needs_network_to_start: false,
        },
        // The fixture speaks to nothing. Saying otherwise would have
        // dark mode refuse it and these tests prove nothing.
        reaches_network: false,
    }
}

/// Answers every permission request the same way, and records what it
/// was asked.
struct Answers {
    /// What to answer.
    allow: Allow,
    /// Every ask that arrived.
    asked: Arc<Mutex<Vec<PermissionAsk>>>,
}

#[async_trait::async_trait]
impl Decide for Answers {
    async fn decide(&self, ask: PermissionAsk) -> Allow {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(ask);
        }
        self.allow
    }
}

/// Collects everything reported, so a test can assert on it.
#[derive(Default)]
struct Collected {
    /// Visible output.
    text: Mutex<String>,
    /// One-line notices.
    notices: Mutex<Vec<String>>,
}

impl Report for Collected {
    fn text(&self, text: &str) {
        if let Ok(mut collected) = self.text.lock() {
            collected.push_str(text);
        }
    }

    fn notice(&self, text: &str) {
        if let Ok(mut notices) = self.notices.lock() {
            notices.push(text.to_owned());
        }
    }
}

/// Runs one prompt against the fixture agent and returns what came back,
/// alongside the permission requests that arrived.
fn run(prompt: &str, allow: Allow) -> (dark_acp::Outcome, Vec<PermissionAsk>, Vec<String>) {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let decide = Arc::new(Answers {
        allow,
        asked: Arc::clone(&asked),
    });
    let report = Arc::new(Collected::default());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");

    let cwd = std::env::current_dir().expect("a working directory");
    let outcome = runtime
        .block_on(dark_acp::run_prompt(
            &echo_agent(),
            &cwd,
            prompt,
            false,
            decide,
            Arc::clone(&report) as Arc<dyn Report>,
        ))
        .expect("the fixture agent answers");

    let asked = asked.lock().expect("not poisoned").clone();
    let notices = report.notices.lock().expect("not poisoned").clone();
    (outcome, asked, notices)
}

#[test]
fn a_prompt_reaches_the_agent_and_its_reply_comes_back() {
    // The whole path: spawn, initialize, open a session, send a prompt,
    // read the streamed reply. If any of the four is wrong, this fails.
    let (outcome, asked, _) = run("hello from the harness", Allow::Once);

    assert!(
        outcome.text.contains("hello from the harness"),
        "the agent's reply reached the client: {outcome:?}"
    );
    assert!(
        asked.is_empty(),
        "this prompt asks no permission: {asked:?}"
    );
    assert!(
        !outcome.stop_reason.is_empty(),
        "the agent said why it stopped"
    );
}

#[test]
fn a_permission_request_reaches_this_harness_and_an_approval_is_carried_out() {
    let (outcome, asked, _) = run("ask:write the file", Allow::Once);

    assert_eq!(asked.len(), 1, "one request arrived: {asked:?}");
    assert_eq!(
        asked[0].title, "write the file",
        "the agent's own title reached the policy"
    );
    // The fixture offers "yes" and "no", and reports the option it was
    // sent. An approval must arrive as the allow option selected.
    assert!(
        outcome.text.contains("Selected") && outcome.text.contains("yes"),
        "the agent saw the approval carried out: {outcome:?}"
    );
}

#[test]
fn a_refusal_is_carried_out_rather_than_quietly_becoming_an_approval() {
    // The mistake this mapping must never make. `bridge` tests the
    // choice in isolation; this proves the refusal survives the round
    // trip and the agent is told which way it went.
    //
    // A refusal is expressed by selecting the agent's *reject* option,
    // not by cancelling: cancelling says "this was never answered",
    // which is a different thing from "no".
    let (outcome, asked, _) = run("ask:delete everything", Allow::Deny);

    assert_eq!(asked.len(), 1, "the request still arrived: {asked:?}");
    assert!(
        outcome.text.contains("no"),
        "the refusal reached the agent as its reject option: {outcome:?}"
    );
    assert!(
        !outcome.text.contains("yes"),
        "a refusal must never arrive as the allow option: {outcome:?}"
    );
}

#[test]
fn the_agent_that_was_started_is_named_before_it_runs() {
    // A person waiting on a subprocess should be told what is starting,
    // and with which command.
    let (_, _, notices) = run("hello", Allow::Once);

    assert!(
        notices.iter().any(|notice| notice.contains("echo")),
        "notices: {notices:?}"
    );
}

#[test]
fn dark_mode_refuses_before_any_process_is_started() {
    // The refusal must come from the check, not from a failed launch:
    // an agent that reaches the network should never be spawned at all
    // in dark mode.
    let mut agent = echo_agent();
    agent.reaches_network = true;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let report = Arc::new(Collected::default());
    let decide = Arc::new(Answers {
        allow: Allow::Once,
        asked: Arc::new(Mutex::new(Vec::new())),
    });

    let err = runtime
        .block_on(dark_acp::run_prompt(
            &agent,
            &std::env::current_dir().expect("a working directory"),
            "hello",
            true,
            decide,
            Arc::clone(&report) as Arc<dyn Report>,
        ))
        .expect_err("dark mode refuses this agent");

    assert_eq!(err.code, dark_contract::ErrCode::PolicyDark);
    assert!(
        report.notices.lock().expect("not poisoned").is_empty(),
        "nothing was started, so nothing was announced"
    );
}
