//! End-to-end test: this file plays Zed's role, a small LSP client, and drives
//! the real `agda-bridge` binary against a real Agda.
//!
//! Agda is taken from `$AGDA`, or `agda` on `PATH`; without it the test is
//! skipped with a message.

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
    fn start(agda: &str) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_agda-bridge"))
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
        let _ = agda;
        Client {
            child,
            stdin,
            incoming,
            next_id: 1,
            received: Vec::new(),
            history: Vec::new(),
        }
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
        let message = self
            .incoming
            .recv_timeout(TIMEOUT)
            .expect("message from agda-bridge");
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
        message
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

    let root: PathBuf =
        std::env::temp_dir().join(format!("agda-bridge-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in ["Spike.agda", "Bad.agda"] {
        std::fs::copy(fixtures.join(name), root.join(name)).unwrap();
    }
    let socket = root.join("bridge.sock");
    // SAFETY: the test sets this before starting any thread that reads it.
    unsafe { std::env::set_var("AGDA_BRIDGE_SOCKET", &socket) };

    let spike = root.join("Spike.agda");
    let spike_uri = uri(&spike);
    let mut text = std::fs::read_to_string(&spike).unwrap();

    let mut client = Client::start(&agda);
    let init = client.request(
        "initialize",
        json!({
            "processId": null,
            "rootUri": uri(&root),
            "workspaceFolders": [{ "uri": uri(&root), "name": "test" }],
            "capabilities": { "window": { "showDocument": { "support": true } } },
            "initializationOptions": { "agdaPath": agda },
        }),
    );
    assert_eq!(init["capabilities"]["hoverProvider"], true);
    assert!(
        init["capabilities"]["executeCommandProvider"]["commands"]
            .as_array()
            .unwrap()
            .contains(&json!("agda.give"))
    );
    client.notify("initialized", json!({}));

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
    let hover = client.hover(&spike_uri, position_of(&text, "{!   !}", 0));
    assert!(
        hover.contains("Goal: ℕ") && hover.contains("n : ℕ"),
        "{hover}"
    );
    let hover = client.hover(&spike_uri, position_of(&text, "{!   !}", 1));
    assert!(
        hover.contains("Goal: 𝔹") && hover.contains("b : 𝔹"),
        "{hover}"
    );
    assert_eq!(
        client.hover(&spike_uri, json!({ "line": 0, "character": 0 })),
        ""
    );

    // Code actions on a filled hole offer give, refine, showing the goal and
    // the output file; a line without goals or problems offers nothing.
    let titles = client.code_actions(&spike_uri, position_of(&text, "suc (n", 0));
    assert_eq!(
        titles,
        [
            "Agda: give ?0",
            "Agda: refine ?0",
            "Agda: show goal ?0 in output",
            "Agda: open output file"
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

    // The debug client takes UTF-8 byte columns, and this line has a
    // two-byte `λ`, a four-byte `𝔹` and a three-byte `→` before the hole.
    let line = text.lines().position(|l| l.starts_with("not = ")).unwrap();
    let column = text.lines().nth(line).unwrap().find("{!").unwrap() + 3;
    let run_client = |command: &str, row: usize, column: usize| {
        Command::new(env!("CARGO_BIN_EXE_agda-bridge"))
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
    assert_eq!(titles, ["Agda: open output file"]);
    client.received.clear();
    client.request(
        "workspace/executeCommand",
        json!({ "command": "agda.openOutput", "arguments": [] }),
    );
    let shown = client.wait_for("showDocument", |m| m["method"] == "window/showDocument");
    assert_eq!(shown["params"]["uri"], uri(&output));

    client.request("shutdown", Value::Null);
    client.notify("exit", Value::Null);
    let _ = client.child.wait();
    let _ = std::fs::remove_dir_all(&root);
}
