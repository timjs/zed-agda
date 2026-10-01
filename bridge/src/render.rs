//! Markdown for hovers and for the output file, in the layout of Emacs's
//! `*Agda information*` buffer. Agda code goes in ```agda fences, so Zed
//! highlights it with the Agda grammar in hovers and in Markdown previews.

use crate::protocol::{DisplayInfo, GoalInfo, message_text};
use serde_json::Value;

const RULE: &str = "────────────────────────────────────────";

/// Goal type and context, as `Cmd_goal_type_context` returns them.
pub fn goal(id: u32, info: &GoalInfo) -> String {
    match info {
        GoalInfo::GoalType {
            ty,
            entries,
            boundary,
            output_forms,
        } => {
            let mut out = format!("**Goal ?{id}**\n\n```agda\nGoal: {ty}\n");
            for value in boundary {
                out.push_str(&format!("Boundary: {}\n", message_text(value)));
            }
            out.push_str(RULE);
            out.push('\n');
            for entry in entries {
                let scope = if entry.in_scope {
                    ""
                } else {
                    "  (not in scope)"
                };
                out.push_str(&format!(
                    "{} : {}{scope}\n",
                    entry.reified_name, entry.binding
                ));
            }
            for form in output_forms {
                out.push_str(&format!("{}\n", message_text(form)));
            }
            out.push_str("```\n");
            out
        }
        GoalInfo::Other => format!(
            "**Goal ?{id}**\n\n(Agda sent goal information this version cannot show yet.)\n"
        ),
    }
}

/// Any `DisplayInfo`, for the output file.
pub fn display_info(raw: &Value) -> String {
    match DisplayInfo::parse(raw) {
        DisplayInfo::AllGoalsWarnings {
            visible_goals,
            invisible_goals,
            warnings,
            errors,
        } => {
            let mut out = String::new();
            section(&mut out, "Errors", errors.iter().map(message_text));
            let goals = visible_goals
                .iter()
                .chain(&invisible_goals)
                .map(|goal| format!("{} : {}", goal.label(), goal.ty.as_deref().unwrap_or("_")));
            section(&mut out, "Goals", goals);
            section(&mut out, "Warnings", warnings.iter().map(message_text));
            if out.is_empty() {
                out.push_str("All done: no goals, errors or warnings.\n");
            }
            out
        }
        DisplayInfo::Error {
            error,
            message,
            warnings,
        } => {
            let text = message
                .or_else(|| error.as_ref().map(message_text))
                .unwrap_or_default();
            let mut out = String::new();
            section(&mut out, "Error", std::iter::once(text));
            section(&mut out, "Warnings", warnings.iter().map(message_text));
            out
        }
        DisplayInfo::GoalSpecific {
            interaction_point,
            goal_info,
        } => goal(interaction_point.id, &goal_info),
        DisplayInfo::Other => format!(
            "```json\n{}\n```\n",
            serde_json::to_string_pretty(raw).unwrap_or_default()
        ),
    }
}

fn section(out: &mut String, title: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("## {title}\n\n```agda\n"));
    for item in items {
        out.push_str(item.trim_end());
        out.push('\n');
    }
    out.push_str("```\n\n");
}
