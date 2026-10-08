//! Responses Agda writes to stdout in `--interaction-json` mode, and the
//! splitter that turns raw stdout bytes into prompts and JSON lines.
//!
//! Types follow agda2-vscode's `src/agda/responses.ts`; only the parts the
//! bridge uses are typed, everything else stays a `serde_json::Value`.

use serde::Deserialize;
use serde_json::Value;

const PROMPT: &[u8] = b"JSON> ";

/// One event on Agda's stdout.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Agda printed `JSON> `: it is ready for the next command. Agda prints one
    /// prompt at startup, before any command, and one after every command.
    Prompt,
    /// A complete line of output, normally one JSON object.
    Line(String),
}

/// Splits Agda's stdout into [`Event`]s.
///
/// The prompt has no trailing newline, so it usually appears glued to the
/// front of the next response line (`JSON> {"kind":...}`), or alone at the
/// end of a chunk while Agda waits for input.
#[derive(Default)]
pub struct Splitter {
    buf: Vec<u8>,
}

impl Splitter {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(chunk);
        let mut events = Vec::new();
        loop {
            if self.buf.starts_with(PROMPT) {
                self.buf.drain(..PROMPT.len());
                events.push(Event::Prompt);
            } else if let Some(newline) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=newline).collect();
                let line = String::from_utf8_lossy(&line).trim().to_string();
                if !line.is_empty() {
                    events.push(Event::Line(line));
                }
            } else {
                break;
            }
        }
        events
    }
}

/// A top-level response; the `kind` field selects the variant.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum Response {
    InteractionPoints {
        interaction_points: Vec<InteractionPoint>,
    },
    DisplayInfo {
        info: Value,
    },
    GiveAction {
        interaction_point: InteractionPoint,
        give_result: GiveResult,
    },
    /// The clauses that replace the clause of a goal after a case split.
    /// They contain lone `?`s for the new goals.
    MakeCase {
        interaction_point: InteractionPoint,
        variant: MakeCaseVariant,
        clauses: Vec<String>,
    },
    /// The goals that unification already solved, for solve.
    SolveAll {
        solutions: Vec<Solution>,
    },
    /// Highlighting for part of the loaded file. The bridge asks for it
    /// `Direct`, so it arrives inline, in several chunks that may repeat
    /// entries.
    HighlightingInfo {
        info: Option<Highlighting>,
    },
    /// The answer to a command stopped with `Cmd_abort`, before its prompt.
    DoneAborting,
    /// Everything else, such as `Status`, `RunningInfo` and `JumpToError`.
    #[serde(other)]
    Other,
}

/// Parse one line of Agda output.
pub fn parse_line(line: &str) -> Result<Response, serde_json::Error> {
    serde_json::from_str(line)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Highlighting {
    pub payload: Vec<HighlightingEntry>,
}

/// One highlighted stretch of the file, as a half-open range of 1-based code
/// point offsets. Its `atoms` say what it is: a kind of name (`function`,
/// `bound`), a lexical class (`keyword`, `comment`) or a problem
/// (`unsolvedmeta`, `terminationproblem`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightingEntry {
    pub range: [usize; 2],
    #[serde(default)]
    pub atoms: Vec<String>,
    pub definition_site: Option<DefinitionSite>,
}

