//! A long-lived `agda --interaction-json` process.
//!
//! Agda handles one command at a time and signals completion with the
//! `JSON> ` prompt, so [`Agda::run`] takes `&mut self`: callers serialise
//! access by keeping the process behind a mutex.
//!
//! A caller may go away while Agda works on its command: Zed cancels a hover
//! when the mouse moves on, and the language server then drops the request.
//! Agda answers anyway, so [`Agda::run`] first drops what such a command left
//! behind; otherwise every later command would read the answer of the one
//! before it.

use std::io;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::protocol::{Event, Response, Splitter, parse_line};

pub struct Agda {
    _child: Child,
    stdin: ChildStdin,
    events: mpsc::UnboundedReceiver<Event>,
    /// Commands sent whose prompt has not been read yet: more than zero
    /// only when a caller went away during [`Agda::run`].
    unanswered: usize,
}

impl Agda {
    /// Start Agda and wait for its first prompt.
    pub async fn spawn(program: &str, args: &[String]) -> io::Result<Agda> {
        let mut child = Command::new(program)
            .arg("--interaction-json")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| {
                io::Error::new(err.kind(), format!("cannot start `{program}`: {err}"))
            })?;

        let stdin = child.stdin.take().expect("stdin is piped");
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let (sender, events) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut splitter = Splitter::default();
            let mut chunk = vec![0; 64 * 1024];
            while let Ok(read) = stdout.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                for event in splitter.push(&chunk[..read]) {
                    if sender.send(event).is_err() {
                        return;
                    }
                }
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                eprintln!("agda: {line}");
            }
        });

        let mut agda = Agda {
            _child: child,
            stdin,
            events,
            unanswered: 0,
        };
        match tokio::time::timeout(Duration::from_secs(60), agda.events.recv()).await {
            Ok(Some(Event::Prompt)) => Ok(agda),
            Ok(Some(Event::Line(line))) => Err(io::Error::other(format!(
                "`{program}` did not start in interaction mode, it printed: {line}"
            ))),
            Ok(None) => Err(io::Error::other(format!(
                "`{program}` exited during startup"
            ))),
            Err(_) => Err(io::Error::other(format!(
                "`{program}` did not respond within 60 s"
            ))),
        }
    }

    /// Send one IOTCM command and collect its responses up to the next prompt.
    pub async fn run(&mut self, command: &str) -> io::Result<Vec<Response>> {
        // Drop the answers of commands whose callers went away.
        while self.unanswered > 0 {
            match self.events.recv().await {
                Some(Event::Prompt) => self.unanswered -= 1,
                Some(Event::Line(_)) => {}
                None => return Err(io::Error::other("Agda exited")),
            }
        }

        eprintln!("agda-bridge: > {command}");
        // Counted as soon as the whole line is written. A caller that goes
        // away before that leaves at most part of a line, which joins the
        // next command into one line with one answer, so the count still
        // matches Agda's prompts.
        self.stdin
            .write_all(format!("{command}\n").as_bytes())
            .await?;
        self.unanswered += 1;
        self.stdin.flush().await?;

        let mut responses = Vec::new();
        loop {
            match self.events.recv().await {
                Some(Event::Prompt) => {
                    self.unanswered -= 1;
                    return Ok(responses);
                }
                Some(Event::Line(line)) => match parse_line(&line) {
                    Ok(response) => responses.push(response),
                    Err(err) => eprintln!("agda-bridge: unparsed output ({err}): {line}"),
                },
                None => return Err(io::Error::other("Agda exited")),
            }
        }
    }
}
