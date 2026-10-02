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

use crate::agda::Agda;
use crate::goals::{self, GOAL_MARKER, Goal};
use crate::highlight::{self, Span};
use crate::links::{self, Link, Target};
use crate::output::Output;
use crate::protocol::{
    DisplayInfo, GiveResult, GoalInfo, HighlightingEntry, InteractionPoint, MakeCaseVariant,
    Response, Solution, message_text,
};
use crate::text::{self, Change};
use crate::{input, iotcm, location, render};

pub const COMMAND_GIVE: &str = "agda.give";
pub const COMMAND_REFINE: &str = "agda.refine";
pub const COMMAND_GOAL: &str = "agda.goal";
pub const COMMAND_CASE_SPLIT: &str = "agda.caseSplit";
pub const COMMAND_AUTO: &str = "agda.auto";
pub const COMMAND_SOLVE: &str = "agda.solve";
pub const COMMAND_SOLVE_ALL: &str = "agda.solveAll";
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
    /// Whether the client accepts `LocationLink`s for go to definition, which
    /// carry the range of the name (Zed underlines it on `cmd`-hover).
    link_support: bool,
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
    output: OnceLock<Output>,
    documents: Mutex<HashMap<PathBuf, Document>>,
    session: AsyncMutex<Option<Session>>,
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
                        DisplayInfo::Other => {}
                    }
                    outcome.displays.push(info);
                }
                Response::HighlightingInfo { info } => {
                    outcome
                        .highlighting
                        .extend(info.into_iter().flat_map(|info| info.payload));
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
        let mut links = links::from_highlighting(&outcome.highlighting, path);
        let mut spans = highlight::from_highlighting(&outcome.highlighting);
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
        let outcome = Outcome::collect(self.run(&mut guard, &request).await?);
        drop(guard);

        let title = command.title();
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
    pub async fn case_split(&self, path: &Path, id: u32) -> Result<String, String> {
        let variables = self.goal_content(path, id)?;
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
        let Some(output) = self.output.get() else {
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
        let Some(output) = self.output.get() else {
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
        let link_support = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.definition.as_ref())
            .and_then(|definition| definition.link_support)
            .unwrap_or(false);
        let _ = self.0.config.set(Config {
            agda_path,
            extra_args,
            root,
            link_support,
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
                definition_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: highlight::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..SemanticTokensOptions::default()
                        },
                    ),
                ),
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
                        COMMAND_CASE_SPLIT.into(),
                        COMMAND_AUTO.into(),
                        COMMAND_SOLVE.into(),
                        COMMAND_SOLVE_ALL.into(),
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
        let found = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let Some(line) = document.text.split('\n').nth(position.line as usize) else {
                return Ok(None);
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            input::complete(line, position.character)
        };
        let Some(found) = found else {
            return Ok(None);
        };
        let range = Range::new(Position::new(position.line, found.start), position);
        let items = found
            .candidates
            .into_iter()
            .enumerate()
            .map(|(rank, candidate)| {
                let code_points: Vec<String> = candidate
                    .symbol
                    .chars()
                    .map(|c| format!("U+{:04X}", c as u32))
                    .collect();
                CompletionItem {
                    // Zed shows the label and then the detail: `→ \to`.
                    label: candidate.symbol.to_string(),
                    detail: Some(candidate.name.clone()),
                    documentation: Some(Documentation::String(code_points.join(" "))),
                    // Zed filters on the word before the cursor, which never
                    // includes the leader.
                    filter_text: Some(candidate.name[1..].to_string()),
                    sort_text: Some(format!("{rank:04}")),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                        range,
                        candidate.symbol.to_string(),
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
                Target::File { path, position } => (origin, path.clone(), *position),
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
        let (goal, on_problem) = {
            let documents = self.0.documents.lock().unwrap();
            let Some(document) = documents.get(&path) else {
                return Ok(None);
            };
            let offset = text::offset_of(&document.text, params.range.start);
            let goal = goals::goal_at(&document.goals, offset)
                .map(|goal| (goal.id, goal.content(&document.text)));
            let line = params.range.start.line;
            let on_problem = document
                .problems
                .iter()
                .any(|problem| problem.range.start.line <= line && line <= problem.range.end.line);
            (goal, on_problem)
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
        if let Some((id, content)) = &goal {
            let id = *id;
            let goal_action = |title: String, command: &str| {
                let arguments = vec![json!(uri.as_str()), json!(id)];
                action(
                    title,
                    command,
                    arguments,
                    Some(CodeActionKind::REFACTOR_REWRITE),
                )
            };
            if !content.is_empty() {
                actions.push(goal_action(format!("Agda: give ?{id}"), COMMAND_GIVE));
            }
            actions.push(goal_action(format!("Agda: refine ?{id}"), COMMAND_REFINE));
            let split = match content.is_empty() {
                true => format!("Agda: case split ?{id}"),
                false => format!("Agda: case split ?{id} on {content}"),
            };
            actions.push(goal_action(split, COMMAND_CASE_SPLIT));
            actions.push(goal_action(format!("Agda: auto ?{id}"), COMMAND_AUTO));
            actions.push(goal_action(format!("Agda: solve ?{id}"), COMMAND_SOLVE));
            actions.push(action(
                "Agda: solve all goals".into(),
                COMMAND_SOLVE_ALL,
                vec![json!(uri.as_str())],
                Some(CodeActionKind::REFACTOR_REWRITE),
            ));
            actions.push(goal_action(
                format!("Agda: show goal ?{id} in output"),
                COMMAND_GOAL,
            ));
        }
        // Offered where output matters, not on every line, so Zed does not
        // show a code action indicator everywhere.
        if goal.is_some() || on_problem {
            actions.push(action(
                "Agda: open output file".into(),
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
                (COMMAND_CASE_SPLIT, Some(id)) => bridge.case_split(&path, id).await,
                (COMMAND_SOLVE, Some(id)) => bridge.solve(&path, Some(id)).await,
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
