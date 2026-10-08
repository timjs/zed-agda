//! End-to-end test: this file plays Zed's role, a small LSP client, and drives
//! the real `agda-bridge` binary against a real Agda.
//!
//! Agda is taken from `$AGDA`, or `agda` on `PATH`; without it the tests that
//! need it are skipped with a message.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(120);

struct Client {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Value>,
    next_id: i64,
    /// Requests and notifications from the server, cleared between steps.
    received: Vec<Value>,
    /// Every request and notification the server sent, never cleared.
    history: Vec<Value>,
}

impl Client {
    /// Start the bridge with its debug client socket at `socket`.
    fn start(socket: &Path) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agda-bridge"))
            .env("AGDA_BRIDGE_SOCKET", socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start agda-bridge");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, incoming) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).unwrap_or(0) == 0 {
                        return;
                    }
                    let header = header.trim();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.strip_prefix("Content-Length: ") {
                        length = value.parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        Client {
            child,
            stdin,
            incoming,
            next_id: 1,
            received: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Initialize the server for the worktree `root`, using `agda`.
    fn initialize(&mut self, root: &Path, agda: &str) -> Value {
        self.initialize_with(
            root,
            agda,
            json!({ "window": { "showDocument": { "support": true } } }),
        )
    }

    /// Initialize the server, announcing the client `capabilities`.
    fn initialize_with(&mut self, root: &Path, agda: &str, capabilities: Value) -> Value {
        self.initialize_full(root, capabilities, json!({ "agdaPath": agda }))
    }

    /// Initialize the server with client `capabilities` and the bridge's
    /// `options`, as Zed passes `lsp.agda-bridge.initialization_options`.
    fn initialize_full(&mut self, root: &Path, capabilities: Value, options: Value) -> Value {
        let init = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": uri(root),
                "workspaceFolders": [{ "uri": uri(root), "name": "test" }],
                "capabilities": capabilities,
                "initializationOptions": options,
            }),
        );
        self.notify("initialized", json!({}));
        init
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Receive one message, answering requests from the server like Zed does.
    fn receive(&mut self) -> Value {
        self.receive_within(TIMEOUT)
            .expect("message from agda-bridge")
    }

    /// Keep receiving messages for `duration`, as Zed would.
    fn receive_for(&mut self, duration: Duration) {
        let end = std::time::Instant::now() + duration;
        while let Some(left) = end.checked_duration_since(std::time::Instant::now())
            && self.receive_within(left).is_some()
        {}
    }

