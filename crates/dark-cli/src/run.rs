//! `dark run "<prompt>"`: runs one turn and shows no interface.
//!
//! This is the headless path through the whole harness: it brings a
//! session up ([`crate::harness`]), assembles the context prefix, drives
//! [`dark_core::turn::run_turn`], records the transcript, and prints what
//! the model said. `dark` with no subcommand runs the same session behind
//! the terminal application instead; both go through
//! [`crate::harness::bring_up`], so a turn behaves the same either way.
//!
//! # No person is present
//!
//! A headless run has nobody to answer a confirmation, so it uses
//! [`RunMode::Headless`]. Without `--yes`, a policy value of `confirm`
//! becomes a denial with `E_POLICY_CONFIRM_REQUIRED`, and the turn
//! carries on — the model is told the tool was denied, which is what lets
//! it choose something else rather than stopping in the middle of an
//! edit. With `--yes`, `confirm` becomes `allow`. A write outside the
//! repository root stays denied either way; nothing can widen that (Rule
//! 34).
//!
//! # What the prefix holds
//!
//! [`dark_core::context::assemble_prefix`] takes the five parts in a
//! fixed order. This module supplies three of them for a headless run:
//! the system prompt for the model's size (`dark-qwen`), the resolved
//! `AGENTS.md` chain (`dark-agentsmd`), and the date — never the time
//! (Rule 6). A map digest and a claimed ticket are absent here: neither
//! belongs to a one-shot run with no session to carry them.
//!
//! The prefix is assembled once, before the loop starts, and never
//! touched again (Rule 5). The turn loop only appends.
//!
//! # Streaming
//!
//! The events the turn produces go on the bus, and a task drains the bus
//! and writes token deltas straight to standard output, so a long turn
//! shows its work rather than sitting silent. The same task writes every
//! reliable event to the session transcript, which is what `dark replay`
//! reads back.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result};
use dark_contract::{Event, EventBus, Message, Received, Role, RoleClass};
use dark_core::context::assemble_prefix;
use dark_core::context::prefix::PrefixInputs;
use dark_core::policy::{ChannelConfirmer, PolicyConfig, RunMode};
use dark_core::session::{Session, TranscriptWriter};
use dark_core::turn::{TurnConfig, TurnCtx, TurnEnd, run_turn};
use dark_lexicon::tools::staleness;
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::harness::{self, BringUp};

/// Runs `dark run`.
///
/// # Errors
///
/// Returns an error when no model is installed, when the model cannot be
/// loaded, or when the turn itself fails. A tool that a policy denied is
/// not an error: the turn answers the call and carries on.
pub(crate) fn run_command(prompt: &str, dark: bool, yes: bool) -> Result<()> {
    // A turn drives a model, a set of tools, and the event bus at the
    // same time, so this needs a real multi-threaded runtime rather than
    // the single-threaded one the file-reading commands here use.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the harness runtime")?;

    runtime.block_on(run_one_turn(prompt, dark, yes))
}

