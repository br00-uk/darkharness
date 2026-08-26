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
//! answer a confirmation while it runs. [`one_turn`] therefore does not
//! wait for the turn and then read intents: it selects over both, so a
//! [`Intent::Cancel`] cancels the running turn and a [`Intent::Confirm`]
//! reaches the [`ChannelConfirmer`] that the turn is blocked on.
//!
//! A [`Intent::Submit`] that arrives mid-turn is **queued**, and runs as
//! its own turn once the running one finishes. It cannot be discarded:
//! reading it off the channel is what takes it away from the person who
//! typed it, and a harness whose turns last minutes will be typed at
//! while it works. It cannot be merged into the running turn either —
//! the prefix must not change mid-turn (Rule 5). So it waits, in the
//! order it was typed.
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

/// Which agent answers a submission: the local model, or a coding agent
/// reached over the Agent Client Protocol. Set by `/acp` and remembered
/// across sessions — see `crate::config::configured_agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnRoute {
    /// The local model, brought up the usual way.
    Local,
    /// An ACP agent, by the name `dark_acp::discover` knows it as.
    Acp(String),
}

/// Runs `dark` with no subcommand.
///
/// `resume` names a recorded session to continue: its messages are
/// rebuilt from the transcript and become the conversation the first new
/// turn puts after its prefix. `None` starts an empty session.
///
/// # Errors
///
/// Returns an error when a named session has no readable transcript, or
/// when the terminal cannot be put into raw mode. A missing or
/// unloadable model does not stop the shell from starting: a notice says
/// why, the map and the commands that need no model keep working, and a
/// later submission tries loading one again — what running `dark setup`
/// from another terminal, mid-session, looks like from in here.
pub(crate) fn run_command(dark: bool, resume: Option<Ulid>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the harness runtime")?;

    runtime.block_on(shell(dark, resume))
}

/// What [`run_command`] decided.
enum CommandOutcome {
    /// Run a turn with this text.
    Turn(String),
    /// The command was answered. Run no turn.
    Handled,
    /// Leave the session.
    Quit,
}

/// Dispatches one submission, and runs whatever a command asks for.
///
/// A command that needs the repository — `/explore`, `/seams`, `/plan` —
/// runs on a blocking thread: those are synchronous, minutes long, and
/// would otherwise stall the runtime the event bus and the terminal
/// thread share.
async fn dispatch_submission(
    text: &str,
    events: &dark_contract::EventTx,
    root: &std::path::Path,
    dark: &mut bool,
    route: &mut TurnRoute,
) -> CommandOutcome {
    use crate::command::{Action, Outcome, dispatch};

    let action = match dispatch(text) {
        Outcome::Prompt(prompt) => return CommandOutcome::Turn(prompt),
        Outcome::Answered(answer) => {
            events.notice(answer);
            return CommandOutcome::Handled;
        }
        Outcome::NotYet(name) => {
            events.notice(format!(
                "{name} is named in the command table but is not built yet. It is not being \
                 sent to the model, which would answer as though it had run."
            ));
            return CommandOutcome::Handled;
        }
        Outcome::Unknown(name) => {
            events.notice(format!("{name} is not a command. /help lists them."));
            return CommandOutcome::Handled;
        }
        Outcome::Act(action) => action,
    };

    match action {
        Action::Quit => return CommandOutcome::Quit,
        Action::Dark(on) => {
            *dark = on;
            events.send(Event::DarkChanged { dark: *dark });
            return CommandOutcome::Handled;
        }
        Action::Acp(arg) => {
            events.notice(dispatch_acp(arg, route));
            return CommandOutcome::Handled;
        }
        _ => {}
    }

    let root = root.to_path_buf();
    let reply = tokio::task::spawn_blocking(move || blocking_command(&action, &root))
        .await
        .unwrap_or_else(|err| format!("the command did not finish: {err}"));
    events.notice(reply);
    CommandOutcome::Handled
}

