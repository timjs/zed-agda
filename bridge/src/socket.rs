//! A debugging client for a running bridge.
//!
//! `agda-bridge client <command> --root DIR --file FILE [--row N --column N]`
//! sends one JSON line to the bridge serving that worktree over a Unix socket
//! and prints the reply, so Agda commands can be tried from a terminal while
//! Zed is running. Rows and columns are 1-based, with columns in UTF-8 bytes,
//! like Zed's `ZED_ROW` and `ZED_COLUMN` task variables, so a user-defined Zed
//! task can call it too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tower_lsp_server::ls_types::MessageType;

use crate::server::{Bridge, GoalCommand};

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    command: String,
    file: PathBuf,
    /// 1-based line, as `ZED_ROW`.
    row: Option<usize>,
    /// 1-based UTF-8 byte column, as `ZED_COLUMN`.
    column: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    ok: bool,
    message: String,
}

/// The socket for a worktree; the bridge and the client compute the same path.
/// `AGDA_BRIDGE_SOCKET` overrides it.
pub fn socket_path(root: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("AGDA_BRIDGE_SOCKET") {
        return PathBuf::from(path);
    }
    let root = root.to_string_lossy();
    let root = root.trim_end_matches('/');
    // FNV-1a: stable across builds, unlike std's DefaultHasher.
    let hash = root.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    std::env::temp_dir().join(format!("agda-bridge-{hash:016x}.sock"))
}

#[cfg(unix)]
pub fn serve(bridge: Arc<Bridge>, root: PathBuf) {
    let path = socket_path(&root);
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("agda-bridge: cannot listen on {}: {err}", path.display());
            return;
        }
    };
    eprintln!(
        "agda-bridge: listening for client requests on {}",
        path.display()
    );
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                if let Err(err) = handle(bridge, stream).await {
                    eprintln!("agda-bridge: client request failed: {err}");
                }
            });
        }
    });
}

#[cfg(not(unix))]
pub fn serve(_bridge: Arc<Bridge>, _root: PathBuf) {
    eprintln!("agda-bridge: client requests are not supported on this platform yet");
}

#[cfg(unix)]
async fn handle(bridge: Arc<Bridge>, stream: tokio::net::UnixStream) -> io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (read, mut write) = stream.into_split();
    let Some(line) = BufReader::new(read).lines().next_line().await? else {
        return Ok(());
    };
    let result = match serde_json::from_str::<Request>(&line) {
        Ok(request) => execute(&bridge, request).await,
        Err(err) => Err(format!("Bad request: {err}")),
    };
    if let Err(message) = &result {
        // Nobody may be watching the client's output, so tell the user in Zed too.
        bridge
            .client
            .show_message(MessageType::WARNING, message)
            .await;
    }
    let reply = match result {
        Ok(message) => Reply { ok: true, message },
        Err(message) => Reply { ok: false, message },
    };
    let mut json = serde_json::to_string(&reply)?;
    json.push('\n');
    write.write_all(json.as_bytes()).await
}

async fn execute(bridge: &Bridge, request: Request) -> Result<String, String> {
    match request.command.as_str() {
        "load" => return bridge.load(&request.file).await,
        "solve-all" => return bridge.solve(&request.file, None).await,
        _ => {}
    }
    let (Some(row), Some(column)) = (request.row, request.column) else {
        return Err(format!("`{}` needs --row and --column.", request.command));
    };
    let id = bridge
        .goal_at_byte_column(&request.file, row, column)
        .ok_or("The cursor is not in a goal.")?;
    match request.command.as_str() {
        "give" => bridge.give(&request.file, id, GoalCommand::Give).await,
        "refine" => bridge.give(&request.file, id, GoalCommand::Refine).await,
        "auto" => bridge.give(&request.file, id, GoalCommand::Auto).await,
        "case-split" => bridge.case_split(&request.file, id, None).await,
        "solve" => bridge.solve(&request.file, Some(id)).await,
        "goal" => {
            let markdown = bridge.goal_info(&request.file, id, true).await?;
            bridge
                .show_output(&format!("Goal ?{id}"), &request.file, &markdown)
                .await;
            Ok(format!("Goal ?{id} is in the output file."))
        }
        other => Err(format!("Unknown command `{other}`.")),
    }
}

const USAGE: &str = "usage: agda-bridge client \
                     <load|goal|give|refine|case-split|auto|solve|solve-all> --file FILE \
                     [--root DIR] [--row N --column N]";

/// The `agda-bridge client` subcommand. Returns the process exit code.
pub async fn run_client(args: &[String]) -> i32 {
    let mut command = None;
    let mut file = None;
    let mut root = None;
    let mut row = None;
    let mut column = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned();
        match arg.as_str() {
            "--file" => file = value().map(PathBuf::from),
            "--root" => root = value().map(PathBuf::from),
            "--row" => row = value().and_then(|v| v.parse().ok()),
            "--column" => column = value().and_then(|v| v.parse().ok()),
            other if command.is_none() && !other.starts_with("--") => {
                command = Some(other.to_string())
            }
            other => {
                eprintln!("unexpected argument `{other}`\n{USAGE}");
                return 2;
            }
        }
    }
    let (Some(command), Some(file)) = (command, file) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let root = root
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let request = Request {
        command,
        file,
        row,
        column,
    };
    match send(&socket_path(&root), &request).await {
        Ok(reply) => {
            println!("{}", reply.message);
            if reply.ok { 0 } else { 1 }
        }
        Err(err) => {
            eprintln!(
                "agda-bridge is not reachable for {} ({err}). Open an .agda file of this worktree in Zed first.",
                root.display()
            );
            1
        }
    }
}

#[cfg(unix)]
async fn send(path: &Path, request: &Request) -> io::Result<Reply> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::UnixStream::connect(path).await?;
    let (read, mut write) = stream.into_split();
    let mut json = serde_json::to_string(request)?;
    json.push('\n');
    write.write_all(json.as_bytes()).await?;
    let line = BufReader::new(read)
        .lines()
        .next_line()
        .await?
        .ok_or_else(|| io::Error::other("the bridge closed the connection"))?;
    Ok(serde_json::from_str(&line)?)
}

#[cfg(not(unix))]
async fn send(_path: &Path, _request: &Request) -> io::Result<Reply> {
    Err(io::Error::other(
        "client requests are not supported on this platform yet",
    ))
}
