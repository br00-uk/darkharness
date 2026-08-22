//! The turn loop: one exchange between a person and a model.
//!
//! A turn runs one request, handles every tool call the model asks for, and
//! runs again until the model stops asking. See task unit `A2`.
//!
//! # The rule this module exists to keep
//!
//! **Every issued tool call gets a [`Role::Tool`] reply.** A denied call, a
//! call whose arguments did not parse, a call to a tool that does not
//! exist, a call that timed out, and a call cancelled half way all still get
//! one. An unanswered call breaks the chat template, and a broken template
//! fails the *next* turn, far from the cause. [`answer_all`] therefore
//! builds one reply for each entry of the issued-call list, and nothing in
//! it returns early: the two lists are the same length by construction. See
//! Do step 10 of task unit `A2`.
//!
//! [`TurnOutcome::history_is_well_formed`] checks that property on a
//! finished turn, so a caller and a test can assert it directly.
//!
//! # What a turn must not do
//!
//! - It must not exit at the round-trip limit. An agent that stops during an
//!   edit leaves broken files. At the limit the loop adds a system message,
//!   sets [`ToolChoice::None`], and takes one more round so the model can
//!   summarise where it got to. See Do steps 7 and 8.
//! - It must not change the resident model mid-turn. The caller resolves the
//!   [`RoleClass`] once and passes it in. See Do step 6 and Rule 5.
//! - It must not change the prefix mid-turn. [`crate::context`] assembles the
//!   prefix once and the loop appends to the tail only. See Rule 5.
//!
//! # The confirmation gap
//!
//! Task unit `A4` requires a confirmation to show the exact unified diff,
//! never a summary. A diff exists only after a tool has worked out what it
//! would change, and the [`Tool`] trait has no way to ask for that without
//! also applying it. This loop therefore gates on the action kind and the
//! exact arguments, which is sound for a command and for a denial, but it
//! cannot yet show a diff for a write it has not run. Closing that needs a
//! preview method on the [`Tool`] trait, which is a change to
//! `dark-contract` and so outside what task unit `A2` owns. See
//! `docs/adr/0004`.

mod accumulate;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dark_contract::{
    Engine, ErrCode, Error, Event, EventTx, FinishReason, Message, Part, Request, Result, Role,
    RoleClass, Tool, ToolChoice, ToolCtx, ToolResult, ToolResultSummary, ToolSchema,
};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

pub use accumulate::{Accumulated, Accumulator, IssuedCall};

use crate::policy::{Action, ActionKind, Confirmer, Decision, Policy};

/// The default round-trip limit for a turn.
pub const DEFAULT_ROUND_TRIP_LIMIT: usize = 40;

/// The round-trip limit on a central-processor profile.
///
/// A central processor generates a few tokens each second, so forty round
/// trips is hours rather than minutes. See section 4.3.
pub const CPU_ROUND_TRIP_LIMIT: usize = 12;

/// How long a running tool has to stop after a cancellation, before the loop
/// abandons it. See Do step 9 of task unit `A2`.
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// The settings for one turn.
#[derive(Debug, Clone, Copy)]
pub struct TurnConfig {
    /// How many times the loop may call the engine before it must wind up.
    pub round_trip_limit: usize,
    /// How long one tool may run.
    pub tool_timeout: Duration,
    /// How long a running tool has to stop after a cancellation.
    pub cancel_grace: Duration,
}

impl Default for TurnConfig {
    fn default() -> Self {
        Self {
            round_trip_limit: DEFAULT_ROUND_TRIP_LIMIT,
            tool_timeout: Duration::from_secs(120),
            cancel_grace: CANCEL_GRACE,
        }
    }
}

impl TurnConfig {
    /// Returns the settings for a central-processor profile.
    #[must_use]
    pub fn for_cpu() -> Self {
        Self {
            round_trip_limit: CPU_ROUND_TRIP_LIMIT,
            ..Self::default()
        }
    }
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    /// The model stopped on its own.
    Stopped,
    /// The loop reached the round-trip limit and took its wind-up round.
    ///
    /// The turn still ended cleanly: the loop asked the model to summarise
    /// the state, and that summary is the last message.
    LimitReached,
    /// A person cancelled the turn.
    Cancelled,
}