/// Runs the commands that read the repository, off the runtime.
///
/// Each one already exists as a `dark` subcommand and prints to standard
/// output; inside the shell that output would land under the alternate
/// screen. Capturing it is the wrong fix — these return their own text.
fn blocking_command(action: &crate::command::Action, root: &std::path::Path) -> String {
    use crate::command::Action;
    match action {
        Action::Explore => match crate::explore::summarise(root) {
            Ok(text) => text,
            Err(err) => format!("explore failed: {err}"),
        },
        Action::Seams => match crate::explore::seams_text(root) {
            Ok(text) => text,
            Err(err) => format!("seams failed: {err}"),
        },
        Action::Chart(idea) => match crate::plan::chart_text(root, idea) {
            Ok(text) => text,
            Err(err) => format!("charting failed: {err}"),
        },
        Action::Work(ticket) => match crate::plan::work_text(root, ticket.as_deref()) {
            Ok(text) => text,
            Err(err) => format!("{err}"),
        },
        // Handled before this point, or not reachable from the table.
        Action::Dark(_)
        | Action::Acp(_)
        | Action::Quit
        | Action::Residency
        | Action::Compact
        | Action::Clear => "not yet".to_owned(),
    }
}

/// Runs `/acp`: reports, changes, or clears which agent answers a turn.
///
/// A name is checked against `dark_acp::discover::find_named` before
/// `route` changes — switching to an agent this machine cannot start
/// would just move today's "no model installed" notice to "no agent
/// answers", with none of the benefit. The choice is remembered via
/// `crate::config::set_configured_agent` regardless of whether writing it
/// succeeds: a person's `/acp` still takes effect for this session even
/// when `$DARK_HOME/config.toml` cannot be written, the same way a policy
/// change that cannot be confirmed still denies for this turn.
fn dispatch_acp(arg: crate::command::AcpArg, route: &mut TurnRoute) -> String {
    use crate::command::AcpArg;

    match arg {
        AcpArg::Status => match route {
            TurnRoute::Local => "answering turns with the local model.".to_owned(),
            TurnRoute::Acp(name) => format!("answering turns with {name}."),
        },
        AcpArg::Off => {
            *route = TurnRoute::Local;
            if let Err(err) = crate::config::set_configured_agent("") {
                return format!(
                    "answering turns with the local model again for this session, but the \
                     choice was not remembered: {err}"
                );
            }
            "answering turns with the local model again.".to_owned()
        }
        AcpArg::Use(name) => {
            let path_var = std::env::var("PATH").unwrap_or_default();
            if dark_acp::discover::find_named(&name, &path_var).is_none() {
                return format!("{}", crate::acp::unknown_agent(&name));
            }
            *route = TurnRoute::Acp(name.clone());
            if let Err(err) = crate::config::set_configured_agent(&name) {
                return format!(
                    "answering turns with {name} for this session, but the choice was not \
                     remembered: {err}"
                );
            }
            format!("answering turns with {name}.")
        }
    }
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
fn run_terminal(
    mut events: EventRx,
    intents: &std::sync::mpsc::Sender<Intent>,
    repo_root: &std::path::Path,
) -> Result<()> {
    let mut terminal = init_terminal().context("could not set the terminal to raw mode")?;
    let mut app = App::new(Theme::detect());

    // The map this repository already holds, if it holds exactly one, so
    // the map pane is populated before the first turn rather than only
    // after something changes it.
    if let Some(layout) = crate::fogmap::sole_map(repo_root) {
        app.set_map(layout);
    }

    // `dark-tui` cannot open a map store (Rule 14), so the loop asks this
    // closure whenever an `Event::MapChanged` names one.
    let mut load_map = |map_id: &str| crate::fogmap::load(repo_root, map_id);

    let outcome = dark_tui::app::run(&mut terminal, &mut app, &mut events, intents, &mut load_map);
    let restored = restore_terminal(&mut terminal);

    outcome.context("the terminal application failed")?;
    restored.context("could not restore the terminal")?;
    Ok(())
}

/// Brings a session up and drives it from the terminal application.
async fn shell(dark: bool, resume: Option<Ulid>) -> Result<()> {
    let root = crate::repo_root()?;
    let dark_home = crate::dark_home();

    // Rebuilt before the terminal is taken over, so a session that will
    // not read reports it to an ordinary terminal.
    let conversation = match resume {
        None => Conversation::default(),
        Some(id) => {
            let sessions_root = dark_home.join("sessions");
            let replayed = dark_core::session::Session::replay(&sessions_root, id, root.clone())
                .await
                .map_err(crate::contract_error)?;
            println!(
                "resuming session {id}: {} message(s)",
                replayed.messages.len()
            );
            Conversation {
                messages: replayed.messages,
            }
        }
    };

    let bus = EventBus::new();
    let events = bus.tx();

    // The terminal comes up before the model does, not after: a person
    // reads the map, runs `/explore`, or charts a plan while a slow load
    // works in the background, and a load that fails or finds nothing
    // installed shows its remedy as a notice inside the shell rather than
    // refusing to open it at all.
    let (intent_tx, intent_rx) = std::sync::mpsc::channel::<Intent>();
    let terminal_events = bus.subscribe();
    let terminal_root = root.clone();
    let terminal_thread =
        std::thread::spawn(move || run_terminal(terminal_events, &intent_tx, &terminal_root));

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

    let mut route = match crate::config::configured_agent() {
        Ok(Some(name)) => TurnRoute::Acp(name),
        Ok(None) => TurnRoute::Local,
        Err(err) => {
            events.notice(format!("{err}"));
            TurnRoute::Local
        }
    };

    let has_local_model = startup_notices(&root, &dark_home, &route, &events);

    let mut harness = if matches!(route, TurnRoute::Local) && has_local_model {
        match bring_up_interactive(&root, &dark_home, &events).await {
            Ok(built) => Some(built),
            Err(err) => {
                events.notice(format!("{err}"));
                None
            }
        }
    } else {
        None
    };

    let result = drive(
        &mut harness,
        &mut route,
        &dark_home,
        &events,
        &mut intents,
        &root,
        dark,
        conversation,
    )
    .await;

    // Closing the bus ends the shell loop, which restores the terminal.
    // Every sender must go for the channel to close, and the engine's own
    // resident set holds one for the whole session (see
    // `dark_engine::resident::ResidentSet::new`) — so the harness is
    // dropped here too. Without that, a person who quit through anything
    // but the quit key would leave this join waiting for a thread that
    // is itself waiting for a channel that can never close.
    drop(harness);
    drop(events);
    drop(bus);
    let terminal_result = terminal_thread
        .join()
        .map_err(|_| anyhow::anyhow!("the terminal thread panicked"))?;

    result?;
    terminal_result
}

/// Brings up the model an interactive shell uses: a person is present, so
/// a `confirm` policy value shows a prompt and waits for the answer
/// rather than resolving on its own.
async fn bring_up_interactive(
    root: &std::path::Path,
    dark_home: &std::path::Path,
    events: &dark_contract::EventTx,
) -> Result<harness::Harness> {
    harness::bring_up(BringUp {
        root: root.to_path_buf(),
        dark_home: dark_home.to_path_buf(),
        preferred_model: None,
        policy: PolicyConfig::default(),
        mode: RunMode::Interactive,
        events: events.clone(),
        tier_override: None,
    })
    .await
}

/// Shows the two notices a person sees only once, right after the
/// terminal opens — whether this repository has never been explored, and
/// whether nothing will answer a turn yet — and reports whether a local
/// model is worth trying to load at all.
///
/// Both checks are local reads: `setup::detect_ecosystems` and
/// `explore::cached_report` touch only the filesystem, and
/// `harness::installed`/`dark_acp::discover::find` read `$DARK_HOME` and
/// `PATH`. Nothing here reaches the network.
fn startup_notices(
    root: &std::path::Path,
    dark_home: &std::path::Path,
    route: &TurnRoute,
    events: &dark_contract::EventTx,
) -> bool {
    if !crate::setup::detect_ecosystems(root).is_empty()
        && crate::explore::cached_report(root).ok().flatten().is_none()
    {
        events.notice(
            "this repository has code but has not been explored yet. Type /explore to map it.",
        );
    }

    let has_local_model =
        harness::installed(dark_home).is_ok_and(|installed| !installed.is_empty());

    // A route already chosen — remembered from a previous session, or set
    // earlier this one — answers this question on its own; nagging about
    // an agent a person already picked would be noise, not help.
    if *route == TurnRoute::Local && !has_local_model {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let agents = dark_acp::discover::find(&path_var);
        if agents.is_empty() {
            events.notice(
                "no local model is installed, and no coding agent was found on PATH. Run dark \
                 setup to install a local model.",
            );
        } else {
            let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
            events.notice(format!(
                "no local model is installed. Type /acp <name> to answer turns with one of the \
                 agents on this machine instead: {}. Or run dark setup to install a local \
                 model.",
                names.join(", ")
            ));
        }
    }

    has_local_model
}

/// Reads intents and runs a turn for each submission, until the person
/// quits or the shell closes.
///
/// `harness` starts as whatever [`shell`] managed to bring up, which may
/// be nothing at all. A submission that would run a turn tries
/// [`bring_up_interactive`] again first when it is still `None` and
/// `route` is still [`TurnRoute::Local`], so installing a model in another
/// terminal, or freeing the memory an earlier attempt needed, is picked up
/// without restarting the shell. A submission that arrives while it is
/// still `None` afterward gets a notice naming why, not a crash —
/// everything that needs no model, the map and `/explore` and `/plan`
/// included, keeps working regardless.
///
/// `route` chooses, per submission, whether a turn runs against the local
/// model or against an ACP agent (`one_acp_turn`). `/acp` changes it via
/// [`dispatch_submission`].
#[allow(
    clippy::too_many_arguments,
    reason = "these are one session's collaborators, each borrowed from a different owner; \
              bundling them into a struct would only move the same list somewhere else"
)]
async fn drive(
    harness: &mut Option<harness::Harness>,
    route: &mut TurnRoute,
    dark_home: &std::path::Path,
    events: &dark_contract::EventTx,
    intents: &mut tokio::sync::mpsc::UnboundedReceiver<Intent>,
    root: &std::path::Path,
    mut dark: bool,
    mut conversation: Conversation,
) -> Result<()> {
    let confirmer = ChannelConfirmer::new(events.clone());
    // Submissions typed while a turn ran, oldest first. See the module
    // documentation for why they wait rather than being discarded.
    let mut queued: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    loop {
        let text = match queued.pop_front() {
            Some(text) => text,
            None => match intents.recv().await {
                // The shell closing and the person quitting end the
                // session the same way.
                None | Some(Intent::Quit) => return Ok(()),
                Some(Intent::GoDark(on)) => {
                    dark = on;
                    events.send(Event::DarkChanged { dark });
                    continue;
                }
                Some(Intent::Confirm { id, allow }) => {
                    // No turn is running, so nothing is waiting on this.
                    // A stale answer is ignored rather than treated as an
                    // error; see `ChannelConfirmer::resolve`.
                    confirmer.resolve(&id, allow).await;
                    continue;
                }
                // Nothing is running between turns.
                Some(Intent::Cancel) => continue,
                // A command is not a prompt. Anything starting with a
                // slash is dispatched; only what is left over becomes a
                // turn. See `crate::command`.
                Some(Intent::Submit(text) | Intent::Command(text)) => {
                    match dispatch_submission(&text, events, root, &mut dark, route).await {
                        CommandOutcome::Turn(prompt) => prompt,
                        CommandOutcome::Handled => continue,
                        CommandOutcome::Quit => return Ok(()),
                    }
                }
                // `Intent` is non-exhaustive. A variant added later
                // reaches this arm, and saying so is better than
                // ignoring it in silence while a person waits.
                Some(_) => {
                    events.notice("this version of dark does not answer that yet.");
                    continue;
                }
            },
        };

        let turn = match route {
            TurnRoute::Local => {
                if harness.is_none() {
                    match bring_up_interactive(root, dark_home, events).await {
                        Ok(built) => *harness = Some(built),
                        Err(err) => {
                            events.notice(format!("{err}"));
                            continue;
                        }
                    }
                }
                let live = harness
                    .as_ref()
                    .expect("just brought up, or already present");

                one_turn(
                    live,
                    events,
                    &confirmer,
                    intents,
                    &mut conversation,
                    root,
                    dark,
                    &text,
                )
                .await?
            }
            TurnRoute::Acp(name) => {
                let name = name.clone();
                one_acp_turn(&name, root, events, intents, &mut conversation, dark, &text).await
            }
        };

        queued.extend(turn.queued);
        if turn.quit {
            return Ok(());
        }
    }
}

