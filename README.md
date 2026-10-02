# Zed Agda

An [Agda](https://agda.readthedocs.io/en/latest/getting-started/what-is-agda.html) extension for [Zed](https://zed.dev). Credits to:

- Tree-sitter: [tree-sitter-agda](https://github.com/tree-sitter/tree-sitter-agda)
- Interaction protocol: [agda2-vscode](https://github.com/willtunnels/agda2-vscode) (MIT), whose protocol handling the bridge ports to Rust; see [`bridge/THIRD-PARTY-NOTICES.md`](bridge/THIRD-PARTY-NOTICES.md)

> **Work in progress.** This branch replaces the Agda Language Server with
> `agda-bridge`, a small language server in `bridge/` that drives Agda through
> its own `--interaction-json` protocol, like Emacs `agda2-mode` does. See
> [`docs/PLAN.md`](docs/PLAN.md) for the design and
> [`docs/PHASE0.md`](docs/PHASE0.md) for what works so far.

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

For debugging, `agda-bridge client` sends a command to the running bridge from
a terminal; run it without arguments for its usage.