    fn receive_within(&mut self, timeout: Duration) -> Option<Value> {
        let message = self.incoming.recv_timeout(timeout).ok()?;
        if let (Some(id), Some(method)) = (
            message.get("id"),
            message.get("method").and_then(Value::as_str),
        ) {
            let result = match method {
                "workspace/applyEdit" => json!({ "applied": true }),
                "window/showDocument" => json!({ "success": true }),
                _ => Value::Null,
            };
            let id = id.clone();
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
        if message.get("method").is_some() {
            self.received.push(message.clone());
            self.history.push(message.clone());
        }
        Some(message)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.receive();
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                assert!(message.get("error").is_none(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    /// Wait for a request or notification from the server matching `pred`.
    fn wait_for(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(found) = self.received.iter().find(|m| pred(m)) {
            return found.clone();
        }
        loop {
            let message = self.receive();
            if message.get("method").is_some() && pred(&message) {
                return message;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "agda-bridge exited while waiting for {what}"
            );
        }
    }

    /// Wait for the next diagnostics of `uri` that satisfy `pred`.
    fn diagnostics(&mut self, uri: &str, pred: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        self.received.clear();
        let message = self.wait_for("diagnostics", |m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == uri
                && pred(m["params"]["diagnostics"].as_array().unwrap())
        });
        message["params"]["diagnostics"].as_array().unwrap().clone()
    }

    fn hover(&mut self, uri: &str, position: Value) -> String {
        let result = self.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri }, "position": position }),
        );
        result["contents"]["value"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    /// The titles of the code actions offered at `position`.
    fn code_actions(&mut self, uri: &str, position: Value) -> Vec<String> {
        let result = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": position, "end": position },
                "context": { "diagnostics": [] },
            }),
        );
        result
            .as_array()
            .map(|actions| {
                actions
                    .iter()
                    .map(|action| action["title"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The definition of the name at `position`: a location, or null.
    fn definition(&mut self, uri: &str, position: Value) -> Value {
        self.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": position }),
        )
    }
}

/// An LSP location at a single position.
fn location(uri: &str, line: u32, character: u32) -> Value {
    let at = json!({ "line": line, "character": character });
    json!({ "uri": uri, "range": { "start": at, "end": at } })
}

/// A fresh worktree with copies of the given fixtures.
fn setup(name: &str, fixtures: &[&str]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("agda-bridge-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for fixture in fixtures {
        std::fs::copy(source.join(fixture), root.join(fixture)).unwrap();
    }
    root
}

fn find_agda() -> Option<String> {
    if let Ok(agda) = std::env::var("AGDA") {
        return Some(agda);
    }
    let found = Command::new("agda").arg("--version").output().ok()?;
    found.status.success().then(|| "agda".to_string())
}

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// An LSP position (0-based line, UTF-16 column) of the `n`th match of `needle`.
fn position_of(text: &str, needle: &str, n: usize) -> Value {
    let byte = text.match_indices(needle).nth(n).expect("needle in text").0;
    let line = text[..byte].matches('\n').count();
    let line_start = text[..byte].rfind('\n').map_or(0, |i| i + 1);
    let character: usize = text[line_start..byte].chars().map(char::len_utf16).sum();
    json!({ "line": line, "character": character })
}

/// Apply one LSP text edit, as Zed would.
fn apply(text: &str, edit: &Value) -> String {
    let offset = |position: &Value| {
        let line = position["line"].as_u64().unwrap() as usize;
        let character = position["character"].as_u64().unwrap() as usize;
        let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        let mut units = 0;
        let mut byte = line_start;
        for ch in text[line_start..].chars() {
            if units >= character || ch == '\n' {
                break;
            }
            units += ch.len_utf16();
            byte += ch.len_utf8();
        }
        byte
    };
    let start = offset(&edit["range"]["start"]);
    let end = offset(&edit["range"]["end"]);
    format!(
        "{}{}{}",
        &text[..start],
        edit["newText"].as_str().unwrap(),
        &text[end..]
    )
}

fn messages(diagnostics: &[Value], severity: u64) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d["severity"] == severity)
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn drives_agda_through_lsp_and_debug_client() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };

    let root = setup("lsp", &["Spike.agda", "Bad.agda", "Nat.agda", "Uses.agda"]);
    let socket = root.join("bridge.sock");

    let spike = root.join("Spike.agda");
    let spike_uri = uri(&spike);
    let mut text = std::fs::read_to_string(&spike).unwrap();

    let mut client = Client::start(&socket);
    let init = client.initialize(&root, &agda);
    assert_eq!(init["capabilities"]["hoverProvider"], true);
    assert!(
        init["capabilities"]["executeCommandProvider"]["commands"]
            .as_array()
            .unwrap()
            .contains(&json!("agda.give"))
    );

    // Opening the file loads it: every goal becomes an Information diagnostic.
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": spike_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    let diagnostics = client.diagnostics(&spike_uri, |d| d.len() == 3);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?1 : ℕ", "?2 : 𝔹"]);
    // `𝔹` takes two UTF-16 units, so goal ?2 starts at the UTF-16 column of `{!`.
    let goal_2 = diagnostics
        .iter()
        .find(|d| d["message"] == "?2 : 𝔹")
        .unwrap();
    assert_eq!(goal_2["range"]["start"], position_of(&text, "{!", 2));

    // The output file was written and Zed was asked to open it once.
    let output = root.join(".zed/agda-output.md");
    let shown = client.wait_for("showDocument", |m| m["method"] == "window/showDocument");
    assert_eq!(shown["params"]["uri"], uri(&output));
    assert_eq!(shown["params"]["takeFocus"], false);
    assert!(std::fs::read_to_string(&output).unwrap().contains("?2 : 𝔹"));

    // Hover on a hole shows its goal and context.
    let goal = position_of(&text, "{!   !}", 0);
    let hover = client.hover(&spike_uri, goal.clone());
    assert!(
        hover.contains("Goal: ℕ") && hover.contains("n : ℕ"),
        "{hover}"
    );
    // The same on every char of the hole, its blank middle too, and at the
    // end of the line right after it.
    let (line, start) = (
        goal["line"].as_u64().unwrap(),
        goal["character"].as_u64().unwrap(),
    );
    for character in start + 1..=start + 7 {
        let at = json!({ "line": line, "character": character });
        assert_eq!(client.hover(&spike_uri, at), hover, "{character}");
    }
    let hover = client.hover(&spike_uri, position_of(&text, "{!   !}", 1));
    assert!(
        hover.contains("Goal: 𝔹") && hover.contains("b : 𝔹") && !hover.contains("Have:"),
        "{hover}"
    );
    // On a hole with text, also the type of that text.
    let hover = client.hover(&spike_uri, position_of(&text, "suc (n", 0));
    assert!(
        hover.starts_with("**Goal ?0**\n\n```agda\nGoal: ℕ\nHave: ℕ\n")
            && hover.ends_with("\nn : ℕ\nm : ℕ\n```\n"),
        "{hover}"
    );
    assert_eq!(
        client.hover(&spike_uri, json!({ "line": 0, "character": 0 })),
        ""
    );
    // Elsewhere, hover on a name shows its type, on a symbol how to type
    // it, and then why the name is in scope; all three for `ℕ`.
    let hover = client.hover(&spike_uri, position_of(&text, "ℕ", 0));
    assert!(
        hover.starts_with("```agda\nℕ : Set\n```\n\n`ℕ` (U+2115) is typed with `\\bN`")
            && hover.contains("`#NN`")
            && hover.contains(
                "\n\n```text\nℕ is in scope as\n  * a data type Spike.ℕ brought into scope by\n    - its definition at Spike.agda:3.6-7"
            ),
        "{hover}"
    );
    // The same hover wherever Zed asks for one `ℕ` of `  suc  : ℕ → ℕ`: on
    // it, on the space after it, and just past the end of the line, which
    // Zed asks at the line's end.
    let at = |character: u32| json!({ "line": 4, "character": character });
    let both = client.hover(&spike_uri, at(9));
    assert!(
        both.starts_with("```agda\nℕ : Set\n```\n\n`ℕ` (U+2115) is typed with"),
        "{both}"
    );
    for character in [10, 13, 14] {
        assert_eq!(client.hover(&spike_uri, at(character)), both, "{character}");
    }
    // `→` is no name: only how to type it.
    assert!(
        client
            .hover(&spike_uri, at(11))
            .starts_with("`→` (U+2192) is typed with")
    );
    // A part of an operator stands for the whole operator; the path in why
    // it is in scope is relative to the project.
    let hover = client.hover(&spike_uri, position_of(&text, "+ m = m", 0));
    assert_eq!(
        hover,
        "```agda\n_+_ : ℕ → ℕ → ℕ\n```\n\n```text\n_+_ is in scope as\n  * a defined name Spike._+_ brought into scope by\n    - its definition at Spike.agda:7.1-4\n```"
    );
    // A bound variable is not in scope at the top level: nothing to show.
    assert_eq!(client.hover(&spike_uri, position_of(&text, "m = m", 0)), "");

    // Go to definition in the same file: `ℕ` in the type of `_+_` leads to
    // `data ℕ` on line 3.
    assert_eq!(
        client.definition(&spike_uri, position_of(&text, "ℕ → ℕ → ℕ", 0)),
        location(&spike_uri, 2, 5)
    );

    // Code actions on a filled hole offer the goal commands, showing the
    // goal and the output file; a line without goals or problems offers
    // nothing.
    let titles = client.code_actions(&spike_uri, position_of(&text, "suc (n", 0));
    assert_eq!(
        titles,
        [
            "Give",
            "Refine",
            "Case split on `suc (n + m)`",
            "With-abstract on `suc (n + m)`",
            "Auto",
            "Solve",
            "Solve all goals",
            "Print goal in output",
            "Print normal form in output",
            "Open output file"
        ]
    );
    let titles = client.code_actions(&spike_uri, json!({ "line": 0, "character": 0 }));
    assert!(titles.is_empty(), "{titles:?}");

    // Give goes through executeCommand and comes back as workspace/applyEdit.
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.give", "arguments": [spike_uri, 0] }),
    );
    let edit = client.wait_for("applyEdit", |m| m["method"] == "workspace/applyEdit");
    let edit = edit["params"]["edit"]["changes"][&spike_uri][0].clone();
    assert_eq!(edit["newText"], "suc (n + m)");
    text = apply(&text, &edit);
    assert!(text.contains("suc n + m = suc (n + m)\n"));
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": spike_uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let diagnostics = client.diagnostics(&spike_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?1 : ℕ", "?2 : 𝔹"]);

    // Type into a hole without saving, then refine: the goal followed the
    // edit, and Agda's `suc ?` becomes a new goal ?3 that hover can query.
    text = text.replacen("double n = {!   !}", "double n = {! suc !}", 1);
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": spike_uri, "version": 3 }, "contentChanges": [{ "text": text }] }),
    );
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.refine", "arguments": [spike_uri, 1] }),
    );
    let edit = client.wait_for("applyEdit", |m| m["method"] == "workspace/applyEdit");
    let edit = edit["params"]["edit"]["changes"][&spike_uri][0].clone();
    assert_eq!(edit["newText"], "suc {!  !}");
    text = apply(&text, &edit);
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": spike_uri, "version": 4 }, "contentChanges": [{ "text": text }] }),
    );
    let diagnostics = client.diagnostics(&spike_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?3 : ℕ", "?2 : 𝔹"]);
    let hover = client.hover(&spike_uri, position_of(&text, "{!  !}", 0));
    assert!(
        hover.contains("Goal ?3") && hover.contains("n : ℕ"),
        "{hover}"
    );

    // Go to definition after unsaved edits: the give and refine above moved
    // both the use of `𝔹` on line 18 and its definition on line 14.
    assert_eq!(
        client.definition(&spike_uri, position_of(&text, "𝔹) →", 0)),
        location(&spike_uri, 13, 5)
    );

    // The debug client takes UTF-8 byte columns, and this line has a
    // two-byte `λ`, a four-byte `𝔹` and a three-byte `→` before the hole.
    let line = text.lines().position(|l| l.starts_with("not = ")).unwrap();
    let column = text.lines().nth(line).unwrap().find("{!").unwrap() + 3;
    let run_client = |command: &str, row: usize, column: usize| {
        Command::new(env!("CARGO_BIN_EXE_agda-bridge"))
            .env("AGDA_BRIDGE_SOCKET", &socket)
            .args(["client", command, "--root"])
            .arg(&root)
            .arg("--file")
            .arg(&spike)
            .args([
                "--row",
                &(row + 1).to_string(),
                "--column",
                &column.to_string(),
            ])
            .output()
            .unwrap()
    };
    let result = run_client("goal", line, column);
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        result.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(stdout.contains("Goal ?2"), "{stdout}");
    assert!(std::fs::read_to_string(&output).unwrap().contains("b : 𝔹"));

    // Outside a goal the client fails, and the user is told in Zed.
    client.received.clear();
    let result = run_client("give", 1, 1);
    assert!(!result.status.success());
    let shown = client.wait_for("showMessage", |m| m["method"] == "window/showMessage");
    assert_eq!(shown["params"]["message"], "The cursor is not in a goal.");

    // Zed sends `didSave` for every file saved in the worktree, whatever its
    // language; saving Markdown must not reach Agda.
    let notes = root.join("TODO.md");
    std::fs::write(&notes, "# Notes\n").unwrap();
    let notes_uri = uri(&notes);
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": notes_uri } }),
    );

    // A type error becomes an Error diagnostic on the right range.
    let bad = root.join("Bad.agda");
    let bad_uri = uri(&bad);
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": bad_uri, "languageId": "agda", "version": 1,
                                  "text": std::fs::read_to_string(&bad).unwrap() } }),
    );
    let diagnostics = client.diagnostics(&bad_uri, |d| !d.is_empty());
    let error = &diagnostics[0];
    assert_eq!(error["severity"], 1);
    assert_eq!(
        error["range"],
        json!({ "start": { "line": 6, "character": 4 }, "end": { "line": 6, "character": 7 } })
    );
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .starts_with("error: [UnequalTerms]"),
        "{error}"
    );
    // The Markdown save above came first, so a load of it would have been
    // reported before this one.
    assert!(
        !client
            .history
            .iter()
            .any(|m| m["params"]["uri"] == notes_uri),
        "the Markdown file was sent to Agda"
    );

    // Every command above rewrote the output file, but Zed was asked to open
    // it only once, so a closed output file never covers the code by itself.
    let shown = client
        .history
        .iter()
        .filter(|m| m["method"] == "window/showDocument")
        .count();
    assert_eq!(shown, 1);

    // On an error line, the output file can be reopened.
    let titles = client.code_actions(&bad_uri, json!({ "line": 6, "character": 4 }));
    assert_eq!(titles, ["Open output file"]);
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.openOutput", "arguments": [] }),
    );
    let shown = client.wait_for("showDocument", |m| m["method"] == "window/showDocument");
    assert_eq!(shown["params"]["uri"], uri(&output));

    // Go to definition into another file: `Uses.agda` imports `Nat.agda`.
    let uses = root.join("Uses.agda");
    let uses_uri = uri(&uses);
    let uses_text = std::fs::read_to_string(&uses).unwrap();
    let nat_uri = uri(&root.join("Nat.agda"));
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": uses_uri, "languageId": "agda", "version": 1, "text": uses_text } }),
    );
    client.diagnostics(&uses_uri, |d| d.is_empty());
    assert_eq!(
        client.definition(&uses_uri, position_of(&uses_text, "suc", 0)),
        location(&nat_uri, 4, 2)
    );
    assert_eq!(
        client.definition(&uses_uri, position_of(&uses_text, "ℕ", 0)),
        location(&nat_uri, 2, 5)
    );
    // `two` in its definition leads to its type signature, in this file.
    assert_eq!(
        client.definition(&uses_uri, position_of(&uses_text, "two =", 0)),
        location(&uses_uri, 4, 0)
    );
    // Keywords have no definition.
    assert_eq!(
        client.definition(&uses_uri, position_of(&uses_text, "open", 0)),
        Value::Null
    );
    // Hover shows the type of a name from the imported module, and that the
    // `open import` brought it in. `Uses.agda` has no goals, and then Agda
    // 2.8 does not say where that `open` is.
    assert_eq!(
        client.hover(&uses_uri, position_of(&uses_text, "suc", 0)),
        "```agda\nsuc : ℕ → ℕ\n```\n\n```text\nsuc is in scope as\n  * a constructor Nat.ℕ.suc brought into scope by\n    - the opening of Nat (Agda gives no location in a file without goals)\n    - its definition at Nat.agda:5.3-6\n```"
    );

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