/// Where a name is defined: a file and the 1-based code point offset of the
/// start of the name in it (offset 1 for a module).
#[derive(Debug, Clone, Deserialize)]
pub struct DefinitionSite {
    pub filepath: String,
    pub position: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InteractionPoint {
    pub id: u32,
    pub range: Vec<Interval>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Interval {
    pub start: Position,
    pub end: Position,
}

/// A position in an Agda source file. Agda also sends 1-based `line` and
/// `col` fields; the bridge only needs `pos`, a 1-based code point offset from
/// the start of the file.
#[derive(Debug, Clone, Deserialize)]
pub struct Position {
    pub pos: usize,
}

/// Where the goal of a case split is: a function clause, or a clause of an
/// extended lambda (`λ { … }` or `λ where`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum MakeCaseVariant {
    Function,
    ExtendedLambda,
}

/// One goal and the expression unification found for it. Unlike elsewhere,
/// the goal is only its number.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Solution {
    pub interaction_point: u32,
    pub expression: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GiveResult {
    /// Replace the goal with this text.
    Str { str: String },
    /// Keep the goal's text, in parentheses when `paren` is true.
    Paren { paren: bool },
}

/// The typed view of a `DisplayInfo` payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum DisplayInfo {
    AllGoalsWarnings {
        visible_goals: Vec<OutputConstraint>,
        invisible_goals: Vec<OutputConstraint>,
        warnings: Vec<Value>,
        errors: Vec<Value>,
    },
    Error {
        /// Agda >= 2.6.2 nests the message in an object; older versions do not.
        error: Option<Value>,
        message: Option<String>,
        #[serde(default)]
        warnings: Vec<Value>,
    },
    GoalSpecific {
        interaction_point: InteractionPoint,
        goal_info: GoalInfo,
    },
    /// What auto says when it found nothing, as in "No solution found".
    Auto { info: String },
    /// The type Agda inferred for an expression.
    InferredType { expr: String },
    /// How a name is in scope: as what, brought in by which `open` or
    /// definition, with locations.
    WhyInScope { message: String },
    #[serde(other)]
    Other,
}

