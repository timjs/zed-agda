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
   `\x → x` is left alone once the space is typed. `#` at first started a
   Typst name only at the start of a line or after whitespace, because Agda
   uses `#` itself, in pragmas such as `{-# OPTIONS --safe #-}` and in names;
   that rule is now the option `symbolOnlyAfterWhitespace`, for both leaders
   alike (see "Symbol options" below). The completion replaces the leader and
   everything typed after it.
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
| `#` in `{-#` and in `x#y` is left alone (now only with `symbolOnlyAfterWhitespace`, see below) | passes; fails without the whitespace check |
| Every character of every abbreviation is a word character or a trigger character | passes |
| End to end, without Agda: the edit turns `\to` into `→`, `\bN` after `𝔹` starts at UTF-16 column 7, `#arrow.r` is completed, a pragma (now only with `symbolOnlyAfterWhitespace`) and ordinary text get nothing, and a lone `\` gets an incomplete list | passes |

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

## Done: symbol options, Typst shorthands and accents

### Options

Three initialization options, read when the bridge starts
(`input::Options`):

| Option | Default | Effect |
| --- | --- | --- |
| `symbolInput` | `"both"` | `"latex"`, `"typst"`, `"both"` or `"none"`; with `"none"` the bridge does not offer completions at all |
| `symbolTrailingSpace` | `false` | a space after the symbol, unless the next character is already whitespace |
| `symbolOnlyAfterWhitespace` | `false` | `\` and `#` count only at the start of a line or after whitespace |

A value of the wrong kind keeps the default and is reported with a warning.
Before, `#` always needed whitespace before it and `\` never did; now both
follow the same option, and by default both count anywhere. An abbreviation
is still tried first, so `\#` stays Agda's `♯`.

Switching Agda's highlighting on or off is not an option of the bridge: it
is Zed's own `semantic_tokens` setting per language. Its default is `"off"`
for every language (`assets/settings/default.json` in Zed), and an extension
can only add rules for tokens (`semantic_token_rules.json`), not defaults for
settings (`crates/extension_host/src/extension_host.rs`). A second switch in
the bridge would only duplicate Zed's.

### Typst shorthands and accents

After `#`, besides symbol names:

1. **Math shorthands**, such as `#->` for `→` and `#[|` for `⟦`: the 38 of
   Typst's math mode, taken from the `data-math-shorthand` attributes of
   Typst's documentation of symbols. `codex` does not contain them.
2. **Accents**, written as Typst writes them in math: `#acute(e)` for `é`,
   with or without the closing parenthesis, around a letter, a symbol name
   (`#hat(alpha)`) or another accent (`#macron(diaer(u))` for `ǖ`). The 20
   accent names come from Typst's documentation of `accent`; the bridge
   puts the matching combining character after the base and composes the
   result (Unicode normalisation NFC, with the `unicode-normalization`
   crate), so `é` is one character where Unicode has one, and `α̂` stays two.

Typst's `sym` module itself has no accented letters, so before this `#`
could not type `é` at all; `\'e` could, and still can. Of the 200 symbols
that only the left-out TeX sequences produced, accents bring back 70, the
letters with two accents above, such as `Ǖ` (`#macron(diaer(U))`); Typst has
no accents below the letter (a cedilla, a dot below), and modifier letters
such as `ʱ` are not accents, so the other 130 still cannot be typed.

### Proven by tests

| Check | Result |
| --- | --- |
| Shorthands: `#->` offers `→` first, then `↠`; `#=>`, `#[\|`, `#!=`, `#-` (the minus sign) and `#...` | passes |
| Accents: `#acute(e)` is `é`, `#diaer(o` is `ö`, `#macron(diaer(U))` is `Ǖ`, `#hat(alpha)` and `#arrow(x)` keep their combining character; an unknown accent or base gives nothing | passes; fails without NFC |
| Options: by default both leaders count anywhere, also in `{-#`; with `symbolOnlyAfterWhitespace` neither does after a letter; each mode offers only its leader; `\#` stays `♯`; a Typst name after a `#` in a failed abbreviation | passes; fails when the option is ignored |
| A space follows the symbol only when asked and not already there | passes |
| The options are read, and wrong values reported | passes |
| End to end: `#->` completes; with the options set through `initializationOptions`, `\to` inserts `→ ` and `{-#` gets nothing; with `"none"` there is no completion provider; a wrong `symbolInput` gives a warning | passes |

### Also fixed: real paths on macOS

On macOS the end-to-end test failed, also before these changes: Agda names
files by their real path (`/private/var/…` for `/var/…`, which is a symbolic
link), so errors in a file opened as `/var/…` landed at the start of the file,
and go to definition into another file pointed outside the project. The
bridge now recognises Agda's real path in messages and maps definition
targets back into the worktree.

### To check in Zed

1. Reinstall the bridge and restart the language server.
2. Type `#->`, `#acute(e)` and `#macron(diaer(u))`.
3. Set `"symbolTrailingSpace": true` and `"symbolOnlyAfterWhitespace": true`
   in `lsp.agda-bridge.initialization_options`, restart the language server,
   and type `\to` and a pragma.

## Done: goal commands

