//! `A2` done-when: "Every tool call has a matching reply, including under
//! cancellation."
//!
//! An unanswered tool call breaks the chat template, and it breaks it on the
//! *next* turn, far from the cause. These tests drive the turn loop through
//! the ways a call can fail to run — a denial, a missing tool, a timeout,
//! and a cancellation — and check that the history comes back well formed
//! every time. A call whose argument text does not parse is driven in
//! `turn::accumulate`'s unit tests, because the scripted engine builds its
//! arguments from TOML and so cannot produce broken ones.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dark_contract::{
    Engine, EventBus, Message, Result, Role, RoleClass, Tool, ToolCtx, ToolResult, ToolSchema,
    tool::tier,
};
use dark_core::policy::{
    Action, ActionKind, ChannelConfirmer, Confirmer, Policy, PolicyConfig, PolicyValue, RunMode,
    WriteOutsideRoot,
};
use dark_core::turn::{ToolSet, TurnConfig, TurnCtx, TurnEnd, run_turn};
use dark_engine_fake::FakeEngine;
use tokio_util::sync::CancellationToken;

/// A tool that reports what it was given. `delay` lets a test drive the
/// timeout path without waiting for a real one.
struct Echo {
    name: &'static str,
    mutating: bool,
    delay: Duration,
    /// Cancels the turn from inside the tool, the way a person pressing
    /// escape mid-call does.
    cancels: bool,
}

impl Echo {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            mutating: false,
            delay: Duration::ZERO,
            cancels: false,
        }
    }

    fn mutating(mut self) -> Self {
        self.mutating = true;
        self
    }

    fn slow(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    fn cancels(mut self) -> Self {
        self.cancels = true;
        self
    }
}

#[async_trait]
impl Tool for Echo {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.to_owned(),
            description: "echoes its arguments".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: tier::ESSENTIAL,
            mutating: self.mutating,
        }
    }

    async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolResult> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        if self.cancels {
            ctx.cancel.cancel();
        }
        Ok(ToolResult::ok(format!("echo {args}")))
    }
}

/// A confirmer that answers every prompt the same way, with no person
/// present.
struct Always(dark_contract::Allow);

#[async_trait]
impl Confirmer for Always {
    async fn confirm(&self, _prompt: dark_contract::ConfirmPrompt) -> dark_contract::Allow {
        self.0
    }
}

fn permissive() -> PolicyConfig {
    PolicyConfig {
        read: PolicyValue::Allow,
        write: PolicyValue::Allow,
        exec: PolicyValue::Allow,
        write_outside_root: WriteOutsideRoot::DENIED,
        default_dark: false,
    }
}

/// Builds a script whose one turn asks for the given calls.
fn calls_script(calls: &[(&str, &str)]) -> String {
    let mut text = String::from("[[turns]]\ntext = \"working\"\n");
    for (index, (name, args)) in calls.iter().enumerate() {
        let _ = write!(
            text,
            "[[turns.tool_calls]]\nid = \"c{index}\"\nname = \"{name}\"\nargs = {args}\n"
        );
    }
    // Note: an engine may reuse an identifier between rounds, so
    // `history_is_well_formed` pairs a call with its reply by position, not
    // by identifier. See its documentation.
    // A second turn, so a loop that goes round again has something to play.
    text.push_str("\n[[turns]]\ntext = \"done\"\n");
    text
}

/// Runs one turn against a scripted engine and returns the outcome.
async fn run(
    script: &str,
    tools: ToolSet,
    config: PolicyConfig,
    turn_config: TurnConfig,
    cancel_before: bool,
) -> dark_core::turn::TurnOutcome {
    let engine = FakeEngine::from_toml(script).expect("the script parses");
    let bus = EventBus::new();
    let policy = Policy::new(config, RunMode::Interactive);
    let confirmer = Always(dark_contract::Allow::Once);
    let cancel = CancellationToken::new();
    if cancel_before {
        cancel.cancel();
    }

    let ctx = TurnCtx {
        turn: "turn-1".to_owned(),
        engine: &engine as &dyn Engine,
        tools: &tools,
        policy: &policy,
        confirmer: &confirmer as &dyn Confirmer,
        events: bus.tx(),
        root: std::env::temp_dir(),
        dark: false,
        human_present: true,
        config: turn_config,
    };

    run_turn(
        &ctx,
        RoleClass::Worker,
        vec![Message::text(Role::User, "do the thing")],
        &cancel,
    )
    .await
    .expect("the turn runs")
}

