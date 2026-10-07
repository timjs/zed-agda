//! Markdown for hovers and for the output file, in the layout of Emacs's
//! `*Agda information*` buffer. Agda code goes in ```agda fences, so Zed
//! highlights it with the Agda grammar in hovers and in Markdown previews.

use crate::protocol::{DisplayInfo, GoalInfo, TypeAux, message_text};
use serde_json::Value;

const RULE: &str = "────────────────────────────────────────";

/// Goal type and context, as `Cmd_goal_type_context` returns them, with the
/// type of the goal's text as `Have:` when `Cmd_goal_type_context_infer`
/// asked for it, as in Emacs.
pub fn goal(id: u32, info: &GoalInfo) -> String {
    match info {
        GoalInfo::GoalType {
            ty,
            type_aux,
            entries,
            boundary,
            output_forms,
        } => {
            let mut out = format!("**Goal ?{id}**\n\n```agda\nGoal: {ty}\n");
            if let TypeAux::GoalAndHave { expr } = type_aux {
                out.push_str(&format!("Have: {expr}\n"));
            }
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
        GoalInfo::NormalForm { expr } => normal_form(id, None, expr),
        GoalInfo::HelperFunction { signature } => {
            format!("**Helper function for ?{id}**\n\n```agda\n{signature}\n```\n")
        }
        GoalInfo::Other => format!(
            "**Goal ?{id}**\n\n(Agda sent goal information this version cannot show yet.)\n"
        ),
    }
}

/// Why the text in a goal has no type, to follow the goal and its context.
pub fn untyped(message: &str) -> String {
    format!("\nAgda cannot infer a type for the text in the goal:\n\n```text\n{message}\n```\n")
}

/// The normal form of the text in goal `id`, with that text when known, on
/// one line.
pub fn normal_form(id: u32, text: Option<&str>, normal: &str) -> String {
    let of = text
        .map(|text| {
            format!(
                " of `{}`",
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            )
        })
        .unwrap_or_default();
    format!("**Normal form{of} in ?{id}**\n\n```agda\n{normal}\n```\n")
}

/// Agda's answer to why a name is in scope, tidied: in a file without goals,
/// Agda 2.8 leaves out where an `open` is (`the opening of Nat at` and
/// nothing after it), so such a line says so instead.
pub fn why_in_scope(message: &str) -> String {
    message
        .lines()
        .map(|line| match line.trim_end().strip_suffix(" at") {
            Some(start) => format!("{start} (Agda gives no location in a file without goals)"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        DisplayInfo::Auto { info } => format!("## Auto\n\n{info}\n"),
        DisplayInfo::InferredType { expr } => format!("```agda\n{expr}\n```\n"),
        DisplayInfo::WhyInScope { message } => format!("```text\n{message}\n```\n"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ContextEntry;

    #[test]
    fn shows_the_type_of_a_goals_text_and_its_normal_form() {
        let info = GoalInfo::GoalType {
            ty: "ℕ".into(),
            type_aux: TypeAux::GoalAndHave {
                expr: "ℕ → ℕ".into(),
            },
            entries: vec![ContextEntry {
                reified_name: "n".into(),
                binding: "ℕ".into(),
                in_scope: true,
            }],
            boundary: vec![],
            output_forms: vec![],
        };
        assert_eq!(
            goal(3, &info),
            format!("**Goal ?3**\n\n```agda\nGoal: ℕ\nHave: ℕ → ℕ\n{RULE}\nn : ℕ\n```\n")
        );
        assert_eq!(
            normal_form(1, Some("two\n  + two"), "suc (suc zero)"),
            "**Normal form of `two + two` in ?1**\n\n```agda\nsuc (suc zero)\n```\n"
        );
        assert!(
            goal(
                1,
                &GoalInfo::NormalForm {
                    expr: "zero".into()
                }
            )
            .starts_with("**Normal form in ?1**")
        );
    }

    #[test]
    fn says_when_agda_leaves_out_a_location() {
        let message = "suc is in scope as\n  * a constructor Nat.ℕ.suc brought into scope by\n    - the opening of Nat at\n    - its definition at Nat.agda:5.3-6";
        assert_eq!(
            why_in_scope(message),
            "suc is in scope as\n  * a constructor Nat.ℕ.suc brought into scope by\n    - the opening of Nat (Agda gives no location in a file without goals)\n    - its definition at Nat.agda:5.3-6"
        );
    }
}
