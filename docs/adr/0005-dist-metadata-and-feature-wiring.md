# ADR 0005: the release pipeline needs two changes outside task unit J4

**Status:** accepted, blocking · **Task unit:** `J4`, and it names `B2` to
`B7` as the owners of the fix

## Context

Task unit `J4`'s brief lists the files it owns:
`.github/workflows/release.yml`, `dist-workspace.toml`,
`xtask/src/release.rs`. `CLAUDE.md`'s conventions are explicit: "Change
only the files that your task unit owns. If you must change another,
stop and write an ADR." Building the release pipeline surfaced two
places where the work J4's "Do" list asks for needs a change to a file
this task unit does not own.

### 1. `cargo-dist` will not package a binary that opts out

Every crate in the workspace inherits `publish = false` from
`[workspace.package]` in the root `Cargo.toml`. `cargo-dist` (checked
against the installed `0.32.0` binary, not assumed) reads `publish =
false` as "do not distribute this" and requires an explicit opt-in on
the package whose binary should ship:

```toml
[package.metadata.dist]
dist = true
```

That stanza belongs in `crates/dark-cli/Cargo.toml`. `dark-cli` is not a
file `J4` owns.

Confirmed by running `dist plan` against this workspace as it stands:

```
× This workspace doesn't have anything for dist to Release!
```

### 2. `dark-cli`'s `cuda` and `metal` features are cfg markers, not real features

`crates/dark-cli/Cargo.toml` already declares:

```toml
[features]
cuda = []
metal = []
```

added for task unit `J3`'s `dark doctor` (Rule 18: the binary must know
which artefact it is, at compile time). Its own comment says task unit
`J4` "wires `cuda` and `metal` through to `dark-engine`'s own features
when it builds the release pipeline." That wiring is not possible yet,
independent of file ownership: `crates/dark-engine/Cargo.toml` depends
on nothing but `dark-contract` today, so there is no `dark-engine`
feature for `dark-cli/cuda` or `dark-cli/metal` to forward to. Task
units `B2` to `B7` bring in mistral.rs and, per `CLAUDE.md`, choose its
feature flags as part of that work.

`dark-cli` also has no `flash-attn` feature at all. Confirmed:

```
$ cargo build -p dark-cli --features cuda,flash-attn
error: the package 'dark-cli' does not contain this feature: flash-attn
```

The README's "Build artefacts" table and Section 4.5 name `cuda,flash-attn`
as `dark-cuda`'s feature set, so this feature needs to exist before the
`dark-cuda` job in `release.yml` can run for real.

## Decision

Do not add `[package.metadata.dist]` to `crates/dark-cli/Cargo.toml`, and
do not add a `flash-attn` feature or a `cuda = ["dark-engine/cuda"]` /
`metal = ["dark-engine/metal"]` forward to it, from this task unit. Both
are two-line, low-risk changes; CLAUDE.md's ownership rule does not carve
out an exception for a change that looks safe, so this ADR is the
documented stop instead.

`dist-workspace.toml` and `release.yml` are written and committed as
though the `dist = true` stanza already existed: they are complete and
correct configuration, checked against the real `dist` binary
(`dist generate`, `dist plan`, `dist init` all run against this
workspace as part of building this task unit), and need no further
change once the stanza lands. Today, `dist plan` and `dist build` report
"nothing to release" — a real, honest answer, not a bug in either file.

`release.yml`'s `dark-metal` and `dark-cuda` jobs are written in full but
gated `if: false`, with a comment naming exactly what flips them on: the
feature-forwarding change above, landing with `B2` to `B7`.

## Consequences

- `cargo xtask check-binary-size` and `cargo xtask check-reproducible`
  (owned by `J4`, in `xtask/src/release.rs`) do not depend on either gap:
  both build `dark-cli` directly with `cargo build`, not through `dist`,
  and both use default features (the `dark-cpu` artefact), which needs
  neither stanza.
- `M5`'s exit condition in PRD Section 7 ("`cargo xtask airgap` passes")
  and Section 9's definition of done both name `dist`-adjacent checks
  that stay short of fully green until this ADR's prerequisite lands
  and `B2` to `B7` complete. That is expected staging, not a regression
  introduced by `J4`.
- Whoever next edits `crates/dark-cli/Cargo.toml` — most likely `B2`,
  which `CLAUDE.md` already asks to touch `crates/dark-engine/Cargo.toml`
  for its own feature flags — should add the `[package.metadata.dist]`
  stanza and the `cuda`/`metal`/`flash-attn` forwarding in the same
  change, then flip the two `if: false` jobs in `release.yml` and delete
  this ADR's "blocking" status.
