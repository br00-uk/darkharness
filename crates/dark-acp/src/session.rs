//! Runs one prompt against a foreign agent and reports what it did.
//!
//! # What is proved here, and what is not
//!
//! [`bridge`](crate::bridge) and [`discover`](crate::discover) are pure
//! and fully tested. This module is the seam that actually speaks to
//! another process, and it is **compile-true rather than exercised**:
//! running it needs an ACP agent installed *and* that agent's own
//! credentials, neither of which a test in this workspace can assume.
//! The same honesty applies here as to `dark-engine`'s live paths — see
//! `docs/adr/0006` — and for the same reason: saying a thing is tested
//! when it has never run is worse than saying it has not.
//!
//! What that means in practice: the mapping decisions this module
//! delegates to [`bridge`](crate::bridge) are tested, and the sequence
//! below is not.
//!
//! # The sequence
//!
//! 1. `initialize` — agree a protocol version.
//! 2. `session/new` — open a session rooted at the repository.
//! 3. `session/prompt` — send the work.
//!
//! While that runs the agent calls back. Permission requests reach
//! [`Decide`], which is where this harness's policy answers. Session
//! updates reach [`Report`], which is where they become
//! [`dark_contract::Event`] values and so reach the terminal and the
//! transcript unchanged.

use std::str::FromStr as _;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use dark_contract::{Allow, ErrCode, Error, Result};

use crate::bridge::{Option_, PermissionAsk};
use crate::discover::Agent;

/// Answers a foreign agent's permission requests.
///
/// The implementation in `dark-cli` asks this harness's own
/// `dark_core::policy::Policy`, so a foreign agent is gated by the same
/// rules, and the same confirmations, as the local model.
#[async_trait]
pub trait Decide: Send + Sync {
    /// Decides whether the agent may do what it asked.
    async fn decide(&self, ask: PermissionAsk) -> Allow;
}

/// Receives what the agent reports as it works.
///
/// The implementation in `dark-cli` sends these on the event bus, which
/// is what puts a foreign agent's output in the terminal application and
/// in the session transcript with no display code of its own.
pub trait Report: Send + Sync {
    /// Visible output from the agent.
    fn text(&self, text: &str);
    /// A one-line notice about what the agent is doing.
    fn notice(&self, text: &str);
}

/// What one prompt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Why the agent stopped, in its own words.
    pub stop_reason: String,
    /// Everything the agent said, joined.
    pub text: String,
}

/// Refuses an agent that cannot start without a download, in dark mode.
///
/// The `npx <package>@latest` form fetches the package every launch, so
/// starting one with egress blocked fails partway rather than cleanly.
/// Refusing here names the real reason, which a connection error would
/// not.
///
/// # Errors
///
/// Returns [`ErrCode::PolicyDark`] when `dark` is set and `agent` needs
/// a download to start.
pub fn check_dark_mode(agent: &Agent, dark: bool) -> Result<()> {
    if !dark {
        return Ok(());
    }

    if agent.launch.needs_network_to_start {
        return Err(Error::new(
            ErrCode::PolicyDark,
            format!(
                "{} is started with `{}`, which downloads the agent before it runs, and dark \
                 mode blocks that",
                agent.name,
                agent.launch.command_line()
            ),
        )
        .with_remedy(
            "Install the agent so it runs from this machine, leave dark mode, or use the local \
             model.",
        ));
    }

    // An agent already on this machine starts with no egress. What it
    // does once running is its own affair, which is why this is a
    // separate judgement from `needs_network_to_start` — and why the
    // caller warns about `reaches_network` rather than this function
    // silently allowing it.
    if agent.reaches_network {
        return Err(Error::new(
            ErrCode::PolicyDark,
            format!(
                "{} sends this repository's code to a remote service, and dark mode blocks that",
                agent.name
            ),
        )
        .with_remedy("Leave dark mode, or use the local model, which needs no network."));
    }

    Ok(())
}