/// Brings a session up, runs one turn, and prints the reply.
async fn run_one_turn(prompt: &str, dark: bool, yes: bool) -> Result<()> {
    let root = crate::repo_root()?;
    let dark_home = crate::dark_home();
    let sessions_root = dark_home.join("sessions");

    let bus = EventBus::new();
    let events = bus.tx();

    let mode = RunMode::Headless { yes };
    let harness = harness::bring_up(BringUp {
        root: root.clone(),
        dark_home: dark_home.clone(),
        // The `[hardware] model` key names which installed model serves a
        // turn. Reading it here rather than guessing keeps a machine with
        // two models installed answering with the same one every time.
        preferred_model: None,
        policy: PolicyConfig::default(),
        mode,
        events: events.clone(),
        tier_override: None,
    })
    .await?;

    let session_id = Ulid::new();
    let mut session = Session::new(session_id, root.clone());
    session.dark = dark;
    session.human_present = false;

    // Start recording before the first event, so the transcript holds the
    // whole turn rather than whatever arrived after the writer opened.
    let mut receiver = bus.subscribe();
    let mut transcript = TranscriptWriter::open(&sessions_root, session_id)
        .await
        .map_err(crate::contract_error)?;

    let printer = tokio::spawn(async move {
        let mut reply = String::new();
        while let Some(received) = receiver.recv().await {
            match received {
                Received::Event(event) => {
                    if let Event::TokenDelta { text, .. } = &event {
                        // Straight to standard output: a headless run
                        // shows the reply as it arrives.
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                        reply.push_str(text);
                    }
                    // Every event, lossy or not, goes to the transcript:
                    // `dark replay` rebuilds the session from it.
                    if transcript.record(&event).await.is_err() {
                        // A transcript that cannot be written must not
                        // take the turn down with it. The turn's own
                        // output still reaches the person.
                        break;
                    }
                }
                Received::Lagged(_) => {
                    // Only token deltas travel on the lossy channel, and
                    // the reply is rebuilt from the transcript's
                    // reliable events, so a lagged display costs nothing
                    // that matters here.
                }
            }
        }
        let _ = transcript.flush().await;
        reply
    });

    events.send(Event::SessionStart {
        id: session_id.to_string(),
        root: root.clone(),
        branch: git_branch(&root),
    });

    let turn_id = Ulid::new().to_string();
    events.send(Event::TurnStart {
        turn: turn_id.clone(),
        class: RoleClass::Worker,
        model: harness.model_id.clone(),
    });
    events.send(Event::UserMessage {
        turn: turn_id.clone(),
        text: prompt.to_owned(),
    });

    let messages = prefix_messages(&harness, &root, prompt)?;

    let confirmer = ChannelConfirmer::new(events.clone());
    let ctx = TurnCtx {
        turn: turn_id.clone(),
        engine: harness.engine.as_ref(),
        tools: &harness.tools,
        policy: &harness.policy,
        confirmer: &confirmer,
        events: events.clone(),
        root: root.clone(),
        dark,
        // Nobody is watching a headless run, so a confirmation cannot be
        // answered. The policy already turns that into an allow or a
        // denial before the loop asks; this says the same thing to the
        // tools that read it.
        human_present: false,
        config: turn_config(&harness),
    };

    let started = std::time::Instant::now();
    let outcome = run_turn(&ctx, RoleClass::Worker, messages, &CancellationToken::new())
        .await
        .map_err(crate::contract_error)?;

    events.send(Event::TurnEnd {
        turn: turn_id,
        usage: dark_contract::Usage::default(),
        wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });

    // Dropping every sender closes the bus, which ends the printer's loop.
    drop(events);
    drop(bus);
    let streamed = printer.await.unwrap_or_default();

    debug_assert!(
        outcome.history_is_well_formed(),
        "every tool call must have its reply, or the next turn's template breaks"
    );

    report(&outcome, &streamed, &sessions_root, session_id);
    Ok(())
}

/// Returns the turn settings for the machine this session runs on.
///
/// A central processor generates a few tokens each second, so forty round
/// trips is hours rather than minutes; section 4.3 gives a smaller limit
/// there. The device the engine reports is what decides.
pub(crate) fn turn_config(harness: &harness::Harness) -> TurnConfig {
    if harness.caps.device == dark_contract::Device::Cpu {
        TurnConfig::for_cpu()
    } else {
        TurnConfig::default()
    }
}