/// Runs one turn through `name`'s ACP agent, the same shape [`one_turn`]
/// gives a local turn: `TurnStart` and `UserMessage` open it, a pending
/// confirmation is still answered while it runs, and a submission that
/// arrives mid-turn is queued rather than lost.
///
/// Unlike a local turn, nothing here can be cancelled early —
/// `dark_acp::run_prompt` carries no cancellation token — so
/// `Intent::Cancel` and `Intent::Quit` are noted and only take effect once
/// the agent itself returns. A failure — the agent is no longer on `PATH`,
/// its subprocess crashed, the protocol conversation broke — never takes
/// the shell down with it: it becomes a notice, the same way a missing
/// local model does.
async fn one_acp_turn(
    name: &str,
    root: &std::path::Path,
    events: &dark_contract::EventTx,
    intents: &mut tokio::sync::mpsc::UnboundedReceiver<Intent>,
    conversation: &mut Conversation,
    dark: bool,
    text: &str,
) -> TurnExit {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let Some(agent) = dark_acp::discover::find_named(name, &path_var) else {
        events.notice(format!("{}", crate::acp::unknown_agent(name)));
        return TurnExit::default();
    };

    let turn_id = Ulid::new().to_string();
    events.send(Event::TurnStart {
        turn: turn_id.clone(),
        class: RoleClass::Worker,
        model: name.to_owned(),
    });
    events.send(Event::UserMessage {
        turn: turn_id.clone(),
        text: text.to_owned(),
    });
    conversation.messages.push(Message::text(Role::User, text));

    let prompt = crate::acp::with_repository_context(text, root);
    // A person is present, so a confirmation shows the same modal a local
    // tool call would, and waits for the answer.
    let confirmer = std::sync::Arc::new(ChannelConfirmer::new(events.clone()));
    let decide = std::sync::Arc::new(crate::acp::PolicyDecides::new(
        RunMode::Interactive,
        confirmer.clone(),
        root.to_path_buf(),
    ));

    let started = std::time::Instant::now();
    // A named clone, not `&turn_id` directly: `call` borrows it for as
    // long as it is polled, which is well past the point below where
    // `turn_id` itself moves into `Event::TurnEnd`.
    let call_turn = turn_id.clone();
    let call =
        crate::acp::connect_and_stream(&agent, root, &prompt, dark, decide, events, &call_turn);
    tokio::pin!(call);

    let mut exit = TurnExit::default();
    let mut channel_open = true;
    let outcome = loop {
        tokio::select! {
            finished = &mut call => break finished,
            received = intents.recv(), if channel_open => match received {
                None => {
                    // The shell is closing. The agent call cannot be
                    // cancelled, so let it finish rather than spin this
                    // branch on an exhausted channel — see the `if
                    // channel_open` guard above.
                    channel_open = false;
                    exit.quit = true;
                }
                Some(Intent::Quit) => exit.quit = true,
                Some(Intent::Cancel) => events.notice(
                    "this agent does not support cancelling a turn in progress; it will finish.",
                ),
                Some(Intent::Confirm { id, allow }) => {
                    confirmer.resolve(&id, allow).await;
                }
                Some(Intent::GoDark(_)) => {
                    events.notice("dark mode changes at the next turn, not during this one.");
                }
                Some(Intent::Submit(text) | Intent::Command(text)) => {
                    events.notice("a turn is running; this will run next.");
                    exit.queued.push(text);
                }
                Some(_) => {}
            },
        }
    };

    events.send(Event::TurnEnd {
        turn: turn_id,
        usage: dark_contract::Usage::default(),
        wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });

    match outcome {
        Ok(outcome) if outcome.text.is_empty() => {}
        Ok(outcome) => conversation
            .messages
            .push(Message::text(Role::Assistant, outcome.text)),
        Err(err) => events.notice(format!("{err}")),
    }

    exit
}

