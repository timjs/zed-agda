# Zed Agda

An [Agda](https://agda.readthedocs.io/en/latest/getting-started/what-is-agda.html) extension for [Zed](https://zed.dev). Credits to:

- Tree-sitter: [tree-sitter-agda](https://github.com/tree-sitter/tree-sitter-agda)
- Interaction protocol: [agda2-vscode](https://github.com/willtunnels/agda2-vscode) (MIT), whose protocol handling the bridge ports to Rust and whose dump of Agda's input method it uses for Unicode input; see [`bridge/THIRD-PARTY-NOTICES.md`](bridge/THIRD-PARTY-NOTICES.md)
- Typst symbol names: [codex](https://github.com/typst/codex) (Apache-2.0)

> **Work in progress.** This branch replaces the Agda Language Server with
> `agda-bridge`, a small language server in `bridge/` that drives Agda through
> its own `--interaction-json` protocol, like Emacs `agda2-mode` does. See
> [`docs/PLAN.md`](docs/PLAN.md) for the design and
> [`docs/PHASE0.md`](docs/PHASE0.md) and [`docs/PHASE1.md`](docs/PHASE1.md)
> for what works so far.

## Installation

You need Agda itself, and Rust to build the bridge:

```sh
cargo install --path bridge
```

This puts `agda-bridge` in `~/.cargo/bin`, which must be on your `PATH`. Then
install this repository in Zed with "Install Dev Extension" on the Extensions
page.

## Configuration

The extension looks for `agda-bridge` on your `PATH`, and the bridge looks for
`agda` on your `PATH`. Both, and the bridge's other settings, are set in Zed's
`settings.json`, under `lsp.agda-bridge`: `binary` for the bridge itself and
`settings` for everything else.

```json
"lsp": {
  "agda-bridge": {
    "binary": { "path": "/path/to/agda-bridge" },
    "settings": {
      "agdaPath": "/path/to/agda",
      "outputFile": ".zed/agda-output.md",
      "symbolInput": "both",
      "symbolTrailingSpace": true,
      "symbolOnlyAfterWhitespace": false
    }
  }
}
```

| Setting | Default | Meaning |
| --- | --- | --- |
| `agdaPath` | `"agda"` | the Agda program |
| `extraArgs` | `[]` | extra command line arguments for Agda, such as `["--safe"]` |
| `outputFile` | `".zed/agda-output.md"` | where Agda's answers appear, relative to the project root (see below) |
| `symbolInput` | `"both"` | how symbols are typed: `"latex"` (`\to`), `"typst"` (`#arrow.r`), `"both"` or `"none"` (see [Unicode input](#unicode-input)) |
| `symbolTrailingSpace` | `true` | put a space after a completed symbol, unless one is already there |
| `symbolOnlyAfterWhitespace` | `false` | let `\` and `#` start a symbol only at the start of a line or after whitespace, so that `{-#` and `x\y` never open the menu |

Changes apply at once, without restarting anything: a new `agdaPath` or
`extraArgs` restarts Agda at the next load, a new `outputFile` is used for the
next answer. A wrong value is reported and its default used instead.

Zed decides what may go under `lsp.agda-bridge` (`binary`, `settings` and a
few more), so the settings cannot sit directly under `agda-bridge`; `settings`
is Zed's place for a language server's own settings.

`outputFile` is the Markdown file where Agda's answers appear (the equivalent
of Emacs's `*Agda information*` buffer), relative to the project root. Add it
to your `.gitignore`. It opens once by itself, in the pane you are editing;
move it to a split. After closing it, reopen it with the code action "Agda:
open output file" on a goal or an error, or with Zed's `pane: reopen closed
item` (`cmd-shift-t` on macOS, `ctrl-shift-t` on Linux). With
`"reveal_if_open": true` in Zed's settings, the code action and the automatic
opening reveal an output file that is already open in another pane, instead of
opening a second copy.

Agda's own highlighting, including backgrounds for unsolved metas and
termination or coverage problems, arrives as semantic tokens. Whether Zed uses
them is Zed's own setting, `semantic_tokens`, which is `"off"` by default for
every language; an extension cannot change that default, so switch it on for
Agda yourself (and set it to `"off"` again to switch Agda's highlighting off):

```json
"languages": {
  "Agda": { "semantic_tokens": "combined" }
}
```

`combined` keeps the tree-sitter highlighting underneath, for text Agda has not
seen yet (lines typed since the last save); `full` shows only Agda's.

## Goals

Opening or saving a file loads it in Agda. Goals then show their types as
diagnostics, hover shows a goal's type and context, and a lone `?` becomes
`{!  !}`. The code actions (`cmd-.` on macOS, `ctrl-.` on Linux) are:

| Where | Code action | Effect |
| --- | --- | --- |
| a type signature | `Add clause` | adds a clause below it, with a name for each argument from its type: `n + m = {!  !}` for `_+_ : ℕ → ℕ → ℕ` (as Idris's "add clause") |
| a goal | `Give`, `Refine` | give or refine the goal with its text |
| an empty goal | ``Case split on `n` `` | one for each variable of the goal's context that can be split |
| an empty goal | `Case split on result` | introduce the missing patterns, or split on the result |
| a goal with text | ``Case split on `x y` `` | split on the variables typed in the goal |
| a goal that is a whole right-hand side | `With-abstract`, ``With-abstract `e` `` | `f n = {! e !}` becomes `f n with e` and `... \| w = {!  !}` (as Idris's "add with") |
| a goal | `Auto`, `Solve`, `Solve all goals` | proof search, or the solutions unification already found |
| a goal or an error | `Show goal in output`, `Open output file` | the output file |

New clauses, from `Add clause`, a case split or `With-abstract`, are loaded
when you save the file; until then their goals have no number.

## Renaming

Renaming is not a code action: Zed only lets a language server answer its own
Rename Symbol, which asks for the new name. Start it with `F2`, `space r` in
Helix mode, `g r n` in vim mode, or Rename Symbol in the menu of a right
click. Every place where Agda found that name changes, in all Agda files open
in Zed. Renaming one part of an operator renames the operator (`⊕` on the `+`
of `n + m` makes `_+_` into `_⊕_`). It works on what Agda checked at the last
save, so save first; text in goals, which Agda does not check, and files that
are not open stay as they are. Afterwards a message lists the files that are
not open but import the module and may use the name.

## Saving

**Auto save is not possible from the extension:** the language server
protocol has no way for a server to save a file, so the files a rename or a
code action changed stay unsaved, and Agda only checks them when you save.
Workarounds:

- Save all files at once with `workspace: save all` (`cmd-alt-s` on macOS,
  `ctrl-alt-s` on Linux).
- Let Zed save by itself with its `autosave` setting, for this project only in
  its `.zed/settings.json`:

  ```json
  { "autosave": "on_focus_change" }
  ```

  Every save loads the file in Agda again, so a short delay
  (`{ "autosave": { "after_delay": { "milliseconds": 1000 } } }`) also checks
  while you type.

## Unicode input

Type a leader and a name, and pick the symbol from the completion menu with
`tab` or `enter`:

| You type | You get | Notation |
| --- | --- | --- |
| `\to`, `\all`, `\bN`, `\Gl`, `\'e` | `→ ∀ ℕ λ é` | the abbreviations of Agda's Emacs mode, which include LaTeX's names |
| `#arrow.r`, `#forall`, `#NN`, `#alpha` | `→ ∀ ℕ α` | the names of [Typst's symbols](https://typst.app/docs/reference/symbols/sym/), with modifiers in any order |
| `#->`, `#=>`, `#<=`, `#!=`, `#[\|` | `→ ⇒ ≤ ≠ ⟦` | Typst's [math shorthands](https://typst.app/docs/reference/symbols/#shorthands) |
| `#acute(e)`, `#diaer(o)`, `#hat(alpha)` | `é ö α̂` | Typst's [accents](https://typst.app/docs/reference/math/accent/), also nested, as in `#macron(diaer(u))` for `ǖ` |

The options `symbolInput`, `symbolTrailingSpace` and `symbolOnlyAfterWhitespace`
above choose the notations, a space after the symbol, and whether a leader
also counts in the middle of a word. By default `\` and `#` count anywhere, so
the `#` of a pragma such as `{-# OPTIONS --safe #-}` also opens the menu; the
next character, a space or `-`, closes it again.

## Debugging

For debugging, `agda-bridge client` sends a command to the running bridge from
a terminal; run it without arguments for its usage.