/// Runs `prompt` against `agent`, in `cwd`.
///
/// See the module documentation: this is the unexercised seam.
///
/// # Errors
///
/// Returns [`ErrCode::PolicyDark`] when dark mode refuses this agent,
/// and [`ErrCode::ToolFailed`] when the agent cannot be started, does
/// not speak the protocol, or fails while working.
pub async fn run_prompt(
    agent: &Agent,
    cwd: &std::path::Path,
    prompt: &str,
    dark: bool,
    decide: Arc<dyn Decide>,
    report: Arc<dyn Report>,
) -> Result<Outcome> {
    check_dark_mode(agent, dark)?;

    report.notice(&format!(
        "starting {} with `{}`",
        agent.name,
        agent.launch.command_line()
    ));

    let spawned = agent_client_protocol::AcpAgent::from_str(&agent.launch.command_line()).map_err(
        |source| {
            Error::new(
                ErrCode::ToolFailed,
                format!("cannot start {}: {source}", agent.name),
            )
            .with_remedy("Run dark acp list to see how this agent is started.")
        },
    )?;

    connect(spawned, cwd, prompt, decide, report).await
}

/// The protocol conversation itself, once the subprocess is described.
///
/// Split out so [`run_prompt`]'s own checks — the ones that are tested —
/// are separate from the part that needs a live agent.
#[allow(
    clippy::too_many_lines,
    reason = "the protocol conversation is one sequence — initialize, open, prompt — with its \
              two callbacks registered inline as the SDK's builder requires; splitting it would \
              scatter one conversation across four functions without making any of them clearer"
)]
async fn connect(
    spawned: agent_client_protocol::AcpAgent,
    cwd: &std::path::Path,
    prompt: &str,
    decide: Arc<dyn Decide>,
    report: Arc<dyn Report>,
) -> Result<Outcome> {
    use agent_client_protocol::schema::ProtocolVersion;
    // The protocol's own role marker, aliased because this crate's
    // `Agent` is the thing a person chose to run.
    use agent_client_protocol::schema::v1::{
        ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
        RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
        SelectedPermissionOutcome, SessionNotification, SessionUpdate, TextContent,
    };
    use agent_client_protocol::{Agent as AgentRole, ConnectionTo};

    // Everything the agent said, so the caller gets the reply as one
    // piece as well as streamed.
    let said: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let stop_reason: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    let cwd = cwd.to_path_buf();
    let prompt = prompt.to_owned();

    let notify_report = Arc::clone(&report);
    let notify_said = Arc::clone(&said);
    let permission_decide = Arc::clone(&decide);
    let outcome_reason = Arc::clone(&stop_reason);

    agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            async move |notification: SessionNotification, _cx| {
                match notification.update {
                    SessionUpdate::AgentMessageChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            notify_report.text(&text.text);
                            if let Ok(mut said) = notify_said.lock() {
                                said.push_str(&text.text);
                            }
                        }
                    }
                    // Reasoning is shown as a notice rather than as the
                    // reply: it is the agent thinking aloud, and folding
                    // it into the answer would misreport what it said.
                    SessionUpdate::AgentThoughtChunk(chunk) => {
                        if let ContentBlock::Text(text) = chunk.content {
                            notify_report.notice(&text.text);
                        }
                    }
                    SessionUpdate::ToolCall(call) => {
                        notify_report.notice(&format!("tool: {}", call.title));
                    }
                    // Every other update — plans, usage, mode changes —
                    // is the agent's own bookkeeping. Reporting each one
                    // would bury the reply.
                    _ => {}
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                let title = request
                    .tool_call
                    .fields
                    .title
                    .clone()
                    .unwrap_or_else(|| "an action".to_owned());
                let ask = PermissionAsk::titled(title);
                // The identifier is this option's position, not its own
                // protocol identifier: the position is what reads the
                // right option back out of `request.options` below,
                // whatever shape the protocol's identifier has.
                let options: Vec<Option_> = request
                    .options
                    .iter()
                    .enumerate()
                    .map(|(index, option)| Option_ {
                        id: index.to_string(),
                        name: option.name.clone(),
                        kind: kind_name(option.kind).to_owned(),
                    })
                    .collect();

                let allow = permission_decide.decide(ask).await;

                // A refusal this harness cannot express as one of the
                // agent's options cancels instead. See
                // `bridge::chosen_option`: choosing some other option
                // would turn a refusal into an approval.
                let picked = crate::bridge::chosen_option(allow, &options)
                    .and_then(|chosen| chosen.id.parse::<usize>().ok())
                    .and_then(|index| request.options.get(index))
                    .map(|option| option.option_id.clone());

                match picked {
                    Some(id) => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id)),
                    )),
                    None => responder.respond(RequestPermissionResponse::new(
                        RequestPermissionOutcome::Cancelled,
                    )),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            spawned,
            move |connection: ConnectionTo<AgentRole>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;

                let opened = connection
                    .send_request(NewSessionRequest::new(cwd))
                    .block_task()
                    .await?;

                let answered = connection
                    .send_request(PromptRequest::new(
                        opened.session_id.clone(),
                        vec![ContentBlock::Text(TextContent::new(prompt))],
                    ))
                    .block_task()
                    .await?;

                if let Ok(mut reason) = outcome_reason.lock() {
                    *reason = format!("{:?}", answered.stop_reason);
                }
                Ok(())
            },
        )
        .await
        .map_err(|source| {
            Error::new(ErrCode::ToolFailed, format!("the agent failed: {source}"))
                .with_remedy("Run dark acp list to check how this agent is started.")
        })?;

    let text = said
        .lock()
        .map_or_else(|_| String::new(), |said| said.clone());
    let stop_reason = stop_reason
        .lock()
        .map_or_else(|_| String::new(), |reason| reason.clone());

    Ok(Outcome { stop_reason, text })
}

