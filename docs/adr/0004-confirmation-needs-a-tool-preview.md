# ADR 0004: a confirmation cannot show a diff the tool has not computed

**Status:** accepted · **Task unit:** `A2`, and it constrains `A4`

## Context

Task unit `A4` is explicit about what a person sees before they approve an
action:

> Show the exact unified diff or the exact command. Do not show a summary.

The turn loop is what gates a call (`A2`, Do step 5.1 and 5.2): it checks the
policy, emits `ConfirmReq`, and waits for the intent before it invokes the
tool. So the loop must produce the diff *before* the tool runs.

It cannot. The `Tool` trait in `dark-contract` has one method that does
anything:

```rust
async fn invoke(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<ToolResult>;
```

A diff exists only after a tool has read the file, applied the edit, and
compared the two. `invoke` computes the diff and performs the write in the
same call. There is no way to ask a tool what it *would* do.

The two requirements are therefore not both satisfiable with the trait as
`Z1` froze it. This is a contradiction in the build specification, not a
choice we are free to make either way.

## Decision

The turn loop gates on the action kind and the exact arguments, and does not
claim to show a diff it does not have.

- **A command is exact already.** `Action::Exec` carries the command line the
  loop is about to run, character for character. `A4` step 3 is satisfied for
  execution today.
- **A denial needs no diff.** A `deny` policy value refuses the call without
  ever showing a prompt, so the missing diff costs nothing there.
- **A write shows its tool name and its exact arguments**, which name the
  file and the intended change, but not a rendered unified diff.

`ToolSet` records an `ActionKind` for each tool at registration, because
`ToolSchema` says whether a tool mutates but not whether it writes a file or
runs a command, and the policy needs that difference: a repository may allow
a write and deny an execution.

## What would close the gap

A preview method on the `Tool` trait:

```rust
async fn preview(&self, args: serde_json::Value, ctx: &ToolCtx) -> Result<Option<ToolResult>>;
```

A tool that can compute its effect without applying it returns the
`ToolResult` it *would* produce, diff included; one that cannot returns
`None`, and the loop falls back to what it does today. The loop then passes
the previewed diff into `Action::Write`.

That is a change to `dark-contract`, which every crate depends on, so it
recompiles the workspace and it is outside what task unit `A2` owns. It also
belongs with the two other contract gaps already recorded, so that one change
covers all three:

- `Event` carries no variant for the text a person submits, so a transcript
  replay rebuilds assistant and tool messages but never a `Role::User` one.
- `Event::ToolResult` carries a summary rather than the tool's content, so a
  replayed tool reply keeps the headline and loses the output.

## Consequences

- A person approving a write today sees the tool name and the exact arguments,
  not a rendered diff. That is weaker than `A4` asks for, and it is recorded
  here rather than hidden behind a summary that would read as if it satisfied
  the rule.
- Nothing about this weakens Rule 34. A write outside the repository root is
  refused by the tool itself, and no configuration reaches that decision.
- The fallback stays after the trait grows a preview: a tool that cannot
  compute its effect cheaply is allowed to say so.
