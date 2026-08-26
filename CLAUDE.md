# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

darkharness is a local coding harness: one Rust binary that contains its own
inference engine, task tracker, documentation index, and terminal application.

The primary requirement drives every design decision: **after `dark setup`
completes, the user disconnects the network and continues to work.** When a
change would need the network at run time, it is the wrong change.

`PRD.md` is the authoritative specification. It divides the work
into task units (`Z1`, `B1`, `A2`, …). Read the task unit before you touch its
files.

## Build status

Every task unit in `PRD.md` has landed, and the `dark-cli` dispatch is
wired to the crates behind it: `dark run` brings up a real session and
runs a turn, `dark` with no subcommand starts the terminal application,
and `config`, `session`, `map`, `pack`, `models`, `blast`, and `update`
all answer for real. `dark session resume` starts the shell with a past
conversation rebuilt from its transcript.

| Done | Task units |
| --- | --- |
| Contract and fake engine | `Z1`, `B1` |
| Core runtime | `A1`–`A4` |
| Engine | `B2`–`B7` |
| Tools | `C1`–`C4` |
| Cartograph | `D1`–`D5` |
| Explore | `F1`–`F5` |
| Lexicon | `G1`–`G5` |
| Plan | `E1`–`E7` |
| Terminal | `H1`–`H5` |
| Qwen support | `I1`–`I4` |
| Instruction files | `K1`–`K3` |
| Network and configuration | `J1`–`J6` |

### What is not proved yet, and why

The code is complete; some of it has never run against real weights,
because no machine in this build has an accelerator or a model on disk.
`docs/adr/0006` names each deferred seam. In short:

- The memory estimator is pinned against five published model
  configurations, not against measured memory. Rule 1 wants the error on
  the high side, and it is, but the 10% claim in `B4`'s Done criterion is
  unverified.
- Live weight loading, real token streaming, and mistral.rs's key-value
  allocator under cancellation are compile-true and unit-tested behind
  `dark-engine`'s module boundary. The lease accounting *is* proved, over
  a thousand cancelled turns.
- `cargo xtask airgap` reports `NO MODEL` for its four turn steps on a
  machine with no weights. That is honest rather than passing: the
  air-gap property holds for every step that ran, but `J5`'s Done
  criterion ("the scripted session completes") needs a machine where
  `dark setup` has installed a model.

Run `cargo xtask airgap` on such a machine to close the last of it.

### Using another agent

`dark acp list` names the coding agents installed on this machine that
speak the Agent Client Protocol, and `dark acp run <agent> "<prompt>"`
hands one a piece of work. The foreign agent runs inside this harness's
permission policy and reports on its event bus, so its writes are
confirmed and its session is recorded like any other.

This does not change the primary requirement. The local model path is
untouched; an ACP agent is a second path, named explicitly, and dark mode
refuses one that needs a download to start or that sends code to a remote
service. The protocol is stdio, so Rule 13 is untouched too. See
`docs/adr/0007`.

The same path reaches the terminal application: `/acp <name>` answers
every submission that follows through that agent instead of the local
model, streamed into the same transcript pane, until `/acp local` or
`/acp off` switches back. `dark` no longer needs a model installed just to
open — it opens, shows a notice naming what is missing, and a submission
tries again. A repository with code and no `dark explore` run yet gets its
own notice, once. `/acp`'s choice is remembered in
`$DARK_HOME/config.toml`, machine-wide, so it does not ask twice. See
`docs/adr/0011`.

### What `dark explore` records, and what it does not

A reference is recorded for a call and, in Rust, for a type named in a
type position — `dyn Engine`, `Vec<Session>`, `impl Trait for Type`.
**The other twelve grammars capture calls only**, so a blast radius over
Go or TypeScript is still a call graph. `rust.scm`'s type patterns were
checked against this repository; writing the same for a language with no
corpus to hand would be a guess. See `docs/adr/0009`.

Cross-crate `use` paths resolve inside a workspace: a first segment naming
a sibling crate's directory resolves against that crate. Everything else
falls back to a repository-wide unique-name match, and a name that is not
unique resolves to nothing — `F2`, "do not report a guessed reference as
resolved". A call to a method named `new` or `run` therefore stops a
transitive walk, which is why `dark blast` reports smaller numbers than a
compiler would.

### The two doors out of discovery

`dark explore` ends by naming what to do next, and never blocks for the
answer — it runs in scripts and in continuous integration, so the choice
belongs to whichever command runs after it.

`dark extend` keeps the language and the house style. Almost everything it
writes is **counted**, not asked: naming convention per definition kind,
documentation density over exported items, test and module layout,
indentation and line width all come from what `dark-explore` already
extracts. One bounded model call adds prose, from those facts and the
module list rather than from the repository, and the output labels which
half is which — an agent that cannot tell a counted convention from a
guessed one applies both with equal confidence.

