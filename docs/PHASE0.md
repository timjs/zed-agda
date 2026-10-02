# Phase 0: spike results

Phase 0 of the roadmap in [`PLAN.md`](PLAN.md) set out to prove the risky
assumptions before building anything large. This document records what was
built, what is now proven (and how), what was learned along the way, the
checks that only a person running Zed can do, and their results.

## What was built

| Part | Files | Purpose |
| --- | --- | --- |
| The bridge | `bridge/` (separate crate, about 2,200 lines including unit tests) | Language server for Zed, driving `agda --interaction-json` |
| Protocol | `bridge/src/iotcm.rs`, `protocol.rs`, `agda.rs` | Commands, responses, the Agda process |
| Positions and goals | `bridge/src/text.rs`, `goals.rs`, `location.rs` | Code points, UTF-16 and UTF-8 columns; goals that follow edits |
| LSP | `bridge/src/server.rs`, `render.rs`, `output.rs` | Load, diagnostics, hover, code actions, give and refine, output file |
| Debug client | `bridge/src/socket.rs` | `agda-bridge client …` sends a command to a running bridge from a terminal |
| Extension | `src/lib.rs`, `extension.toml`, `Cargo.toml` | Starts `agda-bridge` instead of the Agda Language Server |
| Tests | `bridge/src/*.rs` (21 unit tests), `bridge/tests/lsp.rs` | The end-to-end test plays Zed's role against a real Agda |

## What is now proven

The end-to-end test (`bridge/tests/lsp.rs`) acts as an LSP client, the way
Zed does, and runs the real bridge against **Agda 2.8.0** (the official Linux
release binary). Every row below is an assertion in that test.

| Assumption from the plan | Result | How it was checked |
| --- | --- | --- |
| Loading on open (and save) produces diagnostics | proven | opening `Spike.agda` gives three Information diagnostics, `?0 : ℕ`, `?1 : ℕ`, `?2 : 𝔹` |
| Goal ranges are right in UTF-16, also after characters outside the Basic Multilingual Plane | proven | goal `?2` follows a `𝔹` (two UTF-16 units) and its range starts at the right column |
| Hover on a hole shows goal and context | proven | hover shows `Goal: ℕ` with `n : ℕ`, and `Goal: 𝔹` with `b : 𝔹`; hover outside a goal shows nothing |
| Give works through `executeCommand` and `workspace/applyEdit` | proven | the bridge asks the client to replace `{! suc (n + m) !}` by `suc (n + m)` |
| Goals follow unsaved typing | proven | typing `suc` into a hole without saving, then refining, refines the right goal |
| Refine creates new goals that work at once | proven | `suc ?` becomes `suc {!  !}`, the new goal is `?3`, and hover on it shows its context |
| The output file is written, and Zed is asked to open it once without taking focus | proven (bridge side) | `.zed/agda-output.md` contains the goals; one `window/showDocument` with `takeFocus: false` |
| The debug client reaches the running bridge, with UTF-8 byte columns | proven | `agda-bridge client goal --row … --column …` finds `?2` behind `λ`, `𝔹` and `→`, and writes it to the output file |
| Failures of a client request also show in Zed | proven | a request outside a goal fails and the bridge sends `window/showMessage` |
| Type errors become diagnostics on the right range | proven | `Bad.agda` gives an Error on line 7, columns 5 to 8, starting `error: [UnequalTerms]` |
| Saving a file that is not Agda does not reach Agda | proven | a `didSave` for `TODO.md` produces no diagnostics; without the fix this assertion fails |
| The output file is opened only once | proven | every command rewrites it, but the whole test sees exactly one `window/showDocument`; code actions appear only on goals |
| The extension builds for Zed | proven | `cargo build --release --target wasm32-wasip2` succeeds |

The whole end-to-end test takes about 0.2 seconds, including starting Agda
and running eight Agda commands. Agda is fast on small files without imports; projects that
use the standard library will be slower, which is why phase 1 adds caching for
hover.

## What was learned

1. **Agda prints a prompt before the first command.** `JSON> ` appears at
   startup, so the bridge must consume it before sending anything, or every
   later response is attributed to the wrong command.
