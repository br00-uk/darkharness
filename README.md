# darkharness

A coding harness that understands your repository, and hands that
understanding to whichever agent does the work.

darkharness speaks the [Agent Client Protocol](https://agentclientprotocol.com),
so it drives the coding agents you already have installed — Claude Code,
opencode, Codex, Gemini CLI, Qwen Code and others — with no further
setup. It also carries its own inference engine, so it can run a local
model instead and keep working with the network unplugged.

The agent is the interchangeable part. What darkharness adds is the part
that does not come in the box:

- **Discovery.** A static picture of the repository: what depends on
  what, where the seams are, and what a change to one symbol can reach.
- **Retrieval.** Versioned documentation packs, indexed for search, so an
  agent quotes the version you actually depend on.
- **Planning.** A map of decisions rather than a task list, kept in a
  file you commit, that two sessions can work at once.

An agent that starts a task already knowing your repository's shape, your
dependencies' real documentation, and the plan it is working inside is a
better agent than the same one starting cold.

## Using an installed agent

```bash
dark acp list                                   # what this machine can start
dark acp run opencode "add a health check"      # hand it the work
```

When the agent asks permission, darkharness answers with its own policy:
you see the exact diff or the exact command, and a write outside the
repository root is refused whatever the agent asks. The whole session is
recorded, so `dark replay` plays it back like any other.

Be clear about the limit. That gate covers what the agent *asks* for.
darkharness cannot yet force it to ask: the protocol's file and terminal
callbacks are not wired, so an agent that reads and writes files itself
does so outside this policy. Until they are, a foreign agent is gated by
its own good manners plus darkharness's answer — not contained by it.

`dark acp list` prefers an agent binary already on your `PATH` over the
`npx <package>@latest` form the editor extensions use, because that form
downloads the agent on every launch. It shows you which of the two you
are getting.

## Discovery

```bash
dark explore              # analyse the repository
dark seams                # where the natural boundaries are
dark blast <symbol>       # what a change to this can reach
```

Twelve languages — Rust, Go, TypeScript, TSX, JavaScript, Python, Java,
C#, Ruby, C, C++ and SQL — parsed with tree-sitter into a file graph, a
symbol graph, and a module graph, joined with co-change read from
`git log`.

A **seam** is a boundary that a change tends not to cross, scored from
five terms: edge betweenness, whether it crosses a community, how
interface-like the definitions are, how rarely the two sides change
together, and whether tests already cover it. `dark blast` walks the
symbol graph backwards and reports both the unbounded reach and the reach
once the seams stop it — the gap between the two is how well the code
already contains the change you are about to make.

The analysis is deterministic: the same commit and the same configuration
produce identical bytes, so a result can be cached, shared, and compared
between machines.

## Retrieval

```bash
dark pack add ./internal-docs            # a directory of Markdown
dark pack add ./target/doc/api.json --source-kind docsrs
dark pack list
```

A pack is a versioned slice of documentation, chunked and indexed twice:
BM25 for the words that appear, and int8-quantised vectors for the
meaning. Results are combined with reciprocal rank fusion, so a query
that only one index understands still lands.

Each pack records the version it was built from and a staleness policy,
so an agent citing `tokio` cites the `tokio` you depend on rather than
whatever it remembers. Ingest checks the source's licence before it
copies anything.

## Planning

```bash
dark map list
dark map show <map>
dark map health              # are the tickets sized sensibly?
```

A map holds **decision** tickets rather than tasks, and it is complete
when nothing is left to decide. A ticket is research, a prototype, a
question for a person, or ordinary work; tickets block one another, and
the **frontier** is everything unblocked right now. Taking one off the
frontier claims it under a lease, so two sessions never work the same
ticket.

The record is an append-only JSONL journal you commit to Git, marked
`merge=union`, so two people — or two agents — working at once merge
cleanly instead of conflicting. The SQLite database beside it is a
projection: `dark map rebuild` reconstructs it from the journal, and
nothing is lost if you delete it.

## Working offline

darkharness carries its own inference engine, so the whole harness runs
with no network at all:

```bash
dark setup                   # the one command that uses the network
dark run "..." --dark        # blocks every egress for this run
```

`--dark` blocks network egress at the connector, before name lookup, and
refuses an ACP agent that would send your code to a remote service or
that needs a download to start — naming the local model as the remedy.

Discovery and planning never touch the network at all: they read files,
run `git log`, and read a journal. Searching a pack is the same. Two
things do need something: building a pack's dense index needs the local
embedding model to be installed, and adding a pack from a sitemap needs
to fetch those pages, which is why that one source is refused and points
you at fetching them yourself.

This is a capability rather than the point. Most people have a connection
most of the time and will want a frontier model doing the work. It
matters when you are on a plane, inside an air-gapped network, or working
on something that must not leave the machine.

## Status

Every task unit in `PRD.md` has landed, and the command surface is wired
to the crates behind it. `dark models quantize` is the one command that
still answers "not yet": converting between quantisations needs a
mistral.rs path the engine does not expose, and nothing depends on it.

Two things are honestly unproved, both for want of hardware rather than
code. The memory estimator is pinned against five published model
configurations rather than measured memory, and the live weight loading
and token streaming paths are compile-true and unit-tested but have never
run against real weights — no machine in this build has an accelerator.
`cargo xtask airgap` reports `NO MODEL` for its turn steps rather than
passing them. `docs/adr/0006` names each deferred seam.

The ACP client is exercised end to end against a fixture agent that
speaks the real protocol over a real subprocess. Its `fs/*` and
`terminal/*` callbacks are not wired yet, so a foreign agent currently
does its own file access rather than going through this harness's
sandbox. `docs/adr/0007` records that.

## Hardware floor

This applies to the local model only. Driving an installed ACP agent
needs no accelerator at all.

The local coding loop wants a graphics processor or Apple Silicon. A
central-processor-only machine works, but a 14B model generates a few
tokens each second there, so a turn with thinking takes minutes rather
than seconds; on that hardware the default profile drops to a 4B model
with thinking off.

Memory is the limit that shapes everything else. Below 24 GB the
architect, worker and scout roles share one resident model. That is the
default configuration, not a failure state.

Run `dark tune` to measure your machine and `dark doctor` to check the
installation.

## Build artefacts

One binary does not run everywhere. The release ships three:

| Artefact | Features | Platforms |
| --- | --- | --- |
| `dark-cpu` | default | All. This artefact is portable. |
| `dark-metal` | `metal` | macOS arm64 |
| `dark-cuda` | `cuda,flash-attn` | Linux and Windows x64 with NVIDIA |

`dark doctor` detects the accelerator and warns when you run `dark-cpu`
on a machine that has a usable graphics processor.

## Security posture

**The harness runs tools with the privileges of the person who started
it.** It gates actions; it does not contain them. It is not a sandbox.
Read the confirmation prompts: they show the exact diff or the exact
command, never a summary.

Three guarantees hold regardless of configuration. They apply in full to
the local model, and to every permission a foreign agent asks for:

- A write outside the repository root is always denied. No setting
  changes this.
- A repository configuration file cannot widen its own permissions. It
  can only make the policy stricter.
- Dark mode blocks network egress at the connector, before name lookup.
  Only `dark-airlock` can construct an HTTP client, and `cargo deny`
  fails the build if another crate gains one.

File content, command output, and documentation are treated as untrusted
input and are wrapped as data.

An ACP agent is a subprocess this harness did not write. It is gated by
the policy above, but what it sends over its own connection is outside
this harness's control — `dark acp list` says which agents are known to
send code to a remote service, and dark mode refuses them.

## Build from source

```bash
make check     # format, lint, test, dependency rules
make ci        # what CI enforces
cargo run -p dark-cli -- doctor
```

Rust is pinned by `rust-toolchain.toml`, so `rustup` installs the right
toolchain with `rustfmt` and `clippy` on first use.
[`cargo-nextest`](https://nexte.st) and
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) are needed
for `make test` and `make deny`.

## Architecture

```
dark-tui          Events in, intents out. Depends on dark-contract only.
      ▲ Event (broadcast) │ Intent (mpsc)
dark-core         Session, turn loop, context assembly
      │   dark-plan · dark-explore · dark-lexicon · dark-tools
      │   dark-cartograph
      ▲ dyn Engine
dark-engine       mistral.rs, resident set     dark-engine-fake (scripted)

dark-acp          Speaks the Agent Client Protocol to another agent's
                  subprocess, over stdio. Depends on dark-contract only.
```

`dark-contract` underpins everything and depends on no workspace crate.
The layering is enforced, not merely documented: `cargo xtask check-deps`
fails the build when a crate reaches across a boundary, and names the
rule it broke.

See [CLAUDE.md](CLAUDE.md) for the working conventions and
[docs/adr/](docs/adr/) for decisions taken so far.

## Licence

MIT OR Apache-2.0.
