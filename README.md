# Agda Interactive

An [Agda](https://agda.readthedocs.io/en/latest/getting-started/what-is-agda.html) extension for [Zed](https://zed.dev), for interactive development with goals, by Tim Steenvoorden. Credits to:

- Original extension: [agda-zed](https://github.com/haohanyang/agda-zed) by Haohan Yang (Apache 2.0), which this extension is based on; its tree-sitter queries and language configuration are used here
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

Uninstall the Agda extension from Zed's registry (id `agda`) first. Both
extensions define the language "Agda", and that one also starts the Agda
Language Server.

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
move it to a split, and Zed remembers that. After closing it, reopen it with
`pane: reopen closed item` (`cmd-shift-t` on macOS, `ctrl-shift-t` on Linux)
or from the project panel. With `"reveal_if_open": true` in Zed's settings,
the automatic opening reveals an output file that is already open in another
pane instead of opening a second copy.

For debugging, `agda-bridge client` sends a command to the running bridge from
a terminal; run it without arguments for its usage.