2. **agda2-vscode quotes some strings incorrectly.** It writes non-ASCII
   characters as Haskell decimal escapes (`ℕ` becomes `\8469`) but does not
   add the `\&` separator, so `ℕ1` is sent as `\84691`, one wrong character.
   Agda rejects that with a lexical error, which I confirmed by hand; with
   `\8469\&1` Agda reads `ℕ1` correctly. The bridge adds the separator. This is
   worth reporting to agda2-vscode.
3. **`ZED_COLUMN` counts UTF-8 bytes.** Zed computes it from its internal
   `Point`, whose column is a byte offset (`crates/project/src/task_inventory.rs`).
   On the line `not = λ (b : 𝔹) → {!   !}` the hole starts at character 19
   but at byte 25 (counting from 1), because `λ`, `𝔹` and `→` take two, four
   and three bytes. The bridge converts, and the test covers it.
4. **Agda's ranges** carry a 1-based code point offset (`pos`) plus line and
   column, with an exclusive end. Error messages carry their location as text
   (`file:7.5-8` in Agda 2.8, `file:7,5-8` before).
5. **`tower-lsp-server` runs handlers concurrently.** Notifications can
   therefore finish out of order. The bridge keeps `didChange` free of waiting
   and ignores versions older than the one it has; long Agda work runs in
   background tasks, so it never blocks the server's request slots.
6. **The repository's `.gitignore` ignores `*.agda`**, which would have
   silently left the test files out of git. An exception for
   `bridge/tests/fixtures/` was added.
7. **Zed sends `didSave` for every saved file to every language server of the
   worktree**, whatever the file's language (`on_buffer_saved` in
   `crates/project/src/lsp_store.rs`). Saving a Markdown file therefore made
   the bridge ask Agda to load it. The bridge now only loads documents Zed
   opened with it, and refuses paths without an Agda extension.
8. **Zed opens a shown document in the active pane and makes it the visible
   tab**, even with `takeFocus: false`, unless the user enabled
   `reveal_if_open` (off by default; `open_path_preview` in
   `crates/workspace/src/workspace.rs`). The bridge cannot tell whether the
   output file is still open, so reopening it after every command would cover
   the Agda file being edited. Hence it opens automatically only once; Zed's
   own `pane: reopen closed item` brings it back after closing it.
9. **Neither extensions nor language servers can add commands to Zed's
   command palette.** Extensions have no API for it, and the command palette
   (`crates/command_palette`) does not list a language server's commands. So
   "open output file" could not move from the code actions to the palette.

## Checks only Zed can do

These need Zed's user interface, so they could not be automated here.

### Setup

1. Install the bridge (it needs Rust): `cargo install --path bridge` from this
   repository. This puts `agda-bridge` in `~/.cargo/bin`, which must be on your
   `PATH`.
2. In Zed, open the Extensions page, choose "Install Dev Extension" and select
   this repository. If the published Agda extension is installed, Zed marks it
   as overridden by the dev extension.
3. If `agda` is not on your `PATH`, tell the bridge where it is in Zed's
   `settings.json`:

   ```json
   "lsp": {
     "agda-bridge": {
       "initialization_options": { "agdaPath": "/path/to/agda" }
     }
   }
   ```

4. Optionally, bind keys to jump between goals (goals are the only
   Information diagnostics, so these skip errors and warnings), in
   `keymap.json`:

   ```json
   {
     "context": "Editor && extension == agda",
     "bindings": {
       "ctrl-c ctrl-f": ["editor::GoToDiagnostic", { "severity": "information" }],
       "ctrl-c ctrl-b": ["editor::GoToPreviousDiagnostic", { "severity": "information" }]
     }
   }
   ```

5. Optionally, set `"reveal_if_open": true` in Zed's `settings.json`. Then
   the automatic opening reveals an output file that is already open in
   another pane (for example after Zed restored your layout), instead of
   opening a second copy in the pane you are editing.

### Troubleshooting: `Library not loaded: @rpath/libLLVM.dylib` on macOS

