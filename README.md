# Zed Agda

An [Agda](https://agda.readthedocs.io/en/latest/getting-started/what-is-agda.html) extension for [Zed](https://zed.dev). Credits to:

- Tree-sitter: [tree-sitter-agda](https://github.com/tree-sitter/tree-sitter-agda)
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
