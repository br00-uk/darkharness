# 0009. Type references, honest checks, and a map on screen

Date: 2026-08-25

Status: accepted

## Context

Three things the previous session found by running the binary rather than
the tests, and left open.

### `dark blast` was a call graph that stopped at the crate boundary

`dark blast Engine` reported that nothing in this repository references
`Engine` — the trait the whole architecture hangs on. So did `Event`, and
`Session`. Two independent causes:

1. **Every grammar's tags query captured a reference only for a call.**
   `rust.scm` had four `@reference.call` patterns and nothing else. A type
   named in a type position — `dyn Engine`, `Vec<Session>`,
   `impl Engine for RealEngine` — produced no reference at all. In Rust
   that is most of what a change to a trait or a struct reaches.

2. **Only `crate::`-anchored `use` paths resolved to a file.**
   `use dark_contract::Engine` leads with a crate name, `resolve_rust_path`
   returned `None`, and `resolve::plan_updates` then deliberately declined
   to fall back to a repository-wide name match, because the name *was*
   governed by an import — the right call when the import points outside
   the repository, and exactly wrong for a workspace sibling.

The report also contradicted itself: it printed "nothing else in this
repository references it" beside a reach of 7, because its file list drops
the files the symbol is defined in and an empty list was read as an empty
reach.

### `dark doctor` cited task units that had landed

Six checks reported `[PENDING] … remedy: Wait for task unit F1` (or `B2`,
or `G1 to G5`). Every one of those task units had landed. `doctor` said
`dark-explore` registered no tree-sitter grammar while `dark explore` was
parsing 359 files with thirteen of them.

### The fog map had never been handed a map

`views::fogmap` computes a layout and draws it, with its own tests. `App`
held no `Layout`, nothing built one, and `Event::MapChanged` fell through
`apply_domain_event`'s wildcard.

## Decision

**A type in a type position is a reference.** `rust.scm` gains a bare
`(type_identifier) @name @reference.type` and a pattern for the type a
path leads with. A bare pattern necessarily also matches the name each
type definition declares, so `extract::file::partition_tags` drops a
reference whose name node *is* a definition's name node — by node
identity, which is exact, costs one pass, and holds for every grammar. A
query author can now write the broad pattern and rely on that.

**A workspace sibling's crate name resolves to that crate.**
`workspace_crate_dir` maps the first path segment to a directory: a
sibling of the importing file's own crate directory, or a directory at the
repository root, with `_` back to `-`. Both are confirmed against a
`Cargo.toml` in the discovered path set, and the file built from it is
confirmed too, so a first segment naming an external crate resolves to
nothing rather than to a same-named local type. A package whose name
differs from its directory name is not found; that costs a resolution, it
never produces a wrong one.

**A remedy names an action, not a task unit.** `Finding::pending` takes a
remedy string. "Run dark setup to install a model" cannot go stale the way
"Wait for task unit B2" did. Three checks became real rather than pending:
grammar ABI versions (against the range this `tree-sitter` accepts), model
manifest hashes (rehashed and compared), and pack staleness (against each
pack's own refresh policy). A test asserts no remedy anywhere contains the
words "task unit".

**The map reaches the shell through the composition root.** `dark-tui`
depends on `dark-contract` alone (Rule 14) and cannot open a map store, so
`App::set_map` takes a computed `Layout` and `app::run` takes a
`&mut dyn FnMut(&str) -> Option<Layout>` that the caller supplies.
`crates/dark-cli/src/fogmap.rs` is that caller: it converts
`dark-cartograph`'s tickets into the view's own shape, the same joining
role `scrape.rs` and `pack.rs` already play.

`dark-cartograph` gains `snapshot::map_snapshot`, because `frontier`
answers a different question. A drawing needs every ticket, resolved and
out-of-scope included, plus the blocking edges — `Frontier` and `Blocked`
are one stored status split by whether anything still blocks the ticket.

## Consequences

- The S-graph grew from 5267 edges to 7527 on this repository, and the
  F-graph from 122 to 235 as cross-crate imports began resolving. `dark
  seams` now surfaces cross-crate seams it could not see before.
- `blast Event` went from 0 reachable to 288, `blast Transcript` from 0 to
  64.
- **The other twelve grammars still capture calls only.** Rust was fixed
  and verified against a real corpus; Go, TypeScript, Java and the rest
  were not, because a query written without a corpus to check it against is
  a guess. Their blast radii remain call graphs.
- Reference resolution is still capped by `NameOnly` needing a
  repository-wide unique name. A call to a method named `new` or `run`
  resolves to nothing, by design (`F2`, "do not report a guessed reference
  as resolved"), so a transitive reach through such a call stops there.
  This is why `blast Engine` reports 7 rather than a number in the
  hundreds.
- `dark doctor`'s live-generation check is still pending on this machine:
  it needs a model installed, and its remedy now says so. It does not
  attempt a load.
