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

/// `Cmd_make_case <goal> noRange "<variables>"`: case split on the variables
/// typed in a goal, or, with none, introduce the missing patterns or split on
/// the result.
pub fn make_case(file: &Path, goal: u32, variables: &str) -> String {
    iotcm(
        file,
        &format!("Cmd_make_case {goal} noRange {}", haskell_quote(variables)),
    )
}

/// `Cmd_autoOne AsIs <goal> noRange "<hints>"`: proof search for a goal. This
/// is the syntax of Agda 2.7 and later, whose search is called Mimer; earlier
/// versions take no `AsIs`.
pub fn auto_one(file: &Path, goal: u32, hints: &str) -> String {
    iotcm(
        file,
        &format!("Cmd_autoOne AsIs {goal} noRange {}", haskell_quote(hints)),
    )
}

/// `Cmd_solveOne Simplified <goal> noRange ""`: the solution of a goal that
/// unification already found. Like Emacs, `Simplified` for one goal and
/// `AsIs` for all of them.
pub fn solve_one(file: &Path, goal: u32) -> String {
    iotcm(
        file,
        &format!("Cmd_solveOne Simplified {goal} noRange \"\""),
    )
}

/// `Cmd_infer_toplevel Simplified "<expr>"`: the type of an expression in the
/// scope at the top level of the current file.
pub fn infer_toplevel(file: &Path, expr: &str) -> String {
    iotcm(
        file,
        &format!("Cmd_infer_toplevel Simplified {}", haskell_quote(expr)),
    )
}

/// `Cmd_why_in_scope_toplevel "<name>"`: how a name is in scope at the top
/// level of the current file.
pub fn why_in_scope_toplevel(file: &Path, name: &str) -> String {
    iotcm(
        file,
        &format!("Cmd_why_in_scope_toplevel {}", haskell_quote(name)),
    )
}

/// `Cmd_why_in_scope <goal> noRange "<name>"`: how a name is in scope in a
/// goal, its bound variables included.
pub fn why_in_scope(file: &Path, goal: u32, name: &str) -> String {
    iotcm(
        file,
        &format!("Cmd_why_in_scope {goal} noRange {}", haskell_quote(name)),
    )
}

/// `Cmd_solveAll AsIs`: the solutions of all goals that unification already
/// found.
pub fn solve_all(file: &Path) -> String {
    iotcm(file, "Cmd_solveAll AsIs")
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
        let envelope =
            |command: &str| format!(r#"IOTCM "/tmp/A.agda" NonInteractive Direct ({command})"#);
        assert_eq!(
            make_case(file, 0, "n m"),
            envelope(r#"Cmd_make_case 0 noRange "n m""#)
        );
        assert_eq!(
            auto_one(file, 1, ""),
            envelope(r#"Cmd_autoOne AsIs 1 noRange """#)
        );
        assert_eq!(
            solve_one(file, 5),
            envelope(r#"Cmd_solveOne Simplified 5 noRange """#)
        );
        assert_eq!(solve_all(file), envelope("Cmd_solveAll AsIs"));
        assert_eq!(
            why_in_scope_toplevel(file, "suc"),
            envelope(r#"Cmd_why_in_scope_toplevel "suc""#)
        );
        assert_eq!(
            why_in_scope(file, 0, "n"),
            envelope(r#"Cmd_why_in_scope 0 noRange "n""#)
        );
        assert_eq!(
            infer_toplevel(file, "_+_"),
            envelope(r#"Cmd_infer_toplevel Simplified "_+_""#)
        );
    }
}