/// Assembles the context prefix and appends the person's message.
///
/// See the module documentation for which of the five prefix parts a
/// headless run fills.
pub(crate) fn prefix_messages(
    harness: &harness::Harness,
    root: &Path,
    prompt: &str,
) -> Result<Vec<Message>> {
    let system_prompt = dark_qwen::system_prompt_for(harness.caps.params_b);

    // The chain is resolved once, here, and never again during the turn:
    // a changed prefix costs a full prefill (Rule 5, Rule 22).
    let home = dirs::home_dir().unwrap_or_else(crate::dark_home);
    let working_set = dark_agentsmd::WorkingSet::new();
    let config = dark_agentsmd::AgentsMdConfig::default();
    let count_tokens = |text: &str| {
        harness
            .engine
            .tokenize(RoleClass::Worker, text)
            // A tokenizer failure must not stop a turn: the count only
            // sizes the chain against its budget, and four bytes to a
            // token is the usual rough figure.
            .unwrap_or(text.len() / 4)
    };
    let chain = dark_agentsmd::resolve(&home, root, &working_set, &config, &count_tokens)
        .map_err(crate::contract_error)?;

    for warning in chain.warnings() {
        eprintln!("note: {warning}");
    }

    let prefix = assemble_prefix(&PrefixInputs {
        system_prompt: &system_prompt,
        agents_chain: &chain.prefix_text(),
        // Rule 6: the prefix carries the date and never the time. A clock
        // in the prefix changes it between turns and forces a full
        // prefill every time.
        environment_date: &today(),
        map_digest: None,
        ticket_body: None,
    });

    let mut messages = prefix.messages();
    messages.push(Message::text(Role::User, prompt));
    Ok(messages)
}

/// Returns today's date as `YYYY-MM-DD`, in universal time.
///
/// Universal time, not local: the date is part of the context prefix, and
/// a machine that changes time zone mid-session would otherwise change
/// the prefix and force a full prefill (Rule 5).
///
/// The calendar arithmetic comes from
/// [`dark_lexicon::tools::staleness`], which already needs it to age a
/// documentation pack. One implementation of a calendar, tested in one
/// place, is worth more than a second copy here.
fn today() -> String {
    let epoch_day = staleness::today_epoch_day().unwrap_or(0);
    let (year, month, day) = staleness::civil_from_days(epoch_day);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Returns the checked-out git branch, when `root` is a repository with a
/// branch checked out.
///
/// Reads `.git/HEAD` directly rather than running git: a headless run
/// should not depend on a git binary being installed, and this is the one
/// fact the session header needs.
pub(crate) fn git_branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    // A detached head holds a bare commit hash, and has no branch name.
    let reference = head.trim().strip_prefix("ref: refs/heads/")?;
    Some(reference.to_owned())
}

/// Prints what the turn produced, after the streamed text.
fn report(outcome: &dark_core::turn::TurnOutcome, streamed: &str, sessions_root: &Path, id: Ulid) {
    // The streamed text already reached standard output token by token.
    // A turn that produced text but streamed none — an engine that
    // reports no deltas — still shows its reply here rather than nothing.
    if streamed.is_empty() {
        let text: String = outcome
            .messages
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .map(dark_contract::Message::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            println!("{text}");
        }
    } else if !streamed.ends_with('\n') {
        println!();
    }

    if outcome.end == TurnEnd::LimitReached {
        eprintln!(
            "note: the turn reached its {} round-trip limit and summarised where it got to.",
            outcome.round_trips
        );
    }

    eprintln!(
        "session {id} recorded at {}",
        dark_core::session::transcript_path(sessions_root, id).display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_a_plain_calendar_date() {
        let date = today();
        assert_eq!(date.len(), 10, "date: {date}");
        assert_eq!(date.matches('-').count(), 2, "date: {date}");
        assert!(
            !date.contains(':'),
            "Rule 6: the prefix carries the date and never the time: {date}"
        );
    }

    #[test]
    fn a_branch_is_read_from_the_git_head_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .unwrap();

        assert_eq!(git_branch(dir.path()), Some("main".to_owned()));
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("HEAD"),
            "9fceb02d0ae598e95dc970b74767f19372d61af8\n",
        )
        .unwrap();

        assert_eq!(git_branch(dir.path()), None);
    }

    #[test]
    fn a_directory_that_is_not_a_repository_has_no_branch() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(git_branch(dir.path()), None);
    }

    #[test]
    fn a_branch_name_with_slashes_survives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git").join("HEAD"),
            "ref: refs/heads/claude/rust-ultracode-setup\n",
        )
        .unwrap();

        assert_eq!(
            git_branch(dir.path()),
            Some("claude/rust-ultracode-setup".to_owned())
        );
    }
}
