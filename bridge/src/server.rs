//! The language server: LSP towards Zed, `--interaction-json` towards Agda.
//!
//! [`Bridge`] holds all state and implements the Agda operations (load, goal
//! information, give, refine). The LSP handlers in [`Backend`] and the task
//! socket in `socket.rs` both call into it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use tower_lsp_server::jsonrpc::Result as RpcResult;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::agda::Agda;
use crate::goals::{self, GOAL_MARKER, Goal};
use crate::output::Output;
use crate::protocol::{
    DisplayInfo, GiveResult, GoalInfo, InteractionPoint, Response, message_text,
};
use crate::text::{self, Change};
use crate::{iotcm, location, render};

pub const COMMAND_GIVE: &str = "agda.give";
pub const COMMAND_REFINE: &str = "agda.refine";
pub const COMMAND_GOAL: &str = "agda.goal";

pub async fn run() {
    let (service, socket) = LspService::new(|client| Backend(Arc::new(Bridge::new(client))));
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .concurrency_level(16)
        .serve(service)
        .await;
}

struct Config {
    agda_path: String,
    extra_args: Vec<String>,
    root: Option<PathBuf>,
}

#[derive(Default)]
struct Document {
    text: String,
    version: i32,
    goals: Vec<Goal>,
    goal_types: HashMap<u32, String>,
    /// Errors and warnings from the last load, as diagnostics.
    problems: Vec<Diagnostic>,
    /// The text Agda last loaded, to skip reloading identical text.
    loaded_text: Option<String>,
}

struct Session {
    agda: Agda,
    /// Agda keeps one "current file"; goal commands only work on that file.
    current_file: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GoalCommand {
    Give,
    Refine,
}

pub struct Bridge {
    pub client: Client,
    config: OnceLock<Config>,
    output: OnceLock<Output>,
    documents: Mutex<HashMap<PathBuf, Document>>,
    session: AsyncMutex<Option<Session>>,
}

/// Everything one Agda command answered, sorted by purpose.
#[derive(Default)]
struct Outcome {
    interaction_points: Option<Vec<InteractionPoint>>,
    goal_types: HashMap<u32, String>,
    errors: Vec<String>,
    warnings: Vec<String>,
    give: Option<(u32, GiveResult)>,
    goal_info: Option<(u32, GoalInfo)>,
    displays: Vec<Value>,
}

impl Outcome {
    fn collect(responses: Vec<Response>) -> Outcome {
        let mut outcome = Outcome::default();
        for response in responses {
            match response {
                Response::InteractionPoints { interaction_points } => {
                    outcome.interaction_points = Some(interaction_points);
                }
                Response::GiveAction {
                    interaction_point,
                    give_result,
                } => outcome.give = Some((interaction_point.id, give_result)),
                Response::DisplayInfo { info } => {
                    match DisplayInfo::parse(&info) {
                        DisplayInfo::AllGoalsWarnings {
                            visible_goals,
                            warnings,
                            errors,
                            ..
                        } => {
                            for goal in &visible_goals {
                                if let (Some(id), Some(ty)) = (goal.goal_id(), &goal.ty) {
                                    outcome.goal_types.insert(id, ty.clone());
                                }
                            }
                            outcome.errors.extend(errors.iter().map(message_text));
                            outcome.warnings.extend(warnings.iter().map(message_text));
                        }
                        DisplayInfo::Error {
                            error,
                            message,
                            warnings,
                        } => {
                            let text = message.or_else(|| error.as_ref().map(message_text));
                            outcome
                                .errors
                                .push(text.unwrap_or_else(|| "Unknown error".into()));
                            outcome.warnings.extend(warnings.iter().map(message_text));
                        }
                        DisplayInfo::GoalSpecific {
                            interaction_point,
                            goal_info,
                        } => outcome.goal_info = Some((interaction_point.id, goal_info)),
                        DisplayInfo::Other => {}
                    }
                    outcome.displays.push(info);
                }
                Response::Other => {}
            }
        }
        outcome
    }

