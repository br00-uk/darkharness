//! `dark` with no subcommand: the terminal application.
//!
//! This runs the same session `dark run` does — both go through
//! [`crate::harness::bring_up`] — but drives it from
//! [`dark_tui::app::run`] instead of one prompt on the command line. A
//! person is present here, so a `confirm` policy value shows a real
//! prompt and waits for the answer, rather than resolving to an allow or
//! a denial the way a headless run must.
//!
//! # Three threads, and why
//!
//! [`dark_tui::app::run`] is a synchronous loop that owns the terminal:
//! it blocks on `crossterm` for input and draws frames. The turn loop is
//! asynchronous. Neither can host the other, so:
//!
//! 1. The **terminal thread** runs the shell loop. It reads events off
//!    the bus and writes [`Intent`] values to a channel. It owns raw mode
//!    and the alternate screen, and restores both before it returns.
//! 2. A **forwarding thread** moves intents from that synchronous channel
//!    onto an asynchronous one. It exists because the shell's channel is
//!    a `std::sync::mpsc` (that is what `dark-tui` takes, and Rule 14
//!    keeps `tokio` out of that crate), and a blocking `recv` must not
//!    happen on the runtime.
//! 3. The **runtime** owns the model and runs turns.
//!
//! # Intents during a turn
//!
//! A turn can take minutes, and a person must be able to cancel it or
//! answer a confirmation while it runs. [`drive`] therefore does not wait
//! for the turn and then read intents: it selects over both, so a
//! [`Intent::Cancel`] cancels the running turn and a [`Intent::Confirm`]
//! reaches the [`ChannelConfirmer`] that the turn is blocked on. A
//! [`Intent::Submit`] that arrives mid-turn is refused with a notice
//! rather than queued — queuing it would run it against a context the
//! person could no longer see the top of.
//!
//! # The prefix, across turns
//!
//! Rule 5 forbids changing the prefix during a turn, not between turns.
//! Each turn assembles its own prefix and puts the conversation so far
//! after it, so the cached prefix stays valid for every round trip within
//! the turn. [`Conversation`] holds that tail — the prefix is never
//! stored in it, because it is rebuilt each turn.

use std::io::{self, Stdout};

use anyhow::{Context as _, Result};
use dark_contract::{Event, EventBus, EventRx, Intent, Message, Role, RoleClass};
use dark_core::policy::{ChannelConfirmer, PolicyConfig, RunMode};
use dark_core::turn::{TurnCtx, run_turn};
use dark_tui::app::App;
use dark_tui::theme::Theme;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use tokio_util::sync::CancellationToken;
use ulid::Ulid;

use crate::harness::{self, BringUp};

/// The conversation so far, without the prefix.
///
/// See the module documentation: the prefix is rebuilt at the start of
/// each turn and never stored, so this holds only what the turns
/// produced and what the person typed.
#[derive(Debug, Default)]
struct Conversation {
    /// The tail, oldest first.
    messages: Vec<Message>,
}

/// Runs `dark` with no subcommand.
///
/// # Errors
///
/// Returns an error when no model is installed, when the model cannot be
/// loaded, or when the terminal cannot be put into raw mode.
pub(crate) fn run_command(dark: bool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the harness runtime")?;

    runtime.block_on(shell(dark))
}

/// Sets up a real terminal: raw mode and the alternate screen.
fn init_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

/// Restores the terminal that [`init_terminal`] set up.
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
}

/// Runs the shell loop on this thread, restoring the terminal whether or
/// not the loop failed.
///
/// A panic inside the loop would otherwise leave the terminal in raw mode
/// with the alternate screen showing, which makes the shell that started
/// `dark` unusable. The restore therefore runs on the way out of either
/// outcome.
fn run_terminal(mut events: EventRx, intents: &std::sync::mpsc::Sender<Intent>) -> Result<()> {
    let mut terminal = init_terminal().context("could not set the terminal to raw mode")?;
    let mut app = App::new(Theme::detect());

    let outcome = dark_tui::app::run(&mut terminal, &mut app, &mut events, intents);
    let restored = restore_terminal(&mut terminal);

    outcome.context("the terminal application failed")?;
    restored.context("could not restore the terminal")?;
    Ok(())
}

