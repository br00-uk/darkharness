# darkharness

A local coding harness. One Rust binary that contains its own inference
engine, task tracker, documentation index, and terminal application.

**The primary requirement:** after `dark setup` completes, you disconnect the
network and keep working.

## Status

Early. Milestone **M0** is complete: the workspace, the shared contract
(`Z1`), and the scripted engine used for development (`B1`). The remaining
task units are open, so the crates they own are placeholders that compile and
do nothing.

`PRD.md` is the authoritative specification and tracks the plan.

## Hardware floor

The coding loop needs a graphics processor or Apple Silicon. A
central-processor-only machine works, but a 14B model generates a few tokens
each second there, so a turn with thinking takes minutes rather than seconds.
On that hardware the default profile drops to a 4B model with thinking off.

Memory is the limit that shapes everything else. Below 24 GB the architect,
worker, and scout roles share one resident model. That is the default
configuration, not a failure state.

Run `dark tune` to measure your machine and `dark doctor` to check the
installation.

## Build artefacts

One binary does not run everywhere. The release ships three:

| Artefact | Features | Platforms |
| --- | --- | --- |
| `dark-cpu` | default | All. This artefact is portable. |
| `dark-metal` | `metal` | macOS arm64 |
| `dark-cuda` | `cuda,flash-attn` | Linux and Windows x64 with NVIDIA |

`dark doctor` detects the accelerator and warns when you run `dark-cpu` on a
machine that has a usable graphics processor.

## Security posture

**The harness runs tools with the privileges of the person who started it.**
It gates actions; it does not contain them. It is not a sandbox. Read the
confirmation prompts: they show the exact diff or the exact command, never a
summary.

Three guarantees hold regardless of configuration:

- A write outside the repository root is always denied. No setting changes
  this.
- A repository configuration file cannot widen its own permissions. It can
  only make the policy stricter.
- Dark mode blocks network egress at the connector, before name lookup. Only
  `dark-airlock` can construct an HTTP client, and `cargo deny` fails the
  build if another crate gains one.

File content, command output, and documentation are treated as untrusted
input and are wrapped as data.

## Build from source

```bash
make check     # format, lint, test, dependency rules
make ci        # what CI enforces
cargo run -p dark-cli -- doctor
```

Rust is pinned by `rust-toolchain.toml`, so `rustup` installs the right
toolchain with `rustfmt` and `clippy` on first use.
[`cargo-nextest`](https://nexte.st) and
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) are needed for
`make test` and `make deny`.

## Architecture

```
dark-tui          Events in, intents out. Depends on dark-contract only.
      ▲ Event (broadcast) │ Intent (mpsc)
dark-core         Session, turn loop, context assembly
      │   dark-plan · dark-explore · dark-lexicon · dark-tools
      │   dark-cartograph
      ▲ dyn Engine
dark-engine       mistral.rs, resident set     dark-engine-fake (scripted)
```

`dark-contract` underpins everything and depends on no workspace crate. The
layering is enforced, not merely documented: `cargo xtask check-deps` fails
the build when a crate reaches across a boundary, and names the rule it broke.

See [CLAUDE.md](CLAUDE.md) for the working conventions and
[docs/adr/](docs/adr/) for decisions taken so far.

## Licence

MIT OR Apache-2.0.
