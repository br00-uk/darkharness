# 0011. Onboarding notices, and an ACP agent inside the shell

Date: 2026-08-26

Status: accepted

## Context

Two changes landed the same day, both aimed at the same complaint: the
harness asked too much of a person before it would do anything at all.

The first — `dark` refusing to open without a model installed — is its own
ADR-sized change already folded into `shell.rs`'s history; this record
covers what came after it. With the terminal willing to open on a machine
with nothing installed, two gaps were left visible rather than papered
over by a hard failure at startup:

- A repository with real code in it, opened for the first time, showed an
  empty map and no hint that `/explore` exists.
- A machine with no local model, but a coding agent already on `PATH` from
  another project's tooling, could reach that agent only by leaving the
  shell entirely — `dark acp run <agent> "<prompt>"`, a separate process,
  a separate transcript.

## Decision

**Two notices, not a wizard.** `shell::startup_notices` checks two cheap,
local, no-network facts — `setup::detect_ecosystems` finding a manifest
with no matching `explore::cached_report`, and `harness::installed` coming
back empty — and says so once, in the transcript, the same channel
`Event::Notice` already reaches. Neither blocks the shell from opening;
both name the exact command that answers them.

**`/acp <name>` runs a real turn, not a suggestion.** A submission routes
through `TurnRoute::Local` or `TurnRoute::Acp(name)`, chosen by `/acp` and
read at startup from a remembered choice. The ACP path reuses this
morning's `acp::connect_and_stream` — the same printer-free, transcript-
agnostic core `dark acp run` calls — so a reply streams into the shell's
own transcript exactly as a local turn's does, `TurnStart`/`TurnEnd`
included.

**The confirmer has to be shared, not owned.** `dark_acp::run_prompt` takes
`decide: Arc<dyn Decide>`, a `'static` trait object it drives independently
of the caller — unlike `TurnCtx`, which borrows one for the length of a
single `run_turn` future. `PolicyDecides` therefore holds
`Arc<ChannelConfirmer>`, and `one_acp_turn` keeps a clone of the same `Arc`
in its own `intents`-reading loop. Without that, an `Intent::Confirm`
arriving mid-turn would resolve a confirmer nothing is waiting on, and a
real permission prompt during an ACP turn would hang forever.

**`/acp`, not `/agent`.** This codebase already uses "agent" for something
else — the `AGENTS.md` instruction chain `dark-agentsmd` resolves. Naming
the command after the protocol it uses (`dark acp run`, `dark acp list`)
avoids the collision and keeps the CLI and the in-session vocabulary the
same word.

**The choice is machine-wide.** `crate::config::set_configured_agent`
writes `acp.default` to `$DARK_HOME/config.toml` through the existing
`dark config set` write path — the layer a person already owns for every
repository, not a new one. `dark-config`'s `set` refuses a key no layer
defines, so the `[acp]` section has to exist among the built-in defaults
(`acp.default = ""`) before anything can be written to it — an absent key
is not the same thing as an unset one, here.

## Consequences

- `PolicyDecides.confirmer` changed type, from an owned `ChannelConfirmer`
  to `Arc<ChannelConfirmer>`, and gained a `PolicyDecides::new` constructor.
  `dark acp run`'s own call site moved with it; its behaviour and tests are
  unchanged — headless mode never actually waits on the confirmer, so the
  sharing this decision exists for was invisible there.
- An ACP-routed turn is genuinely a fresh session with the agent's own CLI
  every time — `dark_acp::run_prompt` opens a new subprocess and a new ACP
  session per call, the same as `dark acp run` always has. `conversation`
  still records the exchange either way, so the *transcript* reads as one
  continuous conversation regardless of which backend answered a given
  turn, but the agent itself remembers nothing between submissions.
- `setup::detect_ecosystems` and `acp::{with_repository_context,
  unknown_agent, connect_and_stream}` became `pub(crate)`, reached from
  `shell.rs` now as well as their original callers.

## What this does not settle

- **An ACP turn cannot be cancelled.** `dark_acp::run_prompt` carries no
  cancellation token, so `Ctrl+C` and `Esc` during one are noted — a
  notice says the turn will finish — rather than acted on. A local turn's
  `CancellationToken` has no equivalent on this path yet.
- **No header badge for an ACP-driven turn.** The header's "◆ LOCAL
  `model`" comes from `Event::Residency`, which nothing on this path
  produces. `TurnStart.model` already carries the agent's name into the
  transcript; showing it in the header too is a small, separate change to
  `dark-tui`, not required for the feature to be usable from the shell.
- **No agent-side memory across turns.** Giving an ACP agent continuity
  across a session's submissions — rather than one fresh CLI session per
  turn — would need `dark-acp` to expose something below `run_prompt`: a
  connection kept open and reused, not reconnected each call.