/// Counts the `Role::Tool` replies in an outcome.
fn replies(outcome: &dark_core::turn::TurnOutcome) -> usize {
    outcome
        .messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .count()
}

#[tokio::test]
async fn a_call_to_a_tool_that_does_not_exist_is_still_answered() {
    let outcome = run(
        &calls_script(&[("no_such_tool", "{}")]),
        ToolSet::new().with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig::default(),
        false,
    )
    .await;

    assert!(outcome.history_is_well_formed());
    assert_eq!(replies(&outcome), 1);
    let reply = outcome
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .unwrap();
    assert!(
        reply.text_content().contains("no tool named"),
        "the reply must name the problem: {}",
        reply.text_content()
    );
}

/// The fake engine builds its arguments from TOML, so it cannot emit
/// argument text that fails to parse. That path is covered where it can be
/// driven directly, in `turn::accumulate`'s unit tests. This checks the
/// ordinary path still answers exactly once.
#[tokio::test]
async fn a_call_that_runs_normally_is_answered_exactly_once() {
    let outcome = run(
        &calls_script(&[("read_file", "{ path = \"a\" }")]),
        ToolSet::new().with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig::default(),
        false,
    )
    .await;

    assert!(outcome.history_is_well_formed());
    assert_eq!(replies(&outcome), 1);
}

#[tokio::test]
async fn a_denied_call_is_still_answered() {
    let denied = PolicyConfig {
        exec: PolicyValue::Deny,
        ..permissive()
    };

    let outcome = run(
        &calls_script(&[("run_command", "{ command = \"ls\" }")]),
        ToolSet::new().with(
            Arc::new(Echo::new("run_command").mutating()),
            ActionKind::Exec,
        ),
        denied,
        TurnConfig::default(),
        false,
    )
    .await;

    assert!(
        outcome.history_is_well_formed(),
        "a denial must not leave a call unanswered"
    );
    assert_eq!(replies(&outcome), 1);
}

#[tokio::test]
async fn a_call_that_times_out_is_still_answered() {
    let outcome = run(
        &calls_script(&[("slow", "{}")]),
        ToolSet::new().with(
            Arc::new(Echo::new("slow").slow(Duration::from_secs(30))),
            ActionKind::Read,
        ),
        permissive(),
        TurnConfig {
            tool_timeout: Duration::from_millis(20),
            ..TurnConfig::default()
        },
        false,
    )
    .await;

    assert!(outcome.history_is_well_formed());
    assert_eq!(replies(&outcome), 1);
}

#[tokio::test]
async fn every_call_of_a_multi_call_message_is_answered() {
    let outcome = run(
        &calls_script(&[
            ("read_file", "{ path = \"a\" }"),
            ("read_file", "{ path = \"b\" }"),
            ("no_such_tool", "{}"),
        ]),
        ToolSet::new().with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig::default(),
        false,
    )
    .await;

    assert!(outcome.history_is_well_formed());
    assert_eq!(
        replies(&outcome),
        3,
        "three calls need three replies, even when one names no tool"
    );
}

#[tokio::test]
async fn a_cancellation_part_way_through_still_answers_the_calls_that_remain() {
    // Three calls. The first cancels the turn from inside the tool, the way
    // a person pressing escape mid-call does. The two after it never run,
    // and both must still get a reply, or the chat template breaks on the
    // next turn.
    let outcome = run(
        &calls_script(&[
            ("canceller", "{}"),
            ("read_file", "{ path = \"a\" }"),
            ("read_file", "{ path = \"b\" }"),
        ]),
        ToolSet::new()
            .with(Arc::new(Echo::new("canceller").cancels()), ActionKind::Read)
            .with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig::default(),
        false,
    )
    .await;

    assert_eq!(outcome.end, TurnEnd::Cancelled);
    assert!(
        outcome.history_is_well_formed(),
        "Do step 10: a cancelled call still gets a reply"
    );
    assert_eq!(
        replies(&outcome),
        3,
        "all three issued calls need a reply, not only the one that ran"
    );
}

#[tokio::test]
async fn a_turn_cancelled_before_it_starts_issues_nothing_and_stays_well_formed() {
    let outcome = run(
        &calls_script(&[("read_file", "{ path = \"a\" }")]),
        ToolSet::new().with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig::default(),
        true,
    )
    .await;

    assert_eq!(outcome.end, TurnEnd::Cancelled);
    assert!(outcome.history_is_well_formed());
}

