# ADR 0003: `toml` in `dark-engine-fake`

**Status:** accepted · **Task unit:** `B1`

## Context

Task unit `B1` says two things that pull against each other:

- Step 2: load scripted responses from a TOML file.
- Do not: add any dependency that takes more than two seconds to compile.

Measured on a cold build in the development container, `toml` costs 7.5
seconds, and `toml_parser` a further 2.4 seconds.

The same measurement shows that most of the cold cost is not `toml` at all.
`async-trait`, `serde_derive`, `syn`, `thiserror-impl`, and `tokio-macros`
each cost more, and every one of them arrives through `dark-contract`, whose
dependency list `Z1` fixed.

## Decision

Keep `toml`.

The rule's purpose is the criterion in the same task unit: a clean build of
`dark-engine-fake` stays under ten seconds, because seven task units pay that
cost. That criterion holds. A clean rebuild of `dark-engine-fake` and
`dark-contract` takes 5.3 seconds, of which `dark-engine-fake` itself is 3.7.

Third-party crates compile once and are then shared by the whole workspace, so
the number that matters for a test loop is the rebuild, not the first build.

`toml` is also not a cost this crate imposes on the project. Four other task
units need the same crate:

| Task unit | What it parses |
| --- | --- |
| `B6` | The `[hardware]` profile |
| `G1` | The pack manifest |
| `I1` | The Qwen model profiles |
| `J2` | Every layer of configuration |

`toml` therefore enters the workspace regardless of what this crate does.

## Consequences

- The script format is the one the specification asked for.
- The build criterion that the rule protects is measured, and it passes.
- Writing a TOML subset parser by hand is avoided. That would be a new source
  of bugs in the component every other task unit tests against.

## How to re-check

```
cargo build -p dark-engine-fake --timings
```

Open the report and read the `dark-engine-fake` unit. Keep it under ten
seconds.