`dark refactor` asks for a target language and argues a pattern from the
analysis: modularity, community count, bridges, and Martin's `Ca`/`Ce`/
`A`/`I` per module. Every suggestion prints the numbers behind it, because
advice nobody can check is worse than none. Vendor documentation is
**named, never fetched** — the table lives at
`$DARK_HOME/doc-sources.toml`, each pack prints the `dark pack add` that
would fetch it, and dark mode refuses the step rather than skipping it in
silence.

Both write into a marked block in `AGENTS.md`
(`dark_agentsmd::write::upsert`) and record the choice in
`.dark/profile.json`, which `dark plan` then reads. A file that exists
without the markers is **refused, never modified**: appending would put
machine text under a person's heading and rewriting would lose their work.

### Slash commands are dispatched, not sent to the model

`crates/dark-cli/src/command.rs` owns the in-session table. Until it
existed, `Intent::Command` and `Intent::Submit` shared a match arm, so
every slash command reached the model as prose — `/plan` asked a language
model to talk about charting a map. A command the harness cannot run now
says so rather than letting the model answer as though it had.

### The one command that still answers "not yet"

`dark models quantize`. Converting weights from one quantisation to
another needs mistral.rs's own conversion path, which `dark-engine` does
not expose yet. `dark models pull` fetches an already-quantised model, so
nothing depends on it.

`B2` to `B7` pull in mistral.rs, which dominates every build in the
workspace. A change to `crates/dark-engine` rebuilds candle and
mistral.rs; expect ten minutes or more, and run it with nothing else in
flight.

### The terminal application

`crates/dark-tui/src/app/render.rs` draws each pane's contents, not only
its border. `App` owns the `Transcript` view and folds every event into
it; `views::wrap` wraps styled lines before they are drawn, which is what
lets each visual line carry a gutter and lets the pane anchor to its
newest line. The palette is Charmtone by default
(`Palette::charmtone`); `Palette::accretion_disk`, which task unit `H2`
specifies, is still there and `Theme::with_palette` selects it. See
`docs/adr/0008`.

Every view is now reached, the fog map included. `dark-tui` cannot open a
map store (Rule 14), so `app::run` takes a loader closure and
`crates/dark-cli/src/fogmap.rs` supplies it, converting
`dark-cartograph`'s tickets into the view's own shape. A pane with nothing
to show says which, rather than drawing an empty box — a pane that renders
empty cannot be told apart from a pane that was never wired, which is how
the whole layer stayed unwired through `H1` to `H5`.
`crates/dark-tui/tests/panes_render_their_contents.rs` asserts against
the frame `App` actually draws, which is the only place that gap is
visible. See `docs/adr/0008` and `docs/adr/0009`.

`dark-contract` has no known gaps left open. `Tool::preview` reports what a
tool would do so a confirmation shows a real diff, and `Event` carries the
text a person submits, a tool result's full content, and the git branch. See
`docs/adr/0004`. Change `dark-contract` between waves, never while agents
are compiling: every crate rebuilds.

## Commands

`make` is the single entry point; CI runs the same commands, so a green
`make ci` locally means a green CI run.

```bash
make check     # fmt + lint + test + dependency rules
make ci        # what CI enforces, plus cargo-deny and a full build
make test      # nextest across the workspace, then doctests
make deps      # cargo xtask check-deps — Rules 12 to 17
make deny      # advisories, licences, bans, sources
```

Running a subset:

```bash
cargo nextest run -p dark-contract                  # one crate
cargo nextest run -p dark-contract event::          # one module
cargo nextest run -E 'test(is_lossy)'               # nextest filter expression
cargo test --doc -p dark-contract                   # doctests only
cargo build -p dark-engine-fake --timings           # check the build stays cheap
```

`cargo nextest run` does **not** run doctests. `make test` covers both.

Running the binary: `cargo run -p dark-cli -- doctor`. The binary is named
`dark`, never `dh`; that name shadows the Debian helper tool.

## Architecture

Four layers. A crate depends downwards only.

```
dark-tui          Events in, intents out. Depends on dark-contract only.
      ▲ Event (broadcast) │ Intent (mpsc)
dark-core         Session, turn loop, context assembly
      │   dark-plan · dark-explore · dark-lexicon · dark-tools
      │   dark-cartograph
      ▲ dyn Engine
dark-engine       mistral.rs, resident set        dark-engine-fake (scripted)

dark-acp          Speaks the Agent Client Protocol to another agent's
                  subprocess. Depends on dark-contract only (Rule 16).
```

`dark-contract` sits underneath everything and depends on no workspace crate.
It defines the seam: the `Engine` trait, the `Tool` trait, `Event`, `Intent`,
and the error taxonomy. Change it deliberately — every crate recompiles.

### What the composition root does that no crate can

`dark-cli` is the only crate that sees both the engine and the model
family, so two joins live there and nowhere else. Both look like
plumbing and are not.

`crates/dark-cli/src/scrape.rs` wraps the engine so a Qwen model's tool
calls reach the turn loop. Qwen emits a call as plain text; the turn loop
reads `Chunk::ToolCallDelta`; `dark-qwen` knows how to read the text and
`dark-core` must not know that any model family exists (Rule 17), while
`dark-engine` must not grow one model's text format (Rule 12). The
wrapper turns one into the other, and passes a native model's stream
through untouched.