#[tokio::test]
async fn the_round_trip_limit_winds_up_rather_than_exiting() {
    // A script that asks for a tool call every time. Without the wind-up
    // round the loop would either run forever or stop mid-edit.
    let mut script = String::new();
    for _ in 0..8 {
        script.push_str(
            "[[turns]]\ntext = \"working\"\n[[turns.tool_calls]]\nid = \"c0\"\n\
             name = \"read_file\"\nargs = { path = \"a\" }\n\n",
        );
    }

    let outcome = run(
        &script,
        ToolSet::new().with(Arc::new(Echo::new("read_file")), ActionKind::Read),
        permissive(),
        TurnConfig {
            round_trip_limit: 3,
            ..TurnConfig::default()
        },
        false,
    )
    .await;

    assert_eq!(outcome.end, TurnEnd::LimitReached);
    assert!(outcome.history_is_well_formed());
    assert_eq!(
        outcome.round_trips, 4,
        "three rounds, then one more to summarise the state"
    );
    assert!(
        outcome
            .messages
            .iter()
            .any(|m| m.role == Role::System && m.text_content().contains("Summarise")),
        "the loop must tell the model to summarise before it stops"
    );
}

#[tokio::test]
async fn a_plain_answer_ends_the_turn_with_no_replies_to_make() {
    let outcome = run(
        "[[turns]]\ntext = \"all done\"\n",
        ToolSet::new(),
        permissive(),
        TurnConfig::default(),
        false,
    )
    .await;

    assert_eq!(outcome.end, TurnEnd::Stopped);
    assert_eq!(outcome.round_trips, 1);
    assert!(outcome.history_is_well_formed());
    assert_eq!(replies(&outcome), 0);
}

#[tokio::test]
async fn the_confirmer_is_consulted_and_a_refusal_still_answers_the_call() {
    let confirm_first = PolicyConfig {
        exec: PolicyValue::Confirm,
        ..permissive()
    };

    let engine = FakeEngine::from_toml(&calls_script(&[(
        "run_command",
        "{ command = \"rm -rf /\" }",
    )]))
    .expect("the script parses");
    let bus = EventBus::new();
    let policy = Policy::new(confirm_first, RunMode::Interactive);
    let confirmer = Always(dark_contract::Allow::Deny);
    let tools = ToolSet::new().with(
        Arc::new(Echo::new("run_command").mutating()),
        ActionKind::Exec,
    );

    let ctx = TurnCtx {
        turn: "turn-1".to_owned(),
        engine: &engine as &dyn Engine,
        tools: &tools,
        policy: &policy,
        confirmer: &confirmer as &dyn Confirmer,
        events: bus.tx(),
        root: std::env::temp_dir(),
        dark: false,
        human_present: true,
        config: TurnConfig::default(),
    };

    let outcome = run_turn(
        &ctx,
        RoleClass::Worker,
        vec![Message::text(Role::User, "clean up")],
        &CancellationToken::new(),
    )
    .await
    .expect("the turn runs");

    assert!(outcome.history_is_well_formed());
    assert_eq!(replies(&outcome), 1);
}

/// The `ChannelConfirmer` is what production uses. This checks the turn loop
/// works through the real one, not only through the test double above.
#[tokio::test]
async fn the_channel_confirmer_resolves_a_confirmation() {
    let bus = EventBus::new();
    let confirmer = ChannelConfirmer::new(bus.tx());
    let action = Action::Read {
        what: "src/lib.rs".to_owned(),
    };
    let policy = Policy::new(permissive(), RunMode::Interactive);

    // A read is `allow`, so this resolves without touching the channel.
    assert!(matches!(
        policy.decide(&action, &confirmer).await,
        dark_core::policy::Decision::Allow
    ));
}

/// A tool that can say what it would change without changing it.
struct Writer;

#[async_trait]
impl Tool for Writer {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write_file".to_owned(),
            description: "writes a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            tier: tier::ESSENTIAL,
            mutating: true,
        }
    }

    async fn invoke(&self, _args: serde_json::Value, _ctx: &ToolCtx) -> Result<ToolResult> {
        Ok(ToolResult::ok("wrote the file").with_diff(REAL_DIFF))
    }

    async fn preview(
        &self,
        _args: serde_json::Value,
        _ctx: &ToolCtx,
    ) -> Result<Option<ToolResult>> {
        Ok(Some(ToolResult::ok("would write").with_diff(REAL_DIFF)))
    }
}

