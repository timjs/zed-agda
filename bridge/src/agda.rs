//! A long-lived `agda --interaction-json` process.
//!
//! Agda handles one command at a time and signals completion with the
//! `JSON> ` prompt, so [`Agda::run`] takes `&mut self`: callers serialise
//! access by keeping the process behind a mutex.

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
        eprintln!("agda-bridge: > {command}");
        self.stdin.write_all(command.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut responses = Vec::new();
        loop {
            match self.events.recv().await {
                Some(Event::Prompt) => return Ok(responses),
                Some(Event::Line(line)) => match parse_line(&line) {
                    None => {}
                    Some(Ok(response)) => responses.push(response),
                    Some(Err(err)) => eprintln!("agda-bridge: unparsed output ({err}): {line}"),
                },
                None => return Err(io::Error::other("Agda exited")),
            }
        }
    }
}
