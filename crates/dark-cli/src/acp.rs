//! `dark acp`: works with other coding agents over the Agent Client
//! Protocol.
//!
//! `list` reports which agents this machine can start, and how each one
//! would be started. It reads `PATH` and nothing else: no agent is
//! launched, and no network connection is opened, so it answers the same
//! way on a disconnected machine.

use std::io::Write as _;

use anyhow::{Context as _, Result};
use dark_acp::discover;

use crate::AcpAction;

/// Runs the `dark acp` subcommand named by `action`.
pub(crate) fn run_command(action: AcpAction) -> Result<()> {
    match action {
        AcpAction::List => {
            list();
            Ok(())
        }
        AcpAction::Run {
            agent,
            prompt,
            dark,
            yes,
            bare,
        } => run(&agent, &prompt, dark, yes, bare),
    }
}

/// Runs `dark acp run <agent> "<prompt>"`.
///
/// The agent runs inside this harness's permission policy and reports on
/// its event bus, so its session is confirmed and recorded the same way
/// a local turn is. Unless `bare` is set, the prompt carries this
/// repository's own context — the `AGENTS.md` chain and the analysis
/// darkharness has already done — so the agent starts knowing what this
/// harness knows rather than rediscovering it.
fn run(name: &str, prompt: &str, dark: bool, yes: bool, bare: bool) -> Result<()> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let agent = discover::find_named(name, &path_var).ok_or_else(|| unknown_agent(name))?;

    if agent.reaches_network && !dark {
        eprintln!(
            "note: {name} sends this repository's code to a remote service. Pass --dark to \
             refuse that, or use the local model."
        );
    }

    let root = crate::repo_root()?;
    let sessions_root = crate::dark_home().join("sessions");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the harness runtime")?;

    runtime.block_on(run_prompt(
        &agent,
        name,
        &root,
        &sessions_root,
        prompt,
        dark,
        yes,
        bare,
    ))
}

/// Drives one prompt against `agent`, streaming its reply to standard
/// output and recording the session the same way [`crate::run`] does for a
/// local turn — the printer that drains the bus and the transcript writer
/// it feeds are the same shapes, so `dark replay` reads either kind of
/// session back.
#[allow(clippy::too_many_arguments, reason = "one call site, one turn")]
async fn run_prompt(
    agent: &dark_acp::Agent,
    name: &str,
    root: &std::path::Path,
    sessions_root: &std::path::Path,
    prompt: &str,
    dark: bool,
    yes: bool,
    bare: bool,
) -> Result<()> {
    let bus = dark_contract::EventBus::new();
    let events = bus.tx();
    let session_id = ulid::Ulid::new();
    let turn_id = ulid::Ulid::new().to_string();

    // Start recording before the first event, so the transcript holds the
    // whole turn rather than whatever arrived after the writer opened. See
    // `run::drive_one_turn` for the same reasoning.
    let mut receiver = bus.subscribe();
    let mut transcript = dark_core::session::TranscriptWriter::open(sessions_root, session_id)
        .await
        .map_err(crate::contract_error)?;

    let printer = tokio::spawn(async move {
        let mut reply = String::new();
        while let Some(received) = receiver.recv().await {
            match received {
                dark_contract::Received::Event(event) => {
                    let ends_the_turn = matches!(event, dark_contract::Event::TurnEnd { .. });
                    if let dark_contract::Event::TokenDelta { text, .. } = &event {
                        // Straight to standard output: a headless run
                        // shows the reply as it arrives.
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                        reply.push_str(text);
                    }
                    if transcript.record(&event).await.is_err() {
                        // A transcript that cannot be written must not
                        // take the turn down with it.
                        break;
                    }
                    if ends_the_turn {
                        break;
                    }
                }
                dark_contract::Received::Lagged(_) => {
                    // Only token deltas travel on the lossy channel, and
                    // the reply is rebuilt from `outcome.text` below if
                    // nothing streamed, so a lagged display costs nothing
                    // that matters here.
                }
            }
        }
        let _ = transcript.flush().await;
        reply
    });

    events.send(dark_contract::Event::SessionStart {
        id: session_id.to_string(),
        root: root.to_path_buf(),
        branch: crate::run::git_branch(root),
    });
    events.send(dark_contract::Event::TurnStart {
        turn: turn_id.clone(),
        class: dark_contract::RoleClass::Worker,
        model: name.to_owned(),
    });
    events.send(dark_contract::Event::UserMessage {
        turn: turn_id.clone(),
        text: prompt.to_owned(),
    });

    let full_prompt = if bare {
        prompt.to_owned()
    } else {
        with_repository_context(prompt, root)
    };

    // A person is present unless this was asked to run headless. `--yes`
    // turns a confirmation into an approval; without it a confirmation is
    // refused and the agent is told so.
    let confirmer = std::sync::Arc::new(dark_core::policy::ChannelConfirmer::new(events.clone()));
    let decide = std::sync::Arc::new(PolicyDecides::new(
        dark_core::policy::RunMode::Headless { yes },
        confirmer,
        root.to_path_buf(),
    ));

    let started = std::time::Instant::now();
    let outcome =
        connect_and_stream(agent, root, &full_prompt, dark, decide, &events, &turn_id).await;

    events.send(dark_contract::Event::TurnEnd {
        turn: turn_id,
        usage: dark_contract::Usage::default(),
        wall_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });

    // Dropping the bus before reporting keeps the ordering plain: every
    // event this run produced has been sent by the time the summary
    // prints.
    drop(events);
    drop(bus);
    let streamed = printer.await.unwrap_or_default();

    let outcome = outcome?;
    // The streamed text already reached standard output token by token. An
    // agent that reports its final answer without ever sending a delta
    // still shows its reply here rather than nothing.
    if streamed.is_empty() && !outcome.text.is_empty() {
        print!("{}", outcome.text);
    }
    if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
        println!();
    }
    eprintln!("{name} stopped: {}", outcome.stop_reason);
    Ok(())
}

