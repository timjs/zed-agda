//! The language server: LSP towards Zed, `--interaction-json` towards Agda.
//!
//! [`Bridge`] holds all state and implements the Agda operations (load, goal
//! information, give, refine). The LSP handlers in [`Backend`] and the debug
//! socket in `socket.rs` both call into it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use tower_lsp_server::jsonrpc::Result as RpcResult;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};

use crate::agda::{Agda, Stop, Stopper};
use crate::goals::{self, GOAL_MARKER, Goal};
use crate::highlight::{self, Span};
use crate::links::{self, Link, Target};
use crate::output::Output;
use crate::protocol::{
    DisplayInfo, GiveResult, GoalInfo, HighlightingEntry, InteractionPoint, MakeCaseVariant,
    Response, Solution, message_text,
};
use crate::settings::Settings;
use crate::text::{self, Change};
use crate::{clause, helper, input, iotcm, location, rename, render};

/// How long a hover or a code action waits for Agda to finish another
/// command.
const BRIEF_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

pub const COMMAND_GIVE: &str = "agda.give";
pub const COMMAND_REFINE: &str = "agda.refine";
pub const COMMAND_GOAL: &str = "agda.goal";
pub const COMMAND_NORMAL_FORM: &str = "agda.normalForm";
pub const COMMAND_CASE_SPLIT: &str = "agda.caseSplit";
pub const COMMAND_AUTO: &str = "agda.auto";
pub const COMMAND_SOLVE: &str = "agda.solve";
pub const COMMAND_SOLVE_ALL: &str = "agda.solveAll";
pub const COMMAND_ADD_WITH: &str = "agda.addWith";
pub const COMMAND_ADD_CLAUSE: &str = "agda.addClause";
pub const COMMAND_MAKE_HELPER: &str = "agda.makeHelper";
pub const COMMAND_OPEN_OUTPUT: &str = "agda.openOutput";

/// The extensions Agda accepts, as listed in its `InvalidExtensionError`.
const AGDA_EXTENSIONS: &[&str] = &[
    ".agda",
    ".lagda",
    ".lagda.rst",
    ".lagda.tex",
    ".lagda.md",
    ".lagda.org",
    ".lagda.tree",
    ".lagda.typ",
];

fn is_agda_source(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    AGDA_EXTENSIONS
        .iter()
        .any(|extension| name.ends_with(extension))
}

pub async fn run() {
    // tower-lsp-server does not handle Zed's cancel of a progress itself yet.
    let (service, socket) = LspService::build(|client| Backend(Arc::new(Bridge::new(client))))
        .custom_method("window/workDoneProgress/cancel", Backend::cancel_progress)
        .finish();
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .concurrency_level(16)
        .serve(service)
        .await;
}

/// What the client said when the bridge started; it does not change.
struct Config {
    root: Option<PathBuf>,
    /// Whether the client accepts `LocationLink`s for go to definition, which
    /// carry the range of the name (Zed underlines it on `cmd`-hover).
    link_support: bool,
}

/// What hover shows about a name, from Agda.
#[derive(Clone, Default)]
struct NameInfo {
    /// Its type; `None` for a name not in scope at the top level, such as one
    /// from a `where` block.
    ty: Option<String>,
    /// How it is in scope, as Agda explains it, with relative paths.
    scope: Option<String>,
}

#[derive(Default)]
struct Document {
    text: String,
    version: i32,
    goals: Vec<Goal>,
    goal_types: HashMap<u32, String>,
    /// Names and their definitions, from the last load's highlighting.
    links: Vec<Link>,
    /// Agda's highlighting from the last load, for semantic tokens.
    spans: Vec<Span>,
    /// Errors and warnings from the last load, as diagnostics.
    problems: Vec<Diagnostic>,
    /// The text Agda last loaded, to skip reloading identical text.
    loaded_text: Option<String>,
    /// Agda's answers about goals since the last load, cleared also when a
    /// goal command fills a goal, which can change the others.
    goal_answers: HashMap<u32, render::GoalAnswer>,
    /// Whether Agda is loading this file again, so that its answers are from
    /// the load before.
    reloading: bool,
    /// What Agda said about names hovered since the last load.
    names: HashMap<String, NameInfo>,
    /// When Agda last checked this text, to tell whether links into another
    /// file are older than that file's last check.
    loaded_at: Option<std::time::Instant>,
}

impl Document {
    /// Replace the chars `start..end` by `replacement` in this copy, moving
    /// goals, links and highlighting along, and return the same edit for
    /// Zed. Changing the copy first makes the `didChange` that Zed sends for
    /// the edit a no-op, instead of racing with this update. With `stretch`,
    /// highlighting of exactly the replaced text covers the replacement.
    fn replace(&mut self, start: usize, end: usize, replacement: &str, stretch: bool) -> TextEdit {
        let edit = TextEdit::new(range_of(&self.text, start, end), replacement.to_string());
        let chars: Vec<char> = self.text.chars().collect();
        let new_text: String = chars[..start]
            .iter()
            .copied()
            .chain(replacement.chars())
            .chain(chars[end..].iter().copied())
            .collect();
        let change = Change {
            start,
            old_end: end,
            new_len: replacement.chars().count(),
        };
        let old_text = std::mem::replace(&mut self.text, new_text);
        goals::adjust(&mut self.goals, &old_text, &change);
        links::adjust(&mut self.links, &change);
        if stretch {
            highlight::adjust_stretching(&mut self.spans, &change);
        } else {
            highlight::adjust(&mut self.spans, &change);
        }
        edit
    }

    /// Replace every goal that is a lone `?` by `{!  !}`, as Agda's Emacs
    /// mode does after loading. The goals keep their numbers, because Agda
    /// knows them by number only.
    fn expand_question_marks(&mut self) -> Vec<TextEdit> {
        let chars: Vec<char> = self.text.chars().collect();
        let mut lone: Vec<Goal> = self
            .goals
            .iter()
            .filter(|goal| goal.end == goal.start + 1 && chars.get(goal.start) == Some(&'?'))
            .cloned()
            .collect();
        // From last to first: each edit then leaves the positions of the
        // ones before it alone, so all edits refer to the text Zed has.
        lone.sort_by_key(|goal| std::cmp::Reverse(goal.start));
        let width = GOAL_MARKER.chars().count();
        let edits = lone
            .into_iter()
            .map(|goal| {
                // An edit that replaces a goal removes it; put it back.
                let edit = self.replace(goal.start, goal.end, GOAL_MARKER, true);
                self.goals.push(Goal {
                    end: goal.start + width,
                    ..goal
                });
                edit
            })
            .collect();
        self.goals.sort_by_key(|goal| goal.start);
        edits
    }
}

struct Session {
    agda: Agda,
    /// Agda keeps one "current file"; goal commands only work on that file.
    current_file: Option<PathBuf>,
    /// Whether Agda made a helper function since it last loaded a file:
    /// Agda 2.8 then gives the next one names of the first (see
    /// [`Bridge::make_helper`]).
    helper_made: bool,
}

/// The goal commands that end in Agda's `GiveAction`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GoalCommand {
    Give,
    Refine,
    /// Proof search, with the goal's text as hints.
    Auto,
}

impl GoalCommand {
    fn title(self) -> &'static str {
        match self {
            GoalCommand::Give => "Give",
            GoalCommand::Refine => "Refine",
            GoalCommand::Auto => "Auto",
        }
    }
}

pub struct Bridge {
    pub client: Client,
    config: OnceLock<Config>,
    /// The user's settings, which may change while the bridge runs.
    settings: Mutex<Settings>,
    output: Mutex<Option<Arc<Output>>>,
    documents: Mutex<HashMap<PathBuf, Document>>,
    session: AsyncMutex<Option<Session>>,
    /// Stops a long command of the running Agda, without its session lock,
    /// which the command holds.
    stopper: Mutex<Option<Stopper>>,
}

