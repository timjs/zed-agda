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
    /// Everything else, such as `Status`, `RunningInfo` and `JumpToError`.
    #[serde(other)]
    Other,
}

/// Parse one line of Agda output. Highlighting payloads are skipped without
/// being parsed, because they are large and the bridge does not use them yet.
pub fn parse_line(line: &str) -> Option<Result<Response, serde_json::Error>> {
    if line.contains(r#""kind":"HighlightingInfo""#) {
        return None;
    }
    Some(serde_json::from_str(line))
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
        entries: Vec<ContextEntry>,
        #[serde(default)]
        boundary: Vec<Value>,
        #[serde(default)]
        output_forms: Vec<Value>,
    },
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
        match parse_line(give).unwrap().unwrap() {
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
        let Response::DisplayInfo { info } = parse_line(error).unwrap().unwrap() else {
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
        assert!(matches!(
            parse_line(status).unwrap().unwrap(),
            Response::Other
        ));

        let highlighting = r#"{"direct":true,"info":{},"kind":"HighlightingInfo"}"#;
        assert!(parse_line(highlighting).is_none());
    }
}
