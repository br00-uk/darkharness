# ADR 0001: The dependency list for `dark-contract`

**Status:** accepted · **Task unit:** `Z1`

## Context

Rule 15 says that `dark-contract` depends on `serde`, `thiserror`, `ulid`,
`bytes`, and `tokio` only.

Task unit `Z1` also specifies the exact signatures that this crate must
publish. Four of them need a crate that Rule 15 does not list:

| Item | Needs | Why |
| --- | --- | --- |
| `Engine`, `Tool` | `async-trait` | Both traits declare `async fn` and both are used as `dyn`. |
| `Engine::stream` | `futures-core` | The return type is `BoxStream<'static, Result<Chunk>>`. |
| `Engine::stream` | `tokio-util` | The cancel parameter is `CancellationToken`. |
| `ToolCall::args`, `ToolSchema::parameters`, `Tool::invoke` | `serde_json` | The argument type is `serde_json::Value`. |

The two statements cannot both hold. The specified code does not compile
against the specified dependency list.

## Decision

Read Rule 15 as a rule about weight, not about an exact list. The
prohibition that carries the intent is the one in `Z1`: do not add
`mistralrs`, `ratatui`, `reqwest`, or `rusqlite`.

`dark-contract` therefore depends on the five crates that Rule 15 names, plus
`async-trait`, `futures-core`, `tokio-util`, and `serde_json`. Each addition
is needed by a signature that the specification mandates. Each is a small
async or serialisation crate with no native code.

`cargo xtask check-deps` holds this list as an allowlist. A tenth dependency
fails the check, so the boundary stays enforced rather than merely documented.

## Consequences

- The specified signatures compile unchanged.
- The crate stays cheap to build, which is what keeps `dark-engine-fake`
  usable as the development engine.
- Adding a dependency to `dark-contract` is a deliberate act: it fails CI
  until someone edits the allowlist in `xtask/src/check_deps.rs`.

## Alternatives rejected

- **Define our own stream and cancellation types.** This would keep the
  literal list, but every engine implementation would then convert at the
  boundary for no benefit.
- **Move the traits out of `dark-contract`.** This would break the layering
  that the whole workspace is built around.