/// Everything one Agda command answered, sorted by purpose.
#[derive(Default)]
struct Outcome {
    interaction_points: Option<Vec<InteractionPoint>>,
    goal_types: HashMap<u32, String>,
    /// Whether Agda listed all goals, and so their types; give does, auto
    /// does not.
    goals_listed: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
    give: Option<(u32, GiveResult)>,
    make_case: Option<(u32, MakeCaseVariant, Vec<String>)>,
    solutions: Option<Vec<Solution>>,
    /// What auto said when it found nothing.
    auto: Option<String>,
    /// Whether the command was stopped with `Cmd_abort`.
    aborted: bool,
    /// The type Agda inferred for an expression.
    inferred: Option<String>,
    /// How a name is in scope.
    why: Option<String>,
    goal_info: Option<(u32, GoalInfo)>,
    displays: Vec<Value>,
    highlighting: Vec<HighlightingEntry>,
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
                Response::MakeCase {
                    interaction_point,
                    variant,
                    clauses,
                } => outcome.make_case = Some((interaction_point.id, variant, clauses)),
                Response::SolveAll { solutions } => outcome.solutions = Some(solutions),
                Response::DisplayInfo { info } => {
                    match DisplayInfo::parse(&info) {
                        DisplayInfo::AllGoalsWarnings {
                            visible_goals,
                            warnings,
                            errors,
                            ..
                        } => {
                            outcome.goals_listed = true;
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
                        DisplayInfo::Auto { info } => outcome.auto = Some(info),
                        DisplayInfo::InferredType { expr } => outcome.inferred = Some(expr),
                        DisplayInfo::WhyInScope { message } => outcome.why = Some(message),
                        DisplayInfo::Other => {}
                    }
                    outcome.displays.push(info);
                }
                Response::HighlightingInfo { info } => {
                    outcome
                        .highlighting
                        .extend(info.into_iter().flat_map(|info| info.payload));
                }
                Response::DoneAborting => outcome.aborted = true,
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

/// The variables of a goal's context that a case split may work on: those in
/// scope, whose type is not a function type or a sort (Agda cannot split on
/// those). Whether a type is a data type Agda only says when it splits.
fn split_variables(info: &GoalInfo) -> Vec<String> {
    let GoalInfo::GoalType { entries, .. } = info else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| {
            let ty = entry.binding.trim();
            entry.in_scope
                && entry.reified_name != "_"
                && !ty.contains('→')
                && !ty.contains("->")
                && !["Set", "Prop", "Type"]
                    .iter()
                    .any(|sort| ty.starts_with(sort))
        })
        .map(|entry| entry.reified_name.clone())
        .collect()
}

/// Whether Agda's highlighting from the last load has `name` in the
/// document, defined or used, also qualified (`N.suc` for `suc`).
fn names_known(document: &Document, name: &str) -> bool {
    let chars: Vec<char> = document.text.chars().collect();
    document.links.iter().any(|link| {
        chars.get(link.start..link.end).is_some_and(|occurrence| {
            let occurrence: String = occurrence.iter().collect();
            rename::unqualified(&occurrence) == name
        })
    })
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
            settings: Mutex::new(Settings::default()),
            output: Mutex::new(None),
            documents: Mutex::new(HashMap::new()),
            session: AsyncMutex::new(None),
            stopper: Mutex::new(None),
        }
    }

    fn config(&self) -> &Config {
        self.config.get().expect("initialized before use")
    }

    pub fn root(&self) -> Option<&Path> {
        self.config.get()?.root.as_deref()
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn output(&self) -> Option<Arc<Output>> {
        self.output.lock().unwrap().clone()
    }

    /// Take new settings: report wrong ones, restart Agda at the next
    /// command when its program or arguments changed, and move the output
    /// file when its path changed.
    async fn apply_settings(&self, value: &Value) {
        let (settings, problems) = Settings::read(value);
        for problem in problems {
            self.client
                .show_message(MessageType::WARNING, format!("agda-bridge: {problem}"))
                .await;
        }
        let before = std::mem::replace(&mut *self.settings.lock().unwrap(), settings.clone());
        if settings.restarts_agda(&before) {
            *self.session.lock().await = None;
            // Agda checks again from scratch, with the new program or flags.
            for document in self.documents.lock().unwrap().values_mut() {
                document.loaded_text = None;
            }
        }
        let path = match (&settings.output_file, self.root()) {
            (Some(file), Some(root)) => root.join(file),
            (Some(file), None) => PathBuf::from(file),
            (None, Some(root)) => root.join(".zed").join("agda-output.md"),
            (None, None) => std::env::temp_dir().join("agda-output.md"),
        };
        let mut output = self.output.lock().unwrap();
        if output.as_ref().is_none_or(|output| output.path != path) {
            *output = Some(Arc::new(Output::new(path)));
        }
    }

    /// `path` as seen from the worktree. Agda names files by their real
    /// path, which differs from the worktree's when that has a symbolic link
    /// (on macOS `/var` is `/private/var`); Zed would then open the file
    /// outside the project.
    fn in_worktree(&self, path: &Path) -> PathBuf {
        let Some(root) = self.root() else {
            return path.to_path_buf();
        };
        match root.canonicalize() {
            Ok(real_root) if real_root != root => match path.strip_prefix(&real_root) {
                Ok(rest) => root.join(rest),
                Err(_) => path.to_path_buf(),
            },
            _ => path.to_path_buf(),
        }
    }

    /// Lock the Agda session, starting Agda on first use.
    async fn lock_session(&self) -> Result<MutexGuard<'_, Option<Session>>, String> {
        let mut guard = self.session.lock().await;
        if guard.is_none() {
            let settings = self.settings();
            let agda = Agda::spawn(&settings.agda_path, &settings.extra_args)
                .await
                .map_err(|err| format!("{err}. Set `lsp.agda-bridge.settings.agdaPath` in Zed."))?;
            *self.stopper.lock().unwrap() = Some(agda.stopper());
            *guard = Some(Session {
                agda,
                current_file: None,
                helper_made: false,
            });
        }
        Ok(guard)
    }

    /// Lock the Agda session for a hover or a code action, which should not
    /// wait for a long command, such as a load: wait for it at most
    /// [`BRIEF_WAIT`]. Most commands, such as another hover's, take
    /// milliseconds. `None` when Agda stays busy.
    async fn lock_briefly(&self) -> Option<MutexGuard<'_, Option<Session>>> {
        tokio::time::timeout(BRIEF_WAIT, self.session.lock())
            .await
            .ok()
    }

    /// Run a command that may take long, such as a normal form, with a
    /// progress titled `title` that Zed offers to cancel: in the status bar,
    /// and with `editor: cancel language server work`. Cancelling it stops
    /// the command, which Agda then answers with `DoneAborting`.
    async fn run_long(
        &self,
        guard: &mut MutexGuard<'_, Option<Session>>,
        path: &Path,
        command: &str,
        title: &str,
    ) -> Result<Vec<Response>, String> {
        let Some((key, progress)) = self.begin_stoppable_progress(title).await else {
            return self.run(guard, command).await;
        };
        let stop = Stop {
            key,
            abort: iotcm::abort(path),
        };
        let session = guard.as_mut().expect("session started");
        let result = session.agda.run_stoppable(command, stop).await;
        progress.finish().await;
        result.map_err(|err| {
            **guard = None;
            format!("Agda stopped: {err}")
        })
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
        if !is_agda_source(path) {
            return Err(format!("{} is not an Agda file.", path.display()));
        }
        // Whether Zed has the file open; only then can goals be expanded.
        let (snapshot, opened) = match self.documents.lock().unwrap().get(path) {
            Some(document) => (document.text.clone(), true),
            None => (
                std::fs::read_to_string(path)
                    .map_err(|err| format!("Cannot read {}: {err}", path.display()))?,
                false,
            ),
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
        // After a helper function, load even the same text, to reset Agda.
        let helper_made = guard.as_ref().is_some_and(|session| session.helper_made);
        if unchanged && current && !helper_made {
            return Ok("Already loaded.".into());
        }

        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        self.set_reloading(path, true);
        let command = iotcm::load(path, &self.settings().extra_args);
        let result = self
            .run_long(
                &mut guard,
                path,
                &command,
                &format!("Agda: checking {name}"),
            )
            .await;
        let outcome = match result {
            Ok(responses) => Outcome::collect(responses),
            Err(message) => {
                self.set_reloading(path, false);
                return Err(message);
            }
        };
        if outcome.aborted {
            // A stopped load leaves Agda without a file, and Agda then loads
            // a file first when it gets a goal command for it (checked with
            // Agda 2.8.0.2): no such command until the next load. Goals,
            // diagnostics and answers stay those of the last load.
            if let Some(session) = guard.as_mut() {
                session.current_file = None;
            }
            drop(guard);
            self.set_reloading(path, false);
            let markdown = format!("Stopped checking {name}. Save it to load it again.\n");
            self.show_output("Load", path, &markdown).await;
            return Ok(format!("Stopped checking {name}."));
        }
        if let Some(session) = guard.as_mut() {
            session.current_file = Some(path.to_path_buf());
            session.helper_made = false;
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
        let mut links = links::from_highlighting(&outcome.highlighting, path);
        let mut spans = highlight::from_highlighting(&outcome.highlighting);
        // Agda names the file by its real path, which differs when the path
        // has a symbolic link (on macOS `/tmp` is `/private/tmp`).
        let file = match path.canonicalize() {
            Ok(real)
                if outcome
                    .errors
                    .iter()
                    .chain(&outcome.warnings)
                    .any(|message| message.starts_with(&*real.to_string_lossy())) =>
            {
                real.to_string_lossy().into_owned()
            }
            _ => path.to_string_lossy().into_owned(),
        };
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
        let expansions = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents
                .entry(path.to_path_buf())
                .or_insert_with(|| Document {
                    text: snapshot.clone(),
                    ..Document::default()
                });
            if let Some(change) = text::single_change(&snapshot, &document.text) {
                goals::adjust(&mut goals, &snapshot, &change);
                links::adjust(&mut links, &change);
                highlight::adjust(&mut spans, &change);
            }
            document.goals = goals;
            document.links = links;
            document.spans = spans;
            document.goal_types = outcome.goal_types.clone();
            document.problems = problems;
            document.loaded_text = Some(snapshot);
            document.loaded_at = Some(std::time::Instant::now());
            document.goal_answers.clear();
            document.reloading = false;
            document.names.clear();
            if opened {
                document.expand_question_marks()
            } else {
                Vec::new()
            }
        };
        if !expansions.is_empty()
            && let Err(message) = self.apply_edits(path, expansions).await
        {
            self.report(Err(message)).await;
        }

        self.publish(path).await;
        // Zed asked for tokens when the file opened; ask it to ask again.
        let _ = self.client.semantic_tokens_refresh().await;
        self.show_output("Load", path, &outcome.markdown()).await;
        let goal_count = outcome.goal_types.len();
        Ok(match outcome.errors.len() {
            0 => format!("Loaded {name}: {goal_count} goal(s)."),
            n => format!("Loaded {name}: {n} error(s)."),
        })
    }

    /// Goal type and context, rendered as Markdown, and, when the goal has
    /// text, the type of that text (`Have:`): Agda's answer since the last
    /// load for the goal's current text, or else asked from Agda and kept.
    /// With `wait` false, wait only briefly when Agda is busy (see
    /// [`Bridge::lock_briefly`]), and then show what is known.
    pub async fn goal_info(&self, path: &Path, id: u32, wait: bool) -> Result<String, String> {
        // A goal that is no longer in the buffer is asked for without text.
        let text = self.goal_content(path, id).unwrap_or_default();
        let known = self
            .documents
            .lock()
            .unwrap()
            .get(path)
            .and_then(|document| {
                let answer = document.goal_answers.get(&id)?;
                (!document.reloading && answer.text == text).then(|| answer.markdown(id))
            });
        if let Some(markdown) = known {
            return Ok(markdown);
        }
        let mut guard = if wait {
            self.lock_session().await?
        } else {
            match self.lock_briefly().await {
                Some(guard) => guard,
                None => return Ok(self.goal_while_busy(path, id, &text)),
            }
        };
        Self::check_current(&guard, path)?;
        let request = match text.is_empty() {
            true => iotcm::goal_type_context(path, id),
            false => iotcm::goal_type_context_infer(path, id, &text),
        };
        let mut outcome = Outcome::collect(self.run(&mut guard, &request).await?);
        // For text it cannot type, Agda answers only with the error: ask for
        // the goal alone, and show the error after it.
        let mut untyped = None;
        if outcome.goal_info.is_none() && !text.is_empty() {
            untyped = Some(outcome.errors.join("\n"));
            outcome = Outcome::collect(
                self.run(&mut guard, &iotcm::goal_type_context(path, id))
                    .await?,
            );
        }
        match outcome.goal_info {
            Some((id, info)) => {
                let answer = render::GoalAnswer {
                    text,
                    info,
                    untyped,
                };
                let markdown = answer.markdown(id);
                // Kept before Agda is free for a command that fills a goal,
                // which then clears the answers.
                if let Some(document) = self.documents.lock().unwrap().get_mut(path) {
                    document.goal_answers.insert(id, answer);
                }
                drop(guard);
                Ok(markdown)
            }
            None => Err(outcome.errors.join("\n")),
        }
    }

    /// What hover shows about goal `id`, with `text` in it, while Agda stays
    /// busy (see [`render::goal_while_busy`]).
    fn goal_while_busy(&self, path: &Path, id: u32, text: &str) -> String {
        let documents = self.documents.lock().unwrap();
        match documents.get(path) {
            Some(document) => render::goal_while_busy(
                id,
                document.goal_answers.get(&id),
                document.goal_types.get(&id).map(String::as_str),
                text,
                document.reloading,
            ),
            None => render::BUSY.to_string(),
        }
    }

    fn set_reloading(&self, path: &Path, reloading: bool) {
        if let Some(document) = self.documents.lock().unwrap().get_mut(path) {
            document.reloading = reloading;
        }
    }

    /// The normal form of the text in goal `id`, in the output file.
    pub async fn normal_form(&self, path: &Path, id: u32) -> Result<String, String> {
        let text = self.goal_content(path, id)?;
        if text.is_empty() {
            return Err(format!(
                "Type an expression in goal ?{id} first, then compute its normal form."
            ));
        }
        let mut guard = self.lock_session().await?;
        Self::check_current(&guard, path)?;
        // Computing can take long, unlike the other goal commands.
        let responses = self
            .run_long(
                &mut guard,
                path,
                &iotcm::compute(path, id, &text),
                &format!("Agda: normal form of ?{id}"),
            )
            .await?;
        let outcome = Outcome::collect(responses);
        drop(guard);
        let title = format!("Normal form of ?{id}");
        if outcome.aborted {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            self.show_output(
                &title,
                path,
                &format!("Stopped computing the normal form of `{text}` in ?{id}.\n"),
            )
            .await;
            return Ok(format!("Stopped the normal form of ?{id}."));
        }
        match &outcome.goal_info {
            Some((id, GoalInfo::NormalForm { expr })) => {
                self.show_output(&title, path, &render::normal_form(*id, Some(&text), expr))
                    .await;
                Ok(String::new())
            }
            _ => {
                self.show_output(&title, path, &outcome.markdown()).await;
                Err(outcome
                    .errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "Agda gave no normal form.".into()))
            }
        }
    }

    /// The type of `name` in the scope at the top level of `path`, and why it
    /// is in scope, for hover: from the cache, or asked from Agda when `path`
    /// is its current file, waiting only briefly when Agda is busy (see
    /// [`Bridge::lock_briefly`]); `None` when it was not asked.
    async fn name_info(&self, path: &Path, name: &str) -> Option<NameInfo> {
        if let Some(known) = self
            .documents
            .lock()
            .unwrap()
            .get(path)
            .and_then(|document| document.names.get(name).cloned())
        {
            return Some(known);
        }
        let mut guard = self.lock_briefly().await?;
        Self::check_current(&guard, path).ok()?;
        let ty = Outcome::collect(
            self.run(&mut guard, &iotcm::infer_toplevel(path, name))
                .await
                .ok()?,
        )
        .inferred;
        // A name without a type is not in scope: nothing to explain.
        let scope = match ty {
            Some(_) => Outcome::collect(
                self.run(&mut guard, &iotcm::why_in_scope_toplevel(path, name))
                    .await
                    .ok()?,
            )
            .why
            .map(|message| render::why_in_scope(&self.relative_paths(&message))),
            None => None,
        };
        drop(guard);
        let info = NameInfo { ty, scope };
        if let Some(document) = self.documents.lock().unwrap().get_mut(path) {
            document.names.insert(name.to_string(), info.clone());
        }
        Some(info)
    }

    /// The variables of goal `id` to offer for a case split: from Agda's
    /// answer about the goal, asked when there is none yet, waiting only
    /// briefly when Agda is busy.
    async fn goal_split_variables(&self, path: &Path, id: u32) -> Vec<String> {
        let known = || {
            self.documents
                .lock()
                .unwrap()
                .get(path)
                .and_then(|document| document.goal_answers.get(&id))
                .map(|answer| split_variables(&answer.info))
        };
        if let Some(variables) = known() {
            return variables;
        }
        let _ = self.goal_info(path, id, false).await;
        known().unwrap_or_default()
    }

    /// The text typed in a goal, trimmed.
    fn goal_content(&self, path: &Path, id: u32) -> Result<String, String> {
        let documents = self.documents.lock().unwrap();
        let document = documents.get(path).ok_or("This file is not open.")?;
        let goal = document
            .goals
            .iter()
            .find(|goal| goal.id == id)
            .ok_or(format!(
                "Goal ?{id} no longer exists. Save the file to reload it."
            ))?;
        Ok(goal.content(&document.text))
    }

    /// Give, refine or auto a goal, with the expression typed in it (for
    /// auto, hints), and apply Agda's result to the buffer with
    /// `workspace/applyEdit`.
    pub async fn give(&self, path: &Path, id: u32, command: GoalCommand) -> Result<String, String> {
        let expression = self.goal_content(path, id)?;
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
            GoalCommand::Auto => iotcm::auto_one(path, id, &expression),
        };
        // Auto can take long, when the goal's text sets a longer time limit.
        let responses = match command {
            GoalCommand::Auto => {
                let title = format!("Agda: auto on ?{id}");
                self.run_long(&mut guard, path, &request, &title).await?
            }
            _ => self.run(&mut guard, &request).await?,
        };
        let mut outcome = Outcome::collect(responses);
        // Auto sends no new list of goals, though filling one goal can change
        // the types of others: ask for it, as Emacs's "show goals" does.
        if outcome.give.is_some() && !outcome.goals_listed {
            let goals = Outcome::collect(self.run(&mut guard, &iotcm::metas(path)).await?);
            if goals.goals_listed {
                outcome.goal_types = goals.goal_types;
                outcome.goals_listed = true;
            }
        }
        drop(guard);

        let title = command.title();
        if outcome.aborted {
            self.show_output(
                &format!("{title} ?{id}"),
                path,
                &format!("Stopped {} on ?{id}.\n", title.to_lowercase()),
            )
            .await;
            return Ok(format!("Stopped {} on goal ?{id}.", title.to_lowercase()));
        }
        if outcome.give.is_none() {
            self.show_output(&format!("{title} ?{id}"), path, &outcome.markdown())
                .await;
            return Err(outcome
                .auto
                .clone()
                .or_else(|| outcome.errors.first().cloned())
                .unwrap_or_else(|| "Agda did not fill the goal.".into()));
        }
        let given = self.apply_give(path, &outcome).await?;
        self.publish(path).await;
        self.show_output(&format!("{title} ?{given}"), path, &outcome.markdown())
            .await;
        Ok(format!("Filled goal ?{given}."))
    }

    /// Apply the `GiveAction` of `outcome` to the buffer: the goal becomes
    /// Agda's text, and goals in that text get the numbers Agda gave them.
    /// Returns the number of the filled goal.
    async fn apply_give(&self, path: &Path, outcome: &Outcome) -> Result<u32, String> {
        let (given, result) = outcome.give.clone().ok_or("Agda did not fill the goal.")?;
        let edit = {
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
            let edit = document.replace(goal.start, goal.end, &replacement, false);

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
            if outcome.goals_listed {
                document.goal_types = outcome.goal_types.clone();
            } else {
                document.goal_types.remove(&given);
            }
            // Filling a goal can change the types in others.
            document.goal_answers.clear();
            edit
        };
        self.apply_edits(path, vec![edit]).await?;
        Ok(given)
    }

    /// Ask Zed to apply `edits` to the file, all against the same text.
    async fn apply_edits(&self, path: &Path, edits: Vec<TextEdit>) -> Result<(), String> {
        let uri = uri_of(path).ok_or("Invalid file path.")?;
        let edit = WorkspaceEdit {
            changes: Some(HashMap::from([(uri, edits)])),
            ..WorkspaceEdit::default()
        };
        match self.client.apply_edit(edit).await {
            Ok(response) if response.applied => Ok(()),
            Ok(response) => Err(format!(
                "Zed did not apply the edit: {}",
                response.failure_reason.unwrap_or_default()
            )),
            Err(err) => Err(format!("Zed did not apply the edit: {err}")),
        }
    }

    /// Case split on the variables typed in a goal, or, with none, introduce
    /// the missing patterns or split on the result. Agda's new clauses
    /// replace the goal's clause; their goals get numbers when the file is
    /// saved and loaded again (Emacs saves and reloads by itself).
    pub async fn case_split(
        &self,
        path: &Path,
        id: u32,
        variable: Option<String>,
    ) -> Result<String, String> {
        let variables = match variable {
            Some(variable) => variable,
            None => self.goal_content(path, id)?,
        };
        let mut guard = self.lock_session().await?;
        Self::check_current(&guard, path)?;
        let outcome = Outcome::collect(
            self.run(&mut guard, &iotcm::make_case(path, id, &variables))
                .await?,
        );
        drop(guard);

        let Some((split, variant, clauses)) = outcome.make_case.clone() else {
            self.show_output(&format!("Case split ?{id}"), path, &outcome.markdown())
                .await;
            return Err(outcome
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| "Agda did not split the goal.".into()));
        };
        let edit = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents.get_mut(path).ok_or("This file was closed.")?;
            let goal = document
                .goals
                .iter()
                .find(|goal| goal.id == split)
                .cloned()
                .ok_or(format!("Goal ?{split} disappeared while Agda was working."))?;
            let (start, end, replacement) =
                goals::case_split(&document.text, &goal, variant, &clauses);
            document.replace(start, end, &replacement, false)
        };
        self.apply_edits(path, vec![edit]).await?;
        self.publish(path).await;
        let markdown = format!(
            "Split goal ?{split} into {} clauses. Save the file to load them, so \
             that their goals get numbers.\n",
            clauses.len()
        );
        self.show_output(&format!("Case split ?{split}"), path, &markdown)
            .await;
        Ok(format!(
            "Split goal ?{split}. Save the file to load the new clauses."
        ))
    }

    /// Add a with-abstraction on a goal, as Idris's "add with" does; Agda has
    /// no command for it, so the bridge rewrites the clause itself. The new
    /// goals get numbers when the file is saved and loaded again.
    pub async fn add_with(&self, path: &Path, id: u32) -> Result<String, String> {
        let edit = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents.get_mut(path).ok_or("This file is not open.")?;
            let goal = document
                .goals
                .iter()
                .find(|goal| goal.id == id)
                .cloned()
                .ok_or(format!(
                    "Goal ?{id} no longer exists. Save the file to reload it."
                ))?;
            let (start, end, replacement) = goals::add_with(&document.text, &goal).ok_or(
                "A with-abstraction needs a goal that is the whole right-hand side of a clause.",
            )?;
            document.replace(start, end, &replacement, false)
        };
        self.apply_edits(path, vec![edit]).await?;
        self.publish(path).await;
        self.show_output(
            &format!("With abstraction ?{id}"),
            path,
            "Added a with-abstraction. Save the file to load it, so that its goals get numbers.\n",
        )
        .await;
        Ok(format!("Added a with-abstraction on goal ?{id}."))
    }

    /// `text` with the paths of files in the worktree made relative to it,
    /// as Agda writes them in full, and by their real path.
    fn relative_paths(&self, text: &str) -> String {
        let Some(root) = self.root() else {
            return text.to_string();
        };
        let mut text = text.to_string();
        for root in [root.canonicalize().ok(), Some(root.to_path_buf())]
            .into_iter()
            .flatten()
        {
            text = text.replace(&format!("{}/", root.display()), "");
        }
        text
    }

    /// Add a clause for the type signature on `line`, as Idris's "add clause"
    /// does, right after the signature. Agda has no command for it; the
    /// clause comes from the signature's text (see `clause.rs`), and its goal
    /// gets a number when the file is saved and loaded again.
    pub async fn add_clause(&self, path: &Path, line: usize) -> Result<String, String> {
        let edit = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents.get_mut(path).ok_or("This file is not open.")?;
            let signature = clause::signature_at(&document.text, line)
                .ok_or("There is no type signature on this line to add a clause for.")?;
            // The end of the signature's last line, in chars.
            let end: usize = document
                .text
                .split('\n')
                .take(signature.last_line + 1)
                .map(|line| line.chars().count() + 1)
                .sum::<usize>()
                - 1;
            let last = document
                .text
                .split('\n')
                .nth(signature.last_line)
                .unwrap_or("");
            let end = end - usize::from(last.ends_with('\r'));
            let clauses = format!("\n{}", clause::clauses(&signature));
            document.replace(end, end, &clauses, false)
        };
        self.apply_edits(path, vec![edit]).await?;
        self.publish(path).await;
        Ok("Added a clause. Save the file to load it.".into())
    }

    /// Make a helper function for goal `id`, as Idris's "make lemma" does:
    /// the goal's text is the call (`aux n m`). Agda gives the helper's type
    /// (Emacs's `C-c C-h` shows it); the bridge adds that signature, and a
    /// clause for it, above the definition the goal is in (see `helper.rs`),
    /// and puts the call in place of the goal.
    pub async fn make_helper(&self, path: &Path, id: u32) -> Result<String, String> {
        let text = self.goal_content(path, id)?;
        if helper::head(&text).is_none() {
            return Err(format!(
                "Type a call of the helper function in goal ?{id} first, such as `aux n m`."
            ));
        }
        let mut guard = self.lock_session().await?;
        Self::check_current(&guard, path)?;
        // Agda 2.8 names a second helper function, and the variables in its
        // type, after the first one, until it loads the file again.
        if guard.as_ref().is_some_and(|session| session.helper_made) {
            return Err(
                "Save the file first, so that Agda loads it again: until then, \
                 Agda gives a second helper function the names of the first."
                    .into(),
            );
        }
        let outcome = Outcome::collect(
            self.run(&mut guard, &iotcm::helper_function(path, id, &text))
                .await?,
        );
        let Some((_, GoalInfo::HelperFunction { signature })) = outcome.goal_info.clone() else {
            drop(guard);
            self.show_output(&format!("Helper function ?{id}"), path, &outcome.markdown())
                .await;
            return Err(outcome
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| "Agda gave no helper function.".into()));
        };
        if let Some(session) = guard.as_mut() {
            session.helper_made = true;
        }
        drop(guard);

        // Agda's name, which differs from the typed one when that is taken.
        let name = signature
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        let call = helper::call(&text, &name).unwrap_or_else(|| text.clone());
        let edits = {
            let mut documents = self.documents.lock().unwrap();
            let document = documents.get_mut(path).ok_or("This file was closed.")?;
            let goal = document
                .goals
                .iter()
                .find(|goal| goal.id == id)
                .cloned()
                .ok_or(format!("Goal ?{id} disappeared while Agda was working."))?;
            // In parentheses, unless the call is a lone name or the whole
            // right-hand side of a clause.
            let call = match call == name || goals::add_with(&document.text, &goal).is_some() {
                true => call,
                false => format!("({call})"),
            };
            let line = document
                .text
                .chars()
                .take(goal.start)
                .filter(|c| *c == '\n')
                .count();
            let (at, indent) = helper::place(&document.text, line);
            let start: usize = document
                .text
                .split('\n')
                .take(at)
                .map(|line| line.chars().count() + 1)
                .sum();
            // The call first: it comes after the declaration, so both edits
            // refer to the text Zed has.
            let call_edit = document.replace(goal.start, goal.end, &call, false);
            let declaration = helper::declaration(&signature, &indent);
            let declaration_edit = document.replace(start, start, &declaration, false);
            vec![declaration_edit, call_edit]
        };
        self.apply_edits(path, edits).await?;
        self.publish(path).await;
        let markdown = format!(
            "Added `{name}` above the definition of goal ?{id}, and called it there:\n\n\
             ```agda\n{signature}\n```\n\nSave the file to load it, so that its goal gets a number.\n"
        );
        self.show_output(&format!("Helper function {name}"), path, &markdown)
            .await;
        Ok(format!(
            "Added the helper function {name}. Save the file to load it."
        ))
    }

    /// Fill the goals that unification already solved, as Emacs does: Agda
    /// names a solution for each, which is then given. Only goal `id`, or
    /// all goals.
    pub async fn solve(&self, path: &Path, id: Option<u32>) -> Result<String, String> {
        let title = match id {
            Some(id) => format!("Solve ?{id}"),
            None => "Solve all goals".to_string(),
        };
        let mut guard = self.lock_session().await?;
        Self::check_current(&guard, path)?;
        let request = match id {
            Some(id) => iotcm::solve_one(path, id),
            None => iotcm::solve_all(path),
        };
        let outcome = Outcome::collect(self.run(&mut guard, &request).await?);
        let solutions = outcome.solutions.clone().unwrap_or_default();
        if solutions.is_empty() {
            drop(guard);
            if let Some(error) = outcome.errors.first() {
                self.show_output(&title, path, &outcome.markdown()).await;
                return Err(error.clone());
            }
            return Err(match id {
                Some(id) => format!("Agda has no solution for goal ?{id} yet."),
                None => "Agda has no solution for any goal yet.".into(),
            });
        }

        // Keep Agda to this file until every solution is in the buffer, so a
        // load in between cannot renumber the goals.
        let mut filled = Vec::new();
        let mut markdown = String::new();
        for solution in solutions {
            let given = Outcome::collect(
                self.run(
                    &mut guard,
                    &iotcm::give(path, solution.interaction_point, &solution.expression),
                )
                .await?,
            );
            markdown = given.markdown();
            if given.give.is_some() {
                filled.push(self.apply_give(path, &given).await?);
            }
        }
        drop(guard);
        self.publish(path).await;
        self.show_output(&title, path, &markdown).await;
        let filled: Vec<String> = filled.iter().map(|id| format!("?{id}")).collect();
        Ok(format!("Solved {}.", filled.join(", ")))
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

    /// Write the output file. Zed is asked to open it only after the first
    /// write: the bridge cannot tell whether the user closed it since, and
    /// asking again would open it in the pane being edited (see
    /// [`Bridge::open_output`]).
    pub async fn show_output(&self, title: &str, source: &Path, body: &str) {
        let Some(output) = self.output() else {
            return;
        };
        match output.write(title, Some(source), body).await {
            Ok(true) => self.open_output().await,
            Ok(false) => {}
            Err(err) => eprintln!("agda-bridge: cannot write {}: {err}", output.path.display()),
        }
    }

    /// Ask Zed to open the output file, without taking focus. Zed opens it in
    /// the active pane, unless the user enabled `reveal_if_open`, in which
    /// case an open copy in another pane is revealed instead.
    pub async fn open_output(&self) {
        let Some(output) = self.output() else {
            return;
        };
        if let Err(err) = output.ensure_exists().await {
            eprintln!("agda-bridge: cannot write {}: {err}", output.path.display());
            return;
        }
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

    /// A progress that Zed offers to cancel, with its token as a string.
    /// Everything is in the title: Zed shows "title: message" in the status
    /// bar, but only "Cancel title" in its menu.
    async fn begin_stoppable_progress(
        &self,
        title: &str,
    ) -> Option<(
        String,
        tower_lsp_server::OngoingProgress<
            tower_lsp_server::Unbounded,
            tower_lsp_server::Cancellable,
        >,
    )> {
        let key = format!("agda-bridge/{}", self.client.next_request_id());
        let token = ProgressToken::String(key.clone());
        self.client
            .create_work_done_progress(token.clone())
            .await
            .ok()?;
        let progress = self
            .client
            .progress(token, title)
            .with_cancel_button()
            .begin()
            .await;
        Some((key, progress))
    }

    /// Where the name of `link` in the document at `path` is defined: a file
    /// as seen from the worktree, and a 0-based char offset in it.
    fn definition_site(&self, path: &Path, link: &Link) -> (PathBuf, usize) {
        match &link.target {
            Target::Here(offset) => (path.to_path_buf(), *offset),
            Target::File { path, position } => (self.in_worktree(path), position.saturating_sub(1)),
        }
    }

    /// Hover outside goals, in this order: the type of the name under the
    /// cursor (given as its range, its text and its definition site), how to
    /// type the symbol under it (given as its range and the Markdown), and
    /// why the name is in scope.
    async fn hover_outside_goals(
        &self,
        path: &Path,
        name: Option<(Range, String, (PathBuf, usize))>,
        typing: Option<(Range, String)>,
    ) -> Option<Hover> {
        let mut parts = Vec::new();
        let mut range = None;
        let mut scope = None;
        if let Some((name_range, occurrence, (site_path, site))) = name {
            let expression = self.expression_for(&occurrence, &site_path, site);
            if let Some(NameInfo {
                ty: Some(ty),
                scope: why,
            }) = self.name_info(path, &expression).await
            {
                parts.push(format!("```agda\n{expression} : {ty}\n```"));
                range = Some(name_range);
                scope = why;
            }
        }
        if let Some((symbol_range, how)) = typing {
            parts.push(how);
            range = range.or(Some(symbol_range));
        }
        if let Some(why) = scope {
            parts.push(format!("```text\n{why}\n```"));
        }
        if parts.is_empty() {
            return None;
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: parts.join("\n\n"),
            }),
            range,
        })
    }

    /// The expression to ask Agda the type of, for a name written as
    /// `occurrence` and defined at `site` in `site_path`: the name as written,
    /// or, for a part of an operator such as `+`, the whole operator `_+_`
    /// with the qualifier written before the part.
    fn expression_for(&self, occurrence: &str, site_path: &Path, site: usize) -> String {
        let defined = {
            let documents = self.documents.lock().unwrap();
            match documents.get(site_path) {
                Some(document) => Some(document.text.clone()),
                None => std::fs::read_to_string(site_path).ok(),
            }
        }
        .map(|text| {
            text.chars()
                .skip(site)
                .take_while(|c| !c.is_whitespace() && !"(){}\";.@".contains(*c))
                .collect::<String>()
        })
        .unwrap_or_default();
        let base = rename::unqualified(occurrence);
        let qualifier = &occurrence[..occurrence.len() - base.len()];
        let is_part = defined.contains('_') && defined.split('_').any(|part| part == base);
        match is_part && base != defined {
            true => format!("{qualifier}{defined}"),
            false => occurrence.to_string(),
        }
    }

    /// The edits that rename the name at `position` in `path` to `new`, in
    /// every open document, and a summary for the user.
    fn rename_edits(
        &self,
        path: &Path,
        position: Position,
        new: &str,
    ) -> Result<(HashMap<Uri, Vec<TextEdit>>, String), String> {
        let documents = self.documents.lock().unwrap();
        let document = documents.get(path).ok_or("This file is not open.")?;
        let offset = text::offset_of(&document.text, position);
        let link = links::link_at(&document.links, offset)
            .ok_or("Agda knows no name here. Save the file to check new names first.")?;
        let chars = |text: &str, start: usize, end: usize| -> String {
            text.chars().skip(start).take(end - start).collect()
        };
        let occurrence = chars(&document.text, link.start, link.end);
        let (site_path, site) = self.definition_site(path, link);
        let same = |a: &Path, b: &Path| a == b || a.canonicalize().ok() == b.canonicalize().ok();
        let (defining_path, defining) = documents
            .iter()
            .find(|(path, _)| same(path, &site_path))
            .ok_or(format!(
                "`{occurrence}` is defined in {}, which is not open in Zed. Open it to rename the name.",
                site_path.display()
            ))?;
        if defining.loaded_text.as_ref() != Some(&defining.text) {
            return Err(format!(
                "Save {} first, so that Agda has checked it as it is.",
                defining_path.display()
            ));
        }
        let old = match defining.links.iter().find(|link| link.start == site) {
            Some(link) => chars(&defining.text, link.start, link.end),
            None => defining
                .text
                .chars()
                .skip(site)
                .take_while(|c| !c.is_whitespace() && !"(){}\";.@".contains(*c))
                .collect(),
        };
        let old = rename::unqualified(&old).to_string();
        let new = rename::new_name(&old, new, &occurrence)?;

        let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
        let mut places = 0;
        let mut site_renamed = false;
        for (doc_path, doc) in documents.iter() {
            let mut edits = Vec::new();
            for link in &doc.links {
                let (target_path, target) = self.definition_site(doc_path, link);
                if target != site || !same(&target_path, defining_path) {
                    continue;
                }
                // Links into another file are as old as this file's check.
                if !same(doc_path, defining_path) && doc.loaded_at < defining.loaded_at {
                    return Err(format!(
                        "Save {} first, so that Agda checks it against the current {}.",
                        doc_path.display(),
                        defining_path.display()
                    ));
                }
                let text = chars(&doc.text, link.start, link.end);
                if let Some(replacement) = rename::replacement(&old, &new, &text) {
                    site_renamed |= same(doc_path, defining_path) && link.start == site;
                    edits.push(TextEdit::new(
                        range_of(&doc.text, link.start, link.end),
                        replacement,
                    ));
                }
            }
            if same(doc_path, defining_path) && !site_renamed {
                let end = site + old.chars().count();
                edits.push(TextEdit::new(range_of(&doc.text, site, end), new.clone()));
                site_renamed = true;
            }
            if !edits.is_empty() {
                places += edits.len();
                let uri = uri_of(doc_path).ok_or("Invalid file path.")?;
                changes.insert(uri, edits);
            }
        }

        let mut summary = format!(
            "Renamed `{old}` to `{new}` in {places} place(s) in {} open file(s).",
            changes.len()
        );
        if !link.local {
            let others = self.files_that_may_use(&documents, defining_path, &old);
            if !others.is_empty() {
                summary.push_str(&format!(
                    " Files that are not open are not changed; these may use it: {}.",
                    others.join(", ")
                ));
            }
        }
        Ok((changes, summary))
    }

    /// The Agda files of the worktree that are not open, but import the
    /// module of `defining` and mention a part of `name`: those a rename in
    /// the open files may miss.
    fn files_that_may_use(
        &self,
        documents: &HashMap<PathBuf, Document>,
        defining: &Path,
        name: &str,
    ) -> Vec<String> {
        let (Some(root), Some(module)) = (self.root(), defining.file_stem()) else {
            return Vec::new();
        };
        let module = module.to_string_lossy();
        let parts: Vec<&str> = name.split('_').filter(|part| !part.is_empty()).collect();
        let mut found = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') || file_name == "_build" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if is_agda_source(&path)
                    && !documents.contains_key(&path)
                    && let Ok(text) = std::fs::read_to_string(&path)
                    && text
                        .lines()
                        .any(|line| line.contains("import") && line.contains(&*module))
                    && parts.iter().any(|part| text.contains(part))
                {
                    let shown = path.strip_prefix(root).unwrap_or(&path);
                    found.push(shown.display().to_string());
                }
            }
        }
        found.sort();
        found
    }

    /// The answer to go to definition: a link from the name at `origin` to
    /// `at` in `uri`, or only the target for clients without link support.
    fn definition(&self, origin: Range, uri: Uri, at: Position) -> GotoDefinitionResponse {
        let target = Range::new(at, at);
        if self.config().link_support {
            GotoDefinitionResponse::Link(vec![LocationLink {
                origin_selection_range: Some(origin),
                target_uri: uri,
                target_range: target,
                target_selection_range: target,
            }])
        } else {
            GotoDefinitionResponse::Scalar(Location::new(uri, target))
        }
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

impl Backend {
    /// Zed cancels a progress the bridge offered to cancel: stop the command
    /// it is for, if Agda still works on it.
    async fn cancel_progress(&self, params: WorkDoneProgressCancelParams) {
        let key = match params.token {
            ProgressToken::String(key) => key,
            ProgressToken::Number(key) => key.to_string(),
        };
        let stopper = self.0.stopper.lock().unwrap().clone();
        if let Some(stopper) = stopper
            && let Err(err) = stopper.stop(&key).await
        {
            eprintln!("agda-bridge: cannot stop Agda: {err}");
        }
    }
}

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
        let link_support = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.definition.as_ref())
            .and_then(|definition| definition.link_support)
            .unwrap_or(false);
        let _ = self.0.config.set(Config { root, link_support });
        self.0.apply_settings(&options).await;

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
                definition_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                })),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: highlight::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
                // Always offered, as symbol input can be switched on later.
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(input::trigger_characters()),
                    ..CompletionOptions::default()
                }),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        COMMAND_GIVE.into(),
                        COMMAND_REFINE.into(),
                        COMMAND_GOAL.into(),
                        COMMAND_NORMAL_FORM.into(),
                        COMMAND_CASE_SPLIT.into(),
                        COMMAND_AUTO.into(),
                        COMMAND_SOLVE.into(),
                        COMMAND_SOLVE_ALL.into(),
                        COMMAND_ADD_WITH.into(),
                        COMMAND_ADD_CLAUSE.into(),
                        COMMAND_MAKE_HELPER.into(),
                        COMMAND_OPEN_OUTPUT.into(),
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

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        self.0.apply_settings(&params.settings).await;
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
            links::adjust(&mut document.links, &edit);
            highlight::adjust(&mut document.spans, &edit);
        }
        document.text = change.text;
        document.version = params.text_document.version;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let Some(path) = path_of(&params.text_document.uri) else {
            return;
        };
        // Zed sends `didSave` to every language server of the worktree, for
        // every saved file whatever its language. Only load the documents Zed
        // opened with this server, which are Agda files.
        if !self.0.documents.lock().unwrap().contains_key(&path) {
            return;
        }
        let bridge = self.0.clone();
        tokio::spawn(async move { bridge.report(bridge.load(&path).await).await });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Some(path) = path_of(&params.text_document.uri) {
            self.0.documents.lock().unwrap().remove(&path);
        }
    }

    async fn completion(&self, params: CompletionParams) -> RpcResult<Option<CompletionResponse>> {
        let request = params.text_document_position;
        let Some(path) = path_of(&request.text_document.uri) else {
            return Ok(None);
        };
        let position = request.position;
        let symbols = self.0.settings().symbols;
        if !symbols.enabled() {
            return Ok(None);
        }
        let found = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let Some(line) = document.text.split('\n').nth(position.line as usize) else {
                return Ok(None);
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            input::complete(line, position.character, &symbols)
        };
        let Some(found) = found else {
            return Ok(None);
        };
        let range = Range::new(Position::new(position.line, found.start), position);
        let items = found
            .candidates
            .iter()
            .enumerate()
            .map(|(rank, candidate)| {
                let code_points: Vec<String> = candidate
                    .symbol
                    .chars()
                    .map(|c| format!("U+{:04X}", c as u32))
                    .collect();
                CompletionItem {
                    // Zed shows the label and then the detail: `→ \to`.
                    label: candidate.symbol.clone(),
                    detail: Some(candidate.name.clone()),
                    documentation: Some(Documentation::String(code_points.join(" "))),
                    // Zed filters on the word before the cursor, which never
                    // includes the leader.
                    filter_text: Some(candidate.name[1..].to_string()),
                    sort_text: Some(format!("{rank:04}")),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                        range,
                        found.insertion(candidate),
                    ))),
                    ..CompletionItem::default()
                }
            })
            .collect();
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: found.truncated,
            items,
        })))
    }

    async fn hover(&self, params: HoverParams) -> RpcResult<Option<Hover>> {
        let position = params.text_document_position_params;
        let Some(path) = path_of(&position.text_document.uri) else {
            return Ok(None);
        };
        // On a goal: the goal. Elsewhere: the type of the name, and how to
        // type the symbol under the cursor.
        let found = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            // The goal under the mouse, also on the blank inside an empty
            // goal. Otherwise goal, name and symbol all for the same hovered
            // char, which may be the one before the mouse.
            let at = text::offset_of(&document.text, position.position);
            let hovered = text::hovered(&document.text, at);
            let goal = goals::goal_under(&document.goals, at)
                .or_else(|| hovered.and_then(|offset| goals::goal_under(&document.goals, offset)));
            match (goal, hovered) {
                (Some(goal), _) => Ok((goal.id, range_of(&document.text, goal.start, goal.end))),
                (None, None) => return Ok(None),
                (None, Some(offset)) => {
                    let symbols = self.0.settings().symbols;
                    let typing = input::symbol_at(&document.text, offset).and_then(
                        |(start, end, symbol)| {
                            let markdown = input::how_to_type(&symbol, &symbols)?;
                            Some((range_of(&document.text, start, end), markdown))
                        },
                    );
                    let name = links::link_under(&document.links, offset)
                        .filter(|link| !link.local)
                        .map(|link| {
                            let occurrence: String = document
                                .text
                                .chars()
                                .skip(link.start)
                                .take(link.end - link.start)
                                .collect();
                            let range = range_of(&document.text, link.start, link.end);
                            (range, occurrence, self.0.definition_site(&path, link))
                        });
                    Err((name, typing))
                }
            }
        };
        let (goal, range) = match found {
            Ok(goal) => goal,
            Err((name, typing)) => {
                return Ok(self.0.hover_outside_goals(&path, name, typing).await);
            }
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

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> RpcResult<Option<PrepareRenameResponse>> {
        let Some(path) = path_of(&params.text_document.uri) else {
            return Ok(None);
        };
        let documents = self.0.documents.lock().unwrap();
        let Some(document) = documents.get(&path) else {
            return Ok(None);
        };
        let offset = text::offset_of(&document.text, params.position);
        Ok(links::link_at(&document.links, offset).map(|link| {
            PrepareRenameResponse::Range(range_of(&document.text, link.start, link.end))
        }))
    }

    async fn rename(&self, params: RenameParams) -> RpcResult<Option<WorkspaceEdit>> {
        let request = params.text_document_position;
        let Some(path) = path_of(&request.text_document.uri) else {
            return Ok(None);
        };
        match self
            .0
            .rename_edits(&path, request.position, &params.new_name)
        {
            Ok((changes, summary)) => {
                self.0.client.show_message(MessageType::INFO, summary).await;
                Ok(Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                }))
            }
            Err(message) => Err(tower_lsp_server::jsonrpc::Error {
                code: tower_lsp_server::jsonrpc::ErrorCode::InvalidParams,
                message: message.into(),
                data: None,
            }),
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> RpcResult<Option<GotoDefinitionResponse>> {
        let request = params.text_document_position_params;
        let Some(path) = path_of(&request.text_document.uri) else {
            return Ok(None);
        };
        let (origin, target_path, target_position) = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let offset = text::offset_of(&document.text, request.position);
            let Some(link) = links::link_at(&document.links, offset) else {
                return Ok(None);
            };
            // The whole name as Agda sees it: `~>*step` is one name, though
            // Zed's own word boundaries would split it in two.
            let origin = range_of(&document.text, link.start, link.end);
            match &link.target {
                Target::Here(target) => {
                    let at = text::position_of(&document.text, *target);
                    return Ok(Some(self.0.definition(
                        origin,
                        request.text_document.uri,
                        at,
                    )));
                }
                Target::File { path, position } => (origin, self.0.in_worktree(path), *position),
            }
        };
        // Agda's offsets refer to the file as it was on disk when it was
        // loaded, so read it from disk rather than from an open buffer.
        let (Ok(contents), Some(uri)) = (
            tokio::fs::read_to_string(&target_path).await,
            uri_of(&target_path),
        ) else {
            return Ok(None);
        };
        let at = text::position_of(&contents, target_position.saturating_sub(1));
        Ok(Some(self.0.definition(origin, uri, at)))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> RpcResult<Option<SemanticTokensResult>> {
        let Some(path) = path_of(&params.text_document.uri) else {
            return Ok(None);
        };
        let documents = self.0.documents.lock().unwrap();
        let Some(document) = documents.get(&path) else {
            return Ok(None);
        };
        let data = highlight::tokens(&document.spans, &document.text);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn code_action(&self, params: CodeActionParams) -> RpcResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let Some(path) = path_of(&uri) else {
            return Ok(None);
        };
        let (goal, on_problem, signature) = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let offset = text::offset_of(&document.text, params.range.start);
            let goal = goals::goal_at(&document.goals, offset).map(|goal| {
                let with = goals::add_with(&document.text, goal).is_some();
                let content = goal.content(&document.text);
                // Not for a name Agda saw in the file, as in `suc n`.
                let helper = helper::head(&content)
                    .filter(|name| !names_known(document, name))
                    .map(String::from);
                (goal.id, content, with, helper)
            });
            let line = params.range.start.line;
            let on_problem = document
                .problems
                .iter()
                .any(|problem| problem.range.start.line <= line && line <= problem.range.end.line);
            let signature = clause::signature_at(&document.text, line as usize).is_some();
            (goal, on_problem, signature)
        };
        let action = |title: String, command: &str, arguments: Vec<Value>, kind| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: title.clone(),
                kind,
                command: Some(Command {
                    title,
                    command: command.into(),
                    arguments: Some(arguments),
                }),
                ..CodeAction::default()
            })
        };
        let mut actions = Vec::new();
        if signature {
            let line = params.range.start.line;
            actions.push(action(
                "Make clause".into(),
                COMMAND_ADD_CLAUSE,
                vec![json!(uri.as_str()), json!(line)],
                Some(CodeActionKind::REFACTOR_REWRITE),
            ));
        }
        if let Some((id, content, with, helper)) = &goal {
            let id = *id;
            let rewrite = Some(CodeActionKind::REFACTOR_REWRITE);
            let goal_action = |title: &str, command: &str| {
                let arguments = vec![json!(uri.as_str()), json!(id)];
                action(title.to_string(), command, arguments, rewrite.clone())
            };
            if !content.is_empty() {
                actions.push(goal_action("Give", COMMAND_GIVE));
            }
            actions.push(goal_action("Refine", COMMAND_REFINE));
            // With variables typed in the goal, split on those; otherwise
            // offer each variable of the goal's context, and the split on
            // the result (Agda's name for a split without variables).
            if content.is_empty() {
                for variable in self.0.goal_split_variables(&path, id).await {
                    actions.push(action(
                        format!("Case split on `{variable}`"),
                        COMMAND_CASE_SPLIT,
                        vec![json!(uri.as_str()), json!(id), json!(variable)],
                        rewrite.clone(),
                    ));
                }
                actions.push(goal_action("Case split on result", COMMAND_CASE_SPLIT));
            } else {
                actions.push(goal_action(
                    &format!("Case split on `{content}`"),
                    COMMAND_CASE_SPLIT,
                ));
            }
            if *with {
                let title = match content.is_empty() {
                    true => "With-abstract".to_string(),
                    false => format!("With-abstract on `{content}`"),
                };
                actions.push(goal_action(&title, COMMAND_ADD_WITH));
            }
            if let Some(name) = helper {
                actions.push(goal_action(
                    &format!("Make helper function `{name}`"),
                    COMMAND_MAKE_HELPER,
                ));
            }
            actions.push(goal_action("Auto", COMMAND_AUTO));
            actions.push(goal_action("Solve", COMMAND_SOLVE));
            actions.push(action(
                "Solve all goals".into(),
                COMMAND_SOLVE_ALL,
                vec![json!(uri.as_str())],
                rewrite.clone(),
            ));
            actions.push(goal_action("Print goal in output", COMMAND_GOAL));
            if !content.is_empty() {
                actions.push(goal_action(
                    "Print normal form in output",
                    COMMAND_NORMAL_FORM,
                ));
            }
        }
        // Offered where output matters, not on every line, so Zed does not
        // show a code action indicator everywhere.
        if goal.is_some() || on_problem {
            actions.push(action(
                "Open output file".into(),
                COMMAND_OPEN_OUTPUT,
                Vec::new(),
                None,
            ));
        }
        Ok((!actions.is_empty()).then_some(actions))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> RpcResult<Option<LSPAny>> {
        if params.command == COMMAND_OPEN_OUTPUT {
            let bridge = self.0.clone();
            tokio::spawn(async move { bridge.open_output().await });
            return Ok(None);
        }
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
        let Some(path) = uri.as_ref().and_then(path_of) else {
            return Ok(None);
        };
        // Every command but solve all is about one goal.
        if id.is_none() && params.command != COMMAND_SOLVE_ALL {
            return Ok(None);
        }
        // Run in the background, so a long Agda command does not occupy one of
        // the server's request slots while it waits.
        let bridge = self.0.clone();
        tokio::spawn(async move {
            let result = match (params.command.as_str(), id) {
                (COMMAND_SOLVE_ALL, _) => bridge.solve(&path, None).await,
                (COMMAND_GIVE, Some(id)) => bridge.give(&path, id, GoalCommand::Give).await,
                (COMMAND_REFINE, Some(id)) => bridge.give(&path, id, GoalCommand::Refine).await,
                (COMMAND_AUTO, Some(id)) => bridge.give(&path, id, GoalCommand::Auto).await,
                (COMMAND_CASE_SPLIT, Some(id)) => {
                    let variable = params
                        .arguments
                        .get(2)
                        .and_then(Value::as_str)
                        .map(String::from);
                    bridge.case_split(&path, id, variable).await
                }
                (COMMAND_SOLVE, Some(id)) => bridge.solve(&path, Some(id)).await,
                (COMMAND_ADD_WITH, Some(id)) => bridge.add_with(&path, id).await,
                (COMMAND_MAKE_HELPER, Some(id)) => bridge.make_helper(&path, id).await,
                (COMMAND_NORMAL_FORM, Some(id)) => bridge.normal_form(&path, id).await,
                // For this command the number is a line, not a goal.
                (COMMAND_ADD_CLAUSE, Some(line)) => bridge.add_clause(&path, line as usize).await,
                (COMMAND_GOAL, Some(id)) => match bridge.goal_info(&path, id, true).await {
                    Ok(markdown) => {
                        bridge
                            .show_output(&format!("Goal ?{id}"), &path, &markdown)
                            .await;
                        Ok(String::new())
                    }
                    Err(message) => Err(message),
                },
                (other, _) => Err(format!("Unknown command {other}")),
            };
            bridge.report(result).await;
        });
        Ok(None)
    }
}
