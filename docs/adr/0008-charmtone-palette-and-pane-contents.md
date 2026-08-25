# 0008. The shell draws its pane contents, in the Charmtone palette

Date: 2026-08-25

Status: accepted

## Context

Two separate problems, found by running `dark replay` against a recorded
session and looking at a frame rather than at a test.

### The panes were empty

`crates/dark-tui/src/app/render.rs` drew a bordered `Block` for each pane
and nothing inside it. The three views — `views::transcript`,
`views::fogmap`, `views::diff` — were complete and unit-tested, and
nothing outside `views/` ever called any of them. `App` held no field of
any of those types, and `App::apply_event` answered `Event::TokenDelta`
by advancing a rate counter and dropping the text.

So the shell rendered a live header, two empty boxes, a command line, and
a function-key bar, for every session, and had done since task unit `H1`.

Unit tests on a view could not have caught this. The gap was between the
view and the application, and only a test that drives `App` itself
crosses it. `crates/dark-tui/tests/panes_render_their_contents.rs` is that
test.

A second gap sat behind the first. `Event::ConfirmReq` pushed a
`PendingConfirm` that nothing ever removed and no key ever answered, so
the harness could ask a question the shell had no way to answer.

### The palette

Task unit `H2` specifies the accretion-disk palette. It is coherent, but
it puts darkharness in a visual register of its own, and the harness now
spends much of its time beside other terminal agents (see ADR 0007).

## Decision

**Draw the contents.** `render_pane` returns its inner `Rect` and the
caller draws into it. `App` owns a `Transcript`, folds every event into it
by reference before its own match consumes them, and keeps the diff a
confirmation carried so the diff pane has something to show. A pane with
nothing to show says which, rather than rendering an empty box: an empty
box cannot be told apart from missing wiring, and that is exactly what hid
this for so long.

**Answer confirmations.** `App::answer_confirm` removes the oldest pending
request and returns the `Intent::Confirm` that carries the answer. While
one is pending its keys (`y`, `a`, `n`, `Esc`) outrank every other
binding, so a keystroke meant for the modal is never read as command-bar
text. Task unit `A2` requires that every issued call is answered; a shell
that cannot answer one is a way to violate that rule.

**Wrap before drawing.** `views::wrap` wraps styled lines to a width and
hands them back, so a caller can count them, take the tail, and prefix
each visual line. `ratatui::widgets::Paragraph` wraps at draw time and
reports nothing, which makes both a per-line gutter and a bottom-anchored
transcript impossible. A `Line` carries a style separate from its spans'
and `views::diff::render_lines` uses exactly that, so the wrapper carries
the line style onto every line it produces.

**Adopt Charmtone.** `Palette::charmtone` maps the same sixteen tokens
onto the palette Charm's terminal applications use — the `CharmtonePantera`
theme in `charmbracelet/crush`, over `charmbracelet/x/exp/charmtone`. It
is the default. `Palette::accretion_disk` stays, unchanged, as the palette
`H2` specifies; `Theme::with_palette` selects either.

Borrowed with it, because the palette alone is not the look: a solid left
bar marks what a person wrote rather than a name on its own line, a tool
call reads `● name key=value` with `✓` or `×` on its result, wrapped lines
hang under their first line, and every border is rounded.

## Consequences

- `dark-tui` gains one dependency, `unicode-width`, already in the tree
  through `ratatui`. Rule 14 restricts workspace crates and is untouched;
  `cargo xtask check-deps` passes.
- The transcript pane keeps a finished turn on screen. It is the
  conversation, not the running turn — a pane that cleared itself at every
  `TurnStart` would erase the answer as the next question was asked.
- `Transcript::render` takes a scrollback offset, and `PgUp`, `PgDn` and
  `Ctrl+End` drive it.
- The map, files, doc, and explore panes still have nothing behind them.
  Each now says so.

## What this does not settle

`views::fogmap` is still unreached: it needs a `Layout`, which `dark-plan`
builds and nothing forwards to the shell. Wiring that is its own change,
and the map pane names what it is waiting for until then.