    fn markdown(&self) -> String {
        self.displays
            .iter()
            .map(render::display_info)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn uri_of(path: &Path) -> Option<Uri> {
    Uri::from_file_path(path)
}

fn range_of(text: &str, start: usize, end: usize) -> Range {
    Range::new(text::position_of(text, start), text::position_of(text, end))
}

fn problem(text: &str, file: &str, message: &str, severity: DiagnosticSeverity) -> Diagnostic {
    let range = match location::span_in_file(message, file) {
        Some(span) => range_of(
            text,
            text::offset_of_line_col(text, span.start.0, span.start.1),
            text::offset_of_line_col(text, span.end.0, span.end.1),
        ),
        None => Range::default(),
    };
    Diagnostic {
        range,
        severity: Some(severity),
        source: Some("agda".into()),
        message: location::without_location(message, file).trim().to_string(),
        ..Diagnostic::default()
    }
}

impl Bridge {
    fn new(client: Client) -> Bridge {
        Bridge {
            client,
            config: OnceLock::new(),
            output: OnceLock::new(),
            documents: Mutex::new(HashMap::new()),
            session: AsyncMutex::new(None),
        }
    }

    fn config(&self) -> &Config {
        self.config.get().expect("initialized before use")
    }

    pub fn root(&self) -> Option<&Path> {
        self.config.get()?.root.as_deref()
    }

    /// Lock the Agda session, starting Agda on first use.
    async fn lock_session(&self) -> Result<MutexGuard<'_, Option<Session>>, String> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            let config = self.config();
            let agda = Agda::spawn(&config.agda_path, &config.extra_args)
                .await
                .map_err(|err| {
                    format!("{err}. Set `agdaPath` in the agda-bridge initialization options.")
                })?;
            *guard = Some(Session {
                agda,
                current_file: None,
            });
        }
        Ok(guard)
    }

    /// Run one command; a dead Agda is dropped so the next command restarts it.
    async fn run(
        &self,
        guard: &mut MutexGuard<'_, Option<Session>>,
        command: &str,
    ) -> Result<Vec<Response>, String> {
        let session = guard.as_mut().expect("session started");
        match session.agda.run(command).await {
            Ok(responses) => Ok(responses),
            Err(err) => {
                **guard = None;
                Err(format!("Agda stopped: {err}"))
            }
        }
    }

    fn check_current(guard: &MutexGuard<'_, Option<Session>>, path: &Path) -> Result<(), String> {
        match guard
            .as_ref()
            .and_then(|session| session.current_file.as_deref())
        {
            Some(current) if current == path => Ok(()),
            _ => Err("This file is not loaded in Agda yet. Save it to load it.".into()),
        }
    }

