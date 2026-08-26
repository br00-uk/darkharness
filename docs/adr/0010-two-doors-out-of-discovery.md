# 0010. Two doors out of discovery, and a way into the wayfinder

Date: 2026-08-25

Status: accepted

## Context

darkharness has two operating modes. A person starting from nothing uses
the wayfinder — `/plan` charts a map of decisions. A person with an
existing codebase runs seam discovery first, and then wants one of two
things: to **extend** what is there, or to **refactor** it.

Only the first half of that existed, and not in a way anybody could reach.

### The wayfinder had no door

`dark-plan` was not a dependency of `dark-cli`. Nothing outside
`dark-plan` called it. In `shell.rs`, `Intent::Command(text)` and
`Intent::Submit(text)` shared a match arm, so every slash command in the
build specification's table was sent to the model as ordinary text:
`/plan` did not chart a map, it asked a language model to talk about
charting one, and the model obliged. `dark map chart --resume`, which task
unit `E1` step 5 specifies, did not exist.

The charting pipeline was complete and orphaned, the same way the terminal
views were before ADR 0008.

### The joins were designed and never made

`ChartInput.repo_summary` existed as a field every test passed `""` to.
`SeedReport` — stage 2's input — was shaped precisely like `dark
explore`'s output document. Two ends of a cable, both stripped.

## Decision

**A command is not a prompt.** `crates/dark-cli/src/command.rs` holds the
in-session table and returns an outcome the shell acts on. A command with
no implementation reports itself unbuilt rather than reaching the model,
which would answer as though it had run.

**`dark plan` is the wayfinder's door**, joining `dark-plan` to
`dark-explore`'s analysis and `dark-cartograph`'s store in the composition
root — the role `scrape.rs`, `pack.rs` and `fogmap.rs` already play. It
reads the written analysis rather than recomputing it: discovery is its
own step, and a command that quietly spends a minute on it hides where the
time went.

**`dark extend` derives; it does not ask.** The obvious build is a model
reading the codebase and describing its style. That is slow,
unreproducible, and wrong often enough to be dangerous — an agent told the
wrong convention applies it everywhere. `dark_explore::style` computes the
profile from data extraction already holds: every definition's name, kind,
export status and doc comment. One bounded model call adds prose, from
those facts and the module list, never from the repository. The output
separates the halves under **Counted** and **Summary** headings.

**`dark refactor` argues from the metrics.** Suggestions are a lookup
against modularity, community count, bridges, and Martin's coupling
metrics, with the numbers printed. The rule order is itself the argument:
a god-module outranks a high modularity, because distributing a repository
shaped like that distributes the problem.

**Nothing is overwritten.** `dark_agentsmd::write::upsert` replaces a
marked region and refuses a file that exists without markers. A generated
section never touches a line it did not write.

**Vendor documentation is named, never fetched.** This is the one part of
the feature that wants the network at a moment other than `dark setup`, so
it is explicit: the table names sources, each prints the `dark pack add`
that would fetch it, and dark mode refuses the step rather than skipping
it silently.

## Consequences

- The explore report gains a `languages` field and its schema version goes
  to 2. `Language::from_path` already ran on every file and the answer was
  discarded; a report that does not say what a repository is written in
  cannot tell an agent to keep writing it.
- `cached_report` falls back to the newest analysis on disk when the tree
  has moved, and says which tree it is from. Without that, `dark extend`
  failed on its second run — it writes `AGENTS.md`, which changes the
  tree, which invalidated the analysis it had just used.
- `Casing::Other` can never win a convention vote. Markdown headings
  arrive as `section` definitions, and "name a section in no consistent
  convention, 80% of them do" is a sentence that contradicts itself.

## What this does not settle

- The error-handling idiom is still absent from the style profile. It
  cannot be told from definition names and kinds alone, and a guess an
  agent then applies across a codebase is worse than no answer.
- `dark extend`'s summary call has never run against real weights. The
  counted half lands without it, and `--no-summary` skips it outright.
- `/map`, `/docs`, `/model` and `/think` are dispatched but report
  themselves unbuilt. That is a smaller lie than the one they told before.
