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

While `cmd` is held, Zed underlines the name under the mouse. Without help it
uses its own word boundaries, which split an Agda name such as `~>*step` into
`~>*` and `step`. The bridge therefore answers with a `LocationLink`, whose
`originSelectionRange` is the name as Agda highlighted it, and Zed underlines
exactly that range (`crates/editor/src/hover_links.rs`). Clients that do not
announce `linkSupport` still get a plain location.

### Proven by the end-to-end test

| Check | Result |
| --- | --- |
| `ℕ` in the type of `_+_` leads to `data ℕ` in the same file | passes |
| After a give and a refine (unsaved), `𝔹` in `not = λ (b : 𝔹)` still leads to `data 𝔹` | passes; with the link adjustment disabled it fails, jumping to a stale position |
| In `Uses.agda`, `suc` and `ℕ` lead into the imported `Nat.agda` | passes |
| `two` in its definition leads to its type signature | passes |
| A keyword has no definition | passes (null) |
| In `Names.agda`, from `~>*step` and from its `s`, the link covers the whole name and leads to its type signature | passes; fails when the bridge sends a plain location |

### To check in Zed

1. `cmd`-click a name defined in the same file, and one from an imported
   module or the standard library.
2. Type a few lines above a name without saving, then `cmd`-click it again.
3. Hold `cmd` over a name such as `~>*step` or the standard library's
   `+-comm`: the whole name should be underlined.

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

## Done: Unicode input

Type `\` and an abbreviation of Agda's Emacs mode, or `#` and the name of a
Typst symbol, then pick the symbol from the completion menu with `tab` or
`enter`:

| You type | You get |
| --- | --- |
| `\to`, `\->` | → |
| `\all`, `\forall` | ∀ |
| `\bN` | ℕ |
| `\Gl`, `\lambda` | λ |
| `\==` | ≡ |
| `\_1` | ₁ |
| `#arrow.r` | → |
| `#NN` | ℕ |
| `#lt.eq` | ≤ |

### How it works

A Zed extension cannot add an input method or keybindings, so the bridge
offers the symbols as completions (`bridge/src/input.rs`).

1. **Two sources.** The abbreviations are agda2-vscode's dump of Agda's own
   input method (`agda-input.el`, which includes the TeX input method of
   Emacs), copied unchanged into `bridge/src/abbreviations.json`. The Typst
   names come from the `codex` crate, the symbol table of Typst itself, in
   which modifiers may come in any order (`#arrow.long.r` is `#arrow.r.long`).
2. **The bridge finds the leader itself.** Looking back from the cursor, `\`
   starts an abbreviation unless whitespace follows it, so a lambda such as
   `\x → x` is left alone once the space is typed. `#` starts a Typst name
   only at the start of a line or after whitespace, because Agda uses `#`
   itself, in pragmas such as `{-# OPTIONS --safe #-}` and in names. The
   completion replaces the leader and everything typed after it.
3. **Zed must ask again after every character.** Zed only asks for
   completions after a word character or a trigger character, and closes the
   menu after any other character. Abbreviations contain punctuation (`\->`,
   `\==`), so every ASCII punctuation character except `_` is a trigger
   character; outside an abbreviation the bridge answers with nothing. Adding
   those characters to `completion_query_characters` in `config.toml` would
   also keep the menu open, but it would change Zed's own word completions
   everywhere in Agda files, so the bridge uses trigger characters instead.
4. **Order.** The abbreviation itself comes first, then shorter ones before
   longer ones, and the symbols of one abbreviation keep the order of Agda's
   Emacs mode, where the first one is the usual one. Zed filters on the word
   before the cursor (`to` in `\to`), which never includes the leader, so the
   filter text is the name without it. At most 200 candidates are sent at
   once, and the list is then marked incomplete.
5. **Left out:** abbreviations that contain `\` or a space (303 TeX
   sequences, such as `\"\'I` for Ḯ, which the search for the leader would
   cut at the second `\`), plain ASCII results (`#paren.l` is `(`), and
   invisible characters, such as the narrow no-break space of `\,` or
   Typst's `space.*`, which only confuse in source code. That leaves 2,321
   abbreviations for 3,521 symbols, and 1,137 Typst symbols. The 200 symbols
   that only those TeX sequences produce, letters with two accents such as Ǖ
   and rare modifier letters such as ʱ, cannot be typed with `\`.

### Proven by tests

| Check | Result |
| --- | --- |
| `\to`, `\->`, `\==`, `\bN`, `\Gl` and `\_1` offer `→`, `→`, `≡`, `ℕ`, `λ` and `₁` first; `\l` keeps the order of Agda's Emacs mode | passes |
| `#arrow.r`, `#NN` and the partly typed `#alph` offer `→`, `ℕ` and `α` first; modifiers in any order (`#arrow.long.r` offers `⟶`), a partly typed modifier (`#arrow.r.doub` offers `⇒`) and a nested module (`#gender.fem`) work | passes |
| Columns are UTF-16 units: after `𝔹` the leader is found at the right column, and a column inside `𝔹` or beyond the line gets no answer | passes; fails when columns count characters |
| `#` in `{-#` and in `x#y` is left alone | passes; fails without the whitespace check |
| Every character of every abbreviation is a word character or a trigger character | passes |
| End to end, without Agda: the edit turns `\to` into `→`, `\bN` after `𝔹` starts at UTF-16 column 7, `#arrow.r` is completed, a pragma and ordinary text get nothing, and a lone `\` gets an incomplete list | passes |

### To check in Zed

1. Reinstall the bridge (`cargo install --path bridge`) and restart the
   language server.
2. Type `\to` and press `tab`. Type `\->`: the menu should stay open at `-`.
   Type `#arrow.r.long`.
3. Look at the order in the menu: Zed sorts by its own fuzzy score first and
   uses the bridge's order only for ties, so it may differ from the order
   above.
4. Type a pragma such as `{-# OPTIONS --safe #-}`, to see whether the menu at
   the closing `#` gets in the way.

### Limitations

- A symbol must be picked from the menu; Emacs replaces an abbreviation as
  soon as it is unambiguous.
- The menu also appears in comments and strings.
- Literate Agda (`.lagda.md`) opens as Markdown in Zed, so it gets no
  completions.

## Next steps

1. **Goal commands**: a lone `?` becomes `{!  !}` after loading, plus case
   split, auto and solve.
2. **A hover cache**, so hover stays fast during long loads.
3. **How to type a symbol**: hover over `→` to see `\to`, `\->` and
   `#arrow.r`, as agda2-vscode does; the tables are already in the bridge.
4. Stretch goal: hover showing the type of any name in scope at the top
   level.
5. Before publishing: an issue at `haohanyang/agda-zed`, proposing the bridge
   or asking to take over the `agda` id.