/// Names a permission option's kind the way [`crate::bridge`] reads it.
///
/// Written out rather than derived from the enum's `Debug` form. `Debug`
/// prints `AllowOnce`, which lower-cases to `allowonce` and matches
/// nothing — a mistake that cancels every permission request while
/// looking like it works, because a cancelled request is not an error.
/// A live test against a real agent is what caught it; see
/// `tests/speaks_the_protocol.rs`.
fn kind_name(kind: agent_client_protocol::schema::v1::PermissionOptionKind) -> &'static str {
    use agent_client_protocol::schema::v1::PermissionOptionKind as Kind;

    match kind {
        Kind::AllowOnce => "allow_once",
        Kind::AllowAlways => "allow_always",
        Kind::RejectOnce => "reject_once",
        Kind::RejectAlways => "reject_always",
        // The enum may gain a variant. An unrecognised kind matches no
        // answer, so the caller cancels — which is the safe direction.
        _ => "unknown",
    }
}

/// Reads the options out of a permission request.
///
/// Kept beside the conversation because it is the shape the protocol
/// hands back, and tested through [`crate::bridge::chosen_option`].
#[must_use]
pub fn options_from(pairs: &[(String, String, String)]) -> Vec<Option_> {
    pairs
        .iter()
        .map(|(id, name, kind)| Option_ {
            id: id.clone(),
            name: name.clone(),
            kind: kind.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::discover::Launch;

    use super::*;

    fn agent(needs_download: bool, reaches_network: bool) -> Agent {
        Agent {
            name: "test-agent".to_owned(),
            launch: Launch {
                program: if needs_download { "npx" } else { "test-agent" }.to_owned(),
                args: vec!["acp".to_owned()],
                needs_network_to_start: needs_download,
            },
            reaches_network,
        }
    }

    #[test]
    fn dark_mode_off_allows_any_agent() {
        assert!(check_dark_mode(&agent(true, true), false).is_ok());
    }

    #[test]
    fn dark_mode_refuses_an_agent_that_must_be_downloaded() {
        let err = check_dark_mode(&agent(true, true), true).unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(
            err.message.contains("downloads the agent"),
            "the message names the real reason: {}",
            err.message
        );
        assert!(err.remedy.is_some(), "every error carries a remedy");
    }

    #[test]
    fn dark_mode_refuses_an_agent_that_sends_code_away() {
        // Installed locally, so it starts fine — but running it defeats
        // the point of dark mode.
        let err = check_dark_mode(&agent(false, true), true).unwrap_err();
        assert_eq!(err.code, ErrCode::PolicyDark);
        assert!(
            err.message.contains("remote service"),
            "message: {}",
            err.message
        );
    }

    #[test]
    fn dark_mode_allows_a_local_agent_that_sends_nothing_away() {
        // No such agent is in the table today, but a person can name one
        // with `Agent::configured`, and the rule should hold for it.
        assert!(check_dark_mode(&agent(false, false), true).is_ok());
    }

    #[test]
    fn options_are_read_in_the_order_the_agent_gave_them() {
        let read = options_from(&[
            ("a".to_owned(), "Allow".to_owned(), "allow_once".to_owned()),
            ("r".to_owned(), "Deny".to_owned(), "reject_once".to_owned()),
        ]);

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].id, "a");
        assert_eq!(read[1].kind, "reject_once");
    }
}