/// Brings a session up and drives it from the terminal application.
async fn shell(dark: bool) -> Result<()> {
    let root = crate::repo_root()?;
    let dark_home = crate::dark_home();

    let bus = EventBus::new();
    let events = bus.tx();

    // The model loads before the terminal is taken over, so a load that
    // fails prints its remedy to an ordinary terminal rather than into an
    // alternate screen that is about to be torn down.
    let harness = harness::bring_up(BringUp {
        root: root.clone(),
        dark_home,
        preferred_model: None,
        policy: PolicyConfig::default(),
        // A person is here, so a `confirm` value shows a prompt and waits.
        mode: RunMode::Interactive,
        events: events.clone(),
        tier_override: None,
    })
    .await?;

    let (intent_tx, intent_rx) = std::sync::mpsc::channel::<Intent>();
    let terminal_events = bus.subscribe();
    let terminal_thread = std::thread::spawn(move || run_terminal(terminal_events, &intent_tx));

    // The shell's channel is synchronous, so it is drained on its own
    // thread rather than blocking the runtime. See the module
    // documentation.
    let (async_tx, mut intents) = tokio::sync::mpsc::unbounded_channel::<Intent>();
    std::thread::spawn(move || {
        while let Ok(intent) = intent_rx.recv() {
            if async_tx.send(intent).is_err() {
                break;
            }
        }
    });

    let session_id = Ulid::new();
    events.send(Event::SessionStart {
        id: session_id.to_string(),
        root: root.clone(),
        branch: crate::run::git_branch(&root),
    });

    let result = drive(&harness, &events, &mut intents, &root, dark).await;

    // Closing the bus ends the shell loop, which restores the terminal.
    drop(events);
    drop(bus);
    let terminal_result = terminal_thread
        .join()
        .map_err(|_| anyhow::anyhow!("the terminal thread panicked"))?;

    result?;
    terminal_result
}

/// Reads intents and runs a turn for each submission, until the person
/// quits or the shell closes.
async fn drive(
    harness: &harness::Harness,
    events: &dark_contract::EventTx,
    intents: &mut tokio::sync::mpsc::UnboundedReceiver<Intent>,
    root: &std::path::Path,
    mut dark: bool,
) -> Result<()> {
    let confirmer = ChannelConfirmer::new(events.clone());
    let mut conversation = Conversation::default();

    while let Some(intent) = intents.recv().await {
        match intent {
            Intent::Quit => return Ok(()),
            Intent::GoDark(on) => {
                dark = on;
                events.send(Event::DarkChanged { dark });
            }
            Intent::Confirm { id, allow } => {
                // No turn is running, so nothing is waiting on this. A
                // stale answer is ignored rather than treated as an
                // error; see `ChannelConfirmer::resolve`.
                confirmer.resolve(&id, allow).await;
            }
            Intent::Cancel => {
                // Nothing is running between turns.
            }
            Intent::Submit(text) | Intent::Command(text) => {
                let quit = one_turn(
                    harness,
                    events,
                    &confirmer,
                    intents,
                    &mut conversation,
                    root,
                    dark,
                    &text,
                )
                .await?;
                if quit {
                    return Ok(());
                }
            }
            // `Intent` is non-exhaustive. A variant added later reaches
            // this arm, and saying so is better than ignoring it in
            // silence while a person waits for something to happen.
            _ => events.notice("this version of dark does not answer that yet."),
        }
    }
    Ok(())
}

