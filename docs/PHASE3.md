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
   the mouse stays in the goal (`same_info_hover`). Hover now waits a
   moment, and after that shows what it has; see below.
4. **Goals are asked every time.** A name is asked once per load; a goal on
   every hover. With Agda free, in `Spike.agda`, both take under 1 ms (a
   name from the cache 0.13 ms), so this matters in large files with large
   contexts, not in small ones. Fixed with a goal cache, see below.

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

## Done: hover waits briefly for Agda

Hover on a goal or a name, and the code actions on a goal (for the
variables to split), no longer give up at once when another command holds
Agda: they wait for it at most a second (`Bridge::lock_briefly`). Most
commands take milliseconds, such as another hover's, so these now get their
answer. A longer command, such as a load or a normal form, still gets
"Agda is busy" after that second, so a hover never waits for a whole load.
Zed cancels a waiting hover when the mouse moves on, which takes it out of
the line; since the previous step, that is safe also once its command has
been sent.

Commands that change the file (give, case split and the others) and the
load still wait as long as needed, as before.

| Check | Result |
| --- | --- |
| End to end: the 14 cancelled hovers of the previous step, now with every next hover answered at once, without hovering again when Agda was busy | passes; with the old code the first hover after a cancel says "Agda is busy", in three runs out of three |
| End to end: while Agda computes the normal form of `ack three eight` (about 2.5 s, fixture `Slow.agda`), a hover on another goal says "Agda is busy" after a second, while Agda is still busy; afterwards it shows the goal | passes; with the old code the hover gives up after 0.2 ms, in three runs out of three |

## Done: a goal cache

Per goal, the bridge keeps Agda's answer (`render::GoalAnswer`): the goal's
type and context, and, for a goal with text, the type of that text or why
there is none, together with that text.

1. **Hover** on a goal shows the kept answer at once when the goal's text is
   the same, without Agda; otherwise it asks Agda, waiting at most a second,
   and keeps the answer. `Print goal in output` uses it the same way, and the
   code actions take the variables to split from it, so their own cache is
   gone.
2. **While Agda stays busy,** hover shows what is known, and in italics what
   is missing:
   - Agda's answer for other text in the goal, without `Have:`: "Agda is
     busy: the type of the goal's text follows when it is free."
   - Otherwise the goal's type from the last load, which the bridge keeps for
     the diagnostics: "Agda is busy: the context follows when it is free."
   - While Agda loads the file again, also the answer for the same text,
     which is then from before: "Agda is loading the file again: this is
     from the last load."

   Only for a goal that Agda has said nothing about does hover still say
   "Agda is busy".
3. **Cleared** by a load, and when a goal command fills a goal (give,
   refine, solve, auto), since that can change the types in other goals and
   in their contexts. A case split, a with-abstraction or a helper function
   changes nothing in Agda until the next load.