/// Connects to `agent` and drives one prompt through it, streaming
/// [`dark_acp::Report::text`] onto `events` — the same bus a local turn
/// reports on, so a listener already watching it (a transcript writer, the
/// terminal application) sees this turn exactly as it sees one of those.
///
/// `decide` answers the agent's permission requests — build it with
/// [`PolicyDecides::new`], sharing its confirmer with whatever resolves an
/// `Intent::Confirm` for the caller, or a real confirmation hangs forever.
///
/// This sends no `SessionStart`, `TurnStart`, `UserMessage`, or `TurnEnd`
/// — the caller owns the turn's lifecycle and sends those itself, the same
/// way [`crate::shell::one_turn`] does for a local turn.
///
/// # Errors
///
/// Returns an error when the agent cannot be reached or the protocol
/// conversation fails. See [`dark_acp::run_prompt`].
pub(crate) async fn connect_and_stream(
    agent: &dark_acp::Agent,
    root: &std::path::Path,
    prompt: &str,
    dark: bool,
    decide: std::sync::Arc<dyn dark_acp::Decide>,
    events: &dark_contract::EventTx,
    turn: &str,
) -> Result<dark_acp::Outcome> {
    let report = std::sync::Arc::new(ReportsOnBus {
        events: events.clone(),
        turn: turn.to_owned(),
    });

    dark_acp::run_prompt(agent, root, prompt, dark, decide, report)
        .await
        .map_err(crate::contract_error)
}

/// Builds the message for an agent this machine cannot start.
///
/// Names the two different problems differently: an agent this harness
/// has never heard of needs a different answer from one it knows but
/// cannot find.
pub(crate) fn unknown_agent(name: &str) -> anyhow::Error {
    if discover::known_names().contains(&name) {
        anyhow::anyhow!(
            "{name} is not installed on this machine. Run dark acp list to see what is."
        )
    } else {
        anyhow::anyhow!(
            "no agent is called {name}. This harness knows: {}.",
            discover::known_names().join(", ")
        )
    }
}

/// Puts this repository's own context in front of the person's prompt.
///
/// This is what a foreign agent cannot work out for itself in the time
/// it has: the instruction chain this repository declares, and the
/// analysis darkharness has already done. Sending it costs tokens on the
/// agent's own bill, which is why `--bare` exists.
pub(crate) fn with_repository_context(prompt: &str, root: &std::path::Path) -> String {
    let mut parts = Vec::new();

    let home = dirs::home_dir().unwrap_or_else(crate::dark_home);
    let working_set = dark_agentsmd::WorkingSet::new();
    let config = dark_agentsmd::AgentsMdConfig::default();
    // Four bytes to a token is the usual rough figure, and this chain is
    // being sized rather than billed: the agent counts its own tokens.
    let count = |text: &str| text.len() / 4;
    if let Ok(chain) = dark_agentsmd::resolve(&home, root, &working_set, &config, &count) {
        let text = chain.prefix_text();
        if !text.trim().is_empty() {
            parts.push(format!("# This repository's instructions\n\n{text}"));
        }
    }

    parts.push(format!("# The request\n\n{prompt}"));
    parts.join("\n\n")
}

