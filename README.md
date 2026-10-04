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
`agda` on your `PATH`. Both, and the bridge's other options, are set in Zed's
`settings.json`:

```json
"lsp": {
  "agda-bridge": {
    "binary": { "path": "/path/to/agda-bridge" },
    "initialization_options": {
      "agdaPath": "/path/to/agda",
      "outputFile": ".zed/agda-output.md",
      "symbolInput": "both",
      "symbolTrailingSpace": false,
      "symbolOnlyAfterWhitespace": false
    }
  }
}
```

| Option | Default | Meaning |
| --- | --- | --- |
| `agdaPath` | `"agda"` | the Agda program |
| `extraArgs` | `[]` | extra command line arguments for Agda, such as `["--safe"]` |
| `outputFile` | `".zed/agda-output.md"` | where Agda's answers appear, relative to the project root (see below) |
| `symbolInput` | `"both"` | how symbols are typed: `"latex"` (`\to`), `"typst"` (`#arrow.r`), `"both"` or `"none"` (see [Unicode input](#unicode-input)) |
| `symbolTrailingSpace` | `false` | put a space after a completed symbol, unless one is already there |
| `symbolOnlyAfterWhitespace` | `false` | let `\` and `#` start a symbol only at the start of a line or after whitespace, so that `{-#` and `x\y` never open the menu |

The bridge reads these options when it starts: after a change, run `editor:
restart language server` from the command palette. A wrong value is reported
and its default used instead.

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
`{!  !}`. The code actions on a goal (`cmd-.` on macOS, `ctrl-.` on Linux)
give, refine, case split, auto and solve it, or solve all goals. After a case
split, save the file to load the new clauses.

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