type Token = (u64, u64, u64, String, Vec<String>);

/// The semantic tokens of `uri`, decoded to absolute
/// (line, character, length, type, modifiers) with the server's legend.
fn semantic_tokens(client: &mut Client, uri: &str, legend: &Value) -> Vec<Token> {
    let result = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let names = |key: &str| -> Vec<String> {
        legend[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|name| name.as_str().unwrap().to_string())
            .collect()
    };
    let (types, modifiers) = (names("tokenTypes"), names("tokenModifiers"));
    let data: Vec<u64> = result["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect();
    let (mut line, mut character) = (0, 0);
    data.chunks(5)
        .map(|token| {
            line += token[0];
            character = if token[0] == 0 {
                character + token[1]
            } else {
                token[1]
            };
            let applied = (0..modifiers.len())
                .filter(|bit| token[4] & (1 << bit) != 0)
                .map(|bit| modifiers[bit].clone())
                .collect();
            (
                line,
                character,
                token[2],
                types[token[3] as usize].clone(),
                applied,
            )
        })
        .collect()
}

fn token(line: u64, character: u64, length: u64, ty: &str, modifiers: &[&str]) -> Token {
    (
        line,
        character,
        length,
        ty.to_string(),
        modifiers.iter().map(|m| m.to_string()).collect(),
    )
}

#[test]
fn highlights_with_semantic_tokens() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("tokens", &["Spike.agda", "Problems.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    let init = client.initialize(&root, &agda);
    let legend = init["capabilities"]["semanticTokensProvider"]["legend"].clone();
    assert_eq!(init["capabilities"]["semanticTokensProvider"]["full"], true);

    // After loading, the server asks the client to request tokens again.
    let spike = root.join("Spike.agda");
    let spike_uri = uri(&spike);
    let text = std::fs::read_to_string(&spike).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": spike_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&spike_uri, |d| d.len() == 3);
    client.wait_for("refresh", |m| {
        m["method"] == "workspace/semanticTokens/refresh"
    });

    let tokens = semantic_tokens(&mut client, &spike_uri, &legend);
    for expected in [
        token(2, 0, 4, "keyword", &[]),        // data
        token(2, 5, 1, "type", &[]),           // ℕ
        token(4, 2, 3, "enumMember", &[]),     // suc
        token(6, 0, 3, "function", &[]),       // _+_
        token(8, 4, 1, "variable", &[]),       // n
        token(8, 12, 17, "region", &["hole"]), // {! suc (n + m) !}
        token(13, 5, 2, "type", &[]),          // 𝔹, two UTF-16 units
    ] {
        assert!(
            tokens.contains(&expected),
            "{expected:?} not in {tokens:#?}"
        );
    }

    // Tokens follow an unsaved edit: a new first line moves everything down.
    let edited = format!("-- note\n{text}");
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": spike_uri, "version": 2 }, "contentChanges": [{ "text": edited }] }),
    );
    let tokens = semantic_tokens(&mut client, &spike_uri, &legend);
    assert!(
        tokens.contains(&token(3, 0, 4, "keyword", &[])),
        "{tokens:#?}"
    );
    assert!(tokens.contains(&token(3, 5, 1, "type", &[])), "{tokens:#?}");
    assert!(
        tokens.iter().all(|t| t.0 != 0),
        "the new line has no tokens yet"
    );

    // Problems become modifiers; the coverage problem over a whole clause is
    // split so that each name keeps its own type.
    let problems = root.join("Problems.agda");
    let problems_uri = uri(&problems);
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": problems_uri, "languageId": "agda", "version": 1,
                                  "text": std::fs::read_to_string(&problems).unwrap() } }),
    );
    client.diagnostics(&problems_uri, |d| !d.is_empty());
    let tokens = semantic_tokens(&mut client, &problems_uri, &legend);
    for expected in [
        token(6, 0, 4, "function", &["terminationProblem"]), // loop : …
        token(7, 0, 4, "function", &[]),                     // loop n = …
        token(7, 9, 4, "function", &["terminationProblem"]), // … = loop n
        token(10, 0, 7, "function", &["coverageProblem"]),   // partial
        token(10, 7, 1, "region", &["coverageProblem"]),     // the space
        token(10, 8, 4, "enumMember", &["coverageProblem"]), // zero
    ] {
        assert!(
            tokens.contains(&expected),
            "{expected:?} not in {tokens:#?}"
        );
    }

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn links_cover_whole_names() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("names", &["Names.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    // Zed accepts links, which carry the range of the name.
    client.initialize_with(
        &root,
        &agda,
        json!({ "textDocument": { "definition": { "linkSupport": true } } }),
    );

    let names = root.join("Names.agda");
    let names_uri = uri(&names);
    let text = std::fs::read_to_string(&names).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": names_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&names_uri, |d| d.is_empty());

    // From the `s` of `~>*step` in `one = ~>*step zero`: the link covers the
    // whole name, which Zed would otherwise split into `~>*` and `step`, and
    // leads to its type signature.
    let at = |line: u32, character: u32| json!({ "line": line, "character": character });
    let expected = json!([{
        "originSelectionRange": { "start": at(10, 6), "end": at(10, 13) },
        "targetUri": names_uri,
        "targetRange": { "start": at(6, 0), "end": at(6, 0) },
        "targetSelectionRange": { "start": at(6, 0), "end": at(6, 0) },
    }]);
    for needle in ["~>*step zero", "step zero"] {
        assert_eq!(
            client.definition(&names_uri, position_of(&text, needle, 0)),
            expected,
            "{needle}"
        );
    }

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn completes_unicode_input() {
    // Completions need no Agda, so this test runs without one: loading the
    // opened file only fails with a warning.
    let root = setup("input", &[]);
    let mut client = Client::start(&root.join("bridge.sock"));
    let init = client.initialize(&root, "agda-bridge-test-no-agda");
    let triggers = init["capabilities"]["completionProvider"]["triggerCharacters"].clone();
    for c in ["\\", "#", "-", ".", ">"] {
        assert!(
            triggers.as_array().unwrap().contains(&json!(c)),
            "{c} in {triggers}"
        );
    }

    let input_uri = uri(&root.join("Input.agda"));
    let mut text = "id : A \\to\nf : 𝔹 \\bN\n{-#\nx = #arrow.r #->\n".to_string();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": input_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    let complete = |client: &mut Client, line: u32, character: u32| {
        client.request(
            "textDocument/completion",
            json!({ "textDocument": { "uri": input_uri },
                    "position": { "line": line, "character": character } }),
        )
    };

    // `\to` becomes `→` and, by default, a space: the edit replaces the
    // abbreviation and its leader.
    let result = complete(&mut client, 0, 10);
    assert_eq!(result["isIncomplete"], false);
    let first = &result["items"][0];
    assert_eq!(first["label"], "→");
    assert_eq!(first["detail"], "\\to");
    assert_eq!(first["filterText"], "to");
    assert_eq!(first["documentation"], "U+2192");
    text = apply(&text, &first["textEdit"]);
    assert!(text.starts_with("id : A → \n"), "{text}");

    // Columns are UTF-16 units, and `𝔹` takes two of them.
    let result = complete(&mut client, 1, 10);
    assert_eq!(result["items"][0]["label"], "ℕ");
    assert_eq!(
        result["items"][0]["textEdit"]["range"]["start"],
        json!({ "line": 1, "character": 7 })
    );

    // `#` starts a Typst name or shorthand; by default also in a pragma.
    let result = complete(&mut client, 3, 12);
    assert_eq!(result["items"][0]["label"], "→");
    assert_eq!(result["items"][0]["detail"], "#arrow.r");
    let result = complete(&mut client, 3, 16);
    assert_eq!(result["items"][0]["label"], "→");
    assert_eq!(result["items"][0]["detail"], "#->");
    assert_ne!(complete(&mut client, 2, 3), Value::Null);
    // Ordinary text does not.
    assert_eq!(complete(&mut client, 0, 2), Value::Null);

    // After the edit, as Zed sends it, `→` is not completed again; a lone
    // leader offers a list that is cut off, so the client must ask again.
    text.push_str("g = \\\n");
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": input_uri, "version": 2 },
                "contentChanges": [{ "text": text }] }),
    );
    assert_eq!(complete(&mut client, 0, 8), Value::Null);
    let result = complete(&mut client, 4, 5);
    assert_eq!(result["isIncomplete"], true);
    assert!(result["items"].as_array().unwrap().len() <= 200);
    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();

    // The options: leaders only after whitespace, and a space after the
    // symbol.
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize_full(
        &root,
        json!({}),
        json!({ "agdaPath": "agda-bridge-test-no-agda",
                "symbolOnlyAfterWhitespace": true, "symbolTrailingSpace": true }),
    );
    let text = "id : A \\to\n{-#\n".to_string();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": input_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    let result = complete(&mut client, 0, 10);
    assert_eq!(result["items"][0]["textEdit"]["newText"], "→ ");
    assert_eq!(complete(&mut client, 1, 3), Value::Null);
    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();

    // Settings change while the bridge runs, as Zed sends them with
    // `workspace/didChangeConfiguration`: without symbol input there are no
    // completions, switching it on again brings them back, and a wrong value
    // is reported.
    let mut client = Client::start(&root.join("bridge.sock"));
    let init = client.initialize(&root, "agda-bridge-test-no-agda");
    assert!(!init["capabilities"]["completionProvider"].is_null());
    let text = "id : A \\to\n".to_string();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": input_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    let configure = |client: &mut Client, settings: Value| {
        client.notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": settings }),
        );
    };
    configure(&mut client, json!({ "symbolInput": "none" }));
    assert_eq!(complete(&mut client, 0, 10), Value::Null);
    configure(
        &mut client,
        json!({ "symbolInput": "latex", "symbolTrailingSpace": false }),
    );
    let result = complete(&mut client, 0, 10);
    assert_eq!(result["items"][0]["textEdit"]["newText"], "→");
    client.received.clear();
    configure(&mut client, json!({ "symbolInput": "tex" }));
    let message = client.wait_for("showMessage", |m| m["method"] == "window/showMessage");
    assert!(
        message["params"]["message"]
            .as_str()
            .unwrap()
            .contains("symbolInput"),
        "{message}"
    );
    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

