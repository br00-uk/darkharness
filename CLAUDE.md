# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

`make` is the single entry point; CI runs the same commands, so a green
`make ci` locally means a green CI run.

```bash
make check        # fmt + lint + test — the everyday loop
make ci           # what CI enforces: fmt-check + lint + test + build
make fmt          # rewrite to canonical formatting
make lint         # clippy --all-targets --all-features -- -D warnings
make test         # unit + integration + doc tests
```

Running a subset:

```bash
cargo test config::tests::rejects_blank_name   # one test by path
cargo test -- --exact harness::tests::runs_one_task_per_worker
cargo test --lib                               # unit tests only
cargo test --test cli                          # one integration test file
cargo test --doc                               # doctests only (skipped by --all-targets)
cargo test rejects -- --nocapture              # substring filter, show output
```

`cargo test --all-targets` does **not** run doctests. `make test` covers both.

Running the binary: `cargo run -- run --name dev --workers 2`. Verbosity is
`-v` (info), `-vv` (debug), `-vvv` (trace); `RUST_LOG` overrides it when set.

## Architecture

A `lib` + `bin` split in one package. `src/main.rs` is a thin shell that parses
clap arguments, initialises tracing, and delegates; all logic lives in the
library so it is testable without spawning a process. Preserve that split —
logic added to `main.rs` becomes untestable except through `tests/cli.rs`.

```
src/lib.rs      Re-exports the public surface (Config, Harness, Report, Error, Result)
src/config.rs   Config — validated in Config::new, fields private, so any Config value is valid
src/error.rs    Typed Error enum (thiserror), #[non_exhaustive]
src/harness.rs  Harness::run — the seam where real work belongs; currently a placeholder
src/main.rs     CLI shell only
```

Two deliberate choices worth keeping:

- **Errors are typed in the library, `anyhow` only in the binary.** `Error`
  (thiserror) lets callers match on failure modes; `main.rs` converts to
  `anyhow::Error` and adds context for humans. Don't introduce `anyhow` into
  the library.
- **`Harness::run` returns `Result` although it is currently infallible.** That
  is intentional: adding fallible work there must not become a breaking change.

`Config` fields are private with accessors, and `Config::new` is the only
constructor that validates. Adding a public field would break that invariant.

## Lint policy

Lints are declared once in `[workspace.lints]` in `Cargo.toml` and inherited via
`lints.workspace = true`, so they cover this crate and any crate added later.
Set them there, not with crate-level `#![deny(...)]` attributes.

`unsafe_code` is **forbid** (cannot be overridden locally), `missing_docs` is on
— every public item needs a doc comment — and clippy runs at `pedantic`. Because
pedantic includes `missing_errors_doc`, any public function returning `Result`
needs an `# Errors` section. `module_name_repetitions` and `must_use_candidate`
are allowed as more noise than signal.

`rustfmt.toml` uses **stable-only** options on purpose. Nightly-only keys
(`imports_granularity`, `group_imports`, `wrap_comments`) warn on every run on a
stable toolchain, and that noise reads as a fixable problem when it isn't.

## Ultracode mode

This repository is set up for [ultracode](https://code.claude.com/docs/en/workflows#let-claude-decide-with-ultracode)
— `xhigh` reasoning plus automatic dynamic-workflow orchestration.

```bash
claude --effort ultracode        # or /effort ultracode in a running session
```

Four things about it are load-bearing here:

**It cannot be committed as a persistent setting.** Ultracode is session-only.
The `effortLevel` settings key and `CLAUDE_CODE_EFFORT_LEVEL` both reject the
value; only `/effort ultracode`, `--effort ultracode`, or `"ultracode": true`
passed via `--settings` turn it on. `.claude/ultracode.settings.json` exists for
that last form: `claude --settings .claude/ultracode.settings.json`. Do not
"fix" `.claude/settings.json` by adding an `effortLevel` of `ultracode` — it is
not a valid value there.

**Trust the workspace, or the allowlist is inert.** Until the trust dialog is
accepted once on a machine, Claude Code ignores *every* `permissions.allow`
entry in `.claude/settings.json` and prompts per command. Workflow fan-outs then
stall on prompts, which is the exact failure this setup exists to prevent. The
warning appears at startup and in `claude doctor`; accept the dialog in one
interactive session here to clear it.

**Workflows must stay enabled.** Ultracode disappears from the `/effort` menu
when workflows are off, and `--effort ultracode` silently degrades to plain
`xhigh`. `.claude/settings.json` sets `"disableWorkflows": false` so a
user-level opt-out does not silently weaken this repo — but managed
(organisation) settings still win, and `CLAUDE_CODE_DISABLE_WORKFLOWS=1`
overrides at startup.

**The allowlist is the throughput lever.** Workflow subagents run in
`acceptEdits` mode and inherit the tool allowlist, so file edits are automatic,
but a shell command *not* on the allowlist still prompts mid-run. Every routine
cargo/rustup/make command is already allowed. When adding tooling that agents
will call (`cargo nextest`, `cargo deny`, a new script), add it to
`permissions.allow` in the same change, or long runs will block on it.

`workflowSizeGuideline` is `medium` (Claude aims for fewer than 15 agents).
Raise it to `large` for genuine repo-wide sweeps; it is advice to the model, not
a cap. The runtime caps runs at 16 concurrent and 1,000 total agents regardless.

Ultracode costs meaningfully more per task and does not persist across sessions
— drop to `/effort high` for routine work.

### What makes workflows effective here

Workflows are strongest when a check is crisp and fast, because the productive
pattern is "run the check, fix what failed, repeat until it passes." `make check`
is that command, and it is deliberately the same one CI runs. Point workflows at
it rather than at ad-hoc cargo invocations, so agents converge on the definition
of done that CI will actually enforce.

## Conventions

- Add dependencies with `cargo add` so the lockfile stays consistent; the
  lockfile is committed (`publish = false`, this is an application).
- Tests live next to the code in `#[cfg(test)] mod tests` for unit tests, and in
  `tests/` for anything that exercises the compiled binary via
  `env!("CARGO_BIN_EXE_darkharness")`.
- `make fmt` before committing, or CI's format job fails first and hides the
  real errors behind a formatting diff.