/// What one turn produced.
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    /// The messages this turn added, in order: each assistant message,
    /// followed by the [`Role::Tool`] replies that answer its calls.
    pub messages: Vec<Message>,
    /// How the turn ended.
    pub end: TurnEnd,
    /// How many times the loop called the engine.
    pub round_trips: usize,
}

impl TurnOutcome {
    /// Checks that every tool call in these messages has its reply, in the
    /// place the chat template needs it.
    ///
    /// This is the invariant the module exists to keep. See Do step 10 of
    /// task unit `A2`.
    ///
    /// The check is positional, not a count by identifier: an assistant
    /// message that asks for `n` calls must be followed immediately by `n`
    /// [`Role::Tool`] messages, answering those calls in the same order.
    /// That is what a chat template renders. Counting identifiers across the
    /// whole history would instead accept a reply that arrived in the wrong
    /// round, and would reject an engine that reuses an identifier between
    /// rounds, which is allowed.
    #[must_use]
    pub fn history_is_well_formed(&self) -> bool {
        let mut index = 0usize;
        while index < self.messages.len() {
            let message = &self.messages[index];
            index += 1;

            if message.tool_calls.is_empty() {
                continue;
            }

            for call in &message.tool_calls {
                let Some(reply) = self.messages.get(index) else {
                    // The history ended with a call still unanswered.
                    return false;
                };
                if reply.role != Role::Tool
                    || reply.tool_call_id.as_deref() != Some(call.id.as_str())
                {
                    return false;
                }
                index += 1;
            }
        }
        true
    }
}

/// One tool, with the kind of action it performs.
///
/// The [`ToolSchema`] says whether a tool mutates, but not whether it writes
/// a file or runs a command, and [`Policy`] needs that difference: a
/// repository may allow a write and deny an execution. The caller knows
/// which of its tools is which, so it states the kind when it registers one.
struct Registered {
    tool: Arc<dyn Tool>,
    kind: ActionKind,
}

/// The tools a turn may call, by name.
///
/// `dark-core` never depends on `dark-tools`: it holds tools through the
/// [`Tool`] trait from the contract, the same way it holds the engine
/// through `dyn Engine` (Rule 17). The caller builds this from whatever the
/// registry gated for the session (task unit `C4`), and does not change it
/// during a turn, because the schemas sit in the prefix (Rule 5).
#[derive(Default)]
pub struct ToolSet {
    tools: BTreeMap<String, Registered>,
}

impl ToolSet {
    /// Creates an empty tool set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one tool, keyed by the name in its schema.
    ///
    /// `kind` is the action the policy gates this tool on.
    #[must_use]
    pub fn with(mut self, tool: Arc<dyn Tool>, kind: ActionKind) -> Self {
        self.tools
            .insert(tool.schema().name, Registered { tool, kind });
        self
    }

    /// Looks a tool up by the name the model used.
    fn get(&self, name: &str) -> Option<&Registered> {
        self.tools.get(name)
    }

    /// Returns every schema, in a fixed order.
    ///
    /// The order never changes for a given set, because the schemas sit in
    /// the prefix and the prefix must not move during a turn. See Rule 5.
    #[must_use]
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools
            .values()
            .map(|registered| registered.tool.schema())
            .collect()
    }
}

impl std::fmt::Debug for ToolSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSet")
            .field("names", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Everything one turn needs that outlives it.
pub struct TurnCtx<'a> {
    /// The identifier that every event of this turn carries.
    pub turn: String,
    /// The engine, held as a trait object. See Rule 17.
    pub engine: &'a dyn Engine,
    /// The tools the session gated for this model.
    pub tools: &'a ToolSet,
    /// The permission policy.
    pub policy: &'a Policy,
    /// Presents a confirmation and waits for the answer.
    pub confirmer: &'a dyn Confirmer,
    /// Where the loop sends events.
    pub events: EventTx,
    /// The repository root that a tool must not leave.
    pub root: std::path::PathBuf,
    /// Whether dark mode blocks network egress.
    pub dark: bool,
    /// Whether a person can answer a question now.
    pub human_present: bool,
    /// The settings for this turn.
    pub config: TurnConfig,
}