/// Runs `dark acp list`.
fn list() {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let found = discover::find(&path_var);

    if found.is_empty() {
        println!("no agent that speaks the Agent Client Protocol is installed.");
        println!();
        println!(
            "This harness knows how to start: {}.",
            discover::known_names().join(", ")
        );
        println!("Install one of them, or install npx to reach the ones published as packages.");
        return;
    }

    println!("{:<12} {:<10} command", "agent", "starts");
    for agent in &found {
        // "download" rather than "network" because that is the specific
        // cost: the package is fetched before the agent can run at all,
        // which is separate from what the agent does once it is running.
        let starts = if agent.launch.needs_network_to_start {
            "download"
        } else {
            "locally"
        };
        println!(
            "{:<12} {starts:<10} {}",
            agent.name,
            agent.launch.command_line()
        );
    }

    if found.iter().any(|agent| agent.reaches_network) {
        println!();
        println!(
            "Every agent listed sends your code to a remote service when it runs. Dark mode \
             refuses them; the local model keeps working."
        );
    }
}

/// Answers a foreign agent's permission requests from this harness's own
/// policy.
///
/// This is the join that makes the feature worth having: an agent this
/// harness did not write, gated by the rules this harness enforces, with
/// the same confirmations a local turn would show.
///
/// The confirmer is shared, not owned outright: `dark_acp::run_prompt`
/// takes `decide` as an `Arc<dyn Decide>`, a `'static` trait object it
/// drives from inside its own protocol conversation, so whatever answers
/// [`dark_core::policy::Confirmer::confirm`] cannot be borrowed the way
/// [`dark_core::turn::TurnCtx`] borrows one for a local turn. Holding the
/// same [`std::sync::Arc`] the caller resolves against is what lets an
/// `Intent::Confirm` that arrives while this runs actually reach it.
pub(crate) struct PolicyDecides {
    /// The policy the session was brought up with.
    policy: dark_core::policy::Policy,
    /// Presents a confirmation and waits for the answer.
    confirmer: std::sync::Arc<dark_core::policy::ChannelConfirmer>,
    /// The repository root a write must not leave. See [`escapes_root`].
    root: std::path::PathBuf,
}

impl PolicyDecides {
    /// Builds a policy-backed [`dark_acp::Decide`] for one turn.
    ///
    /// `confirmer` is shared with the caller so it can resolve a
    /// confirmation that arrives while this runs — see the struct's own
    /// documentation.
    pub(crate) fn new(
        mode: dark_core::policy::RunMode,
        confirmer: std::sync::Arc<dark_core::policy::ChannelConfirmer>,
        root: std::path::PathBuf,
    ) -> Self {
        Self {
            policy: dark_core::policy::Policy::new(
                dark_core::policy::PolicyConfig::default(),
                mode,
            ),
            confirmer,
            root,
        }
    }
}

/// Reports whether writing `path` would land outside `root`.
///
/// `dark_core::policy::Action::Write` requires the caller to work this
/// out, and `Policy` denies such a write outright whatever the
/// configured value says (Rule 34). Passing `false` without checking
/// would switch that rule off for a foreign agent — the one actor in
/// this system whose behaviour this harness did not write, and so the
/// one it should check hardest.
///
/// A path that does not exist yet is resolved through its nearest
/// existing ancestor, because a new file is the ordinary case for a
/// write. A path that cannot be resolved at all is reported as outside,
/// which denies it: a check that cannot answer must not answer "allow".
fn escapes_root(path: &std::path::Path, root: &std::path::Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return true;
    };

    // Relative paths are the agent's, and are meant against the session's
    // own root.
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    // Walk up to something that exists, canonicalise that, then put the
    // remainder back on. This resolves any symbolic link on the way,
    // which is the escape this check exists to catch.
    let mut existing = absolute.as_path();
    let mut trailing = std::path::PathBuf::new();
    loop {
        if existing.exists() {
            break;
        }
        let Some(parent) = existing.parent() else {
            return true;
        };
        let Some(name) = existing.file_name() else {
            return true;
        };
        trailing = std::path::Path::new(name).join(&trailing);
        existing = parent;
    }

    let Ok(resolved) = existing.canonicalize() else {
        return true;
    };
    !resolved.join(trailing).starts_with(&root)
}