    /// Type-check `path` with `Cmd_load`, then refresh goals and diagnostics.
    pub async fn load(&self, path: &Path) -> Result<String, String> {
        let snapshot = match self.documents.lock().unwrap().get(path) {
            Some(document) => document.text.clone(),
            None => std::fs::read_to_string(path)
                .map_err(|err| format!("Cannot read {}: {err}", path.display()))?,
        };

        let mut guard = self.lock_session().await?;
        let unchanged = self
            .documents
            .lock()
            .unwrap()
            .get(path)
            .and_then(|d| d.loaded_text.as_ref())
            == Some(&snapshot);
        let current = guard.as_ref().and_then(|s| s.current_file.as_deref()) == Some(path);
        if unchanged && current {
            return Ok("Already loaded.".into());
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let progress = self.begin_progress(&format!("checking {name}")).await;
        let result = self
            .run(&mut guard, &iotcm::load(path, &self.config().extra_args))
            .await;
        if let Some(progress) = progress {
            progress.finish().await;
        }
        let outcome = Outcome::collect(result?);
        if let Some(session) = guard.as_mut() {
            session.current_file = Some(path.to_path_buf());
        }
        drop(guard);

        // Agda's ranges refer to the snapshot; move them to the current text.
        let mut goals: Vec<Goal> = outcome
            .interaction_points
            .iter()
            .flatten()
            .filter_map(|point| {
                let interval = point.range.first()?;
                Some(Goal {
                    id: point.id,
                    start: interval.start.pos - 1,
                    end: interval.end.pos - 1,
                })
            })
            .collect();
        let file = path.to_string_lossy();
        let mut problems: Vec<Diagnostic> = outcome
            .errors
            .iter()
            .map(|message| problem(&snapshot, &file, message, DiagnosticSeverity::ERROR))
            .collect();
        problems.extend(
            outcome
                .warnings
                .iter()
                .map(|message| problem(&snapshot, &file, message, DiagnosticSeverity::WARNING)),
        );
        {
            let mut documents = self.documents.lock().unwrap();
            let document = documents
                .entry(path.to_path_buf())
                .or_insert_with(|| Document {
                    text: snapshot.clone(),
                    ..Document::default()
                });
            if let Some(change) = text::single_change(&snapshot, &document.text) {
                goals::adjust(&mut goals, &snapshot, &change);
            }
            document.goals = goals;
            document.goal_types = outcome.goal_types.clone();
            document.problems = problems;
            document.loaded_text = Some(snapshot);
        }

        self.publish(path).await;
        self.show_output("Load", path, &outcome.markdown()).await;
        let goal_count = outcome.goal_types.len();
        Ok(match outcome.errors.len() {
            0 => format!("Loaded {name}: {goal_count} goal(s)."),
            n => format!("Loaded {name}: {n} error(s)."),
        })
    }

    /// Goal type and context, rendered as Markdown. With `wait` false, return
    /// at once when Agda is busy instead of queueing behind a long load.
    pub async fn goal_info(&self, path: &Path, id: u32, wait: bool) -> Result<String, String> {
        let mut guard = if wait {
            self.lock_session().await?
        } else {
            self.session
                .try_lock()
                .map_err(|_| "Agda is busy. Hover again in a moment.".to_string())?
        };
        Self::check_current(&guard, path)?;
        let outcome = Outcome::collect(
            self.run(&mut guard, &iotcm::goal_type_context(path, id))
                .await?,
        );
        drop(guard);
        match outcome.goal_info {
            Some((id, info)) => Ok(render::goal(id, &info)),
            None => Err(outcome.errors.join("\n")),
        }
    }

    /// Give or refine a goal with the expression typed in it, and apply
    /// Agda's result to the buffer with `workspace/applyEdit`.
    pub async fn give(&self, path: &Path, id: u32, command: GoalCommand) -> Result<String, String> {
        let expression = {
            let documents = self.documents.lock().unwrap();
            let document = documents.get(path).ok_or("This file is not open.")?;
            let goal = document
                .goals
                .iter()
                .find(|goal| goal.id == id)
                .ok_or(format!(
                    "Goal ?{id} no longer exists. Save the file to reload it."
                ))?;
            goal.content(&document.text)
        };
        if command == GoalCommand::Give && expression.is_empty() {
            return Err(format!(
                "Type an expression in goal ?{id} first, then give it."
            ));
        }

        let mut guard = self.lock_session().await?;
        Self::check_current(&guard, path)?;
        let request = match command {
            GoalCommand::Give => iotcm::give(path, id, &expression),
            GoalCommand::Refine => iotcm::refine(path, id, &expression),
        };
        let outcome = Outcome::collect(self.run(&mut guard, &request).await?);
        drop(guard);

        let Some((given, result)) = outcome.give.clone() else {
            self.show_output(&format!("Give ?{id}"), path, &outcome.markdown())
                .await;
            return Err(outcome
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| "Agda did not fill the goal.".into()));
        };

        // Compute the edit against the current text, and apply it to the
        // bridge's copy right away, so the `didChange` that Zed sends for it
        // turns out to be a no-op instead of racing with this update.
        let (uri, edit) = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents.get_mut(path).ok_or("This file was closed.")?;
            let goal = document
                .goals
                .iter()
                .find(|goal| goal.id == given)
                .cloned()
                .ok_or(format!("Goal ?{given} disappeared while Agda was working."))?;
            let replacement = goals::expand_question_marks(&match result {
                GiveResult::Str { str } => str,
                GiveResult::Paren { paren: true } => format!("({})", goal.content(&document.text)),
                GiveResult::Paren { paren: false } => goal.content(&document.text),
            });
            let range = range_of(&document.text, goal.start, goal.end);
            let chars: Vec<char> = document.text.chars().collect();
            let new_text: String = chars[..goal.start]
                .iter()
                .copied()
                .chain(replacement.chars())
                .chain(chars[goal.end..].iter().copied())
                .collect();
            let change = Change {
                start: goal.start,
                old_end: goal.end,
                new_len: replacement.chars().count(),
            };
            let old_text = std::mem::replace(&mut document.text, new_text);
            goals::adjust(&mut document.goals, &old_text, &change);

            // Goals created by the result (`suc ?`) get the ids Agda did not know before.
            if let Some(points) = &outcome.interaction_points {
                let known: HashSet<u32> = document.goals.iter().map(|goal| goal.id).collect();
                let mut fresh: Vec<u32> = points
                    .iter()
                    .map(|p| p.id)
                    .filter(|id| !known.contains(id))
                    .collect();
                fresh.sort_unstable();
                let width = GOAL_MARKER.chars().count();
                for (new_id, offset) in fresh.into_iter().zip(goals::marker_offsets(&replacement)) {
                    let start = goal.start + offset;
                    document.goals.push(Goal {
                        id: new_id,
                        start,
                        end: start + width,
                    });
                }
                document.goals.sort_by_key(|goal| goal.start);
            }
            document.goal_types = outcome.goal_types.clone();
            let uri = uri_of(path).ok_or("Invalid file path.")?;
            (uri, TextEdit::new(range, replacement))
        };

        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(uri, vec![edit])])),
            ..WorkspaceEdit::default()
        };
        match self.client.apply_edit(edit).await {
            Ok(response) if response.applied => {}
            Ok(response) => {
                return Err(format!(
                    "Zed did not apply the edit: {}",
                    response.failure_reason.unwrap_or_default()
                ));
            }
            Err(err) => return Err(format!("Zed did not apply the edit: {err}")),
        }
        self.publish(path).await;
        self.show_output(&format!("Give ?{given}"), path, &outcome.markdown())
            .await;
        Ok(format!("Filled goal ?{given}."))
    }

    /// The goal at a 1-based row and UTF-8 byte column, as Zed's task
    /// variables `ZED_ROW` and `ZED_COLUMN` give them.
    pub fn goal_at_byte_column(&self, path: &Path, row: usize, column: usize) -> Option<u32> {
        let documents = self.documents.lock().unwrap();
        let document = documents.get(path)?;
        let offset = text::offset_of_row_byte_column(&document.text, row, column);
        goals::goal_at(&document.goals, offset).map(|goal| goal.id)
    }

    async fn publish(&self, path: &Path) {
        let Some(uri) = uri_of(path) else { return };
        let diagnostics = {
            let documents = self.documents.lock().unwrap();
            let Some(document) = documents.get(path) else {
                return;
            };
            let mut diagnostics = document.problems.clone();
            diagnostics.extend(document.goals.iter().map(|goal| {
                let ty = document
                    .goal_types
                    .get(&goal.id)
                    .map(String::as_str)
                    .unwrap_or("?");
                Diagnostic {
                    range: range_of(&document.text, goal.start, goal.end),
                    severity: Some(DiagnosticSeverity::INFORMATION),
                    source: Some("agda".into()),
                    message: format!("?{} : {ty}", goal.id),
                    ..Diagnostic::default()
                }
            }));
            diagnostics
        };
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }

    pub async fn show_output(&self, title: &str, source: &Path, body: &str) {
        let Some(output) = self.output.get() else {
            return;
        };
        match output.write(title, Some(source), body).await {
            Ok(true) => {
                if let Some(uri) = uri_of(&output.path) {
                    let params = ShowDocumentParams {
                        uri,
                        external: Some(false),
                        take_focus: Some(false),
                        selection: None,
                    };
                    let _ = self.client.show_document(params).await;
                }
            }
            Ok(false) => {}
            Err(err) => eprintln!("agda-bridge: cannot write {}: {err}", output.path.display()),
        }
    }

    async fn begin_progress(
        &self,
        message: &str,
    ) -> Option<
        tower_lsp_server::OngoingProgress<
            tower_lsp_server::Unbounded,
            tower_lsp_server::NotCancellable,
        >,
    > {
        let token = ProgressToken::String(format!("agda-bridge/{}", self.client.next_request_id()));
        self.client
            .create_work_done_progress(token.clone())
            .await
            .ok()?;
        Some(
            self.client
                .progress(token, "Agda")
                .with_message(message)
                .begin()
                .await,
        )
    }

    /// Report the result of a background operation as a notification.
    async fn report(&self, result: Result<String, String>) {
        if let Err(message) = result {
            self.client
                .show_message(MessageType::WARNING, message)
                .await;
        }
    }
}