/// Runs one turn.
///
/// `messages` is the conversation the context assembler produced: the
/// prefix, then the tail. The loop appends to it and never rewrites what is
/// already there, so the cached prefix stays valid for every round trip.
///
/// `cancel` stops the turn. A cancelled turn still returns its messages, and
/// every call it issued still has a reply.
///
/// # Errors
///
/// Returns an error when the engine fails to start a stream, or fails part
/// way through one. A failure *inside a tool* is not an error here: it
/// becomes a [`Role::Tool`] reply with the error flag set, because the model
/// must see what went wrong and the chat template must stay well formed.
pub async fn run_turn(
    ctx: &TurnCtx<'_>,
    class: RoleClass,
    mut messages: Vec<Message>,
    cancel: &CancellationToken,
) -> Result<TurnOutcome> {
    let mut produced: Vec<Message> = Vec::new();
    let mut round_trips = 0usize;
    let schemas = ctx.tools.schemas();

    loop {
        // Do step 7: at the limit, wind up rather than exit. An agent that
        // stops during an edit leaves broken files.
        let at_limit = round_trips >= ctx.config.round_trip_limit;
        if at_limit {
            let notice = Message::text(
                Role::System,
                "You reached the tool-call limit for this turn. Summarise the state now. Say \
                 what you changed, what you learned, and what remains. Do not call a tool.",
            );
            messages.push(notice.clone());
            produced.push(notice);
        }

        let request = Request {
            tools: schemas.clone(),
            tool_choice: if at_limit {
                ToolChoice::None
            } else {
                ToolChoice::Auto
            },
            ..Request::new(class, messages.clone())
        };

        let accumulated = stream_one_round(ctx, request, cancel).await?;
        round_trips += 1;

        messages.push(accumulated.message.clone());
        produced.push(accumulated.message.clone());

        // Do step 10: answer every issued call, whatever happened to it.
        let cancelled = accumulated.finish == FinishReason::Cancelled || cancel.is_cancelled();
        for reply in answer_all(ctx, &accumulated.calls, cancelled, cancel).await {
            messages.push(reply.clone());
            produced.push(reply);
        }

        let end = if cancelled {
            Some(TurnEnd::Cancelled)
        } else if at_limit {
            Some(TurnEnd::LimitReached)
        } else if accumulated.finish == FinishReason::ToolCalls && !accumulated.calls.is_empty() {
            // Do step 6: go round again on the same model.
            None
        } else {
            Some(TurnEnd::Stopped)
        };

        if let Some(end) = end {
            return Ok(TurnOutcome {
                messages: produced,
                end,
                round_trips,
            });
        }
    }
}

/// Runs one round trip and folds its chunks into one message.
///
/// Forwards token and reasoning deltas to the event bus as they arrive, so
/// the terminal application renders them while the model is still
/// generating.
async fn stream_one_round(
    ctx: &TurnCtx<'_>,
    request: Request,
    cancel: &CancellationToken,
) -> Result<Accumulated> {
    let mut stream = ctx.engine.stream(request, cancel.clone()).await?;
    let mut acc = Accumulator::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        match &chunk {
            dark_contract::Chunk::Text(text) => ctx.events.send(Event::TokenDelta {
                turn: ctx.turn.clone(),
                text: text.clone(),
            }),
            dark_contract::Chunk::Reasoning(text) => ctx.events.send(Event::ReasonDelta {
                turn: ctx.turn.clone(),
                text: text.clone(),
            }),
            dark_contract::Chunk::ModelLoading { model, progress } => {
                ctx.events.send(Event::ModelLoading {
                    model: model.clone(),
                    progress: *progress,
                });
            }
            _ => {}
        }
        acc.push(&chunk);
        if acc.is_done() {
            break;
        }
    }

    Ok(acc.finish(cancel.is_cancelled()))
}

