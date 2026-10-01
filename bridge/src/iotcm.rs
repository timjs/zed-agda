//! Builders for the commands Agda accepts on stdin in `--interaction-json` mode.
//!
//! Every command is wrapped in an `IOTCM` envelope:
//! `IOTCM "<file>" <highlighting level> <highlighting method> (<command>)`.
//! Ported from agda2-vscode's `src/util/iotcm.ts` and `src/agda/commands.ts`.

use std::fmt::Write as _;
use std::path::Path;

/// Quote a string as a Haskell string literal.
///
/// Non-ASCII characters become decimal escapes (`ℕ` becomes `\8469`). When such
/// an escape is followed by an ASCII digit, Haskell would read the digit as part
/// of the escape, so the empty escape `\&` is inserted to end it.
pub fn haskell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut after_numeric_escape = false;
    for ch in s.chars() {
        if after_numeric_escape && ch.is_ascii_digit() {
            out.push_str("\\&");
        }
        after_numeric_escape = false;
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' '..='~' => out.push(ch),
            _ => {
                let _ = write!(out, "\\{}", ch as u32);
                after_numeric_escape = true;
            }
        }
    }
    out.push('"');
    out
}

fn haskell_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| haskell_quote(item)).collect();
    format!("[{}]", quoted.join(","))
}

/// Wrap a command in the `IOTCM` envelope.
///
/// Highlighting is requested `Direct`, so it arrives inline as JSON instead of
/// through temporary files (the Emacs mode uses `Indirect`).
fn iotcm(file: &Path, command: &str) -> String {
    format!(
        "IOTCM {} NonInteractive Direct ({command})",
        haskell_quote(&file.to_string_lossy())
    )
}

/// `Cmd_load "<file>" [<flags>]`: type-check a file.
pub fn load(file: &Path, flags: &[String]) -> String {
    let path = haskell_quote(&file.to_string_lossy());
    iotcm(file, &format!("Cmd_load {path} {}", haskell_list(flags)))
}

/// `Cmd_goal_type_context <rewrite> <goal> noRange ""`: goal type and context.
pub fn goal_type_context(file: &Path, goal: u32) -> String {
    iotcm(
        file,
        &format!("Cmd_goal_type_context Simplified {goal} noRange \"\""),
    )
}

/// `Cmd_give WithoutForce <goal> noRange "<expr>"`: fill a goal.
pub fn give(file: &Path, goal: u32, expr: &str) -> String {
    iotcm(
        file,
        &format!(
            "Cmd_give WithoutForce {goal} noRange {}",
            haskell_quote(expr)
        ),
    )
}

/// `Cmd_refine_or_intro False <goal> noRange "<expr>"`: refine a goal.
pub fn refine(file: &Path, goal: u32, expr: &str) -> String {
    iotcm(
        file,
        &format!(
            "Cmd_refine_or_intro False {goal} noRange {}",
            haskell_quote(expr)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_ascii_and_escapes() {
        assert_eq!(haskell_quote("a \"b\" \\ c"), r#""a \"b\" \\ c""#);
        assert_eq!(haskell_quote("x\ny"), r#""x\ny""#);
    }

    #[test]
    fn escapes_non_ascii_as_decimal() {
        assert_eq!(haskell_quote("ℕ"), r#""\8469""#);
        assert_eq!(haskell_quote("λ x → x"), r#""\955 x \8594 x""#);
    }

    #[test]
    fn separates_numeric_escape_from_following_digit() {
        // Without `\&`, Haskell would read `\84691` as a single character.
        assert_eq!(haskell_quote("ℕ1"), r#""\8469\&1""#);
        assert_eq!(haskell_quote("ℕa1"), r#""\8469a1""#);
    }

    #[test]
    fn builds_envelopes() {
        let file = Path::new("/tmp/A.agda");
        assert_eq!(
            load(file, &[]),
            r#"IOTCM "/tmp/A.agda" NonInteractive Direct (Cmd_load "/tmp/A.agda" [])"#
        );
        assert_eq!(
            give(file, 3, "suc n"),
            r#"IOTCM "/tmp/A.agda" NonInteractive Direct (Cmd_give WithoutForce 3 noRange "suc n")"#
        );
    }
}
