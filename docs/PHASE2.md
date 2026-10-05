# Phase 2: progress

Phase 2 polishes what phase 1 ([`PHASE1.md`](PHASE1.md)) built, after using it
every day: options for symbol input, clearer code actions, renaming, settings
that change while Zed runs, the outline, `Make clause` and hover on symbols.
`PLAN.md` calls this phase "Polish". The sections follow the order in which
the work was done.

## Done: symbol options, Typst shorthands and accents

### Options

Three settings (`input::Options`; since then under
`lsp.agda-bridge.settings`, see "Settings" below):

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
| End to end: `#->` completes; with the options set through `initializationOptions`, `\to` inserts `→ ` and `{-#` gets nothing; with `"none"` there is no completion provider (since then: no completions); a wrong `symbolInput` gives a warning | passes |

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
   in `lsp.agda-bridge.settings` (at first `initialization_options`, with a
   restart),
   and type `\to` and a pragma.

## Done: cleaner code actions, case split per variable, renaming

### Code actions

The titles lost their `Agda:` prefix and the goal number: `Give`, `Refine`,
`Case split on n`, `Auto`, `Solve`, `Solve all goals`, `Show goal in output`,
`Open output file`. The goal is the one under the cursor anyway.

An empty goal now offers a case split on each variable of its context, and
`Case split on result` (Agda's own name for a split without variables, from
the prompt of `agda2-make-case` in `agda2-mode.el`). The variables come from
`Cmd_goal_type_context`, which Agda answers with each variable's name, type
and whether it is in scope. Variables whose type is a function type or a sort
(`Set`, `Prop`) are left out, because Agda cannot split on them; whether any
other type is a data type Agda only says when it splits. The context is asked
once per goal, only when Agda is not busy, and kept until the next load; a
hover on the goal fills it too. With variables typed in the goal, the one
action splits on those, as before.

### The comment after a case split

Dropping it was not Agda's doing: Agda 2.8 sends the clauses with the code
after the goal (`g zero m = ? + m` for `g n m = {! n !} + m`) but without the
comment, and Emacs's `agda2-make-case-action`, which the bridge ported,
replaces the whole line. The bridge now puts a line comment after the goal,
with the space before it, at the end of the first new clause.

### Renaming

Agda's interaction protocol has no rename command, so the bridge builds one on
the definition sites it already keeps for go to definition
(`bridge/src/rename.rs`):

1. **What changes.** Every place in the open Agda files whose link leads to
   the same definition site, and the definition itself. Agda's highlighting
   gives a link for every use, a bound variable included, a qualified name as
   a whole (`N.suc`, which becomes `N.succ`) and each part of an operator
   (`+` in `n + m`, linked to `_+_`).
2. **Operators.** A new name must have the holes of the old one (`_+_` to
   `_⊕_`, not to `plus`); a name without underscores, typed on one part of an
   operator, renames that part (`⊕` on `+` gives `_⊕_`). A name with spaces or
   one of `(){}";.@` is refused.
3. **Safety checks.** The defining file must be open in Zed and saved since
   its last check, and every other file whose links point into it must have
   been checked after it, or the offsets would be stale; otherwise the rename
   is refused with what to save. Files that are not open are not changed:
   afterwards a message lists those that import the defining module and
   mention the name (not for a bound variable, which is local anyway).
4. **Not renamed:** text in goals, which Agda does not check (the `+` in
   `{! suc (n + m) !}`), and names typed since the last save.

### Proven by tests

| Check | Result |
| --- | --- |
| A comment after the goal stays on the first clause; code after the goal comes from Agda's clauses; `x--y` is no comment | passes; fails without keeping the comment |
| End to end: the new titles; an empty goal offers `Case split on m` and `Case split on result`, but not `Give`; the split on `m` gives `zero + zero` and `zero + suc m` | passes |
| New names: full, on one part of an operator, `if_then_else_`; wrong holes, spaces and reserved characters refused | passes |
| Places: full, qualified (`N.suc`) and operator parts (`N.+`) | passes |
| End to end: renaming `suc` at its definition in `Nat.agda` also changes `Uses.agda` (3 places in 2 files); a keyword cannot be renamed; `a b` is refused; `⊕` on `+` renames `_+_` but not inside a goal; a bound variable `b` | passes; fails when only the file of the request is searched |

### To check in Zed

1. Reinstall the bridge and restart the language server.
2. On an empty goal, open the code actions: a case split per variable.
3. Case split a goal with `-- a comment` after it.
4. Press `F2` on a name used in two open files, and on the `+` of an operator.

## Done: settings under `lsp.agda-bridge.settings`, changing live

