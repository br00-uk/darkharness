# darkharness

A Rust application skeleton, set up so that large agent-driven changes
(Claude Code's [ultracode mode](https://code.claude.com/docs/en/workflows)) can
run against it without stalling.

The application itself is intentionally small: a `lib` + `bin` split with real
error handling, CLI parsing, logging, and a green test suite. The domain logic
in `src/harness.rs` is a placeholder — replace it. Everything around it (lint
policy, one-command checks, CI, agent permissions) is the part that is meant to
last.

## Requirements

Rust is pinned by `rust-toolchain.toml`, so `rustup` installs the right
toolchain with `rustfmt` and `clippy` on first use. No other setup is needed.

## Quick start

```bash
make check                  # format, lint, and test
cargo run -- run --name dev --workers 2
```

```
run "dev" completed 2 task(s)
```

Add `-v` for info logs, `-vv` for debug, or set `RUST_LOG` directly.

## Commands

| Command | What it does |
| --- | --- |
| `make check` | Format, lint, test. The everyday loop |
| `make ci` | Exactly what CI enforces; verifies formatting instead of rewriting it |
| `make fmt` | Rewrite sources to canonical formatting |
| `make lint` | `clippy --all-targets --all-features -- -D warnings` |
| `make test` | Unit, integration, and doc tests |

Run a single test with `cargo test <substring>`, a single file of integration
tests with `cargo test --test cli`, and add `-- --nocapture` to see output.

## Layout

```
src/lib.rs        Public API surface and re-exports
src/config.rs     Config, validated at construction
src/error.rs      Typed errors (thiserror)
src/harness.rs    Harness::run — replace this with real work
src/main.rs       Thin CLI shell (clap + tracing)
tests/cli.rs      Tests that invoke the compiled binary
```

Logic lives in the library so it is testable without spawning a process; the
binary only parses arguments and prints results.

## Lint policy

Lints are declared once in `[workspace.lints]` in `Cargo.toml`, so they apply to
this crate and to any crate added later. `unsafe_code` is `forbid`,
`missing_docs` is on, and clippy runs at `pedantic`. CI treats warnings as
errors, so `make check` passing locally means CI passes too.

## Using ultracode mode

Ultracode combines `xhigh` reasoning with automatic
[dynamic workflow](https://code.claude.com/docs/en/workflows) orchestration.
Start a session with it on:

```bash
claude --effort ultracode
```

**Trust the workspace first.** Until you accept the trust dialog once, Claude
Code ignores every `permissions.allow` entry in `.claude/settings.json` and
prompts for each command instead — which is what makes long parallel runs stall.
Run `claude` interactively here once and accept, then verify:

```bash
claude doctor
```

See [CLAUDE.md](CLAUDE.md) for the full details, including why ultracode cannot
be committed as a persistent setting.

## License

MIT OR Apache-2.0.