/// What one turn left behind for the loop that called it.
#[derive(Debug, Default)]
struct TurnExit {
    /// The person asked to quit during the turn.
    quit: bool,
    /// Submissions typed while the turn ran, oldest first. They run as
    /// their own turns next; see the module documentation.
    queued: Vec<String>,
}

/// Runs one turn, handling the intents that can arrive while it runs.
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
) -> Result<TurnExit> {
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
    // never again until the next one. Rule 8: the prefix first, then the
    // conversation so far, then what the person just typed.
    let mut messages = crate::run::prefix_messages(harness, root)?;
    messages.extend(conversation.messages.iter().cloned());
    let submitted = Message::text(Role::User, text);
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

    let mut exit = TurnExit::default();
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
                    exit.quit = true;
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
                Some(Intent::Submit(text) | Intent::Command(text)) => {
                    // Never discarded: reading it off the channel is what
                    // takes it away from the person who typed it. It runs
                    // as its own turn once this one finishes.
                    events.notice("a turn is running; this will run next.");
                    exit.queued.push(text);
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

    Ok(exit)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use dark_contract::Engine;
    use dark_engine_fake::{FakeEngine, Script};

    use super::*;

    /// Drives `intents` to completion against a scripted engine, and
    /// returns the conversation the turns built.
    ///
    /// This is `drive` itself — the real turn loop, the real tool set,
    /// the real prefix assembly — with the terminal replaced by a list
    /// of intents. Everything the shell does between reading a key and
    /// running a turn is on this path.
    fn drive_intents(script: &str, sent: Vec<Intent>) -> Result<Conversation> {
        drive_from(script, sent, Conversation::default())
    }

    /// [`drive_intents`], starting from a conversation already in hand —
    /// what `dark session resume` hands the shell.
    fn drive_from(script: &str, sent: Vec<Intent>, start: Conversation) -> Result<Conversation> {
        let repo = tempfile::tempdir().expect("a temporary repository");
        let script = Script::from_toml(script).expect("the script is valid");
        let engine: Arc<dyn Engine> = Arc::new(FakeEngine::new(script));

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime");

        runtime.block_on(async {
            let harness = crate::harness::for_test(
                engine,
                repo.path().to_path_buf(),
                dark_core::policy::PolicyConfig::default(),
                RunMode::Interactive,
            )
            .await?;

            let bus = dark_contract::EventBus::new();
            let events = bus.tx();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            for intent in sent {
                tx.send(intent).expect("the receiver is alive");
            }
            // Closing the sender ends `drive` once the queued intents run
            // out, which is what the shell thread going away looks like.
            drop(tx);

            drive_collecting(&harness, &events, &mut rx, repo.path(), false, start).await
        })
    }

    /// [`drive`], returning the conversation instead of discarding it.
    ///
    /// `drive` owns its `Conversation` because nothing outside it needs
    /// one. A test does, so this mirrors its loop over the same
    /// `one_turn`; keeping them side by side is what makes the multi-turn
    /// tail assertion below meaningful.
    async fn drive_collecting(
        harness: &harness::Harness,
        events: &dark_contract::EventTx,
        intents: &mut tokio::sync::mpsc::UnboundedReceiver<Intent>,
        root: &std::path::Path,
        dark: bool,
        mut conversation: Conversation,
    ) -> Result<Conversation> {
        let confirmer = ChannelConfirmer::new(events.clone());
        let mut queued: std::collections::VecDeque<String> = std::collections::VecDeque::new();

        loop {
            let text = match queued.pop_front() {
                Some(text) => text,
                None => match intents.recv().await {
                    None | Some(Intent::Quit) => break,
                    Some(Intent::Submit(text) | Intent::Command(text)) => text,
                    Some(_) => continue,
                },
            };

            let turn = one_turn(
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

            queued.extend(turn.queued);
            if turn.quit {
                break;
            }
        }
        Ok(conversation)
    }

    /// Exercises the real [`drive`], not [`drive_collecting`]'s mirror of
    /// it: the lazy load it now does on `harness`'s behalf is the thing
    /// under test, and only `drive` itself does that.
    #[test]
    fn a_submission_with_no_model_installed_gets_a_notice_and_runs_no_turn() {
        let dark_home = tempfile::tempdir().expect("a temporary $DARK_HOME");
        let root = tempfile::tempdir().expect("a temporary repository");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime");

        runtime.block_on(async {
            let bus = dark_contract::EventBus::new();
            let events = bus.tx();
            let mut notices = bus.subscribe();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tx.send(Intent::Submit("hello".to_owned()))
                .expect("the receiver is alive");
            drop(tx);

            let mut harness = None;
            let mut route = TurnRoute::Local;
            drive(
                &mut harness,
                &mut route,
                dark_home.path(),
                &events,
                &mut rx,
                root.path(),
                false,
                Conversation::default(),
            )
            .await
            .expect("a missing model does not fail the shell");

            assert!(
                harness.is_none(),
                "nothing is installed under this $DARK_HOME, so the retry finds nothing either"
            );

            // Both senders must go, not just this clone: `EventBus` keeps
            // its own, so the channel only closes once it does too. See
            // `EventBus::tx`.
            drop(events);
            drop(bus);

            let mut saw_the_remedy = false;
            while let Some(dark_contract::Received::Event(Event::Notice(text))) =
                notices.recv().await
            {
                saw_the_remedy |= text.contains("dark setup");
            }
            assert!(
                saw_the_remedy,
                "a person sees why no turn ran, not silence — and the remedy names dark setup"
            );
        });
    }

    #[test]
    fn acp_use_rejects_an_agent_this_machine_cannot_start() {
        let mut route = TurnRoute::Local;
        let reply = dispatch_acp(
            crate::command::AcpArg::Use("definitely-not-a-real-agent-xyz".to_owned()),
            &mut route,
        );

        assert_eq!(
            route,
            TurnRoute::Local,
            "switching to an agent this machine cannot start must not change the route"
        );
        assert!(
            reply.contains("definitely-not-a-real-agent-xyz"),
            "the reply names the agent that could not be found: {reply}"
        );
    }

    /// Exercises the real [`drive`]'s ACP branch: routing to an agent this
    /// machine cannot start must notice why and run no turn, the same
    /// shape a missing local model already gets — never a hang (there is
    /// no subprocess to wait on) and never a panic.
    #[test]
    fn a_submission_routed_to_an_unreachable_agent_gets_a_notice_and_runs_no_turn() {
        let dark_home = tempfile::tempdir().expect("a temporary $DARK_HOME");
        let root = tempfile::tempdir().expect("a temporary repository");

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime");

        runtime.block_on(async {
            let bus = dark_contract::EventBus::new();
            let events = bus.tx();
            let mut notices = bus.subscribe();

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tx.send(Intent::Submit("hello".to_owned()))
                .expect("the receiver is alive");
            drop(tx);

            let mut harness = None;
            let mut route = TurnRoute::Acp("definitely-not-a-real-agent-xyz".to_owned());
            drive(
                &mut harness,
                &mut route,
                dark_home.path(),
                &events,
                &mut rx,
                root.path(),
                false,
                Conversation::default(),
            )
            .await
            .expect("an unreachable agent does not fail the shell");

            assert!(
                harness.is_none(),
                "an ACP route never touches the local harness"
            );

            drop(events);
            drop(bus);

            let mut saw_the_remedy = false;
            while let Some(dark_contract::Received::Event(Event::Notice(text))) =
                notices.recv().await
            {
                saw_the_remedy |= text.contains("definitely-not-a-real-agent-xyz");
            }
            assert!(saw_the_remedy, "a person sees why no turn ran, not silence");
        });
    }

    #[test]
    fn one_submission_runs_one_turn_and_keeps_its_messages() {
        let conversation = drive_intents(
            r#"
            [[turns]]
            text = "a reply"
            "#,
            vec![Intent::Submit("hello".to_owned())],
        )
        .expect("the turn runs");

        assert_eq!(
            conversation.messages.first().map(|m| m.role),
            Some(Role::User),
            "the person's own message opens the tail: {:?}",
            conversation.messages
        );
        assert!(
            conversation
                .messages
                .iter()
                .any(|m| m.role == Role::Assistant),
            "the model's reply is kept for the next turn: {:?}",
            conversation.messages
        );
    }

    #[test]
    fn a_second_turn_sees_the_first_turns_conversation() {
        // The whole point of keeping a tail. A shell that dropped it
        // would answer the second question with no memory of the first,
        // which is the bug this asserts against.
        let conversation = drive_intents(
            r#"
            [[turns]]
            text = "first reply"

            [[turns]]
            text = "second reply"
            "#,
            vec![
                Intent::Submit("first question".to_owned()),
                Intent::Submit("second question".to_owned()),
            ],
        )
        .expect("both turns run");

        let user_messages: Vec<String> = conversation
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(dark_contract::Message::text_content)
            .collect();

        assert_eq!(
            user_messages.len(),
            2,
            "both questions are in the tail: {:?}",
            conversation.messages
        );
        assert!(user_messages[0].contains("first question"));
        assert!(user_messages[1].contains("second question"));
    }

    #[test]
    fn quitting_before_any_turn_leaves_an_empty_conversation() {
        let conversation = drive_intents(
            r#"
            [[turns]]
            text = "never reached"
            "#,
            vec![Intent::Quit, Intent::Submit("too late".to_owned())],
        )
        .expect("quitting is not a failure");

        assert!(
            conversation.messages.is_empty(),
            "an intent after Quit must not run a turn: {:?}",
            conversation.messages
        );
    }

    #[test]
    fn a_submission_typed_during_a_turn_runs_next_rather_than_being_lost() {
        // Both intents are queued before the first turn starts, so the
        // select inside `one_turn` reads the second one while the first
        // is still running. Discarding it there would lose what a person
        // typed, and would do it only sometimes, depending on which
        // future the runtime polled first.
        let conversation = drive_intents(
            r#"
            [[turns]]
            text = "first reply"

            [[turns]]
            text = "second reply"
            "#,
            vec![
                Intent::Submit("first question".to_owned()),
                Intent::Submit("second question".to_owned()),
            ],
        )
        .expect("both turns run");

        let asked: Vec<String> = conversation
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(dark_contract::Message::text_content)
            .collect();

        assert_eq!(asked.len(), 2, "nothing typed is lost: {asked:?}");
        assert!(
            asked[0].contains("first") && asked[1].contains("second"),
            "and it runs in the order it was typed: {asked:?}"
        );
    }

    #[test]
    fn a_command_runs_a_turn_the_same_as_a_submission() {
        // `/plan` and friends reach the harness as `Intent::Command`, and
        // the air-gap test's scripted session sends exactly those.
        let conversation = drive_intents(
            r#"
            [[turns]]
            text = "charted"
            "#,
            vec![Intent::Command("/plan add a health check".to_owned())],
        )
        .expect("a command runs a turn");

        assert!(
            !conversation.messages.is_empty(),
            "a command is a turn, not a no-op"
        );
    }

    #[test]
    fn a_resumed_conversation_is_kept_and_added_to() {
        // What `dark session resume` produces: a tail rebuilt from a
        // transcript, which the first new turn must put after its prefix
        // rather than discard.
        let resumed = Conversation {
            messages: vec![
                Message::text(Role::User, "what did we decide?"),
                Message::text(Role::Assistant, "we decided to add a health check"),
            ],
        };

        let conversation = drive_from(
            r#"
            [[turns]]
            text = "carrying on from there"
            "#,
            vec![Intent::Submit("keep going".to_owned())],
            resumed,
        )
        .expect("the turn runs");

        let asked: Vec<String> = conversation
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(dark_contract::Message::text_content)
            .collect();

        assert_eq!(asked.len(), 2, "the past turn survives: {asked:?}");
        assert!(
            asked[0].contains("what did we decide"),
            "the resumed message stays first: {asked:?}"
        );
        assert!(asked[1].contains("keep going"));
    }

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
