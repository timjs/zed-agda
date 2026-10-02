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
`agda` on your `PATH`. Both can be set in Zed's `settings.json`:

```json
"lsp": {
  "agda-bridge": {
    "binary": { "path": "/path/to/agda-bridge" },
    "initialization_options": {
      "agdaPath": "/path/to/agda",
      "outputFile": ".zed/agda-output.md"
    }
  }
}
```

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
termination or coverage problems, arrives as semantic tokens, which Zed only
uses when they are switched on:

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

Type `\` and an abbreviation of Agda's Emacs mode (`\to`, `\all`, `\bN`,
`\Gl`), or `#` and the name of a [Typst symbol](https://typst.app/docs/reference/symbols/sym/)
(`#arrow.r`, `#NN`, `#lt.eq`), and pick the symbol from the completion menu
with `tab` or `enter`. A `#` only starts a Typst name at the start of a line or
after a space, so pragmas such as `{-# OPTIONS --safe #-}` are left alone.

## Debugging

For debugging, `agda-bridge client` sends a command to the running bridge from
a terminal; run it without arguments for its usage.