The settings were initialization options, an implementation detail. They
cannot sit directly under `lsp.agda-bridge`: Zed's `LspSettings`
(`crates/settings_content/src/project.rs`) has fixed fields, `binary`,
`initialization_options`, `settings`, `enable_lsp_tasks` and `fetch`, and the
extension API hands an extension only the first three. `settings` is Zed's
field for a server's own settings, so they moved there.

1. **The extension** (`src/lib.rs`) passes `settings` both as
   initialization options, so Agda starts with the right program, and as the
   workspace
   configuration, which Zed sends with `workspace/didChangeConfiguration`
   after the start and after every change (`crates/project/src/lsp_store.rs`).
2. **The bridge** (`bridge/src/settings.rs`) reads them with the same checks
   for every kind of value, and applies a change at once: a new `agdaPath` or
   `extraArgs` stops Agda, which starts again at the next load (and every
   file loads again, as Agda's flags may have changed); a new `outputFile` is
   used for the next answer; symbol settings for the next completion. The
   completion provider is therefore always registered, and answers nothing
   when symbol input is `"none"`.
3. **Not yet:** a JSON schema, so that Zed completes and checks the settings
   in `settings.json`. The extension API has
   `language_server_workspace_configuration_schema` from version 0.8.0, which
   is not on crates.io yet (the latest is 0.7.0).

### Proven by tests

| Check | Result |
| --- | --- |
| Settings are read; wrong `agdaPath`, `extraArgs` and `outputFile` keep their defaults and are reported; whether Agda must restart | passes |
| End to end: `didChangeConfiguration` with `"symbolInput": "none"` stops completions, `"latex"` with a trailing space brings them back, and `"tex"` is reported | passes |
| End to end: a missing `agdaPath` is reported at the next save, and the right one loads the file again | passes |

### A note on the test fixtures

`bridge/tests/fixtures/Spike.agda` in the working copy had been edited in
Zed (renaming `suc`, a case split), which made three end-to-end tests wait
for goals that were no longer there. The tests ran against the committed
fixtures instead, in a separate worktree; the edited file was left as it was.
To try things in Zed, a copy outside `tests/fixtures` keeps the tests intact.

## Done: no more `initialization_options`; with-abstraction; on rename

### Settings only under `settings`

The extension no longer reads `lsp.agda-bridge.initialization_options`; the
settings are only read from `lsp.agda-bridge.settings`. (The extension still
hands them to the bridge as initialization options at its start, which is how
Agda starts with the right program; that is internal.)

### Add with abstraction

A code action on a goal that is the whole right-hand side of a clause on one
line, after Idris's "add with", which adds a `with` and a clause for its
result. Agda has no
command for it, so the bridge rewrites the clause itself (`goals::add_with`):

```agda
f n = {! even n !}
```

becomes

```agda
f n with even n
... | w = {!  !}
```

The goal's text becomes the with-expression; an empty goal gives a goal
there (`f n with {!  !}`), which Agda accepts. The name `w` is replaced by
`w₁`, `w₂` and so on when the clause already uses it; a comment after the
goal stays; `...` also works in a with-clause itself. Checked against Agda
2.8.0: both forms load, and a case split on `w` gives `... | true = ?` and
`... | false = ?`. As after a case split, the new goals get numbers when the
file is saved.

### A `Rename` code action, and saving after a rename: not possible

- A code action can run a command on the server, or one of two commands Zed
  handles itself (a task, or showing locations; `try_handle_client_command`
  in `crates/editor/src/code_lens.rs`); none opens Zed's rename prompt, and
  LSP has no way for a server to ask for a text. Zed does show Rename Symbol
  in the menu of a right click (`crates/editor/src/mouse_context_menu.rs`),
  next to Go to Definition.
- LSP has no request to save a file, so a language server cannot save the
  renamed files. Writing them to disk behind Zed's back would race with its
  buffers. Zed's own `autosave` setting does it, also for one project in its
  `.zed/settings.json`; the README shows how.

### Proven by tests

| Check | Result |
| --- | --- |
| The with-abstraction: the goal's text as expression, an empty goal, the indentation, a comment, `w` taken so `w₁`, a with-clause; not for part of a right-hand side or a lambda | passes |
| End to end: the code actions offer `Add with abstraction on n`; after a case split, the with-abstraction on `suc n + m = {!  !}` gives `suc n + m with {!  !}` and `... \| w = {!  !}`, and after saving Agda accepts the file with six goals and no errors | passes |

## Fixed: the outline

The outline (and the breadcrumbs) come from tree-sitter, through
`languages/agda/outline.scm`, which came with the original extension. Three
things were wrong in it:

1. **`ℕ Set` under `ℕ`.** There were two patterns for `data` (and for
   `record`), one with the type and one without. A declaration with a type
   matched both, so Zed showed two items with the same range, the second
   nested in the first. Now one pattern per kind has optional parts
   (`":"? @context (expr)? @context`), which tree-sitter matches once.
2. **`ff` under `tt`.** The signature pattern matched each name of
   `tt ff : 𝔹` separately, but both with the whole signature as their range,
   and Zed nests an item whose range lies inside another's. Now each name is
   its own item with only the name as its range; Zed takes the text of an
   item from its captures, wherever they are (`next_outline_item` in
   `crates/language/src/buffer.rs`), so the type still shows.
3. **No `:`.** Zed joins the captured pieces with a space, so the `:` token is
   captured too: `suc : ℕ → ℕ`, `List (A : Set) : Set` (the parameters are new
   as well). Record fields now show like constructors, and `data Vec A where`,
   the definition of a declared `data`, appears again (the earlier pattern for
   `data` without a type missed it when parameters came between).

Checked with a small program that runs the query with the grammar of
`extension.toml` (tree-sitter-agda `e8d47a6`) and nests the items the way Zed
does: it reproduced the wrong outline first, then gave the right one for all
fixtures and a file with every kind of declaration.

**Limitation, in the grammar:** an empty `record … where` without a type gives
a parse error that takes in the declarations after it, so these show nested
under the record (also an empty `data … where` right after an empty record).
That is tree-sitter-agda's, not the query's.

## Done: add clause, new titles, a space after symbols

### Add clause

A code action on the first line of a type signature, after Idris's "add
clause": `_+_ : ℕ → ℕ → ℕ` gets `n + m = {!  !}` right below the signature.
Agda has no command for it, so the bridge reads the signature's text
(`bridge/src/clause.rs`):

1. **Which lines.** `names : type` at the cursor, with the type going on over
   more indented lines, but not in a block of constructors, fields,
   postulates, generalised variables or primitives (the nearest less indented
   line starts with `data`, `record`, `field`, `postulate`, `variable` or
   `primitive`). One clause per name for `f g : …`.
2. **Which arguments.** The type is split at its arrows outside brackets.
   Implicit and instance arguments (`{A : Set}`, `⦃ _ ⦄`) get no pattern, as
   Agda does not need one; named ones keep their name (`(n : ℕ)`, `∀ m n`).
3. **Which names.** In the spirit of Idris's naming hints: `ℕ` gives `n`, `m`,
   `k`, `l`, `j`; `List` and `Vec` give `xs`, `ys`, `zs`; a function type `f`,
   `g`, `h`; `Set` `A`, `B`, `C`; an equality `p`, `q`, `r`; a type variable
   `x`, `y`, `z`; `𝔹` `b`; `Fin` `i`; another type the first letter of its
   name (`Tree A` gives `t`). A name already taken, by a binder or the
   function itself, moves to the next one, and then to `n₁`, `n₂`.
4. **Operators** are written in mixfix when every hole gets an argument:
   `n + m`, `if b then x else y`; otherwise in prefix, as `_+_ n m k`.

Checked with Agda 2.8: the clauses for `_+_`, `map`, `if_then_else_` and
`replicate` (with implicit and named arguments) load without errors.

### Titles and the space after a symbol

- `Add with abstraction` is now `With-abstract`, with the expression in
  backticks when the goal has one.
- `Case split on` puts the variables in backticks (``Case split on `n` ``),
  but not `result`, which is no variable.
- `symbolTrailingSpace` is now on by default.

### Renaming and saving

Rename Symbol is started with `F2`, `space r` in Helix mode (`vim.json`,
context `helix_normal`) or `g r n` in vim mode, or from the menu of a right
click; the README now says so, and that auto save is not possible, with its
workarounds (`workspace: save all`, `cmd-alt-s` on macOS, and Zed's
`autosave` setting).

### Proven by tests

| Check | Result |
| --- | --- |
| Names after types: `_+_`, `not`, `map`, `zipWith`, `sym`, `id`, `const` with `->`, `if_then_else_`, a constant, `Tree A`, six arguments of type `ℕ` | passes |
| Names of binders: `(n : ℕ)`, `∀ {n} (xs : …) (i : …)`, `∀ m n`; an implicit `n` and a function named `n` push the name on | passes |
| Signatures over several lines, in a `where` block, with a comment; one clause per name; not for constructors, fields, postulates, `data` lines, definitions, comments or lambdas | passes |
| End to end: `Add clause` on the `_+_` signature and not on a constructor; it inserts `n + m = {!  !}` after the signature; the new titles; a space after `\to` by default | passes |

## Done: how to type a symbol, clearer titles, moving between goals

### Hover on a symbol

Outside a goal, hover on a symbol says how to type it, with the notations
that `symbolInput` switches on, from the same tables as completion, turned
around (`input::how_to_type`), as agda2-vscode does:

> `→` (U+2192) is typed with `\r`, `\->`, `\r-`, `\to`, `\rightarrow`, or in
> Typst's notation `#arrow.r`, `#->`.

Abbreviations that give the symbol as their first choice come first, the
shortest first, at most six of each notation. A letter with accents and no
Typst name gets its accents (`é` gives `#acute(e)`, `Ǖ` gives
`#macron(diaer(U))`, from its Unicode decomposition). The symbol is the
character under the cursor with the combining marks and variation selectors
after it. On a goal, hover still shows the goal.

### Titles

`Make clause` (was `Add clause`), `Print goal in output` (was `Show goal in
output`), and ``With-abstract on `e` `` (was ``With-abstract `e` ``).

### Moving between goals

Goals are the only diagnostics of the severity "information", and Zed's
`editor::GoToDiagnostic` and `editor::GoToPreviousDiagnostic` take a severity
filter (`GoToDiagnosticSeverityFilter` in
`crates/project/src/project_settings.rs`, as Zed's own `f8` binding uses), so
a keymap entry can move between goals only; the README shows one for `[ g`
and `] g` in vim and Helix mode, limited to Agda files with the
`extension == agda` key context (`crates/editor/src/editor.rs`). Neither key
is bound in Zed's vim keymap.

### Proven by tests

| Check | Result |
| --- | --- |
| How to type `→` (both notations, the order), only the notations switched on, accents for `é` and `Ǖ`, nothing for a symbol no notation has | passes |
| The symbol under the cursor, with a variation selector; nothing for ASCII, a space or beyond the text | passes |
| End to end: hover on `ℕ` outside a goal shows `\bN` and `#NN`; the new titles | passes |

## Done: hover showing the type of a name

Outside goals, hover on a name shows its type, asked from Agda with
`Cmd_infer_toplevel Simplified "<name>"`, which answers
`{"kind": "InferredType", "expr": "ℕ → ℕ"}` (checked with Agda 2.8.0):

```agda
suc : ℕ → ℕ
```

1. **Which name.** The link under the cursor, from Agda's highlighting, as it
   is written (`N.suc` stays qualified). A part of an operator (`+` in
   `n + m`) stands for the whole operator: the bridge reads the name at the
   definition site (`_+_`) and keeps the qualifier, so `N.+` asks for
   `N._+_`.
2. **Which names have no type to show.** Agda answers in the scope at the top
   level of its current file, so a bound variable (left out from the start), a
   name from a `where` block, and a name of an inner module written without its
   module (`three` for `Inner.three`) give "Not in scope", and hover shows
   nothing for them. Only the file Agda loaded last can be asked.
3. **Speed.** The answers, also the "nothing", are kept per file until its
   next load, and Agda is only asked when it is not busy, so hover never waits
   for a load.
4. **With a symbol** such as `ℕ`, the type comes first and how to type the
   symbol after it.

| Check | Result |
| --- | --- |
| Agda's `InferredType` answer is read; the command is built | passes |
| End to end: `ℕ : Set` with how to type `ℕ`; `_+_ : ℕ → ℕ → ℕ` from the `+` of `zero + m`; nothing for the bound `m`; `suc : ℕ → ℕ` imported in `Uses.agda` | passes; fails when the operator part is asked as written |

## Next steps

Still to do in this phase:

1. **Why a name is in scope**, as a code action on a name, with Agda's answer
   (`Cmd_why_in_scope`, or `Cmd_why_in_scope_toplevel` outside a goal) in the
   output file: where the name was defined or imported from.
2. **Create a helper function**, as a code action on a goal with an
   expression: Agda gives the type of a function that abstracts the goal's
   expression over its free variables (`Cmd_helper_function`); the bridge
   adds that signature, and a clause for it, above the definition the goal is
   in, and calls it in the goal.
3. **The goal's type with the type of its expression**, as a code action on a
   goal with an expression, with Agda's answer in the output file
   (`Cmd_goal_type_context_infer`): the goal's type and context, and the type
   of what is typed in it, to compare the two.
4. **The normal form of the goal's expression**, as a code action on a goal
   with an expression, with the answer in the output file (`Cmd_compute`).
5. Before publishing: an issue at `haohanyang/agda-zed`, proposing the bridge
   or asking to take over the `agda` id.

The hover cache moved to phase 3, optimisations ([`PHASE3.md`](PHASE3.md)).