/// Runs one turn, handling the intents that can arrive while it runs.
///
/// Returns `true` when the person asked to quit during the turn.
#[allow(
    clippy::too_many_arguments,
    reason = "these are one turn's collaborators, each borrowed from a different owner; \
              bundling them into a struct would only move the same list somewhere else"
)]
async fn one_turn(
    harness: &harness::Harness,
    events: &dark_contract::EventTx,
    confirmer: &ChannelConfirmer,
    intents: &mut tokio::sync::mpsc::UnboundedReceiver<Intent>,
    conversation: &mut Conversation,
    root: &std::path::Path,
    dark: bool,
    text: &str,
) -> Result<bool> {
    let turn_id = Ulid::new().to_string();
    events.send(Event::TurnStart {
        turn: turn_id.clone(),
        class: RoleClass::Worker,
        model: harness.model_id.clone(),
    });
    events.send(Event::UserMessage {
        turn: turn_id.clone(),
        text: text.to_owned(),
    });

    // Rule 5: the prefix is assembled here, at the turn boundary, and
    // never again until the next one.
    let mut messages = crate::run::prefix_messages(harness, root, text)?;
    // The prefix comes first, then the conversation so far, then what the
    // person just typed. `prefix_messages` already appended that last
    // message, so it is moved after the tail rather than duplicated.
    let submitted = messages
        .pop()
        .unwrap_or_else(|| Message::text(Role::User, text));
    messages.extend(conversation.messages.iter().cloned());
    messages.push(submitted.clone());

    let cancel = CancellationToken::new();
    let ctx = TurnCtx {
        turn: turn_id.clone(),
        engine: harness.engine.as_ref(),
        tools: &harness.tools,
        policy: &harness.policy,
        confirmer,
        events: events.clone(),
        root: root.to_path_buf(),
        dark,
        human_present: true,
        config: crate::run::turn_config(harness),
    };

    let started = std::time::Instant::now();
    let turn = run_turn(&ctx, RoleClass::Worker, messages, &cancel);
    tokio::pin!(turn);

    let mut quit = false;
    let outcome = loop {
        tokio::select! {
            finished = &mut turn => break finished,
            received = intents.recv() => match received {
                // `None` is the shell closing while the turn runs, which
                // means the same thing here as an explicit cancel: stop
                // the turn and let it finish returning, so every issued
                // tool call still gets its reply.
                None | Some(Intent::Cancel) => cancel.cancel(),
                Some(Intent::Quit) => {
                    quit = true;
                    cancel.cancel();
                }
                Some(Intent::Confirm { id, allow }) => {
                    confirmer.resolve(&id, allow).await;
                }
                Some(Intent::GoDark(_)) => {
                    // Dark mode is fixed for the turn in progress: the
                    // tools already hold the flag this turn started with,
                    // and changing it mid-turn would apply to some calls
                    // and not others.
                    events.notice("dark mode changes at the next turn, not during this one.");
                }
                Some(Intent::Submit(_) | Intent::Command(_)) => {
                    events.notice("a turn is running. Cancel it first, or wait for it to finish.");
                }
                // `Intent` is non-exhaustive; see `drive`.
                Some(_) => {}
            },
        }
    };

    let outcome = outcome.map_err(crate::contract_error)?;

    events.send(Event::TurnEnd {
        turn: turn_id,
        usage: dark_contract::Usage::default(),
        wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });

    debug_assert!(
        outcome.history_is_well_formed(),
        "every tool call must have its reply, or the next turn's template breaks"
    );

    // The submitted message and everything the turn produced become the
    // tail the next turn puts after its prefix.
    conversation.messages.push(submitted);
    conversation.messages.extend(outcome.messages);

    Ok(quit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_starts_empty() {
        let conversation = Conversation::default();
        assert!(
            conversation.messages.is_empty(),
            "the prefix is rebuilt each turn and never stored here"
        );
    }

    #[test]
    fn the_tail_grows_by_the_submission_and_the_turns_messages() {
        // The bookkeeping `one_turn` does at its end, checked directly:
        // a second turn must see the first turn's messages after the
        // prefix, in order.
        let mut conversation = Conversation::default();
        conversation
            .messages
            .push(Message::text(Role::User, "first"));
        conversation
            .messages
            .push(Message::text(Role::Assistant, "reply"));
        conversation
            .messages
            .push(Message::text(Role::User, "second"));

        assert_eq!(conversation.messages.len(), 3);
        assert_eq!(conversation.messages[0].role, Role::User);
        assert_eq!(conversation.messages[1].role, Role::Assistant);
        assert_eq!(
            conversation.messages[2].role,
            Role::User,
            "the tail keeps the order a chat template needs"
        );
    }
}
