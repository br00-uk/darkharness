# ADR 0007: driving other coding agents over the Agent Client Protocol

**Status:** accepted · **Crate:** `crates/dark-acp`

## Context

The primary requirement in `CLAUDE.md` says a person disconnects the
network after `dark setup` and keeps working, and that a change needing
the network at run time is the wrong change. Every decision in this
repository so far has followed from it.

The person building this harness has since said that they have a network
connection most of the time, and asked to use the coding agents already
installed on their machine — Claude Code, opencode, Codex, Gemini CLI and
the rest — without further setup. Those agents speak a common protocol,
the Agent Client Protocol: JSON-RPC 2.0 over a subprocess's standard
input and output.

This ADR records what that means for the premise, and why the answer is
additive rather than a reversal.

## Decision: the offline path is unchanged, and this is a second path

The local model stays exactly as it is. `dark run` and `dark` with no
subcommand still bring up `dark-engine`, still work with no network, and
gain nothing and lose nothing from this work. An ACP agent is a second
way to run a session, named explicitly, never a default and never a
fallback.

That keeps the primary requirement true as written: after `dark setup`,
disconnect and keep working. It is now also true that a person with a
connection can spend it on a better model, which is what they asked for.

## Decision: this does not weaken Rule 13

Rule 13 says only `dark-airlock` may construct an HTTP client, and
`cargo xtask check-deps` and `cargo deny` both enforce it.

The protocol is stdio. `crates/dark-acp` opens no socket, and
`agent-client-protocol` pulls in no HTTP client — checked with
`cargo tree`, not assumed. The rule is untouched and the checks keep
passing unchanged.

What *does* reach the network is the agent subprocess, on its own
account, outside this harness entirely. That is a real difference from
everything else here, and the honest thing is to name it rather than
hide it behind a rule that does not apply. `Agent::reaches_network`
records what is known about each agent; `dark acp list` prints it; and
dark mode refuses those agents outright (below).

## Decision: two ways to start an agent, and the difference is load-bearing

The editor extensions that pioneered this protocol launch agents as
`npx <package>@latest`, which downloads the package on **every** launch.
That is reasonable for an editor and wrong here: an agent that cannot
start without a download breaks the offline promise before it has done
any work.

`discover::find` therefore prefers a native binary already on `PATH`, and
treats the npx form as a fallback marked `needs_network_to_start`. On a
machine where a person has actually installed `opencode`, the agent
starts from disk.

`session::check_dark_mode` refuses, with `E_POLICY_DARK`:

- an agent that must be downloaded to start, naming the download as the
  reason rather than letting it fail partway; and
- an agent known to send the repository's code to a remote service,
  which is what dark mode exists to prevent.

Both messages name the local model as the remedy, because that is the
thing that still works.

## Decision: the foreign agent runs inside this harness's policy

The protocol turns the usual control flow around. In a local session
`dark-core` owns the turn loop and calls out to tools. In an ACP session
the agent owns its loop and calls *back* for permission, for file reads
and writes, and to run commands.

This harness already owns every one of those, so the foreign agent is
wired into them rather than given its own:

| The protocol asks the client for | What answers it |
| --- | --- |
| `session/request_permission` | `dark_core::policy::Policy` (task unit `A4`) |
| session updates | `dark_contract::Event`, so the terminal and the transcript need no new code |

A foreign agent's write is therefore gated by `policy.write`, its
commands by `policy.exec`, and its session replays through
`dark replay` like any other. That is the reason to speak the protocol
rather than shell out to the agent's own command line, which would give
up all of it.

## Decision: a mapping that is uncertain resolves towards refusing

`bridge::to_prompt` classifies a permission request by what it carries,
not by the word the agent used for it: a request carrying a diff is a
write however it is labelled. Task unit `A4` requires a confirmation to
show the exact diff and never a summary, and an agent that names its
actions differently from this harness must not cost a person that.

`bridge::chosen_option` picks the agent's option that carries out the
person's answer, and returns nothing when none matches — the caller then
cancels. Two rules follow from "never widen a permission":

- `Allow::Always` falls back to an allow-once option, because approving
  one action is narrower than approving all of them.
- `Allow::Once` never falls back to allow-always, and a refusal with no
  refusing option offered picks nothing at all rather than an allow
  option that happened to be in the list.

These are the mistakes that would matter, so they are the ones with
tests.

## What is proved, and what is not

`discover` and `bridge` are pure and tested, including the dark-mode
refusals and every widening rule above.

`session::connect` — the conversation itself — **is exercised**, against
a real subprocess speaking the real protocol. This ADR originally
recorded it as compile-true and never run, on the grounds that the agents
speaking this protocol are other people's programs needing their own
credentials. That was true of *those* agents and not of the protocol: the
same crate writes the agent side, so `crates/dark-acp/src/bin/echo_agent.rs`
is an agent that answers from a script. It needs no credential and opens
no socket, so `tests/speaks_the_protocol.rs` drives the shipping client
path anywhere, including with the network unplugged. The idea is
`dark-engine-fake`'s: to test a harness that drives something expensive,
build a cheap thing with the same shape.

Writing it found two defects that no amount of re-reading had:

- Permission option kinds were built with `format!("{:?}")`, which
  produces `AllowOnce` and lower-cases to `allowonce`. `bridge` matches
  `allow_once`, so **every permission request from every agent would have
  been cancelled** — and a cancelled request is not an error, so the
  feature would have looked like it worked while refusing everything.
  `session::kind_name` now writes the names out.
- The option chosen was matched back to the protocol's own identifier
  through its `Debug` form. The position is used instead, which is what
  actually reads the right option out of the list.

What remains unproved is narrower than before and worth stating: the
fixture answers one prompt, streams one message, and asks one permission.
A real agent will send tool-call updates, plans, usage reports and modes
this harness currently ignores, and will exercise the `fs/*` and
`terminal/*` callbacks that are not wired at all yet. The first person to
run `dark acp run <agent> "<prompt>"` against a real agent is still doing
a test this workspace cannot.

## Consequences

- A new crate, `crates/dark-acp`, depending on `dark-contract` and
  `agent-client-protocol` only. It holds no session state and no policy
  of its own; `dark-cli` composes it as it composes everything else.
- `agent-client-protocol` brings the `async-io`/`async-process` family
  into the tree. It coexists with tokio — the SDK's own example runs
  under `#[tokio::main]` — but it is a second async ecosystem in one
  binary, which is a real cost recorded here rather than discovered
  later.
- A fixture binary, `dark-acp-echo-agent`, ships in the crate. It is
  built by `cargo build --workspace` and is not distributed
  (`dist-workspace.toml` marks only `dark-cli`). The cost is one small
  binary in the build; the return is that the client path is tested at
  all.
- The agent table in `discover` will go stale as agents change their
  command lines. `Agent::configured` exists so a person is never blocked
  on this repository catching up, and `dark acp list` shows the exact
  command it would run so a wrong entry is visible rather than puzzling.
