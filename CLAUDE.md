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

Milestone M0 is complete: `Z1` (workspace and contract) and `B1` (fake engine).
Every other task unit is open. Crates other than `dark-contract` and
`dark-engine-fake` are placeholders that compile and do nothing.

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
```

`dark-contract` sits underneath everything and depends on no workspace crate.
It defines the seam: the `Engine` trait, the `Tool` trait, `Event`, `Intent`,
and the error taxonomy. Change it deliberately — every crate recompiles.

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