After loading, every goal written as a lone `?` becomes `{!  !}`, as in
Emacs. On a goal, the code actions (`cmd-.` on macOS, `ctrl-.` on Linux) now
offer, besides give and refine:

| Code action | Agda command | Result |
| --- | --- | --- |
| case split ?n (on `x`) | `Cmd_make_case` | Agda's new clauses replace the clause of the goal |
| auto ?n | `Cmd_autoOne AsIs` | proof search fills the goal, with the goal's text as hints |
| solve ?n, solve all goals | `Cmd_solveOne Simplified`, `Cmd_solveAll AsIs` | goals that unification already solved are filled, such as `{?}` in `id {?} (suc zero)` |

The rewrite modes are the defaults of Agda's Emacs mode (`agda2-mode.el`).

### What was learned

All of it was checked against Agda 2.8.0's real answers.

1. **Auto answers like give**, with a `GiveAction`, but without the list of
   all goals that give sends. The first version of the bridge therefore lost
   the types of the other goals after auto (the end-to-end test caught it);
   now they are kept, and only the filled goal's type goes. When auto finds
   nothing, Agda says "No solution found".
2. **Solve fills nothing itself.** Agda answers with a solution for each goal
   (`{"interactionPoint": 5, "expression": "ℕ"}`, with the goal as a bare
   number, unlike elsewhere), and Emacs then gives each one. The bridge does
   the same, and keeps Agda to itself until all of them are in the buffer, so
   that a load in between cannot renumber the goals.
3. **Case split replaces text the way Emacs does**, ported from
   `agda2-make-case-action` and `agda2-make-case-action-extendlam`: in a
   function clause, the line of the goal from its indentation on (with the
   rest of that line); in `λ { … }`, only the clause, which starts after the
   `{` or `;` before the goal (skipping the braces of implicit arguments),
   with the new clauses separated by `;`; in `λ where`, the line, with one
   line per clause.
4. **After a case split, Emacs saves and loads again.** A language server
   cannot save a file in Zed, and Agda reads the file from disk, so the new
   goals get their numbers when you save. Until then they are plain text,
   without code actions or hover; the output file says so.
5. **Expanding `?` needs no reload.** Agda knows goals only by number, so the
   bridge replaces the text, keeps the numbers, and lets the hole's
   highlighting grow along (an ordinary edit over highlighted text removes
   it). The file is then modified, as in Emacs. `{?}` becomes `{{!  !}}`,
   which Agda accepts although `{{` also opens an instance argument (checked
   with Agda 2.8.0). Only files open in Zed are expanded, all `?`s in one
   edit.
6. **The edits stay unversioned.** Zed can move a versioned edit along with
   typing that happened in between, but solving several goals sends several
   edits, and the version after each one only arrives later, with Zed's
   `didChange`. The bridge computes each edit from its own copy, which is up
   to date, so only typing during the few milliseconds the edit travels could
   get in the way.

### Proven by tests

| Check | Result |
| --- | --- |
| Agda 2.8.0's answers to case split (both variants), solve and a failed auto are read | passes |
| Case split of a function clause: Agda's two clauses replace the line, keep the indentation in a `where` block, drop a comment after the goal (as Emacs does) and keep a Windows line end | passes |
| Case split in an extended lambda: only the clause after the last `;`, with the braces of `{y}` skipped; in `λ where` one clause per line | passes; fails without counting braces |
| The hole of a `?` stretches with its expansion; an ordinary edit removes it | passes |
| End to end with `Goals.agda`: after loading both `?`s become `{!  !}` in one edit and keep their numbers ?1 and ?2, with the hole highlighting over all six characters; the code actions; auto fills `p` with `refl` and the other goals keep their types; solve on ?0 says there is no solution; solve all fills `{ℕ}`; case split replaces line 11 by two clauses, and in the lambda only the clause; after saving, Agda accepts the result, numbers the four new goals and nothing needs expanding | passes; fails without stretching the highlighting |

### To check in Zed

1. Reinstall the bridge (`cargo install --path bridge`) and restart the
   language server.
2. Open a file with `?` goals: after loading they should read `{!  !}`, and
   the file should be modified.
3. Type a variable in a goal, press `cmd-.` and choose case split; save, and
   the new goals should get numbers.
4. Try auto on a simple goal, and solve all goals on `id {?} (suc zero)`.

### Limitations

- The goals of a case split get numbers only when the file is saved.
- After a load that expanded `?`s, the file is modified until it is saved.
- Auto uses the syntax of Agda 2.7 and later; Agda 2.6 needs the version
  gates planned for phase 4.
- Case split assumes the goal is on one line, as Emacs does.

## Next steps

1. **A hover cache**, so hover stays fast during long loads.
2. **How to type a symbol**: hover over `→` to see `\to`, `\->` and
   `#arrow.r`, as agda2-vscode does; the tables are already in the bridge.
3. **Goal commands that only show information**: the goal's type together
   with the type of the expression in it (`Cmd_goal_type_context_infer`), the
   normal form of an expression (`Cmd_compute`), why a name is in scope, and
   the type of a helper function.
4. Stretch goal: hover showing the type of any name in scope at the top
   level.
5. Before publishing: an issue at `haohanyang/agda-zed`, proposing the bridge
   or asking to take over the `agda` id.