/// Produces one [`Role::Tool`] reply for each issued call, in order.
///
/// The returned vector is always the same length as `calls`. No path through
/// this function returns early, and no path skips a call. That is what makes
/// Do step 10 hold rather than merely be intended.
async fn answer_all(
    ctx: &TurnCtx<'_>,
    calls: &[IssuedCall],
    cancelled: bool,
    cancel: &CancellationToken,
) -> Vec<Message> {
    let mut replies = Vec::with_capacity(calls.len());

    for issued in calls {
        ctx.events.send(Event::ToolCall {
            turn: ctx.turn.clone(),
            call: issued.call.clone(),
        });

        // Re-check on every call, not once for the round: a tool can
        // cancel the turn itself, and every call after it must still get a
        // reply. Do steps 9 and 10.
        let result = if cancelled || cancel.is_cancelled() {
            ToolResult::error("The person cancelled this turn, so this call did not run.")
        } else {
            run_one_call(ctx, issued, cancel).await
        };

        ctx.events.send(Event::ToolResult {
            turn: ctx.turn.clone(),
            call_id: issued.call.id.clone(),
            result: ToolResultSummary {
                name: issued.call.name.clone(),
                is_error: result.is_error,
                bytes: result.content.len(),
                headline: result.content.lines().next().unwrap_or_default().to_owned(),
                has_diff: result.diff.is_some(),
            },
        });

        let mut reply = Message::tool_reply(issued.call.id.clone(), result.content);
        if let Some(diff) = result.diff {
            reply.parts.push(Part::Text(diff));
        }
        replies.push(reply);
    }

    debug_assert_eq!(
        replies.len(),
        calls.len(),
        "every issued call must have exactly one reply"
    );
    replies
}

/// Checks the policy, then runs one tool.
///
/// Every failure path produces a [`ToolResult`], never an `Err`: the caller
/// must be able to answer the call whatever happened to it.
async fn run_one_call(
    ctx: &TurnCtx<'_>,
    issued: &IssuedCall,
    cancel: &CancellationToken,
) -> ToolResult {
    if !issued.args_parsed {
        // Name what to do about it. A small model recovers from an
        // instruction; it does not recover from "invalid arguments".
        return ToolResult::error(format!(
            "The arguments for `{}` were not valid JSON. Send them again as one JSON object.",
            issued.call.name
        ));
    }

    let Some(registered) = ctx.tools.get(&issued.call.name) else {
        return ToolResult::error(format!(
            "There is no tool named `{}`. Call a tool from the list you were given.",
            issued.call.name
        ));
    };

    // Do step 5.1 and 5.2: check the policy, and wait for the person when it
    // asks for a confirmation.
    let action = action_for(registered.kind, &issued.call.name, &issued.call.args, ctx);
    match ctx.policy.decide(&action, ctx.confirmer).await {
        Decision::Allow => {}
        Decision::Denied(error) => return ToolResult::error(error.to_string()),
        Decision::NeedsConfirmation(_) => {
            // Policy::decide resolves a confirmation before it returns, so
            // this arm is unreachable. Answer the call rather than panic:
            // an unanswered call breaks the chat template.
            return ToolResult::error(
                "The harness could not resolve the confirmation for this call.",
            );
        }
    }

    let tool_ctx = ToolCtx {
        root: ctx.root.clone(),
        events: ctx.events.clone(),
        cancel: cancel.clone(),
        dark: ctx.dark,
        human_present: ctx.human_present,
    };

    // Do step 5.3: apply a timeout.
    match tokio::time::timeout(
        ctx.config.tool_timeout,
        registered.tool.invoke(issued.call.args.clone(), &tool_ctx),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(error)) => ToolResult::error(error.to_string()),
        Err(_) => ToolResult::error(
            Error::new(
                ErrCode::ToolTimeout,
                format!(
                    "`{}` did not finish inside {} seconds.",
                    issued.call.name,
                    ctx.config.tool_timeout.as_secs()
                ),
            )
            .to_string(),
        ),
    }
}

