# Phase 1: progress

Phase 1 turns the phase 0 spike into something usable every day. The order
follows the decisions in [`PHASE0.md`](PHASE0.md): go to definition first,
then Agda's own highlighting, Unicode input, and the remaining goal commands.

## Done: go to definition

`cmd`-click (or F12, or `g d` in vim mode) on a name jumps to its definition,
in the same file, in another file of the project, or in a library.

### How it works

While loading a file, Agda sends highlighting information for every name:
its range and, for most names, the site of its definition (a file and an
offset). The bridge used to skip these messages; it now keeps the definition
sites as links (`bridge/src/links.rs`) and answers `textDocument/definition`
from them.

Three details matter, all checked against Agda 2.8.0's real output:

1. **Ranges are half-open, in 1-based code points**, and a definition site
   is the offset of the first character of the name (offset 1 for a module).
2. **Agda sends the same entries several times**, in several chunks during a
   load; the bridge keeps each range once.
3. **Links follow unsaved edits**, like goals: edits before a name shift it,
   edits that touch it remove it, and targets in the same file move along.
   Targets in other files are read from disk, because Agda's offsets refer to
   the file as it was loaded.

### Proven by the end-to-end test

| Check | Result |
| --- | --- |
| `ℕ` in the type of `_+_` leads to `data ℕ` in the same file | passes |
| After a give and a refine (unsaved), `𝔹` in `not = λ (b : 𝔹)` still leads to `data 𝔹` | passes; with the link adjustment disabled it fails, jumping to a stale position |
| In `Uses.agda`, `suc` and `ℕ` lead into the imported `Nat.agda` | passes |
| `two` in its definition leads to its type signature | passes |
| A keyword has no definition | passes (null) |

### To check in Zed

1. `cmd`-click a name defined in the same file, and one from an imported
   module or the standard library.
2. Type a few lines above a name without saving, then `cmd`-click it again.

### Limitations

- Names typed since the last save have no definition until the file is saved
  (and loaded) again; this includes the result of give and refine.
- Rename and highlighting of all uses of a name (which use the same data) are
  not done yet.

## Done: Agda's own highlighting

The same highlighting messages say, through their `atoms`, what each stretch
of text is. The bridge sends them to Zed as semantic tokens
(`bridge/src/highlight.rs`): kinds of names, keywords and comments become
token types, and problems become modifiers that
`languages/agda/semantic_token_rules.json` shows as background colours, as in
Agda's Emacs mode (unsolved metas yellow, termination problems salmon,
coverage problems wheat, holes light blue, and so on).

Zed uses semantic tokens only when they are switched on, for example with
`"languages": { "Agda": { "semantic_tokens": "combined" } }` (see the
README).

### What was learned

1. **Problems arrive as separate entries.** Agda sends `loop` once as a
   `function` and once, with the same range, as a `terminationproblem`; a
   coverage problem covers a whole clause such as `partial zero`. Entries
   with the same range are merged into one token.
2. **Zed does not combine overlapping tokens of one server.** All of them
   share one highlight layer (`HighlightKey::SemanticToken` in
   `crates/editor/src/display_map/custom_highlights.rs`), so an inner token
   would cancel the outer one where they overlap. The bridge therefore
   flattens overlapping ranges into pieces that do not overlap: in
   `partial zero`, `partial` is a function with a coverage problem, the space
   only has the coverage problem, and `zero` is a constructor with it.
3. **Rules cannot tell light themes from dark ones.** A rule has one colour,
   so the backgrounds are agda2-vscode's light theme colours made translucent;
   your own `semantic_token_rules` in Zed's settings override them.
4. **Rules apply from low to high priority**: Zed's defaults first, then the
   extension's, then the user's, each overriding fields of the previous
   ones, so a modifier rule adds a background on top of the default colour of
   a `function`.

### Proven by tests

| Check | Result |
| --- | --- |
| Agda's real highlighting of `Spike.agda` becomes the expected tokens: keyword `data`, type `ℕ`, constructor `suc`, function `_+_`, variable `n`, a hole as a decoration, `𝔹` two UTF-16 units long | passes |
| After loading, the server asks Zed to request tokens again (`workspace/semanticTokens/refresh`) | passes |
| Tokens follow an unsaved edit (a new first line moves them down; the new line has none) | passes; fails with the adjustment disabled |
| In `Problems.agda`, `loop` carries a termination problem in its signature and its recursive call, and the coverage problem over `partial zero` is split into three pieces | passes; fails without the flattening |
| Every type and modifier in the rules file exists in the bridge's legend, and every decoration has a rule | passes |

### To check in Zed

1. Rebuild the dev extension (the rules file is part of it), reinstall the
   bridge, and switch semantic tokens on.
2. Open `bridge/tests/fixtures/Problems.agda` (a copy): `loop` should have a
   salmon background, the clause `partial zero` a wheat one.
3. Compare the colours with tree-sitter's, in a light and a dark theme.

### Limitations

- Lines typed since the last save keep only tree-sitter's highlighting (in
  `combined` mode) until the file is saved and loaded again.
- The colours are the same in light and dark themes.

## Next steps

1. **Unicode input** through completions: `\` with the abbreviations of
   Agda's Emacs mode (which include the LaTeX names), and `#` with Typst's
   symbol names from the `codex` crate, only after a space or at the start of
   a line, because pragmas start with `{-#`.
2. **Goal commands**: a lone `?` becomes `{!  !}` after loading, plus case
   split, auto and solve.
3. **A hover cache**, so hover stays fast during long loads.
4. Stretch goal: hover showing the type of any name in scope at the top
   level.
