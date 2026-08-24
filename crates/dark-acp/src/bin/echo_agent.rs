//! A minimal agent that speaks the Agent Client Protocol, so this
//! workspace can prove its own client.
//!
//! # Why this exists
//!
//! [`dark_acp::session::run_prompt`] drives a real subprocess over a real
//! protocol. Nothing in this workspace could exercise it: the agents that
//! speak this protocol are other people's programs, and running one needs
//! that agent's own credentials and a network connection. So the client
//! path shipped compile-true and unexercised, which
//! `docs/adr/0007-agent-client-protocol.md` recorded as the honest state.
//!
//! This closes that. It is an agent — the other side of the same
//! protocol, built with the same crate — that answers from a script
//! instead of from a model. It needs no credentials and opens no socket,
//! so a test can spawn it anywhere, including on a machine with the
//! network unplugged.
//!
//! It is the same idea as `dark-engine-fake`: the way to test a harness
//! that drives something expensive is to build a cheap thing with the
//! same shape.
//!
//! # What it does
//!
//! Answers `initialize` and `session/new`, then, for each `session/prompt`:
//!
//! 1. Streams the prompt back as an agent message chunk, so a test can
//!    check that text reaches the client.
//! 2. Asks permission first when the prompt begins with `ask:`, so a test
//!    can check that a permission request reaches this harness's policy
//!    and that the answer is carried out.
//!
//! The `ask:` convention keeps one binary able to drive both paths,
//! rather than needing a second fixture that differs only in whether it
//! asks.

use agent_client_protocol::schema::v1::{
    AgentCapabilities, ContentBlock, ContentChunk, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionId,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionRequest, SessionId,
    SessionNotification, SessionUpdate, StopReason, TextContent, ToolCallId, ToolCallUpdate,
    ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Result, Stdio};

/// The session identifier this agent hands out.
///
/// One fixed value: this agent serves one session at a time, and a test
/// that wants two runs it twice.
const SESSION: &str = "echo-session";

/// The prefix that makes this agent ask permission before answering.
const ASK_PREFIX: &str = "ask:";

/// Sends one agent message chunk, which is how this agent says anything.
fn say(
    connection: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
    session: SessionId,
    text: String,
) -> Result<()> {
    connection.send_notification(SessionNotification::new(
        session,
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}

#[tokio::main]
async fn main() -> Result<()> {
    Agent
        .builder()
        .name("dark-acp-echo-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(AgentCapabilities::new()),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_request: NewSessionRequest, responder, _connection| {
                responder.respond(NewSessionResponse::new(SessionId::new(SESSION)))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection| {
                let asked: String = request
                    .prompt
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text(text) => Some(text.text.clone()),
                        _ => None,
                    })
                    .collect();

                let session = SessionId::new(SESSION);

                let Some(rest) = asked.strip_prefix(ASK_PREFIX) else {
                    say(&connection, session, asked)?;
                    return responder.respond(PromptResponse::new(StopReason::EndTurn));
                };

                // The permission path. The answer must not be awaited
                // here: this handler runs on the dispatch loop, and
                // waiting on it would stop the loop that has to read the
                // answer — the deadlock `SentRequest::block_task`'s own
                // documentation warns about. So the reply is sent, and
                // the prompt answered, from the callback instead.
                let replying = connection.clone();
                connection
                    .send_request(RequestPermissionRequest::new(
                        session.clone(),
                        ToolCallUpdate::new(
                            ToolCallId::new("echo-call"),
                            ToolCallUpdateFields::new().title(rest.trim().to_owned()),
                        ),
                        vec![
                            PermissionOption::new(
                                PermissionOptionId::new("yes"),
                                "Allow".to_owned(),
                                PermissionOptionKind::AllowOnce,
                            ),
                            PermissionOption::new(
                                PermissionOptionId::new("no"),
                                "Deny".to_owned(),
                                PermissionOptionKind::RejectOnce,
                            ),
                        ],
                    ))
                    .on_receiving_result(move |result| async move {
                        let answer = match result {
                            Ok(response) => format!("permission: {:?}", response.outcome),
                            Err(err) => format!("permission failed: {err}"),
                        };
                        say(&replying, session, answer)?;
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_to(Stdio::new())
        .await
}