/// Builds the [`Action`] that [`Policy`] judges one call by.
///
/// The exact arguments go into the action, never a summary, because that is
/// what a person sees before they approve it. See Do step 3 of task unit
/// `A4`, and the note on the confirmation gap in the module documentation.
fn action_for(kind: ActionKind, name: &str, args: &serde_json::Value, ctx: &TurnCtx<'_>) -> Action {
    match kind {
        ActionKind::Read => Action::Read {
            what: format!("{name} {args}"),
        },
        ActionKind::Exec => Action::Exec {
            command: format!("{name} {args}"),
            cwd: ctx.root.clone(),
            // The command tool takes `shell` as an argument, and a shell
            // command needs the louder confirmation. See task unit `C3`.
            shell: args
                .get("shell")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        },
        ActionKind::Write => Action::Write {
            path: args
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map_or_else(|| ctx.root.clone(), |path| ctx.root.join(path)),
            // The diff is not available before the tool runs. See the note
            // on the confirmation gap in the module documentation.
            diff: format!("{name} {args}"),
            // The tool itself refuses a path outside the root (Rule 34), and
            // this loop cannot resolve a symbolic link without touching the
            // file system. Denial stays with the tool.
            outside_root: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dark_contract::ToolCall;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.to_owned(),
            name: "read_file".to_owned(),
            args: serde_json::json!({}),
        }
    }

    fn asks_for(ids: &[&str]) -> Message {
        let mut message = Message::text(Role::Assistant, "calling");
        message.tool_calls = ids.iter().map(|id| call(id)).collect();
        message
    }

    fn outcome(messages: Vec<Message>) -> TurnOutcome {
        TurnOutcome {
            messages,
            end: TurnEnd::Stopped,
            round_trips: 1,
        }
    }

    #[test]
    fn a_call_followed_by_its_reply_is_well_formed() {
        let history = outcome(vec![asks_for(&["a"]), Message::tool_reply("a", "done")]);
        assert!(history.history_is_well_formed());
    }

    #[test]
    fn a_call_with_no_reply_is_not_well_formed() {
        let history = outcome(vec![asks_for(&["a"])]);
        assert!(
            !history.history_is_well_formed(),
            "an unanswered call must be reported, or the check is worthless"
        );
    }

    #[test]
    fn a_reply_for_the_wrong_call_is_not_well_formed() {
        let history = outcome(vec![asks_for(&["a"]), Message::tool_reply("b", "done")]);
        assert!(!history.history_is_well_formed());
    }

    #[test]
    fn replies_out_of_order_are_not_well_formed() {
        let history = outcome(vec![
            asks_for(&["a", "b"]),
            Message::tool_reply("b", "done"),
            Message::tool_reply("a", "done"),
        ]);
        assert!(
            !history.history_is_well_formed(),
            "a chat template renders replies in the order the calls were made"
        );
    }

    #[test]
    fn a_missing_reply_in_the_middle_is_not_well_formed() {
        let history = outcome(vec![
            asks_for(&["a", "b"]),
            Message::tool_reply("a", "done"),
            Message::text(Role::Assistant, "carrying on"),
        ]);
        assert!(!history.history_is_well_formed());
    }

    #[test]
    fn an_engine_that_reuses_an_identifier_between_rounds_is_well_formed() {
        // Nothing forbids an engine from numbering its calls from zero each
        // round. Pairing by position rather than by identifier accepts that.
        let history = outcome(vec![
            asks_for(&["c0"]),
            Message::tool_reply("c0", "first round"),
            asks_for(&["c0"]),
            Message::tool_reply("c0", "second round"),
        ]);
        assert!(history.history_is_well_formed());
    }

    #[test]
    fn a_turn_with_no_calls_at_all_is_well_formed() {
        let history = outcome(vec![Message::text(Role::Assistant, "no tools needed")]);
        assert!(history.history_is_well_formed());
    }

    #[test]
    fn the_cpu_profile_lowers_the_round_trip_limit() {
        assert_eq!(TurnConfig::for_cpu().round_trip_limit, CPU_ROUND_TRIP_LIMIT);
        assert!(TurnConfig::for_cpu().round_trip_limit < DEFAULT_ROUND_TRIP_LIMIT);
    }
}