/// Wait until the output file contains `needle`: the bridge writes it after
/// publishing diagnostics, so it may lag behind them.
fn wait_for_output(path: &Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let output = std::fs::read_to_string(path).unwrap_or_default();
        if output.contains(needle) || std::time::Instant::now() > deadline {
            return output;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Apply all edits of one `workspace/applyEdit`, which refer to the same
/// text, from the last to the first.
fn apply_all(text: &str, edits: &[Value]) -> String {
    let mut edits = edits.to_vec();
    let key = |edit: &Value| {
        let start = &edit["range"]["start"];
        (start["line"].as_u64(), start["character"].as_u64())
    };
    edits.sort_by_key(|edit| std::cmp::Reverse(key(edit)));
    edits
        .iter()
        .fold(text.to_string(), |text, edit| apply(&text, edit))
}

#[test]
fn goal_commands() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("goal-commands", &["Goals.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    let init = client.initialize(&root, &agda);
    let legend = init["capabilities"]["semanticTokensProvider"]["legend"].clone();
    let commands = init["capabilities"]["executeCommandProvider"]["commands"].clone();
    for command in ["agda.caseSplit", "agda.auto", "agda.solve", "agda.solveAll"] {
        assert!(
            commands.as_array().unwrap().contains(&json!(command)),
            "{command}"
        );
    }

    let goals = root.join("Goals.agda");
    let goals_uri = uri(&goals);
    let mut text = std::fs::read_to_string(&goals).unwrap();
    let mut version = 1;
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": goals_uri, "languageId": "agda", "version": version, "text": text } }),
    );

    // After loading, both lone `?`s become `{!  !}` in one edit, and keep
    // their goal numbers.
    let edit = client.wait_for("applyEdit", |m| m["method"] == "workspace/applyEdit");
    let edits = edit["params"]["edit"]["changes"][&goals_uri]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "{!  !}"));
    text = apply_all(&text, &edits);
    assert!(
        text.contains("p = {!  !}\n") && text.contains("id {{!  !}} (suc"),
        "{text}"
    );
    let change = |client: &mut Client, text: &str, version: i32| {
        client.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": goals_uri, "version": version },
                    "contentChanges": [{ "text": text }] }),
        );
    };
    version += 1;
    change(&mut client, &text, version);
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 4);
    assert_eq!(
        messages(&diagnostics, 3),
        ["?0 : ℕ", "?1 : zero ≡ zero", "?2 : Set", "?3 : ℕ"]
    );
    let p = position_of(&text, "{!  !}", 0);
    let goal_1 = diagnostics
        .iter()
        .find(|d| d["message"] == "?1 : zero ≡ zero")
        .unwrap();
    assert_eq!(goal_1["range"]["start"], p);
    assert_eq!(goal_1["range"]["end"]["character"], 10);
    // The hole's highlighting grew with it.
    let tokens = semantic_tokens(&mut client, &goals_uri, &legend);
    assert!(
        tokens.contains(&token(13, 4, 6, "region", &["hole"])),
        "{tokens:#?}"
    );

    // Code actions on a goal offer every goal command.
    let titles = client.code_actions(&goals_uri, position_of(&text, "{! n !}", 0));
    assert_eq!(
        titles,
        [
            "Give",
            "Refine",
            "Case split on `n`",
            "With-abstract on `n`",
            "Auto",
            "Solve",
            "Solve all goals",
            "Print goal in output",
            "Print normal form in output",
            "Open output file"
        ]
    );

    // The type of a goal's text, and its normal form, in goal 3 of
    // `f = λ { x → {! x !} }`, where `id` is in scope (in goal 0 it is not:
    // it is defined further down).
    let output = root.join(".zed/agda-output.md");
    let with_text = text.replace("{! x !}", "{! id (suc x) !}");
    version += 1;
    change(&mut client, &with_text, version);
    let hover = client.hover(&goals_uri, position_of(&with_text, "id (suc x)", 0));
    assert!(
        hover.contains("Goal: ℕ\nHave: ℕ\n") && hover.ends_with("\nx : ℕ\n```\n"),
        "{hover}"
    );
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.normalForm", "arguments": [goals_uri, 3] }),
    );
    // Computing shows progress, which Zed is asked to create first, and
    // offers to cancel.
    let end = client.wait_for("progress", |m| {
        m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end"
    });
    let begin = client.history.iter().rev().find(|m| {
        m["method"] == "$/progress"
            && m["params"]["token"] == end["params"]["token"]
            && m["params"]["value"]["kind"] == "begin"
    });
    let begin = &begin.unwrap()["params"]["value"];
    assert_eq!(begin["title"], "Agda: normal form of ?3");
    assert_eq!(begin["cancellable"], true);
    let normal = "**Normal form of `id (suc x)` in ?3**\n\n```agda\nsuc x\n```\n";
    let written = wait_for_output(&output, normal);
    assert!(written.contains(normal), "{written}");
    // Text Agda cannot type: the goal and its context, then why.
    let untyped = text.replace("{! x !}", "{! nope !}");
    version += 1;
    change(&mut client, &untyped, version);
    let hover = client.hover(&goals_uri, position_of(&untyped, "nope", 0));
    assert!(
        hover.contains("Goal: ℕ\n")
            && !hover.contains("Have:")
            && hover.contains("x : ℕ\n```\n\nAgda cannot infer a type for the text in the goal:\n\n```text\n1.1-5: error: [NotInScope]\nNot in scope:\n  nope at 1.1-5"),
        "{hover}"
    );
    version += 1;
    change(&mut client, &text, version);

    // Run a goal command and return the edits Zed is asked to apply.
    let run = |client: &mut Client, command: &str, arguments: Value| -> Vec<Value> {
        client.received.clear();
        client.request(
            "workspace/executeCommand",
            json!({ "command": command, "arguments": arguments }),
        );
        let edit = client.wait_for(command, |m| m["method"] == "workspace/applyEdit");
        edit["params"]["edit"]["changes"][&goals_uri]
            .as_array()
            .unwrap()
            .clone()
    };

    // Auto finds `refl` for `p`.
    let edits = run(&mut client, "agda.auto", json!([goals_uri, 1]));
    assert_eq!(edits[0]["newText"], "refl");
    text = apply_all(&text, &edits);
    version += 1;
    change(&mut client, &text, version);
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 3);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?2 : Set", "?3 : ℕ"]);

    // Solve: Agda has nothing for ?0, but unification solved ?2.
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.solve", "arguments": [goals_uri, 0] }),
    );
    let message = client.wait_for("showMessage", |m| m["method"] == "window/showMessage");
    assert_eq!(
        message["params"]["message"],
        "Agda has no solution for goal ?0 yet."
    );
    let edits = run(&mut client, "agda.solveAll", json!([goals_uri]));
    assert_eq!(edits[0]["newText"], "ℕ");
    text = apply_all(&text, &edits);
    assert!(text.contains("two = id {ℕ} (suc (suc zero))"), "{text}");
    version += 1;
    change(&mut client, &text, version);
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?3 : ℕ"]);

    // Case split in a function clause replaces its line; the new goals have
    // no numbers until the file is loaded again.
    let edits = run(&mut client, "agda.caseSplit", json!([goals_uri, 0]));
    assert_eq!(edits[0]["newText"], "zero + m = {!  !}\nsuc n + m = {!  !}");
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 10, "character": 0 })
    );
    assert_eq!(
        edits[0]["range"]["end"],
        json!({ "line": 10, "character": 15 })
    );
    text = apply_all(&text, &edits);
    version += 1;
    change(&mut client, &text, version);
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 1);
    assert_eq!(messages(&diagnostics, 3), ["?3 : ℕ"]);
    let output = wait_for_output(&root.join(".zed/agda-output.md"), "Save the file");
    assert!(output.contains("Save the file to load them"), "{output}");

    // In an extended lambda only the clause is replaced.
    let edits = run(&mut client, "agda.caseSplit", json!([goals_uri, 3]));
    text = apply_all(&text, &edits);
    assert!(
        text.contains("f = λ { zero → {!  !} ; (suc x) → {!  !} }\n"),
        "{text}"
    );
    version += 1;
    change(&mut client, &text, version);

    // Saving loads the new clauses: Agda accepts them and numbers their four
    // goals, and nothing needs expanding.
    std::fs::write(&goals, &text).unwrap();
    client.received.clear();
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": goals_uri } }),
    );
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 4);
    assert_eq!(
        messages(&diagnostics, 3),
        ["?0 : ℕ", "?1 : ℕ", "?2 : ℕ", "?3 : ℕ"]
    );
    assert!(
        !client
            .received
            .iter()
            .any(|m| m["method"] == "workspace/applyEdit")
    );

    // An empty goal offers a case split on each variable of its context
    // that can be split (`m : ℕ`, not the function `f`), and on the result.
    let titles = client.code_actions(&goals_uri, json!({ "line": 10, "character": 14 }));
    assert!(
        titles.contains(&"Case split on `m`".to_string())
            && titles.contains(&"Case split on result".to_string())
            && !titles
                .iter()
                .any(|t| t == "Give" || t == "Print normal form in output"),
        "{titles:?}"
    );
    let edits = run(&mut client, "agda.caseSplit", json!([goals_uri, 0, "m"]));
    assert_eq!(
        edits[0]["newText"],
        "zero + zero = {!  !}\nzero + suc m = {!  !}"
    );
    text = apply_all(&text, &edits);
    version += 1;
    change(&mut client, &text, version);

    // A with-abstraction on `suc n + m = {!  !}`, with a goal as its
    // expression; after saving, Agda accepts it and numbers its goals.
    let edits = run(&mut client, "agda.addWith", json!([goals_uri, 1]));
    assert_eq!(
        edits[0]["newText"],
        "suc n + m with {!  !}\n... | w = {!  !}"
    );
    text = apply_all(&text, &edits);
    version += 1;
    change(&mut client, &text, version);
    std::fs::write(&goals, &text).unwrap();
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": goals_uri } }),
    );
    let diagnostics = client.diagnostics(&goals_uri, |d| d.len() == 6);
    assert!(
        diagnostics.iter().all(|d| d["severity"] == 3),
        "{diagnostics:#?}"
    );

    // On a type signature, `Make clause` adds a clause right below it, with
    // names from the types; a constructor gets none.
    let titles = client.code_actions(&goals_uri, json!({ "line": 9, "character": 0 }));
    assert_eq!(titles, ["Make clause"]);
    let titles = client.code_actions(&goals_uri, json!({ "line": 3, "character": 2 }));
    assert!(!titles.iter().any(|t| t == "Make clause"), "{titles:?}");
    let edits = run(&mut client, "agda.addClause", json!([goals_uri, 9]));
    assert_eq!(edits[0]["newText"], "\nn + m = {!  !}");
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 9, "character": 15 })
    );

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelled_requests_leave_no_answers_behind() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("cancelled", &["Spike.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let spike = root.join("Spike.agda");
    let spike_uri = uri(&spike);
    let text = std::fs::read_to_string(&spike).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": spike_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&spike_uri, |d| d.len() == 3);
    let nat_goal = position_of(&text, "{!   !}", 0);
    let bool_goal = position_of(&text, "{!   !}", 1);
    let nat = client.hover(&spike_uri, nat_goal.clone());
    let bool = client.hover(&spike_uri, bool_goal.clone());
    assert!(
        nat.contains("Goal: ℕ") && bool.contains("Goal: 𝔹"),
        "{nat}{bool}"
    );

    // Zed cancels a hover when the mouse moves on. Cancelled right away, on
    // goals and on names Agda was not asked about yet, a hover often goes
    // while Agda works on it; the next hover waits for that, and gets its
    // own answer.
    let cancel = |client: &mut Client, position: Value| {
        let id = client.next_id;
        client.next_id += 1;
        client.send(
            json!({ "jsonrpc": "2.0", "id": id, "method": "textDocument/hover",
            "params": { "textDocument": { "uri": uri(&spike) }, "position": position } }),
        );
        client.notify("$/cancelRequest", json!({ "id": id }));
    };
    let names = ["ℕ", "zero", "suc", "double", "𝔹", "tt", "not"];
    for (round, name) in names.iter().enumerate() {
        cancel(&mut client, bool_goal.clone());
        assert_eq!(client.hover(&spike_uri, nat_goal.clone()), nat, "{round}");
        cancel(&mut client, position_of(&text, name, 0));
        assert_eq!(client.hover(&spike_uri, bool_goal.clone()), bool, "{name}");
    }

    // A name Agda was not asked about gets its own type, and a load its own
    // goals.
    assert_eq!(
        client.hover(&spike_uri, position_of(&text, "+ m = m", 0)),
        "```agda\n_+_ : ℕ → ℕ → ℕ\n```\n\n```text\n_+_ is in scope as\n  * a defined name Spike._+_ brought into scope by\n    - its definition at Spike.agda:7.1-4\n```"
    );
    let changed = format!("{text}\n-- changed\n");
    std::fs::write(&spike, &changed).unwrap();
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": spike_uri, "version": 2 },
                "contentChanges": [{ "text": changed }] }),
    );
    client.received.clear();
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": spike_uri } }),
    );
    let diagnostics = client.diagnostics(&spike_uri, |_| true);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?1 : ℕ", "?2 : 𝔹"]);

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_while_agda_is_busy() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("busy", &["Slow.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let file = root.join("Slow.agda");
    let file_uri = uri(&file);
    let mut text = std::fs::read_to_string(&file).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": file_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&file_uri, |d| d.len() == 3);
    let quick = position_of(&text, "{!  !}", 0);
    let other = position_of(&text, "{!  !}", 1);
    let expected = client.hover(&file_uri, quick.clone());
    assert!(expected.contains("n : ℕ"), "{expected}");

    // The normal form of `ack three eight` keeps Agda busy for seconds.
    let busy = |client: &mut Client| {
        client.received.clear();
        client.request(
            "workspace/executeCommand",
            json!({ "command": "agda.normalForm", "arguments": [uri(&file), 0] }),
        );
        client.wait_for("progress", |m| {
            m["method"] == "$/progress" && m["params"]["value"]["kind"] == "begin"
        });
    };
    let ended = |m: &Value| m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end";
    let timed = |client: &mut Client, at: &Value| {
        let start = std::time::Instant::now();
        let hover = client.hover(&uri(&file), at.clone());
        (hover, start.elapsed())
    };

    busy(&mut client);
    // A goal hovered before shows Agda's answer at once.
    let (hover, waited) = timed(&mut client, &quick);
    assert_eq!(hover, expected);
    assert!(waited < Duration::from_millis(500), "{waited:?}");
    // One not hovered yet waits a second, then shows its type from the load.
    let (hover, waited) = timed(&mut client, &other);
    assert_eq!(
        hover,
        "**Goal ?2**\n\n```agda\nGoal: ℕ\n```\n\n*Agda is busy: the context follows when it is free.*\n"
    );
    assert!(waited >= Duration::from_millis(950), "{waited:?}");
    assert!(!client.received.iter().any(ended), "Agda was done first");
    client.wait_for("progress", ended);
    assert!(client.hover(&file_uri, other).contains("m : ℕ"));

    // With other text in it, a goal hovered before waits a second, then
    // shows Agda's answer without `Have:`, which is about the old text.
    text = text.replacen("quick n = {!  !}", "quick n = {! n !}", 1);
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": file_uri, "version": 2 },
                "contentChanges": [{ "text": text }] }),
    );
    busy(&mut client);
    let (hover, waited) = timed(&mut client, &quick);
    assert!(
        hover.contains("n : ℕ")
            && !hover.contains("Have:")
            && hover.ends_with(
                "\n*Agda is busy: the type of the goal's text follows when it is free.*\n"
            ),
        "{hover}"
    );
    assert!(waited >= Duration::from_millis(950), "{waited:?}");
    assert!(!client.received.iter().any(ended), "Agda was done first");
    client.wait_for("progress", ended);
    assert!(client.hover(&file_uri, quick).contains("Have: ℕ"));

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hover_while_agda_loads_again() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("reload", &["SlowLoad.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let file = root.join("SlowLoad.agda");
    let file_uri = uri(&file);
    let text = std::fs::read_to_string(&file).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": file_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&file_uri, |d| d.len() == 1);
    let goal = position_of(&text, "{!  !}", 0);
    let expected = client.hover(&file_uri, goal.clone());
    assert!(expected.contains("n : ℕ"), "{expected}");

    // Loading this file takes seconds, for `ack 3 8 ≡ 2045`. Meanwhile, after
    // a second, hover shows Agda's answer from the last load, and says so.
    let changed = format!("{text}\n-- changed\n");
    std::fs::write(&file, &changed).unwrap();
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": file_uri, "version": 2 },
                "contentChanges": [{ "text": changed }] }),
    );
    client.received.clear();
    client.notify(
        "textDocument/didSave",
        json!({ "textDocument": { "uri": file_uri } }),
    );
    client.wait_for("progress", |m| {
        m["method"] == "$/progress" && m["params"]["value"]["kind"] == "begin"
    });
    let start = std::time::Instant::now();
    let hover = client.hover(&file_uri, goal.clone());
    let waited = start.elapsed();
    assert_eq!(
        hover,
        format!("{expected}\n*Agda is loading the file again: this is from the last load.*\n")
    );
    assert!(waited >= Duration::from_millis(950), "{waited:?}");
    assert!(
        !client
            .received
            .iter()
            .any(|m| m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end"),
        "Agda was done first"
    );
    // After the load, Agda's new answer, without the note.
    client.diagnostics(&file_uri, |d| d.len() == 1);
    assert_eq!(client.hover(&file_uri, goal), expected);

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

/// Start a long goal command, and return the token of its progress, once
/// it began.
fn start_long(client: &mut Client, command: &str, uri: &str, goal: u32) -> Value {
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": command, "arguments": [uri, goal] }),
    );
    let begin = client.wait_for("progress", |m| {
        m["method"] == "$/progress" && m["params"]["value"]["kind"] == "begin"
    });
    assert_eq!(begin["params"]["value"]["cancellable"], true, "{begin}");
    begin["params"]["token"].clone()
}