`crates/dark-cli/src/pack.rs` wraps the engine as a `dark-lexicon`
`Embedder`, whose `embed` is synchronous while the engine's is not. The
adapter drives the future on a runtime handle, so **every call must come
from a thread outside the runtime** — `with_embedder` is the one place
that arranges it, and calling it from inside an async task panics.

### Rules that are enforced, not merely documented

`cargo xtask check-deps` fails the build when any of these break. It reports
the rule number and the remedy.

- Only `dark-engine` depends on `mistralrs` (Rule 12).
- Only `dark-airlock` constructs an HTTP client (Rule 13). `cargo deny` also
  catches one arriving transitively.
- `dark-tui` depends on `dark-contract` only (Rule 14).
- `dark-contract` has an explicit dependency allowlist (Rule 15). Adding a
  tenth dependency fails the check by design. See `docs/adr/0001`.
- `dark-explore`, `dark-lexicon`, and `dark-cartograph` reach for no other
  workspace crate (Rule 16).
- Only `dark-cli` and `dark-engine` take a normal dependency on `dark-engine`.
  Everything else holds `dyn Engine` and tests against `dark-engine-fake`
  (Rule 17). See `docs/adr/0002`.

### Constraints that shape the code

These come from hardware limits. Do not design around them.

- **The context prefix must not change during a turn** (Rules 5 to 8). The
  engine caches the prefix key-value tensors; changing it forces a full
  prefill, which costs 15 to 30 seconds on a 32B model. Assemble the prefix at
  the start of a turn. Append to the tail. Never put a clock in the prefix.
  Compact only at a turn boundary.
- **Memory is the dominant limit** (Rules 1 to 4). Estimate before loading;
  never discover a limit by allocation failure. Never evict a pinned model or
  one holding a turn lease. Budget against `Caps::granted_context`, never
  `Caps::max_context`.
- **Determinism in `/explore`** (Rules 29 to 32). Stages 1 to 5 use no model
  and must produce identical bytes for the same commit and configuration. Sort
  paths with a byte comparator. Fix every seed and visit order. Exclude
  timestamps from hashed output.
- **Tool calls must always be answered** (task unit `A2`). Write a `Role::Tool`
  reply for every issued call, cancelled ones included. An unanswered call
  breaks the chat template.

## Conventions

- Write tests before the code. Task units list their `Verify` commands; run
  them before reporting completion.
- Change only the files that your task unit owns. If you must change another,
  stop and write an ADR in `docs/adr/`.
- Errors carry a code, a message, and a remedy. Use the `ErrCode` taxonomy;
  the string forms are stable and appear in the transcript.
- Add dependencies with `cargo add -p <crate>` so the lockfile stays
  consistent. The lockfile is committed.
- Documentation, comments, and error messages follow ASD-STE100 rules: active
  voice, one instruction per sentence, and the same word for the same thing
  every time. Section 2 of the build specification is the approved term list.
- `make fmt` before committing, or the format job fails first and hides the
  real errors behind a formatting diff.

## Lint policy

Lints are declared once in `[workspace.lints]` in the root `Cargo.toml` and
inherited via `lints.workspace = true`. Set them there, not with crate-level
`#![deny(...)]` attributes.

`unsafe_code` is **forbid**, `missing_docs` is on — every public item, enum
variant, and struct field needs a doc comment — and clippy runs at `pedantic`.
Because pedantic includes `missing_errors_doc`, any public function returning
`Result` needs an `# Errors` section.

`rustfmt.toml` uses **stable-only** options on purpose. Nightly-only keys warn
on every run on a stable toolchain, and that noise reads as a fixable problem
when it is not.

## Ultracode mode

This work suits [ultracode](https://code.claude.com/docs/en/workflows#let-claude-decide-with-ultracode):
`xhigh` reasoning plus automatic dynamic-workflow orchestration.

```bash
claude --effort ultracode        # or /effort ultracode in a running session
```

Ultracode is **session-only**. The `effortLevel` settings key and
`CLAUDE_CODE_EFFORT_LEVEL` both reject the value, so it cannot be committed.
`.claude/ultracode.settings.json` exists for the one file-based route:
`claude --settings .claude/ultracode.settings.json`.

**Trust the workspace, or the allowlist is inert.** Until the trust dialog is
accepted once on a machine, Claude Code ignores *every* `permissions.allow`
entry in `.claude/settings.json` and prompts per command. Workflow fan-outs
then stall on prompts, which is the exact failure this setup prevents.

The allowlist is the throughput lever: workflow subagents run in `acceptEdits`
and inherit it, but a shell command missing from it still prompts mid-run. When
you add tooling that agents will call, add it to `permissions.allow` in the
same change.

The build specification is written for parallel agents: each task unit names
what it owns, what it needs, and how to verify it. Point a workflow at a task
unit and its `Verify` commands rather than at an ad-hoc prompt.