#[async_trait::async_trait]
impl dark_acp::Decide for PolicyDecides {
    async fn decide(&self, ask: dark_acp::PermissionAsk) -> dark_contract::Allow {
        let prompt = dark_acp::to_prompt(&ask);
        // The policy classifies by what the action is, so a foreign
        // agent's write is gated by `policy.write` exactly as a local
        // tool's write is.
        let action = match &prompt {
            dark_contract::ConfirmPrompt::Write { path, diff } => {
                dark_core::policy::Action::Write {
                    path: path.clone(),
                    diff: diff.clone(),
                    outside_root: escapes_root(path, &self.root),
                }
            }
            dark_contract::ConfirmPrompt::Exec { command, cwd, .. } => {
                dark_core::policy::Action::Exec {
                    command: command.clone(),
                    cwd: cwd.clone(),
                    // The agent runs its own command; the request does
                    // not say whether a shell reads it. See
                    // `dark_acp::bridge::to_prompt`.
                    shell: false,
                }
            }
            dark_contract::ConfirmPrompt::Other { summary, .. } => {
                dark_core::policy::Action::Read {
                    what: summary.clone(),
                }
            }
        };

        match self.policy.decide(&action, self.confirmer.as_ref()).await {
            dark_core::policy::Decision::Allow => dark_contract::Allow::Once,
            // A denial and a decision this harness could not make are
            // both refusals. Neither may become an approval.
            _ => dark_contract::Allow::Deny,
        }
    }
}

/// Sends what the agent reports onto the event bus, so the terminal and
/// the transcript see it exactly as they see a local turn.
struct ReportsOnBus {
    /// Where the events go.
    events: dark_contract::EventTx,
    /// The turn these events belong to.
    turn: String,
}

impl dark_acp::Report for ReportsOnBus {
    fn text(&self, text: &str) {
        self.events.send(dark_contract::Event::TokenDelta {
            turn: self.turn.clone(),
            text: text.to_owned(),
        });
    }

    fn notice(&self, text: &str) {
        self.events.notice(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_root_does_not_escape() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.rs"), "").unwrap();

        assert!(!escapes_root(&root.path().join("a.rs"), root.path()));
    }

    #[test]
    fn a_new_file_inside_the_root_does_not_escape() {
        // The ordinary case for a write: the file does not exist yet.
        let root = tempfile::tempdir().unwrap();
        assert!(!escapes_root(&root.path().join("new.rs"), root.path()));
    }

    #[test]
    fn a_new_file_in_a_new_directory_inside_the_root_does_not_escape() {
        let root = tempfile::tempdir().unwrap();
        assert!(!escapes_root(
            &root.path().join("src/deep/new.rs"),
            root.path()
        ));
    }

    #[test]
    fn a_relative_path_is_read_against_the_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(!escapes_root(
            std::path::Path::new("src/new.rs"),
            root.path()
        ));
    }

    #[test]
    fn a_path_outside_the_root_escapes() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "").unwrap();

        assert!(escapes_root(&outside.path().join("secret"), root.path()));
    }

    #[test]
    fn climbing_out_with_dot_dot_escapes() {
        let root = tempfile::tempdir().unwrap();
        assert!(escapes_root(
            &root.path().join("../escaped.rs"),
            root.path()
        ));
    }

    #[test]
    #[cfg(unix)]
    fn a_symbolic_link_pointing_out_of_the_root_escapes() {
        // The reason this check canonicalises rather than comparing
        // strings: a link inside the root can name a file outside it,
        // and Rule 34 exists to stop exactly that write.
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link"))
            .unwrap();

        assert!(
            escapes_root(&root.path().join("link"), root.path()),
            "a link out of the root is a write out of the root"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_new_file_through_a_linked_directory_pointing_out_escapes() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("elsewhere")).unwrap();

        assert!(
            escapes_root(&root.path().join("elsewhere/new.rs"), root.path()),
            "the link is resolved before the new name is put back on"
        );
    }

    #[test]
    fn a_root_that_cannot_be_resolved_reports_an_escape() {
        // A check that cannot answer must not answer "allow": Rule 34
        // then denies, which is the safe direction.
        assert!(escapes_root(
            std::path::Path::new("/tmp/anything"),
            std::path::Path::new("/no/such/root/here")
        ));
    }

    #[test]
    fn an_unknown_agent_name_lists_the_ones_that_exist() {
        let message = unknown_agent("not-an-agent").to_string();
        assert!(message.contains("no agent is called"), "message: {message}");
        assert!(message.contains("opencode"), "message: {message}");
    }

    #[test]
    fn a_known_but_absent_agent_says_it_is_not_installed() {
        // A different problem with a different remedy from a name this
        // harness has never heard of.
        let message = unknown_agent("opencode").to_string();
        assert!(message.contains("not installed"), "message: {message}");
        assert!(message.contains("dark acp list"), "message: {message}");
    }
}