/// Cancel a progress as Zed does, and return how long its command then
/// took to end.
fn cancel(client: &mut Client, token: &Value) -> Duration {
    let start = std::time::Instant::now();
    client.notify("window/workDoneProgress/cancel", json!({ "token": token }));
    client.wait_for("progress", |m| {
        m["method"] == "$/progress"
            && m["params"]["token"] == *token
            && m["params"]["value"]["kind"] == "end"
    });
    start.elapsed()
}

#[test]
fn stops_long_commands() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("stop", &["Slow.agda", "Search.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let output = root.join(".zed/agda-output.md");
    let slow = root.join("Slow.agda");
    let slow_uri = uri(&slow);
    let mut text = std::fs::read_to_string(&slow).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": slow_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    client.diagnostics(&slow_uri, |d| d.len() == 3);

    // The normal form of `ack three (suc eight)` takes about 10 s. A cancel
    // with another token leaves it; Zed's cancel stops it at once.
    text = text.replacen("ack three eight", "ack three (suc eight)", 1);
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": slow_uri, "version": 2 },
                "contentChanges": [{ "text": text }] }),
    );
    let token = start_long(&mut client, "agda.normalForm", &slow_uri, 0);
    client.notify(
        "window/workDoneProgress/cancel",
        json!({ "token": "agda-bridge/other" }),
    );
    client.receive_for(Duration::from_millis(500));
    assert!(
        !client
            .received
            .iter()
            .any(|m| m["method"] == "$/progress" && m["params"]["value"]["kind"] == "end"),
        "another token stopped it"
    );
    let took = cancel(&mut client, &token);
    assert!(took < Duration::from_secs(2), "{took:?}");
    let stopped = "Stopped computing the normal form of `ack three (suc eight)` in ?0.";
    assert!(wait_for_output(&output, stopped).contains(stopped));

    // A cancel after the end does nothing, and Agda answers the next
    // commands as before: the goal not hovered yet, and a quick normal form.
    client.notify("window/workDoneProgress/cancel", json!({ "token": token }));
    assert!(
        client
            .hover(&slow_uri, position_of(&text, "{!  !}", 1))
            .contains("m : ℕ")
    );
    text = text.replacen("ack three (suc eight)", "ack zero zero", 1);
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": slow_uri, "version": 3 },
                "contentChanges": [{ "text": text }] }),
    );
    let token = start_long(&mut client, "agda.normalForm", &slow_uri, 0);
    client.wait_for("progress", |m| {
        m["method"] == "$/progress"
            && m["params"]["token"] == token
            && m["params"]["value"]["kind"] == "end"
    });
    let normal = "**Normal form of `ack zero zero` in ?0**\n\n```agda\nsuc zero\n```\n";
    assert!(wait_for_output(&output, normal).contains(normal));

    // Auto with a time limit of 10 s, on a goal it cannot fill, stops too.
    let search = root.join("Search.agda");
    let search_uri = uri(&search);
    let search_text = std::fs::read_to_string(&search).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": search_uri, "languageId": "agda", "version": 1, "text": search_text } }),
    );
    client.diagnostics(&search_uri, |d| d.len() == 1);
    let token = start_long(&mut client, "agda.auto", &search_uri, 0);
    let took = cancel(&mut client, &token);
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(wait_for_output(&output, "Stopped auto on ?0.").contains("Stopped auto on ?0."));
    let hover = client.hover(&search_uri, position_of(&search_text, "{!", 0));
    assert!(hover.contains("m : ℕ"), "{hover}");

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn goal_types_follow_auto() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("follow-auto", &["Depends.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let file = root.join("Depends.agda");
    let file_uri = uri(&file);
    let text = std::fs::read_to_string(&file).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": file_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    // The type of ?1 is a meta that ?0 will solve, such as `_B_11`.
    let diagnostics = client.diagnostics(&file_uri, |d| d.len() == 2);
    let goals = messages(&diagnostics, 3);
    assert!(
        goals[0] == "?0 : Set" && goals[1].starts_with("?1 : _"),
        "{goals:?}"
    );
    let hover = client.hover(&file_uri, position_of(&text, "{!  !}", 1));
    assert!(hover.contains("Goal: _"), "{hover}");

    // Auto fills ?0 with `ℕ`, which Agda sends no new list of goals for;
    // the diagnostic and the hover of ?1 follow anyway.
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.auto", "arguments": [file_uri, 0] }),
    );
    let edit = client.wait_for("applyEdit", |m| m["method"] == "workspace/applyEdit");
    let edits = edit["params"]["edit"]["changes"][&file_uri]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(edits[0]["newText"], "ℕ");
    let text = apply_all(&text, &edits);
    let diagnostics = client.diagnostics(&file_uri, |d| d.len() == 1);
    assert_eq!(messages(&diagnostics, 3), ["?1 : ℕ"]);
    let hover = client.hover(&file_uri, position_of(&text, "{!  !}", 0));
    assert!(hover.contains("Goal: ℕ"), "{hover}");

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn makes_helper_functions() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("helper-functions", &["Helper.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    client.initialize(&root, &agda);
    let file = root.join("Helper.agda");
    let file_uri = uri(&file);
    let mut text = std::fs::read_to_string(&file).unwrap();
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": file_uri, "languageId": "agda", "version": 1, "text": text } }),
    );
    let diagnostics = client.diagnostics(&file_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?1 : ℕ"]);
    let mut version = 1;
    let mut change = |client: &mut Client, text: &str| {
        version += 1;
        client.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": file_uri, "version": version },
                    "contentChanges": [{ "text": text }] }),
        );
    };
    let save = |client: &mut Client, text: &str| {
        std::fs::write(&file, text).unwrap();
        client.received.clear();
        client.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri(&file) } }),
        );
    };
    let run = |client: &mut Client, goal: u32| -> Vec<Value> {
        client.received.clear();
        client.request(
            "workspace/executeCommand",
            json!({ "command": "agda.makeHelper", "arguments": [uri(&file), goal] }),
        );
        let edit = client.wait_for("applyEdit", |m| m["method"] == "workspace/applyEdit");
        edit["params"]["edit"]["changes"][&uri(&file)]
            .as_array()
            .unwrap()
            .clone()
    };

    // Offered for a new name, not for one Agda saw in the file (`suc`, the
    // bound `n`).
    let titles = client.code_actions(&file_uri, position_of(&text, "aux n m", 0));
    assert_eq!(
        titles,
        [
            "Give",
            "Refine",
            "Case split on `aux n m`",
            "Make helper function `aux`",
            "Auto",
            "Solve",
            "Solve all goals",
            "Print goal in output",
            "Print normal form in output",
            "Open output file"
        ]
    );

    // The helper goes above `_+_`, and the call, inside `suc`, in
    // parentheses.
    let edits = run(&mut client, 0);
    text = apply_all(&text, &edits);
    assert!(
        text.contains(
            "aux : (n m : ℕ) → ℕ\naux n m = {!  !}\n\n_+_ : ℕ → ℕ → ℕ\nzero  + m = m\nsuc n + m = suc (aux n m)\n"
        ),
        "{text}"
    );
    change(&mut client, &text);

    // A second helper waits for a save: Agda would name it `aux`.
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.makeHelper", "arguments": [file_uri, 1] }),
    );
    let message = client.wait_for("showMessage", |m| m["method"] == "window/showMessage");
    assert!(
        message["params"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Save the file first"),
        "{message}"
    );

    // Agda accepts the helper; in a `where` block, the next one goes above
    // `twice`, at its indentation, and the call is the whole right-hand side.
    save(&mut client, &text);
    let diagnostics = client.diagnostics(&file_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?1 : ℕ"]);
    let with_aux = text.clone();
    let where_block = "  where\n    go : (k : ℕ) → ℕ\n    go k = {!  !}\n\n    twice : ℕ → ℕ\n    twice k = go k\n";
    let edits = run(&mut client, 1);
    text = apply_all(&text, &edits);
    assert!(text.ends_with(where_block), "{text}");
    change(&mut client, &text);

    // Undone and saved, the text is the one Agda loaded, but Agda still
    // loads it again, so the same helper can be made once more.
    change(&mut client, &with_aux);
    save(&mut client, &with_aux);
    client.diagnostics(&file_uri, |d| d.len() == 2);
    let edits = run(&mut client, 1);
    text = apply_all(&with_aux, &edits);
    assert!(text.ends_with(where_block), "{text}");
    change(&mut client, &text);
    save(&mut client, &text);
    let diagnostics = client.diagnostics(&file_uri, |d| d.len() == 2);
    assert_eq!(messages(&diagnostics, 3), ["?0 : ℕ", "?1 : ℕ"]);

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn renames_across_open_files() {
    let Some(agda) = find_agda() else {
        eprintln!("skipping: no Agda found (set $AGDA or put agda on PATH)");
        return;
    };
    let root = setup("rename", &["Nat.agda", "Uses.agda", "Spike.agda"]);
    let mut client = Client::start(&root.join("bridge.sock"));
    let init = client.initialize(&root, &agda);
    assert_eq!(
        init["capabilities"]["renameProvider"]["prepareProvider"],
        true
    );
    let open = |client: &mut Client, name: &str| -> (String, String) {
        let path = root.join(name);
        let (uri, text) = (uri(&path), std::fs::read_to_string(&path).unwrap());
        client.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "agda", "version": 1, "text": text } }),
        );
        (uri, text)
    };
    let (nat_uri, nat_text) = open(&mut client, "Nat.agda");
    client.diagnostics(&nat_uri, |d| d.is_empty());
    let (uses_uri, uses_text) = open(&mut client, "Uses.agda");
    client.diagnostics(&uses_uri, |d| d.is_empty());
    let rename = |client: &mut Client, uri: &str, position: Value, name: &str| {
        client.request(
            "textDocument/rename",
            json!({ "textDocument": { "uri": uri }, "position": position, "newName": name }),
        )
    };

    // Renaming `suc` changes both open files, also when asked at the
    // definition in `Nat.agda`: the uses in `Uses.agda` are found through
    // that file's links.
    let at = position_of(&uses_text, "suc", 0);
    let prepared = client.request(
        "textDocument/prepareRename",
        json!({ "textDocument": { "uri": uses_uri }, "position": at }),
    );
    assert_eq!(prepared["end"]["character"], 9);
    let edit = rename(
        &mut client,
        &nat_uri,
        position_of(&nat_text, "suc", 0),
        "succ",
    );
    let uses_edits = edit["changes"][&uses_uri].as_array().unwrap();
    let nat_edits = edit["changes"][&nat_uri].as_array().unwrap();
    assert_eq!(
        apply_all(&uses_text, uses_edits),
        uses_text.replace("suc", "succ")
    );
    assert_eq!(
        apply_all(&nat_text, nat_edits),
        nat_text.replace("suc", "succ")
    );
    let message = client.wait_for("showMessage", |m| m["method"] == "window/showMessage");
    assert!(
        message["params"]["message"]
            .as_str()
            .unwrap()
            .contains("in 3 place(s) in 2 open file(s)"),
        "{message}"
    );
    // A keyword has nothing to rename, and a wrong name is refused.
    let keyword = client.request(
        "textDocument/prepareRename",
        json!({ "textDocument": { "uri": uses_uri }, "position": position_of(&uses_text, "open", 0) }),
    );
    assert_eq!(keyword, Value::Null);
    client.send(
        json!({ "jsonrpc": "2.0", "id": 999, "method": "textDocument/rename",
        "params": { "textDocument": { "uri": uses_uri }, "position": at, "newName": "a b" } }),
    );
    let refused = loop {
        let message = client.receive();
        if message["id"] == 999 {
            break message;
        }
    };
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a name")
    );

    // An operator renamed from one of its parts: `⊕` on `+` makes `_⊕_`.
    // Text in a goal is not checked by Agda, so it keeps `+`.
    let (spike_uri, spike_text) = open(&mut client, "Spike.agda");
    client.diagnostics(&spike_uri, |d| d.len() == 3);
    let edit = rename(
        &mut client,
        &spike_uri,
        position_of(&spike_text, "+ m = m", 0),
        "⊕",
    );
    let renamed = apply_all(&spike_text, edit["changes"][&spike_uri].as_array().unwrap());
    assert!(
        renamed.contains("_⊕_ : ℕ → ℕ → ℕ\nzero  ⊕ m = m\nsuc n ⊕ m = {! suc (n + m) !}"),
        "{renamed}"
    );
    // A bound variable only changes within its clause.
    let edit = rename(
        &mut client,
        &spike_uri,
        position_of(&spike_text, "b : 𝔹) →", 0),
        "x",
    );
    let renamed = apply_all(&spike_text, edit["changes"][&spike_uri].as_array().unwrap());
    assert!(renamed.contains("λ (x : 𝔹) →"), "{renamed}");

    // A new `agdaPath` restarts Agda at the next load: a missing program is
    // reported, and the right one works again.
    let save = |client: &mut Client, uri: &str| {
        client.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": uri } }),
        );
    };
    client.received.clear();
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "agdaPath": "agda-bridge-test-no-agda" } }),
    );
    save(&mut client, &nat_uri);
    client.wait_for("agdaPath", |m| {
        m["method"] == "window/showMessage"
            && m["params"]["message"]
                .as_str()
                .is_some_and(|text| text.contains("agdaPath"))
    });
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "agdaPath": agda } }),
    );
    save(&mut client, &nat_uri);
    client.diagnostics(&nat_uri, |d| d.is_empty());

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}
