# Plan: interactive Agda in Zed without the Agda Language Server

This document investigates how to turn this repository into a Zed extension for
interactive Agda development, comparable to Emacs `agda2-mode` and to
[agda2-vscode](https://github.com/willtunnels/agda2-vscode), without using the
[Agda Language Server](https://github.com/agda/agda-language-server) (ALS).

It explores two directions, as requested:

1. **LSP tricks** in the style of the Zed Idris 2 extension, where information
   appears where you hover and actions appear in the code action menu.
2. **Emacs-like commands** in the style of agda2-vscode, where every Agda
   command has its own key chord.

Each direction is assessed on what it takes, which features are available,
which are not and why. The document ends with a recommendation and a phased
roadmap. The sources consulted, and how each was used, are listed at the end.

---

## 1. Summary

- A Zed extension is a WebAssembly module that Zed calls at fixed moments. It
  can only run a process to completion (`process::Command::output()`), so it
  can never keep an Agda session alive by itself. The only long-lived process
  Zed lets an extension start is a **language server**, and the only channel
  that process has to the editor is **LSP**.
- Therefore both directions need the same core: a native Rust program, here
  called **`agda-bridge`**, which speaks LSP to Zed on one side and Agda's own
  `agda --interaction-json` protocol on the other. This is exactly the role
  agda2-vscode's TypeScript core plays inside VS Code, and roughly 3,600 lines
  of its code (protocol, goals, edits, highlighting, offsets) can be ported
  almost one to one.
- **Direction 1** maps very well onto what Zed supports today: hover on a hole
  for its goal and context, diagnostics for errors and goals, code actions and
  clickable code lenses for give, refine, case split and auto, semantic tokens
  for Agda's own highlighting (including background colours for unsolved
  metas), go to definition, and Unicode input through completions.
- **Direction 2** is possible, but only indirectly: Zed extensions cannot ship
  commands or keybindings, so the extension would ship *tasks* (which Zed does
  load from extensions) that call a tiny client, and the user pastes a keymap
  snippet that binds Emacs chords such as `ctrl-c ctrl-l` to those tasks.
- Your `output.md` idea works well in Zed: the bridge rewrites the file, Zed
  reloads it automatically, and Zed's Markdown preview can render it beside
  the code, with Agda code blocks highlighted.
- **Recommendation**: keep this repository, replace its engine. Build the
  bridge once, ship direction 1 as the primary experience, and add direction 2
  afterwards as a thin layer on the same bridge (a local socket, a client
  subcommand and a `tasks.json`), so that you get hover-driven exploration
  *and* Emacs muscle memory. Publish
  under a new extension id, unless the owner of the `agda` id upstream agrees
  to take the rewrite.

---

## 2. Starting point: what this repository contains today

| File | What it does | Keep? |
| --- | --- | --- |
| `extension.toml` | id `agda`, tree-sitter-agda grammar pinned at `e8d47a6`, language server `als` | Keep grammar, replace server entry |
| `languages/agda/config.toml` | name, `.agda` suffix, comment tokens | Keep, extend |
| `languages/agda/highlights.scm` | tree-sitter highlighting (copied from tree-sitter-agda) | Keep as fallback |
| `languages/agda/outline.scm` | outline for functions, modules, data, records, postulates | Keep |
| `languages/agda/brackets.scm` | bracket pairs | Keep |
| `src/lib.rs` (about 50 lines) | finds `als` on `PATH` or via `lsp.als.binary.path` and launches it | Rewrite |

Two facts about its position in the ecosystem matter for section 8:

- This repository (`timjs/zed-agda`) is a fork. The `agda` id in Zed's
  extension registry points at `haohanyang/agda-zed`, version 0.2.1
  (`extensions.toml` and `.gitmodules` in `zed-industries/extensions`).
- The ALS integration was added upstream on purpose (commit `53d659c`,
  "added Agda Language Server"), so removing it is a change of direction for
  that project, not a bug fix.

---

## 3. The hard constraints: what Zed allows

All statements below were verified in Zed's source at commit `17d3378`
(1 October 2026), not taken from memory. File references are given so they
can be checked.

### 3.1 What an extension can and cannot be

- Extensions can provide languages, language servers, debuggers, themes, icon
  themes, snippets and MCP servers (`docs/src/extensions/developing-extensions.md`).
  There is no API for custom panels, webviews, commands in the command palette,
  keybindings, text decorations or input boxes.
- The extension's Rust code is compiled to `wasm32-wasip2` and runs only inside
  callbacks such as `language_server_command`. The process API offers only
  `Command::output()`, which runs a program to completion
  (`crates/extension_api/src/process.rs`, API version 0.8.0). A persistent
  `agda --interaction-json` session is therefore impossible inside the
  extension itself.
- The extension *can* download files and query GitHub releases
  (`download_file`, `latest_github_release`), which is how most Zed extensions
  install their language server binary. It could also download Agda itself,
  as agda2-vscode does.

**Consequence**: the interactive core must be a separate native program that
Zed starts as a language server. This is also why the Idris extension is only
30 lines of Rust: all of its "tricks" live in the separate `idris2-lsp` server.

### 3.2 What Zed's LSP client supports

From the `ClientCapabilities` Zed sends on `initialize`
(`crates/lsp/src/lsp.rs`, `default_initialize_params`):

| LSP feature | Supported | Relevance for Agda |
| --- | --- | --- |
| `textDocument/hover` (Markdown) | yes | goal type and context on a hole |
| `publishDiagnostics` and pull diagnostics | yes | errors, warnings, goals |
| `textDocument/codeAction` with `resolve` and `data` | yes | give, refine, case split, auto, solve |
| `workspace/executeCommand` | yes | run an Agda command when an action is chosen |
| `workspace/applyEdit` (server to client) | yes | insert give results and case splits |
| `textDocument/codeLens` | yes, drawn above lines and clickable when `"code_lens": "on"` (`crates/editor/src/code_lens.rs`) | clickable "Give, Refine, Case split" above a hole |
| `textDocument/inlayHint` | yes | goal type shown inline after a hole |
| `textDocument/semanticTokens` (full and delta, custom types) | yes | Agda's own highlighting |
| `textDocument/definition` | yes | jump to definition sites reported by Agda |
| `textDocument/documentHighlight`, `rename` | yes | highlight and rename within a file |
| `textDocument/completion` (with `textEdit`) | yes | Unicode input (`\to` becomes `→`) |
| `window/showMessage` | yes, shown as a notification (`crates/project/src/lsp_store.rs`) | short messages such as "No goal at cursor" |
| `window/showMessageRequest` | yes, buttons only | yes or no questions, no free text |
| `window/showDocument` | yes, opens the file in the active pane, optionally with a selection and without stealing focus (`crates/editor/src/items.rs`) | open `output.md` |
| `$/progress` (work done progress) | yes | "Agda: checking Foo.agda" |

Semantic tokens deserve a special mention. A language extension may ship a
`semantic_token_rules.json` that maps custom token types to theme styles, and
rules may set `background_color`, `underline` and `font_style`
(`docs/src/extensions/languages.md`). That means Agda's background highlights
(unsolved metas, termination problems, coverage problems) can be reproduced,
which is something I did not expect before reading the source.

### 3.3 What is missing or limited

- **No free text input.** LSP has no input box. agda2-vscode prompts for an
  expression when a hole is empty, or for a search term; that has no LSP
  equivalent.
- **No binding of a key to one specific code action.** Zed only has
  `editor::ToggleCodeActions` (open the menu) and `editor::ConfirmCodeAction`
  (pick an index while the menu is open). `action::Sequence` exists, but its
  own documentation says it "does not wait for asynchronous actions to
  complete before running the next action" (`crates/settings/src/keymap_file.rs`),
  so chaining "open menu, pick item 2" would race against the server.
- **The server cannot save a buffer.** Agda's `Cmd_load` reads the file from
  disk, so a load can only see what has been saved.
- **Several features are off by default** (`assets/settings/default.json`):
  `semantic_tokens` (`"off"`), `inlay_hints.enabled` (`false`), `code_lens`
  (`"off"`), inline diagnostics (`false`) and `autosave` (`"off"`). An
  extension cannot change user settings, so the README must provide a
  recommended settings block.

### 3.4 Tasks: the back door for commands

- A language directory inside an extension may contain a `tasks.json`, which
  Zed loads as task templates for that language
  (`crates/extension_host/src/extension_host.rs`, `TaskTemplates::FILE_NAME`).
- Tasks receive the cursor and file through variables such as `ZED_FILE`,
  `ZED_ROW`, `ZED_COLUMN` and `ZED_SELECTED_TEXT` (`docs/src/tasks.md`).
- Tasks can save the current buffer first with `"save": "current"`
  (`crates/task/src/task_template.rs`, `SaveStrategy`).
- Users can bind keys, including multi key chords, to a task with
  `["task::Spawn", { "task_name": "..." }]`, and a keymap context
  `Editor && extension == agda` restricts the chord to Agda files
  (`crates/editor/src/editor.rs`, `key_context` sets `extension`).

This is what makes direction 2 feasible at all.

---

## 4. The shared core: `agda-bridge`

Both directions need the same program. Its architecture:

```
 ┌──────────────────────── Zed ────────────────────────┐
 │  Agda extension (WASM)                               │
 │   - finds or downloads agda-bridge (and maybe Agda)  │
 │   - returns the command to start it                  │
 │   - ships tasks.json, semantic_token_rules.json      │
 └───────────────┬──────────────────────────────────────┘
                 │ LSP over stdio
        ┌────────▼─────────┐  stdin/stdout   ┌────────────────────────┐
        │   agda-bridge    │◄───────────────►│ agda --interaction-json │
        │  (native Rust)   │   IOTCM / JSON  └────────────────────────┘
        └──┬───────────┬───┘
           │           │ writes
           │           ▼
           │      output.md  (Agda information buffer)
           │
           └─ local socket (only for direction 2: task client)
```

### 4.1 Modules, and where each comes from in agda2-vscode

agda2-vscode is MIT licensed (its Unicode engine, taken from vscode-lean4, is
Apache 2.0), so porting its logic is allowed with attribution.

| Bridge module | Responsibility | Ported from (agda2-vscode) | Size there |
| --- | --- | --- | --- |
| `protocol::iotcm` | Haskell string quoting, `IOTCM` envelope | `util/iotcm.ts` | 48 |
| `protocol::commands` | builders for every `Cmd_*`, with version gates for 2.6.1 to 2.8 | `agda/commands.ts`, `agda/version.ts` | 618 |
| `protocol::responses` | serde types for all JSON responses, normalisation | `agda/responses.ts` | 263 |
| `agda::process` | spawn Agda, line buffered parser, detect the `JSON> ` prompt | `agda/process.ts`, `agda/protocol.ts` | 340 |
| `agda::queue` | one command in flight, streaming highlight responses, abort | `core/commandQueue.ts` | 133 |
| `text::offsets` | Agda's 1 based code point offsets to LSP UTF-16 positions | `util/offsets.ts`, `util/position.ts` | 132 |
| `goals` | interaction points, `?` to `{!  !}` expansion, ranges kept in sync with edits | `core/goals.ts`, `util/editAdjust.ts` | 842 |
| `edits` | give, refine, make case, solve all turned into text edits, cursor placement | `core/responseProcessor.ts` | 310 |
| `highlighting` | Agda aspects to semantic tokens, definition sites | `core/highlighting.ts` | 657 |
| `locations` | parse `file:10.5-15` (and the pre 2.8 `10,5-15` form) | `util/agdaLocation.ts` | 326 |
| `render` | `DisplayInfo` to Markdown (new, but modelled on the info panel) | `editor/infoPanel.ts` | 778 |
| `unicode` | abbreviation table (2,627 entries generated from Agda's Emacs input method) | `unicode/abbreviations.json` | data |
| `lsp` | the LSP server itself | new | |

The VS Code specific parts (key sequence state machine, webview, input box,
VSCodeVim undo handling, eager abbreviation rewriter) do not carry over.

Suggested Rust stack: `tokio` for the Agda pipe and the LSP loop,
`tower-lsp-server` (the maintained fork of `tower-lsp`) or `async-lsp` for the
LSP side, `lsp-types`, `serde_json`. An async design matters here, because
Agda streams highlighting while Zed keeps sending requests.

### 4.2 Design problems the bridge must solve (in both directions)

1. **When to load.** Agda reads from disk and the bridge cannot save, so the
   natural trigger is `textDocument/didSave` (plus `didOpen`). With Zed's
   `autosave` set to `on_focus_change` or `after_delay`, this feels like
   continuous checking. Direction 2 can also save explicitly through
   `"save": "current"`.
2. **Goals that move while you type.** Between two loads the buffer changes.
   The bridge mirrors every `didChange` and shifts goal ranges, removing goals
   whose `{! !}` delimiters were edited, exactly like agda2-vscode's
   `editAdjust.ts`. Goal commands (give, refine, case split) only need the goal
   id and the text inside the hole, so they keep working on an unsaved buffer
   as long as the goal survived. This is an improvement over idris2-lsp, which
   simply refuses hover and code actions while a file has unsaved changes.
3. **Agda is strictly serial.** Only one command can run at a time, and a
   load can take minutes. Hover must not wait behind a load, so the bridge
   caches: after every load it already has every goal's type (from the
   `AllGoalsWarnings` response), it then fetches `Cmd_goal_type_context` for
   each goal in the background, and hover answers from that cache. Cached
   entries are dropped on reload, give and edits inside the goal.
4. **One Agda process, one current file.** Agda keeps a single "current file"
   (agda2-vscode tracks it in `WorkspaceState.currentFile`). A goal command in
   a different file first reloads that file. One Agda process per open file is
   possible but expensive in memory.
5. **Offsets.** Agda uses 1 based code point offsets, and Zed only negotiates
   UTF-16 positions. Characters outside the Basic Multilingual Plane (for
   example mathematical script letters) are two UTF-16 units, which is where
   off by one bugs come from. agda2-vscode's tests for this can be ported.
6. **Undo.** When a give is undone, the restored text must not resurrect the
   goal (Emacs semantics). Zed sends an undo as an ordinary `didChange`, so the
   bridge needs the "merge into one change, then check delimiter crossings"
   logic from `computeSingleChange`.
7. **Versions.** Command syntax differs between Agda 2.6.1, 2.6.2, 2.7 and
   2.8. The gate table at the top of agda2-vscode's `commands.ts` is the
   checklist.

---

## 5. Direction 1: LSP tricks, in the style of the Idris 2 extension

### 5.1 What the Idris extension actually does

The Zed Idris 2 extension (`dylanbraithwaite/zed-idris2-lsp`) contains a
tree-sitter grammar reference, a few queries and a `lib.rs` that runs
`idris2-lsp` from `PATH`. Everything interactive is done by
[idris2-lsp](https://github.com/idris-community/idris2-lsp), which offers:

- hover with `name : type` for the identifier or hole under the cursor,
  rendered as an `idris` code block;
- code actions: case split, expression search, generate definition, intro,
  make case, make with, make lemma, refine hole, add clause, computed eagerly
  when the menu opens and returned with a ready made `edit`;
- semantic tokens, go to definition, document highlight, document symbols,
  completion, signature help;
- custom `workspace/executeCommand` commands (`repl`, `metavars`), which Zed
  has no way to call, so the full hole context is not reachable from Zed.

Notably idris2-lsp's hover shows only the type, not the context, and does not
work while the file has unsaved changes. For Agda we can do better on both.

### 5.2 How each Agda feature maps onto Zed

| Agda feature (Emacs key) | LSP mechanism | What the user sees in Zed |
| --- | --- | --- |
| Load (`C-c C-l`) | `didSave` triggers `Cmd_load`; `$/progress` while checking | save the file, status shows "checking", results appear |
| Errors and warnings | diagnostics, ranges parsed from Agda's locations | red and yellow squiggles, the diagnostics panel, full text in `output.md` |
| Goal list (`C-c C-?`) | one Information diagnostic per hole, `?3 : ℕ → ℕ` | all holes listed in the project diagnostics panel; `F8` and `shift-F8` jump between all diagnostics |
| Next and previous goal (`C-c C-f`, `C-c C-b`) | Zed's own `editor::GoToDiagnostic` with `{ "severity": "information" }`, which skips errors and warnings (`crates/project/src/project_settings.rs`) | a real keybinding, no task needed, provided goals are the only Information diagnostics |
| Goal type and context (`C-c C-,`) | hover on `?` or `{! !}` | popup with goal type, context and (for cubical Agda) boundary, as an `agda` code block |
| Goal type inline | inlay hint after the hole | `{! !} : ℕ → ℕ` (needs `inlay_hints.enabled`) |
| Give, refine, case split, auto, solve (`C-c C-SPC`, `C-r`, `C-c`, `C-a`, `C-s`) | code actions on a hole, executed through `workspace/executeCommand`, results applied with `workspace/applyEdit` | `ctrl-.` (or `g .` in vim mode) on a hole shows "Give", "Refine", "Case split on n", and so on |
| Same, one click | code lens above the hole's line | `?3 · Give · Refine · Case split · Auto` (needs `"code_lens": "on"`) |
| Case split without typing the variable | the bridge already knows the context, so it offers one action per variable | "Case split on xs", "Case split on n", which is nicer than Emacs |
| Infer type, normalise (`C-c C-d`, `C-c C-n`) inside a hole | code action on the hole, uses the hole's contents | result in hover cache and `output.md` |
| Infer type, normalise at top level | code action on a *selection* | select `foo bar`, `ctrl-.`, "Infer type of selection" |
| Type of a global name | hover on an identifier runs `Cmd_infer_toplevel` on it | `foo : ℕ → ℕ` in a popup, like idris2-lsp |
| Why in scope, module contents (`C-c C-w`, `C-c C-o`) | code action on the selected name | result in `output.md` |
| Search about (`C-c C-z`) | code action on the selection, or on the hole's contents | result in `output.md` |
| Highlighting | semantic tokens from Agda's highlighting info, plus `semantic_token_rules.json` | Agda's exact highlighting, with backgrounds for unsolved metas and termination problems (needs `"semantic_tokens": "combined"`) |
| Go to definition | definition sites from Agda's highlighting | `F12` or `cmd-click`, also into the standard library |
| Highlight occurrences, rename (current file only) | document highlight and rename from the same data | as in agda2-vscode |
| Unicode input (`\to`) | completion items with `\` as trigger character | type `\to`, pick `→` with Enter or Tab; alternatives such as `\to` giving `→ ⟶ ⇒` are separate items |
| Which abbreviation types this symbol | hover on a Unicode character | "Type ⊓ using `\glb` or `\sqcap`" |
| Restart, abort, toggle implicit arguments, compile | "source" code actions available anywhere in an Agda file; Zed's own "restart language server" also restarts Agda | `ctrl-.` anywhere, or the command palette for restart |
| Agda information buffer | `output.md`, see section 6 | rendered beside the code |

### 5.3 What will not work in direction 1, and why

- **No dedicated key per command.** Zed cannot bind a key to one specific code
  action (section 3.3), so every command goes through the `ctrl-.` menu or a
  code lens click. Two keystrokes plus a choice, instead of one chord.
- **No prompts.** When a hole is empty and you choose "Give", there is nowhere
  to type the expression. The Emacs workflow (type into the hole first, then
  give) works; agda2-vscode's convenience prompt does not.
- **Normalisation levels** (the `C-u` prefixes) cannot be a modifier. They
  become either extra entries in the menu ("Goal type, normalised") or a
  setting.
- **Hover is ephemeral.** Long contexts are cramped in a popup, which is why
  `output.md` is still needed as the persistent view.
- **Loading needs a save**, and the nice parts need settings switched on
  (semantic tokens, inlay hints, code lens), which only the user can do.
- **Eager Unicode replacement** (Emacs replaces `\to` as you type) would mean
  the server editing the buffer on every keystroke, which races with typing
  and pollutes undo history. Completions are the realistic option.

---

## 6. Displaying output: the `output.md` idea, assessed

Your proposal is sound, and Zed's behaviour supports it well:

- **Mechanism.** After every command the bridge writes Markdown (headings,
  goal types and contexts in fenced `agda` code blocks, errors as text) to a
  single file, the equivalent of Emacs's `*Agda information*` buffer. Zed
  reloads a file changed on disk automatically as long as its buffer has no
  unsaved edits.
- **Rendered view.** Zed's Markdown preview ("markdown: open preview to the
  side") re-renders when the buffer changes, and code fences are highlighted
  with the Agda grammar, so goal types appear coloured.
- **Opening it.** The bridge can call `window/showDocument` on the first load
  with `takeFocus: false`. Zed opens it in the active pane (it cannot request a
  split), so the user moves it to a split once; Zed remembers the layout per
  workspace afterwards.
- **Location.** Make it a setting (`lsp.agda-bridge.settings.outputFile`).
  Inside the worktree (for example `.zed/agda-output.md`) it is easy to open
  from the project panel but must be git ignored; outside the worktree it
  causes no noise but is harder to find. I would default to inside the
  worktree and document the `.gitignore` line.
- **Pitfalls.** Write atomically (temporary file, then rename) so Zed never
  reads a half written file. If the user edits `output.md`, Zed stops
  reloading it, so the header should say that the file is generated. Whether
  links such as `Foo.agda:10.5-15` can be made clickable in the preview needs a
  spike, because I could not verify how Zed's preview resolves line anchors.

In practice the output is spread over four channels: hover for quick looks,
diagnostics for anything that should stay visible, `showMessage` for short
notices, and `output.md` for everything long.

---

## 7. Direction 2: Emacs-like commands, in the style of agda2-vscode

### 7.1 How commands reach the bridge

Since Zed extensions cannot define commands or keybindings, direction 2 uses
tasks:

1. The extension ships `languages/agda/tasks.json` with one task per Agda
   command, for example:

   ```json
   [
     {
       "label": "agda: load",
       "command": "agda-bridge",
       "args": ["client", "load", "--file", "$ZED_FILE"],
       "save": "current",
       "reveal": "never",
       "hide": "on_success"
     },
     {
       "label": "agda: give",
       "command": "agda-bridge",
       "args": ["client", "give", "--file", "$ZED_FILE",
                "--row", "$ZED_ROW", "--column", "$ZED_COLUMN"],
       "reveal": "never",
       "hide": "on_success"
     }
   ]
   ```

2. `agda-bridge client` is a tiny mode of the same binary. It connects to the
   running bridge over a local socket (a Unix domain socket, or a named pipe on
   Windows, whose name is derived from the worktree root), sends the request
   and exits.
3. The bridge runs the Agda command, applies edits through
   `workspace/applyEdit` and writes `output.md`, exactly as in direction 1.
4. The README provides a keymap snippet, for example:

   ```json
   {
     "context": "Editor && extension == agda",
     "bindings": {
       "ctrl-c ctrl-l": ["task::Spawn", { "task_name": "agda: load" }],
       "ctrl-c ctrl-space": ["task::Spawn", { "task_name": "agda: give" }],
       "ctrl-c ctrl-c": ["task::Spawn", { "task_name": "agda: case split" }]
     }
   }
   ```

   and an evil style variant (`space m l`) using Zed's vim mode contexts.
   Next and previous goal need no task at all: they bind directly to
   `["editor::GoToDiagnostic", { "severity": "information" }]` and its
   `GoToPreviousDiagnostic` counterpart (see section 5.2).

### 7.2 Features in direction 2

Every command in agda2-vscode's README becomes a task: load, give (with and
without force), elaborate and give, refine (with and without pattern matching
lambda), auto, case split, goal type, goal type and context (plus inferred or
checked type), context, helper function type, infer type, compute normal form,
why in scope, search about, module contents, constraints, show goals, solve,
next and previous goal, restart, abort, toggle implicit and irrelevant
arguments, remove annotations, compile. "Maybe top level" commands work the
same way, because the client sends the cursor position and the bridge decides
whether it is inside a hole.

Two things direction 2 can do that direction 1 cannot:

- **Free text prompts.** A task with `"reveal": "always"` runs in Zed's
  terminal, so `agda-bridge client search-about` can simply ask for a line of
  input there and then close. This brings back agda2-vscode's prompts (without
  Unicode abbreviations inside the prompt).
- **Normalisation prefixes.** `ctrl-c ctrl-u ctrl-t` can be bound to a
  separate task that passes `--rewrite instantiated`.

### 7.3 What will not work in direction 2, and why

- **The extension cannot install the keybindings.** The user must paste the
  keymap snippet once. Tasks themselves do come with the extension.
- **`ctrl-c` is copy on Linux and Windows.** Binding `ctrl-c ctrl-l` makes
  `ctrl-c` a prefix in Agda files, so Zed waits briefly for a second key
  before copying. On macOS (where copy is `cmd-c`) there is no conflict. The
  snippet should offer an alternative leader.
- **Each command starts a process.** That costs some tens of milliseconds and
  may briefly show a terminal tab, depending on `reveal` and `hide`. It is
  noticeably less snappy than a native command.
- **A race on fast typing.** The task reads row and column at spawn time,
  while the bridge's copy of the buffer is only as fresh as the last
  `didChange`. In practice Zed sends changes promptly, but the bridge should
  validate that the position is inside a known goal before acting.
- **Still no decorations or status bar item.** Goal labels such as `?0` and
  the "busy" indicator come from inlay hints and progress, as in direction 1.

---

## 8. Extend this repository or start a new one?

| Option | Pros | Cons |
| --- | --- | --- |
| Contribute to `haohanyang/agda-zed` (keeps the `agda` id) | existing users get it automatically | it is a rewrite that removes ALS, which upstream deliberately added; needs the owner's agreement |
| Keep this fork, rewrite the engine, publish under a new id | keeps history and the tree-sitter queries; full control | two extensions would both define the language "Agda", so users must uninstall the old one |
| Brand new repository | clean start | loses history for no technical gain, since only `lib.rs` is replaced anyway |

What is reusable from this repository is small but real: the grammar pin, the
four query files, `config.toml`, and the "binary path from `lsp.*.binary.path`
settings" pattern in `lib.rs`. Everything else is new.

**Recommended layout**, all in this repository (Zed's registry supports
extensions in a subdirectory through a `path` field, which 123 registry
entries already use):

```
zed-agda/
  extension.toml            id, grammar, [language_servers.agda-bridge]
  Cargo.toml                the WASM extension crate only
  src/lib.rs                find or download agda-bridge, start it
  languages/agda/
    config.toml             plus word_characters for "\"
    highlights.scm  outline.scm  brackets.scm
    tasks.json              direction 2
    semantic_token_rules.json
  bridge/                   separate crate, NOT in the extension's workspace
    Cargo.toml              tokio, tower-lsp-server, lsp-types, serde
    src/...
  .github/workflows/        build bridge binaries for 5 platforms on release
  docs/PLAN.md
```

Keeping `bridge/` outside the extension's Cargo workspace matters, because Zed
compiles the extension crate for `wasm32-wasip2`, where tokio and process
pipes do not build.

---

## 9. Recommendation

**Build the bridge once, ship direction 1 as the primary experience, then add
direction 2 as a thin layer on top.** The two directions are not alternatives:
they are two front ends to the same engine, and direction 2 adds roughly a
socket listener, a client subcommand, a `tasks.json` and documentation.

Why direction 1 first:

- it is native to Zed, so it works with the mouse, in vim mode, in remote
  development and in the command palette, without any keymap editing;
- it covers the commands used most often (load, goal and context, give,
  refine, case split, auto) with arguably a better experience than Emacs
  (hover instead of a command, case split suggestions per variable);
- semantic tokens give Agda's own highlighting, which removes the dependency on
  the incomplete tree-sitter grammar for correctness.

Why direction 2 still:

- you explicitly value Emacs style chords, and they are the only way to get
  one key per command in Zed today;
- it restores free text prompts, which LSP cannot provide.

### 9.1 Roadmap

| Phase | Goal | Contents |
| --- | --- | --- |
| 0. Spike | prove the risky assumptions | extension starts bridge; `Cmd_load` on save; one diagnostic; hover on a hole; give through `executeCommand` plus `applyEdit`; `output.md` reload and preview; task, socket, bridge round trip |
| 1. MVP | usable daily | process, queue, protocol types, offsets, goal tracking, `?` expansion, diagnostics, hover with cached goal info, code actions for give, refine, case split, auto, solve, `output.md` |
| 2. Polish | match agda2-vscode's comfort | semantic tokens and rules, go to definition, document highlight, inlay hints, code lenses, Unicode completions and hover, top level commands on a selection |
| 3. Commands | direction 2 | `tasks.json`, `agda-bridge client`, socket, keymap snippets (Emacs and vim leader), prompts in the terminal |
| 4. Distribution | easy install | release CI for the bridge, automatic download in the extension, optional Agda download, version gates 2.6.1 to 2.8, tests ported from agda2-vscode's fixtures plus integration tests against a real Agda |

### 9.2 Risks and open questions

| Risk | Mitigation |
| --- | --- |
| Goal tracking bugs during edits and undo | port agda2-vscode's `editAdjust` tests first, they encode many edge cases |
| Hover feels slow during long loads | cache per goal, never block hover on the queue, return "Agda is busy" instead |
| Users do not enable semantic tokens, inlay hints, code lens | settings block in the README; everything essential (hover, diagnostics, code actions) works with defaults |
| `showDocument` opens `output.md` in the same pane as the code | only call it once, document how to move it to a split |
| Clickable locations in `output.md` | to be verified in phase 0 |
| Duplicate "Agda" language with the old extension installed | clear README note, or coordinate with upstream about the `agda` id |
| Literate Agda (`.lagda.md`, `.lagda.tex`) | out of scope for now; needs grammar injections and different offsets |

---

## 10. Sources and how they were used

| Source | What I took from it |
| --- | --- |
| This repository (`src/lib.rs`, `extension.toml`, `languages/agda/*`, git log) | current state, what is reusable, that ALS was added upstream on purpose |
| [dylanbraithwaite/zed-idris2-lsp](https://github.com/dylanbraithwaite/zed-idris2-lsp) (`src/lib.rs`, `extension.toml`) | showed that the Idris extension has no logic of its own; all tricks live in the server |
| [idris-community/idris2-lsp](https://github.com/idris-community/idris2-lsp) (`README.md`, `doc/commands.md`, `src/Server/Capabilities.idr`, `src/Server/ProcessMessage.idr`, `src/Language/LSP/CodeAction/CaseSplit.idr`) | the list of LSP tricks, that hover shows only `name : type`, that hover refuses dirty files, that code actions are computed eagerly, that code lens is unsupported |
| [willtunnels/agda2-vscode](https://github.com/willtunnels/agda2-vscode) (`README.md`, `ARCHITECTURE.md`, `src/agda/*.ts`, `src/core/*.ts`, `src/editor/commands.ts`, `LICENSE`) | the full command list, the `--interaction-json` protocol and its version differences, the module structure to port, line counts, the licence |
| [zed-industries/zed](https://github.com/zed-industries/zed) at commit `17d3378` (docs and source files cited in section 3) | every statement about what Zed and its extension API can do, verified in code rather than recalled |
| [zed-industries/extensions](https://github.com/zed-industries/extensions) (`extensions.toml`, `.gitmodules`) | who owns the `agda` id, and that subdirectory extensions are supported |

The Zed website (zed.dev) was blocked by the network proxy during this
investigation, so the Markdown documentation was read from the `docs/` folder
of the Zed repository instead, which is the source the website is built from.