4. **After auto,** which sends no new list of goals, the bridge asks for one
   (`Cmd_metas AsIs`, Emacs's "show goals"), so that the stored types, and
   the goal diagnostics, stay current. Without it, a goal of type `_B_11`
   kept that type after auto filled the goal it depends on with `ℕ` (checked
   with Agda 2.8.0.2).
5. **No answer from before a give.** A hover keeps its answer while it still
   holds Agda, so a give cannot come between Agda's answer and keeping it.

| Check | Result |
| --- | --- |
| Unit: what is shown while Agda is busy: the whole answer, with the note only while loading; without `Have:` for other text, and complete for a goal without text; the type from the load; nothing known | passes |
| End to end (`Slow.agda`): while Agda computes a normal form, a goal hovered before shows its answer at once (under 0.5 s); one not hovered yet shows, after a second, its type from the load with the note; with other text in it, a goal hovered before shows, after a second, its answer without `Have:`; afterwards Agda's answers | passes |
| End to end (`SlowLoad.agda`, whose load checks `ack 3 8 ≡ 2045` twice, about 2.6 s): during a new load, after a second, the answer from before with the note; after the load, the new answer | passes |
| End to end (`Depends.agda`): after auto fills `?0` with `ℕ`, the diagnostic and the hover of `?1` say `ℕ` instead of the meta | passes |
| Seven mutations: no cache hit; a cache that ignores the text; no clearing after a give; no `Cmd_metas` after auto; no fallback while busy; the reload flag ignored, or never set | each fails at least one of these tests |

## Done: stopping a long command

The normal form, and auto, which runs as long as the time limit in the
goal's text says, can take long. While they run, other goal commands wait,
and hover shows what it has.

1. **In Zed.** The bridge marks their progress as cancellable. Zed then
   offers to cancel it in two places (checked in Zed's source): clicking the
   progress in the status bar opens a menu with "Cancel …"
   (`crates/activity_indicator/src/activity_indicator.rs`), and the command
   `editor: cancel language server work` cancels the work for the active
   file (`CancelLanguageServerWork` in `crates/editor`). An extension cannot
   add a command of its own to the command palette, but this one is Zed's,
   and can be bound to a key. Both send `window/workDoneProgress/cancel`
   with the progress's token (`cancel_language_server_work` in
   `crates/project/src/lsp_store.rs`). tower-lsp-server 0.23 does not handle
   that notification yet (a `TODO` in its `src/server.rs`), so the bridge
   registers it with `custom_method`.
2. **The title.** Zed shows "title: message" in the status bar, but only
   "Cancel title" in its menu, so the title says everything: "Agda: normal
   form of ?0", "Agda: auto on ?3".
3. **In Agda.** `Cmd_abort` stops the command Agda works on: Agda reads it
   while it works, and answers the stopped command with
   `{"kind":"DoneAborting"}` and its prompt (checked with Agda 2.8.0.2, on
   the normal form of `ack 3 9`, about 10 s, and on auto with
   `-t 10000 trans sym cong +-zero +-suc` for `n + m ≡ m + n`, which was
   still searching after 40 s). `Cmd_abort` itself gets no answer and no
   prompt, also when Agda is idle, so it is not counted as a command (see
   "commands that survive a cancelled request").
4. **In the bridge.** The command holds Agda's session, so the cancel cannot
   wait for it: Agda's input has a lock of its own, which a `Stopper` shares
   (`agda.rs`). It sends `Cmd_abort` only while Agda works on the command
   of that token: the token is set when its whole line is written, and
   cleared when its prompt is read or the next command is written. A late
   cancel, or one with another token, does nothing.
5. **Afterwards** the output file says "Stopped computing the normal form of
   `…` in ?0." or "Stopped auto on ?0.", and nothing changes in the file.

| Check | Result |
| --- | --- |
| Agda's `DoneAborting` is read; `Cmd_abort` is built | passes |
| End to end: the normal form of `ack three (suc eight)` and auto with a 10 s limit (fixture `Search.agda`) are offered as cancellable, and stop after a cancel (in 52 ms and 5 ms); a cancel with another token, or after the end, does nothing; afterwards a hover and a normal form get their own answers | passes; fails when the stop sends nothing, when it ignores the token, or when the normal form does not say it stopped |
| The goal test: the normal form's progress has the title "Agda: normal form of ?3", and can be cancelled | passes |

## Done: stopping a load

A load can be stopped the same way: its progress, "Agda: checking A.agda",
is cancellable too, and the cancel sends `Cmd_abort`.

1. **What Agda does** (checked with Agda 2.8.0.2, with `SlowLoad.agda`,
   whose load takes about 2.6 s): it stops within 60 ms, and answers the
   load with `DoneAborting` and its prompt, as for a normal form. Then it has
   no file loaded, also not the one it had before: a goal command for a
   file, the stopped one or the one loaded before, then took as long as a
   whole load, and the next one was at once. So Agda loads a file first when
   it gets a goal command for it, without saying so.
2. **What the bridge does.** It takes Agda's file to be none, so it sends no
   goal command for any file until the next load: hover and the goal
   commands say "This file is not loaded in Agda yet. Save it to load it.",
   instead of starting a hidden load. It changes nothing of the last load:
   goals, diagnostics, highlighting and Agda's kept answers stay, and hover
   shows those answers as before. The output file says "Stopped checking
   A.agda. Save it to load it again.", and saving loads the file, also when
   its text is the one Agda loaded last.

| Check | Result |
| --- | --- |
| End to end: a load after saving a change is cancellable, with the title "Agda: checking SlowLoad.agda", and stops after a cancel (in under 1 s); the diagnostics stay; hover on the goal shows the kept answer, and on a name Agda was not asked about shows nothing, both at once; saving again loads the file, and the name then has its type | passes; fails when the bridge keeps Agda's file (the name then shows its type, after a hidden load), when it takes the stopped load for a load, or when the load cannot be stopped |

A test that changed with it: `completes_unicode_input` waited for any
notification after a wrong setting, and sometimes got the earlier one that
Agda cannot start (the test has no Agda), which comes late from loading the
file. It now waits for the one about the setting; it failed once in five
runs before, and passed ten runs out of ten after.

## Possible next steps

- **Ask for every goal after a load**, in the background, so that even the
  first hover on a goal is instant. It costs Agda one command per goal after
  every save.