pub struct Backend(pub Arc<Bridge>);

fn path_of(uri: &Uri) -> Option<PathBuf> {
    uri.to_file_path().map(|path| path.into_owned())
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> RpcResult<InitializeResult> {
        let options = params.initialization_options.unwrap_or(Value::Null);
        #[allow(deprecated)]
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|folder| path_of(&folder.uri))
            .or_else(|| params.root_uri.as_ref().and_then(path_of));
        let agda_path = options["agdaPath"].as_str().unwrap_or("agda").to_string();
        let extra_args = options["extraArgs"]
            .as_array()
            .map(|args| {
                args.iter()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let output_path = match (options["outputFile"].as_str(), &root) {
            (Some(file), Some(root)) => root.join(file),
            (Some(file), None) => PathBuf::from(file),
            (None, Some(root)) => root.join(".zed").join("agda-output.md"),
            (None, None) => std::env::temp_dir().join("agda-output.md"),
        };
        let _ = self.0.output.set(Output::new(output_path));
        let _ = self.0.config.set(Config {
            agda_path,
            extra_args,
            root,
        });

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                        ..TextDocumentSyncOptions::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        COMMAND_GIVE.into(),
                        COMMAND_REFINE.into(),
                        COMMAND_GOAL.into(),
                    ],
                    ..ExecuteCommandOptions::default()
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "agda-bridge".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        if let Some(root) = self.0.root() {
            crate::socket::serve(self.0.clone(), root.to_path_buf());
        }
    }

    async fn shutdown(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        self.0.documents.lock().unwrap().insert(
            path.clone(),
            Document {
                text: params.text_document.text,
                version: params.text_document.version,
                ..Document::default()
            },
        );
        let bridge = self.0.clone();
        tokio::spawn(async move { bridge.report(bridge.load(&path).await).await });
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let mut documents = self.0.documents.lock().unwrap();
        let Some(document) = documents.get_mut(&path) else {
            return;
        };
        // Handlers may run concurrently, so an older version can arrive late.
        if params.text_document.version <= document.version {
            return;
        }
        if let Some(edit) = text::single_change(&document.text, &change.text) {
            goals::adjust(&mut document.goals, &document.text, &edit);
        }
        document.text = change.text;
        document.version = params.text_document.version;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        let bridge = self.0.clone();
        tokio::spawn(async move { bridge.report(bridge.load(&path).await).await });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Some(path) = path_of(&params.text_document.uri) {
            self.0.documents.lock().unwrap().remove(&path);
        }
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let position = params.text_document_position_params;
        let Some(path) = path_of(&position.text_document.uri) else {
            return Ok(None);
        };
        let (goal, range) = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let offset = text::offset_of(&document.text, position.position);
            let Some(goal) = goals::goal_at(&document.goals, offset) else {
                return Ok(None);
            };
            (goal.id, range_of(&document.text, goal.start, goal.end))
        };
        let markdown = self
            .0
            .goal_info(&path, goal, false)
            .await
            .unwrap_or_else(|message| message);
        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(range),
        }))
    }

    async fn code_action(&self, params: CodeActionParams) -> RpcResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let Some(path) = path_of(&uri) else {
            return Ok(None);
        };
        let (id, has_content) = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let offset = text::offset_of(&document.text, params.range.start);
            let Some(goal) = goals::goal_at(&document.goals, offset) else {
                return Ok(None);
            };
            (goal.id, !goal.content(&document.text).is_empty())
        };
        let action = |title: String, command: &str| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: title.clone(),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                command: Some(Command {
                    title,
                    command: command.into(),
                    arguments: Some(vec![json!(uri.as_str()), json!(id)]),
                }),
                ..CodeAction::default()
            })
        };
        let mut actions = Vec::new();
        if has_content {
            actions.push(action(format!("Agda: give ?{id}"), COMMAND_GIVE));
        }
        actions.push(action(format!("Agda: refine ?{id}"), COMMAND_REFINE));
        actions.push(action(
            format!("Agda: show goal ?{id} in output"),
            COMMAND_GOAL,
        ));
        Ok(Some(actions))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> RpcResult<Option<LSPAny>> {
        let uri = params
            .arguments
            .first()
            .and_then(Value::as_str)
            .and_then(|uri| uri.parse::<Uri>().ok());
        let id = params
            .arguments
            .get(1)
            .and_then(Value::as_u64)
            .map(|id| id as u32);
        let (Some(path), Some(id)) = (uri.as_ref().and_then(path_of), id) else {
            return Ok(None);
        };
        // Run in the background, so a long Agda command does not occupy one of
        // the server's request slots while it waits.
        let bridge = self.0.clone();
        tokio::spawn(async move {
            let result = match params.command.as_str() {
                COMMAND_GIVE => bridge.give(&path, id, GoalCommand::Give).await,
                COMMAND_REFINE => bridge.give(&path, id, GoalCommand::Refine).await,
                COMMAND_GOAL => match bridge.goal_info(&path, id, true).await {
                    Ok(markdown) => {
                        bridge
                            .show_output(&format!("Goal ?{id}"), &path, &markdown)
                            .await;
                        Ok(String::new())
                    }
                    Err(message) => Err(message),
                },
                other => Err(format!("Unknown command {other}")),
            };
            bridge.report(result).await;
        });
        Ok(None)
    }
}