If installing the dev extension fails with this error from `rust-lld`, the
`cargo` on your `PATH` is not rustup's proxy. On macOS, `rust-lld` finds
`libLLVM.dylib` only through the `DYLD_FALLBACK_LIBRARY_PATH` that rustup's
proxies set, so running a toolchain's `cargo` directly (for example from
`~/.rustup/toolchains/stable-aarch64-apple-darwin/bin`) breaks every
WebAssembly build, including Zed extensions. `rustup run stable cargo build
--target wasm32-wasip2` confirms it: that command works.

The fix is to put rustup's proxies first on your `PATH` and remove the
toolchain directory from it. They live in `~/.cargo/bin` for the standard
installer, and in `$(brew --prefix rustup)/bin` for Homebrew's rustup, which
is keg-only and links only `rustup` itself into Homebrew's `bin`. Then quit
Zed completely and reopen it, because Zed reads the shell environment at
startup.

### Results in Zed

Run on macOS on 2 October 2026, with `bridge/tests/fixtures/Spike.agda`.
Code actions were opened with `space a` (a vim mode binding).

| # | Check | Result |
| --- | --- | --- |
| 1 | Opening the file: "Agda Bridge" in the LSP log, goals underlined | works |
| 2 | Hover over a hole: goal and context, highlighted as Agda | works |
| 3 | Code actions in `{! suc (n + m) !}`: give, refine, show goal in output | works |
| 4 | "Agda: give ?0" replaces the hole | works |
| 5 | The output file opens in the active pane, can be moved to a split, and updates there | works; but once closed it did not come back on the next save, see learned item 8; reopen it with `pane: reopen closed item` |
| 6 | Markdown preview of the output file updates live | not reported yet, still open |
| 7 | Refine on the hole after `double n =` | works through the code action; the Emacs-style task did not work |
| 8 | Goal ?2 after `λ`, `𝔹` and `→` in the output file | works through the code action |
| 9 | Jumping between goals only, with `editor::GoToDiagnostic` | works (a native Zed binding, not an Agda task) |
| 10, 11 | Task error notification, and the speed of tasks | not applicable: the tasks were removed |

One problem turned up: saving a Markdown file in the project made the bridge
ask Agda to load it, which put an `InvalidExtensionError` in the output file
and on the file's first character. That is fixed (learned item 7) and covered
by the end-to-end test.

### Decisions after phase 0

- **The LSP path comes first.** The Emacs-style route through Zed tasks is
  dropped: `languages/agda/tasks.json` is removed, and so is its keymap. The
  `agda-bridge client` subcommand stays, as a debugging tool.
- **Priorities for phase 1**, in addition to the plan: go to definition
  (`cmd`-click on a name), and Unicode input in two styles, `\` followed by a
  LaTeX name or one of the abbreviations of Agda's Emacs mode, and `#`
  followed by a Typst symbol name as used in Typst's math mode (for example
  `#arrow.r`). Typst publishes its symbol table as the Rust crate `codex`, so
  the bridge can use the real names. Hover showing the type of any name is a
  stretch goal.
- **No code action for the output file.** A command palette entry was the
  preferred replacement, but is not possible (learned item 9); Zed's own
  `pane: reopen closed item` and the project panel cover it.
- **A separate extension identity**: id `agda-interactive`, name "Agda
  Interactive", version 0.3.0, by Tim Steenvoorden, crediting Haohan Yang's
  `agda-zed` it is based on. Zed's publishing rules ask to propose
  improvements to the existing extension before publishing a competing one,
  so publishing starts with that conversation.

## Deliberately left out of the spike

These belong to phase 1 or later, as the plan describes:

- a lone `?` is not yet expanded to `{!  !}` after loading, so give needs a
  `{! … !}` hole (refine works on `?` too);
- documents are synchronised in full on every change, not incrementally;
- no semantic highlighting, go to definition, case split, auto, solve, abort,
  Unicode input or inlay hints yet;
- errors in imported files are placed at the top of the current file;
- the debug client uses a Unix socket, so it does not work on Windows yet;
- hover asks Agda every time (no cache) and answers "Agda is busy" during a
  load instead of waiting;
- `agda-bridge` must be installed by hand; downloading it belongs to phase 4;
- publishing is not settled yet: the extension now has its own id, but Zed
  expects a conversation with the existing extension's owner first (see the
  decisions above and section 8 of the plan).