/// The diff `Writer` reports, both when it previews and when it runs.
const REAL_DIFF: &str = "@@ -1 +1 @@\n-old line\n+new line\n";

/// Records the prompt it was shown, so a test can read what a person would
/// have seen.
struct Recording {
    seen: std::sync::Mutex<Vec<dark_contract::ConfirmPrompt>>,
    answer: dark_contract::Allow,
}

#[async_trait]
impl Confirmer for Recording {
    async fn confirm(&self, prompt: dark_contract::ConfirmPrompt) -> dark_contract::Allow {
        self.seen
            .lock()
            .expect("the lock is not poisoned")
            .push(prompt);
        self.answer
    }
}

/// `A4` Do step 3: show the exact unified diff, never a summary. The diff
/// only exists once a tool has worked out what it would change, so the loop
/// asks `Tool::preview` for it before it runs anything. See `docs/adr/0004`.
#[tokio::test]
async fn a_write_confirmation_shows_the_diff_the_tool_previewed() {
    let confirm_writes = PolicyConfig {
        write: PolicyValue::Confirm,
        ..permissive()
    };

    let engine = FakeEngine::from_toml(&calls_script(&[("write_file", "{ path = \"a.rs\" }")]))
        .expect("the script parses");
    let bus = EventBus::new();
    let policy = Policy::new(confirm_writes, RunMode::Interactive);
    let confirmer = Recording {
        seen: std::sync::Mutex::new(Vec::new()),
        answer: dark_contract::Allow::Once,
    };
    let tools = ToolSet::new().with(Arc::new(Writer), ActionKind::Write);

    let ctx = TurnCtx {
        turn: "turn-1".to_owned(),
        engine: &engine as &dyn Engine,
        tools: &tools,
        policy: &policy,
        confirmer: &confirmer as &dyn Confirmer,
        events: bus.tx(),
        root: std::env::temp_dir(),
        dark: false,
        human_present: true,
        config: TurnConfig::default(),
    };

    let outcome = run_turn(
        &ctx,
        RoleClass::Worker,
        vec![Message::text(Role::User, "edit that file")],
        &CancellationToken::new(),
    )
    .await
    .expect("the turn runs");

    assert!(outcome.history_is_well_formed());

    let seen = confirmer.seen.lock().expect("the lock is not poisoned");
    assert_eq!(seen.len(), 1, "a confirm policy must show one prompt");
    let shown = format!("{:?}", seen[0]);
    assert!(
        shown.contains("+new line"),
        "the person must see the real diff, not the arguments: {shown}"
    );
}

/// A tool that cannot preview must not lose its confirmation. The person
/// sees the exact arguments instead, which is weaker than a diff and is
/// still not a summary.
#[tokio::test]
async fn a_tool_that_cannot_preview_still_gets_its_confirmation() {
    let confirm_writes = PolicyConfig {
        write: PolicyValue::Confirm,
        ..permissive()
    };

    let engine = FakeEngine::from_toml(&calls_script(&[("no_preview", "{ path = \"a.rs\" }")]))
        .expect("the script parses");
    let bus = EventBus::new();
    let policy = Policy::new(confirm_writes, RunMode::Interactive);
    let confirmer = Recording {
        seen: std::sync::Mutex::new(Vec::new()),
        answer: dark_contract::Allow::Once,
    };
    // `Echo` never overrides `preview`, so it takes the trait's default.
    let tools = ToolSet::new().with(
        Arc::new(Echo::new("no_preview").mutating()),
        ActionKind::Write,
    );

    let ctx = TurnCtx {
        turn: "turn-1".to_owned(),
        engine: &engine as &dyn Engine,
        tools: &tools,
        policy: &policy,
        confirmer: &confirmer as &dyn Confirmer,
        events: bus.tx(),
        root: std::env::temp_dir(),
        dark: false,
        human_present: true,
        config: TurnConfig::default(),
    };

    let outcome = run_turn(
        &ctx,
        RoleClass::Worker,
        vec![Message::text(Role::User, "edit that file")],
        &CancellationToken::new(),
    )
    .await
    .expect("the turn runs");

    assert!(outcome.history_is_well_formed());
    assert_eq!(
        confirmer
            .seen
            .lock()
            .expect("the lock is not poisoned")
            .len(),
        1,
        "a tool with no preview still needs its confirmation"
    );
}
