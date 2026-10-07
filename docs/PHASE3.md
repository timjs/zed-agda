# Phase 3: optimisations

Phase 3 makes what phases 1 and 2 ([`PHASE1.md`](PHASE1.md),
[`PHASE2.md`](PHASE2.md)) built faster and more reliable, starting with hover
on goals. The goal commands that only show information (the goal's type with
the type of its expression, the normal form, why a name is in scope) and
creating a helper function belong to phase 2, polish.

## Subsumes the original phase 3

The original plan ([`PLAN.md`](PLAN.md), section 9.1) had a phase 3 called
"Commands": Agda's commands as Zed tasks (`tasks.json`), the
`agda-bridge client` with its socket, keymap snippets for Emacs and vim
leaders, and prompts in the terminal. This phase replaces it:

- The route through tasks was dropped after phase 0 ([`PHASE0.md`](PHASE0.md),
  "Decisions after phase 0"), because the language server can do the same
  with code actions, without a terminal. `languages/agda/tasks.json` and its
  keymap are gone.
- `agda-bridge client` and its socket stay, as a tool for debugging.
- What the tasks were for, Agda's commands with a typed expression, comes back
  in phase 2 as code actions on goals ([`PHASE2.md`](PHASE2.md), "Next
  steps"), where the expression is the goal's text instead of a prompt.

## Why hover on goals was slow, or empty

Hover on a name was quick, hover on a goal slower, and sometimes it showed
nothing. Each cause below was reproduced in an end-to-end test against Agda
2.8.0.2:

1. **The blank middle of an empty goal** showed nothing: in `{!   !}`, the
   second and third space. Hover decided on one char first
   (`text::hovered`, from the fix for hovers that differed in phase 2), and
   a space after a space has none. Fixed, see below.
2. **A cancelled request shifts Agda's answers.** When the mouse moves on,
   Zed drops the hover and sends `$/cancelRequest` (`show_hover` in
   `crates/editor/src/hover_popover.rs` replaces the task, and
   `cancel_on_drop` in `crates/lsp/src/lsp.rs` sends the cancel);
   `tower-lsp-server` then drops the handler (`future::abortable` in
   `src/service/pending.rs`). When that handler was waiting for Agda, Agda's
   answer stays in the queue, and every later command reads the answer of
   the one before it, until the bridge restarts. A goal hover then shows
   nothing when it reads a name's answer, or another goal when it reads a
   goal's; a name can show nothing until the next load, and a load can
   publish no goals at all. Fixed, see below.
3. **"Agda is busy".** Hover asks Agda only when it is free at that moment.
   It often is not, briefly: Zed sends a hover for every new mouse position,
   and a code action request for every cursor move, which on an empty goal
   asks Agda for the variables to split. Zed then keeps that popover while
   the mouse stays in the goal (`same_info_hover`).
4. **Goals are asked every time.** A name is asked once per load; a goal on
   every hover. With Agda free, in `Spike.agda`, both take under 1 ms (a
   name from the cache 0.13 ms), so this matters in large files with large
   contexts, not in small ones.

## Done: hover in the blank middle of an empty goal

Hover now looks for a goal at the position under the mouse first, so the
blank inside `{!   !}` counts. Only when there is none does it decide on a
char as before, which may be the one before the mouse: just past a goal, a
name or a symbol, and at the end of a line. Every position that showed a
goal before still does.

| Check | Result |
| --- | --- |
| End to end: every char of `{!   !}` and the end of the line right after it show the same hover as its `{` | passes; with the old lookup the second space shows nothing |

## Done: commands that survive a cancelled request

`Agda` (`agda.rs`) counts the commands it sent whose prompt it has not read
yet. That is more than zero only when a caller went away while Agda worked,
as a hover that Zed cancelled. Before the next command, it reads and drops
what such a command left, up to its prompt, so every command reads its own
answer again. Agda still finishes the cancelled command first, as it did
before, since it does one command at a time.

A command counts as soon as its whole line is written, in one write. A
caller that goes away before that leaves at most part of a line, which
joins the next command into one line with one answer, so the count still
matches Agda's prompts.

| Check | Result |
| --- | --- |
| End to end: 14 hovers, on goals and on names Agda was not asked about yet, each cancelled right away; after each, a hover on a goal shows that goal; then `_+_` its own type, and a load its three goals | passes in three runs, and in each all 14 cancelled hovers left an answer that was dropped (counted with a temporary log line); with the old `agda.rs` it fails in five runs out of five, at the first hover, which shows the other goal |

## Planned

1. **Hover waits briefly for Agda** (cause 3), up to about a second,
   instead of saying "Agda is busy" at once; most commands take
   milliseconds, and Zed cancels a hover it no longer needs.
2. **A goal cache** (causes 3 and 4). Per goal, what is known about it: after
   a load only its type (which the bridge keeps for the diagnostics), after
   the first hover the whole answer, for a goal with text together with that
   text. Hover shows it at once, and while Agda is busy shows what it has,
   the type at least. A load, give, refine, solve or auto clears it, since
   filling one goal can change the types in others; the variables offered
   for a case split come from it too. After auto, which sends no new list of
   goals, the bridge asks for one (`Cmd_metas AsIs`, Emacs's "show goals"),
   so that the stored types, and the goal diagnostics, stay current: without
   it, a goal of type `_B_11` keeps that type after auto fills the goal it
   depends on with `ℕ` (checked with Agda 2.8.0.2).