impl DisplayInfo {
    pub fn parse(info: &Value) -> DisplayInfo {
        serde_json::from_value(info.clone()).unwrap_or(DisplayInfo::Other)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputConstraint {
    pub constraint_obj: Value,
    #[serde(rename = "type")]
    pub ty: Option<String>,
}

impl OutputConstraint {
    /// The goal number of a visible goal (`?3`), if this constraint is one.
    pub fn goal_id(&self) -> Option<u32> {
        self.constraint_obj.get("id")?.as_u64().map(|id| id as u32)
    }

    /// How Agda names this goal or meta: `?3` or `_12`.
    pub fn label(&self) -> String {
        match (&self.constraint_obj, self.goal_id()) {
            (_, Some(id)) => format!("?{id}"),
            (Value::Object(obj), None) => obj
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("_")
                .to_string(),
            (Value::String(name), None) => name.clone(),
            _ => "_".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum GoalInfo {
    GoalType {
        #[serde(rename = "type")]
        ty: String,
        /// What comes with the goal's type: with
        /// `Cmd_goal_type_context_infer`, the type of the goal's text.
        #[serde(default)]
        type_aux: TypeAux,
        entries: Vec<ContextEntry>,
        #[serde(default)]
        boundary: Vec<Value>,
        #[serde(default)]
        output_forms: Vec<Value>,
    },
    /// The normal form of an expression in a goal, from `Cmd_compute`.
    NormalForm { expr: String },
    /// The signature of a helper function, from `Cmd_helper_function`, such
    /// as `aux : (n m : ℕ) → ℕ`.
    HelperFunction { signature: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
pub enum TypeAux {
    /// Only the goal's type.
    #[default]
    GoalOnly,
    /// The type Agda inferred for the goal's text, shown as `Have:`.
    GoalAndHave { expr: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEntry {
    pub reified_name: String,
    pub binding: String,
    pub in_scope: bool,
}

/// The text of a warning or error, which is a bare string before Agda 2.6.2
/// and an object with a `message` field from 2.6.2 on.
pub fn message_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(obj) => match obj.get("message") {
            Some(Value::String(text)) => text.clone(),
            _ => value.to_string(),
        },
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_startup_prompt_and_glued_prompts() {
        let mut splitter = Splitter::default();
        let events = splitter.push(b"JSON> {\"kind\":\"ClearRunningInfo\"}\n{\"a\":1}\nJSON> ");
        assert_eq!(
            events,
            vec![
                Event::Prompt,
                Event::Line("{\"kind\":\"ClearRunningInfo\"}".into()),
                Event::Line("{\"a\":1}".into()),
                Event::Prompt,
            ]
        );
    }

    #[test]
    fn waits_for_incomplete_lines_and_prompts() {
        let mut splitter = Splitter::default();
        assert_eq!(splitter.push(b"JSO"), vec![]);
        assert_eq!(splitter.push(b"N> {\"x\""), vec![Event::Prompt]);
        assert_eq!(
            splitter.push(b":2}\n"),
            vec![Event::Line("{\"x\":2}".into())]
        );
    }

    #[test]
    fn keeps_multibyte_characters_split_across_chunks() {
        let mut splitter = Splitter::default();
        let bytes = "\"ℕ\"\n".as_bytes();
        assert_eq!(splitter.push(&bytes[..2]), vec![]);
        assert_eq!(
            splitter.push(&bytes[2..]),
            vec![Event::Line("\"ℕ\"".into())]
        );
    }

    #[test]
    fn parses_responses_seen_from_agda_2_8() {
        let give = r#"{"giveResult":{"str":"suc (n + m)"},"interactionPoint":{"id":0,"range":[{"end":{"col":30,"line":9,"pos":126},"start":{"col":13,"line":9,"pos":109}}]},"kind":"GiveAction"}"#;
        match parse_line(give).unwrap() {
            Response::GiveAction {
                interaction_point,
                give_result: GiveResult::Str { str },
            } => {
                assert_eq!(interaction_point.id, 0);
                assert_eq!(str, "suc (n + m)");
            }
            other => panic!("unexpected {other:?}"),
        }

        let error = r#"{"info":{"error":{"message":"/x/Bad.agda:7.5-8: error: [UnequalTerms]\nSet₁ !=< ℕ"},"kind":"Error","warnings":[]},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(error).unwrap() else {
            panic!("expected DisplayInfo");
        };
        match DisplayInfo::parse(&info) {
            DisplayInfo::Error {
                error: Some(error), ..
            } => {
                assert!(message_text(&error).starts_with("/x/Bad.agda:7.5-8"));
            }
            other => panic!("unexpected {other:?}"),
        }

        let status = r#"{"kind":"Status","status":{"checked":false}}"#;
        assert!(matches!(parse_line(status).unwrap(), Response::Other));
        let aborted = r#"{"kind":"DoneAborting"}"#;
        assert!(matches!(
            parse_line(aborted).unwrap(),
            Response::DoneAborting
        ));

        // Shortened from Agda 2.8.0's answer when loading `Uses.agda`.
        let highlighting = r#"{"direct":true,"info":{"payload":[{"atoms":["keyword"],"definitionSite":null,"note":"","range":[1,7],"tokenBased":"TokenBased"},{"atoms":["datatype"],"definitionSite":{"filepath":"/x/Nat.agda","position":24},"note":"","range":[43,44],"tokenBased":"NotOnlyTokenBased"}],"remove":false},"kind":"HighlightingInfo"}"#;
        let Response::HighlightingInfo { info: Some(info) } = parse_line(highlighting).unwrap()
        else {
            panic!("expected HighlightingInfo");
        };
        assert_eq!(info.payload.len(), 2);
        assert_eq!(info.payload[0].range, [1, 7]);
        assert_eq!(info.payload[0].atoms, ["keyword"]);
        assert!(info.payload[0].definition_site.is_none());
        let site = info.payload[1].definition_site.as_ref().unwrap();
        assert_eq!((site.filepath.as_str(), site.position), ("/x/Nat.agda", 24));
    }

    #[test]
    fn parses_goal_command_responses_from_agda_2_8() {
        // Agda 2.8.0's answers to case split, solve and a failed auto.
        let make_case = r#"{"clauses":["zero + m = ?","suc n + m = ?"],"interactionPoint":{"id":0,"range":[{"end":{"col":16,"line":8,"pos":98},"start":{"col":9,"line":8,"pos":91}}]},"kind":"MakeCase","variant":"Function"}"#;
        match parse_line(make_case).unwrap() {
            Response::MakeCase {
                interaction_point,
                variant,
                clauses,
            } => {
                assert_eq!(interaction_point.id, 0);
                assert_eq!(variant, MakeCaseVariant::Function);
                assert_eq!(clauses, ["zero + m = ?", "suc n + m = ?"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        let lambda = r#"{"clauses":["zero → ?","(suc x) → ?"],"interactionPoint":{"id":3,"range":[]},"kind":"MakeCase","variant":"ExtendedLambda"}"#;
        assert!(matches!(
            parse_line(lambda).unwrap(),
            Response::MakeCase {
                variant: MakeCaseVariant::ExtendedLambda,
                ..
            }
        ));

        let solve = r#"{"kind":"SolveAll","solutions":[{"expression":"ℕ","interactionPoint":5}]}"#;
        let Response::SolveAll { solutions } = parse_line(solve).unwrap() else {
            panic!("expected SolveAll");
        };
        assert_eq!(solutions.len(), 1);
        assert_eq!(
            (
                solutions[0].interaction_point,
                solutions[0].expression.as_str()
            ),
            (5, "ℕ")
        );

        let inferred = r#"{"info":{"commandState":{"currentFile":["/x/P.agda","2026-10-05T13:13:24Z"],"interactionPoints":[]},"expr":"ℕ → ℕ","kind":"InferredType","time":null},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(inferred).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(
            matches!(DisplayInfo::parse(&info), DisplayInfo::InferredType { expr } if expr == "ℕ → ℕ")
        );

        let why = r#"{"info":{"filepath":"/x","kind":"WhyInScope","message":"suc is in scope as\n  * a constructor Nat.ℕ.suc brought into scope by\n    - the opening of Nat at /x/P.agda:3.13-16","thing":"suc"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(why).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(matches!(
            DisplayInfo::parse(&info),
            DisplayInfo::WhyInScope { message }
                if message.starts_with("suc is in scope as\n  * a constructor")
        ));

        // The goal with the type of its text, and the normal form of its
        // text, shortened from Agda 2.8.0's answers.
        let have = r#"{"info":{"goalInfo":{"boundary":[],"entries":[{"binding":"ℕ","inScope":true,"originalName":"n","reifiedName":"n"}],"kind":"GoalType","outputForms":[],"rewrite":"Simplified","type":"ℕ","typeAux":{"expr":"ℕ → ℕ","kind":"GoalAndHave"}},"interactionPoint":{"id":3,"range":[]},"kind":"GoalSpecific"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(have).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(matches!(
            DisplayInfo::parse(&info),
            DisplayInfo::GoalSpecific {
                goal_info: GoalInfo::GoalType { type_aux: TypeAux::GoalAndHave { expr }, entries, .. },
                ..
            } if expr == "ℕ → ℕ" && entries.len() == 1
        ));
        let only = r#"{"info":{"goalInfo":{"entries":[],"kind":"GoalType","type":"ℕ","typeAux":{"kind":"GoalOnly"}},"interactionPoint":{"id":4,"range":[]},"kind":"GoalSpecific"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(only).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(matches!(
            DisplayInfo::parse(&info),
            DisplayInfo::GoalSpecific {
                goal_info: GoalInfo::GoalType {
                    type_aux: TypeAux::GoalOnly,
                    ..
                },
                ..
            }
        ));
        let normal = r#"{"info":{"goalInfo":{"computeMode":"DefaultCompute","expr":"suc (suc (suc (suc zero)))","kind":"NormalForm"},"interactionPoint":{"id":1,"range":[]},"kind":"GoalSpecific"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(normal).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(matches!(
            DisplayInfo::parse(&info),
            DisplayInfo::GoalSpecific {
                goal_info: GoalInfo::NormalForm { expr },
                ..
            } if expr == "suc (suc (suc (suc zero)))"
        ));

        let helper = r#"{"info":{"goalInfo":{"kind":"HelperFunction","signature":"aux : (n m : ℕ) → ℕ"},"interactionPoint":{"id":0,"range":[]},"kind":"GoalSpecific"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(helper).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(matches!(
            DisplayInfo::parse(&info),
            DisplayInfo::GoalSpecific {
                goal_info: GoalInfo::HelperFunction { signature },
                ..
            } if signature == "aux : (n m : ℕ) → ℕ"
        ));

        let auto = r#"{"info":{"info":"No solution found","kind":"Auto"},"kind":"DisplayInfo"}"#;
        let Response::DisplayInfo { info } = parse_line(auto).unwrap() else {
            panic!("expected DisplayInfo");
        };
        assert!(
            matches!(DisplayInfo::parse(&info), DisplayInfo::Auto { info } if info == "No solution found")
        );
    }
}
