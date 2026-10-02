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

## Next steps

1. **Agda's own highlighting** as semantic tokens, from the same messages
   (their `atoms`), with background colours for unsolved metas, termination
   and coverage problems through `semantic_token_rules.json`.
2. **Unicode input** through completions: `\` with the abbreviations of
   Agda's Emacs mode (which include the LaTeX names), and `#` with Typst's
   symbol names from the `codex` crate, only after a space or at the start of
   a line, because pragmas start with `{-#`.
3. **Goal commands**: a lone `?` becomes `{!  !}` after loading, plus case
   split, auto and solve.
4. **A hover cache**, so hover stays fast during long loads.
5. Stretch goal: hover showing the type of any name in scope at the top
   level.
