# ADR 0002: `dark-cli` is the composition root for the engine

**Status:** accepted · **Task unit:** `Z1`

## Context

Rule 17 says that every crate except `dark-engine` builds against
`dark-engine-fake` during development.

Something must still construct the real engine and hand it to `dark-core`.
The rule does not say which crate does that.

## Decision

`dark-cli` is the composition root. It is the only crate besides
`dark-engine` that may take a normal dependency on `dark-engine`.

Every library crate holds the engine as `dyn Engine` and takes a development
dependency on `dark-engine-fake` for its tests.

`cargo xtask check-deps` enforces this: a normal dependency on `dark-engine`
from any crate other than `dark-engine` or `dark-cli` fails the check.

## Consequences

- A test suite never pays the mistral.rs build cost.
- Task unit `B1` stays on the critical path, because it supplies the engine
  that seven other task units test against.
- Swapping the engine means editing one crate.
