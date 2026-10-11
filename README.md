# Zed Agda

An [Agda](https://agda.readthedocs.io/en/latest/getting-started/what-is-agda.html) extension for [Zed](https://zed.dev). Credits to:

- Tree-sitter: [tree-sitter-agda](https://github.com/tree-sitter/tree-sitter-agda) and [tree-sitter-typst](https://github.com/uben0/tree-sitter-typst)
- Typst queries for literate Agda: the [Typst extension](https://github.com/WeetHet/typst.zed) (Apache-2.0)
- Language server: [agda-bridge](https://github.com/timjs/agda-bridge)

The extension starts [agda-bridge](https://github.com/timjs/agda-bridge), a
language server that drives Agda through its own `--interaction-json`
protocol, as Emacs's `agda2-mode` does. Its README explains how to install,
configure and use it.

## Installation

You need Agda itself, and Rust to build agda-bridge:

```sh
cargo install --locked --git https://github.com/timjs/agda-bridge
```

Then install this repository in Zed with "Install Dev Extension" on the
Extensions page.

## Literate Agda in Typst

Files ending in `.lagda.typ` get the language "Literate Agda (Typst)": the
prose is highlighted as Typst, and the blocks Agda checks, ```` ```agda ````
blocks and bare ```` ``` ```` blocks, as Agda. Blocks in another language keep
their own highlighting, and Agda skips them. agda-bridge checks the whole
file, and offers its completions, hover and code actions inside the Agda
blocks only.

Agda's own highlighting needs the same setting as for `.agda` files:

```json
"languages": {
  "Literate Agda (Typst)": { "semantic_tokens": "combined" }
}
```

Keymap bindings that use `extension == agda`, such as those for moving
between goals in agda-bridge's README, need `extension == typ` for these
files, which also covers plain Typst files.
