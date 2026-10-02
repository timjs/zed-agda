# Phase 0: spike results

Phase 0 of the roadmap in [`PLAN.md`](PLAN.md) set out to prove the risky
assumptions before building anything large. This document records what was
built, what is now proven (and how), what was learned along the way, and the
checks that only a person running Zed can do.

## What was built

| Part | Files | Purpose |
| --- | --- | --- |
| The bridge | `bridge/` (separate crate, about 2,200 lines including unit tests) | Language server for Zed, driving `agda --interaction-json` |
| Protocol | `bridge/src/iotcm.rs`, `protocol.rs`, `agda.rs` | Commands, responses, the Agda process |
| Positions and goals | `bridge/src/text.rs`, `goals.rs`, `location.rs` | Code points, UTF-16 and UTF-8 columns; goals that follow edits |
| LSP | `bridge/src/server.rs`, `render.rs`, `output.rs` | Load, diagnostics, hover, code actions, give and refine, output file |
| Task route | `bridge/src/socket.rs`, `languages/agda/tasks.json` | `agda-bridge client …` for Emacs-style keybindings |
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
| Task, socket, bridge round trip works, with Zed's byte columns | proven | `agda-bridge client goal --row … --column …` finds `?2` behind `λ`, `𝔹` and `→`, and writes it to the output file |
| Failures of a hidden task still reach the user | proven | a task outside a goal fails and the bridge sends `window/showMessage` |
| Type errors become diagnostics on the right range | proven | `Bad.agda` gives an Error on line 7, columns 5 to 8, starting `error: [UnequalTerms]` |
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

## Checks only Zed can do

These need Zed's user interface, so they could not be automated here. Each
takes a minute.

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

4. For the Emacs-style keys, add this to `keymap.json`:

   ```json
   {
     "context": "Editor && extension == agda",
     "bindings": {
       "ctrl-c ctrl-l": ["task::Spawn", { "task_name": "agda: load" }],
       "ctrl-c ctrl-,": ["task::Spawn", { "task_name": "agda: goal type and context" }],
       "ctrl-c ctrl-space": ["task::Spawn", { "task_name": "agda: give" }],
       "ctrl-c ctrl-r": ["task::Spawn", { "task_name": "agda: refine" }],
       "ctrl-c ctrl-f": ["editor::GoToDiagnostic", { "severity": "information" }],
       "ctrl-c ctrl-b": ["editor::GoToPreviousDiagnostic", { "severity": "information" }]
     }
   }
   ```

   On Linux and Windows `ctrl-c` is also copy, so Zed waits briefly after
   `ctrl-c` for a second key before copying; on macOS there is no conflict.

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

### Checklist

Open `bridge/tests/fixtures/Spike.agda` (a copy, so the fixture stays intact)
and go through these:

| # | Do this | Expected |
| --- | --- | --- |
| 1 | Open the file | the LSP log lists "Agda Bridge"; the three holes get Information underlines |
| 2 | Hover over `{!   !}` after `double n =` | a popup with `Goal: ℕ` and `n : ℕ`, highlighted as Agda |
| 3 | Put the cursor in `{! suc (n + m) !}`, press `ctrl-.` (`cmd-.` on macOS, `g .` in vim mode) | "Agda: give ?0", "Agda: refine ?0" and "Agda: show goal ?0 in output" |
| 4 | Choose "Agda: give ?0" | the hole becomes `suc (n + m)`, its underline disappears |
| 5 | Look at where `.zed/agda-output.md` opened | **open question**: it opens in the active pane; drag it to a split, then save the Agda file and check that it updates |
| 6 | Run "markdown: open preview to the side" on the output file | **open question**: does the preview update live, with Agda highlighting? |
| 7 | Type `suc` into the hole after `double n =`, then `ctrl-c ctrl-r` | the hole becomes `suc {!  !}` without a terminal appearing |
| 8 | Put the cursor in the hole after `not = λ (b : 𝔹) →`, press `ctrl-c ctrl-,` | the output file shows `b : 𝔹` (this checks the byte column conversion in real Zed) |
| 9 | Press `ctrl-c ctrl-f` a few times | the cursor jumps between goals only, skipping errors |
| 10 | Press `ctrl-c ctrl-space` outside a goal | a notification says "The cursor is not in a goal." |
| 11 | Overall | does a task-based command feel fast enough? |

Rows 5, 6 and 11 are the open questions from the plan; the others confirm
in real Zed what the test already proves against the protocol.

## Deliberately left out of the spike

These belong to phase 1 or later, as the plan describes:

- a lone `?` is not yet expanded to `{!  !}` after loading, so give needs a
  `{! … !}` hole (refine works on `?` too);
- documents are synchronised in full on every change, not incrementally;
- no semantic highlighting, go to definition, case split, auto, solve, abort,
  Unicode input or inlay hints yet;
- errors in imported files are placed at the top of the current file;
- the task route uses a Unix socket, so it does not work on Windows yet;
- hover asks Agda every time (no cache) and answers "Agda is busy" during a
  load instead of waiting;
- `agda-bridge` must be installed by hand; downloading it belongs to phase 4;
- the extension still uses the id `agda`, which is fine for a dev extension
  but must be settled before publishing (see section 8 of the plan).
