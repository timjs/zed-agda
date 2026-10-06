//! Goals (Agda's interaction points) and how they follow edits.
//!
//! Agda reports goal ranges for the text it loaded. Between loads the user
//! keeps typing, so the bridge moves the ranges along with every change:
//! edits before a goal shift it, edits strictly inside `{! … !}` grow or
//! shrink it, and edits that touch its delimiters remove it (the same rule as
//! agda2-vscode's `adjustRangeContaining`).

use crate::protocol::MakeCaseVariant;
use crate::text::Change;

/// The text Agda's Emacs mode puts in place of a lone `?`.
pub const GOAL_MARKER: &str = "{!  !}";

/// A goal, as a half-open range of 0-based char offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub id: u32,
    pub start: usize,
    pub end: usize,
}

impl Goal {
    /// The editable interior, between `{!` and `!}`; empty for a lone `?`.
    fn interior(&self, text: &[char]) -> Option<(usize, usize)> {
        let is_hole = self.end >= self.start + 4
            && text.get(self.start..self.start + 2) == Some(&['{', '!'][..])
            && text.get(self.end - 2..self.end) == Some(&['!', '}'][..]);
        is_hole.then_some((self.start + 2, self.end - 2))
    }

    /// The expression typed in the goal, trimmed; empty for `?` and `{!  !}`.
    pub fn content(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        match self.interior(&chars) {
            Some((from, to)) => chars[from..to]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
            None => String::new(),
        }
    }

    /// Whether `offset` lies in the goal, counting the position right after it,
    /// so that a cursor just behind `!}` still finds the goal.
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset <= self.end
    }
}

/// Move goals along with one change to the text they refer to. `old_text` is
/// the text before the change, needed to recognise the goal delimiters.
pub fn adjust(goals: &mut Vec<Goal>, old_text: &str, change: &Change) {
    let chars: Vec<char> = old_text.chars().collect();
    let delta = change.delta();
    goals.retain_mut(|goal| {
        if change.old_end <= goal.start {
            // Entirely before the goal (an insertion at its start counts as before).
            goal.start = goal.start.saturating_add_signed(delta);
            goal.end = goal.end.saturating_add_signed(delta);
            true
        } else if change.start >= goal.end {
            // Entirely after the goal.
            true
        } else if let Some((from, to)) = goal.interior(&chars)
            && change.start >= from
            && change.old_end <= to
        {
            // Strictly inside the hole: the goal survives and resizes.
            goal.end = goal.end.saturating_add_signed(delta);
            true
        } else {
            // The edit touches the delimiters or replaces the goal.
            false
        }
    });
}

/// The name typed in `goal` at, or right before, the cursor at `offset`:
/// the characters around it that can be part of a name, `.` included for a
/// qualified name. `None` outside the goal's text.
pub fn word_at(text: &str, goal: &Goal, offset: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let (from, to) = goal.interior(&chars)?;
    let part = |c: char| !c.is_whitespace() && !"(){}\";@".contains(c);
    let inside = |i: usize| from <= i && i < to;
    let at = [offset, offset.wrapping_sub(1)]
        .into_iter()
        .find(|&i| inside(i) && part(chars[i]))?;
    let start = (from..=at).rev().take_while(|&i| part(chars[i])).last()?;
    let end = (at..to).take_while(|&i| part(chars[i])).last()? + 1;
    Some(chars[start..end].iter().collect())
}

/// The goal that covers the char at `offset`, for hover.
pub fn goal_under(goals: &[Goal], offset: usize) -> Option<&Goal> {
    goals
        .iter()
        .find(|goal| goal.start <= offset && offset < goal.end)
}

/// Find goal by offset.
pub fn goal_at(goals: &[Goal], offset: usize) -> Option<&Goal> {
    goals.iter().find(|goal| goal.contains(offset))
}

/// Replace every lone `?` in `text` by [`GOAL_MARKER`], as Agda's Emacs mode
/// does with give and refine results. A `?` counts as lone when its neighbours
/// cannot be part of a name, so `_≟?_` stays intact.
pub fn expand_question_marks(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &ch) in chars.iter().enumerate() {
        let before = index.checked_sub(1).and_then(|i| chars.get(i)).copied();
        let after = chars.get(index + 1).copied();
        if ch == '?' && !is_name_char(before) && !is_name_char(after) {
            out.push_str(GOAL_MARKER);
        } else {
            out.push(ch);
        }
    }
    out
}

/// Characters that end a name in Agda's lexer (agda2-vscode's `isNameChar`).
fn is_name_char(ch: Option<char>) -> bool {
    match ch {
        None => false,
        Some(ch) => !(ch.is_whitespace() || "(){}\";.@".contains(ch)),
    }
}

/// The text a case split on `goal` replaces, as a half-open range of char
/// offsets, and its replacement: Agda's new `clauses`, with their lone `?`s
/// expanded. Ported from Emacs's `agda2-make-case-action` and
/// `agda2-make-case-action-extendlam`, which assume the goal is on one line.
pub fn case_split(
    text: &str,
    goal: &Goal,
    variant: MakeCaseVariant,
    clauses: &[String],
) -> (usize, usize, String) {
    let chars: Vec<char> = text.chars().collect();
    let line_start = chars[..goal.start]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1);
    let indent: String = chars[line_start..]
        .iter()
        .take_while(|&&c| c == ' ' || c == '\t')
        .collect();
    let code_start = line_start + indent.chars().count();
    let clauses: Vec<String> = clauses.iter().map(|c| expand_question_marks(c)).collect();
    let next_line = format!("\n{indent}");

    match variant {
        // The whole line of the goal, after its indentation, becomes the
        // clauses, one per line.
        MakeCaseVariant::Function => {
            let mut line_end = chars[goal.start..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(chars.len(), |i| goal.start + i);
            if line_end > code_start && chars[line_end - 1] == '\r' {
                line_end -= 1;
            }
            // Agda's clauses repeat the code after the goal (`? + m`) but
            // not a comment; keep a line comment after the goal, with the
            // space before it, at the end of the first clause.
            let rest: String = chars[goal.end..line_end].iter().collect();
            let mut clauses = clauses;
            if let (Some(first), Some(comment)) = (clauses.first_mut(), line_comment(&rest)) {
                first.push_str(comment);
            }
            (code_start, line_end, clauses.join(&next_line))
        }
        // Only the clause of the goal, up to the goal's end: in `λ { … }` it
        // starts after the `{` or the `;` before it, and the clauses are
        // separated by `;`; in `λ where` it is the whole line.
        MakeCaseVariant::ExtendedLambda => {
            let mut start = goal.start;
            let mut depth = 0i32;
            let before = |i: usize| i.checked_sub(1).map(|i| chars[i]);
            while before(start) != Some(';') && depth >= 0 && start > code_start {
                start -= 1;
                match before(start) {
                    Some('}') => depth += 1,
                    Some('{') => depth -= 1,
                    _ => {}
                }
            }
            if start == code_start {
                (start, goal.end, clauses.join(&next_line))
            } else {
                (start, goal.end, format!(" {}", clauses.join(" ; ")))
            }
        }
    }
}

/// The text that adds a with-abstraction on `goal`, as Idris's "add with"
/// does: the clause `lhs = {! e !}` becomes `lhs with e` and a clause
/// `... | w = {!  !}` for its result, `w` being a name the clause does not
/// use yet. Without an expression in the goal, the with-expression is a new
/// goal. Only for a goal that is the whole right-hand side of a clause on one
/// line; returns the replaced char range and its replacement.
pub fn add_with(text: &str, goal: &Goal) -> Option<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let line_start = chars[..goal.start]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1);
    let indent: String = chars[line_start..]
        .iter()
        .take_while(|&&c| c == ' ' || c == '\t')
        .collect();
    let code_start = line_start + indent.chars().count();
    let mut line_end = chars[goal.end..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(chars.len(), |i| goal.end + i);
    if line_end > goal.end && chars[line_end - 1] == '\r' {
        line_end -= 1;
    }
    // `lhs =` before the goal, and nothing but a comment after it.
    let before: String = chars[code_start..goal.start].iter().collect();
    let lhs = before.trim_end().strip_suffix('=')?;
    if lhs.is_empty() || !lhs.ends_with(char::is_whitespace) {
        return None;
    }
    let lhs = lhs.trim_end();
    let rest: String = chars[goal.end..line_end].iter().collect();
    let comment = line_comment(&rest);
    if comment.is_none() && !rest.trim().is_empty() {
        return None;
    }
    let content = goal.content(text);
    let expression = match content.is_empty() {
        true => GOAL_MARKER.to_string(),
        false => content.clone(),
    };
    let used = |name: &str| {
        format!("{lhs} {content}")
            .split(|c: char| c.is_whitespace() || "(){}\";.@".contains(c))
            .any(|word| word == name)
    };
    let name = std::iter::once("w".to_string())
        .chain(["₁", "₂", "₃", "₄", "₅", "₆", "₇", "₈", "₉"].map(|i| format!("w{i}")))
        .find(|name| !used(name))?;
    let replacement = format!(
        "{lhs} with {expression}{}\n{indent}... | {name} = {GOAL_MARKER}",
        comment.unwrap_or_default()
    );
    Some((code_start, line_end, replacement))
}

/// The line comment at the end of `rest`, with the whitespace before it: a
/// `--` at the start or after whitespace (elsewhere it is part of a name).
fn line_comment(rest: &str) -> Option<&str> {
    let dashes = rest.char_indices().find_map(|(i, c)| {
        let starts = c == '-'
            && rest[i..].starts_with("--")
            && rest[..i]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
        starts.then_some(i)
    })?;
    let space = rest[..dashes].len() - rest[..dashes].trim_end().len();
    Some(&rest[dashes - space..])
}

/// Char offsets (relative to `text`) where [`GOAL_MARKER`]s start.
pub fn marker_offsets(text: &str) -> Vec<usize> {
    let chars: Vec<char> = text.chars().collect();
    let marker: Vec<char> = GOAL_MARKER.chars().collect();
    (0..chars.len())
        .filter(|&i| chars[i..].starts_with(&marker))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::single_change;

    fn edit(goals: &mut Vec<Goal>, old: &str, new: &str) {
        let change = single_change(old, new).unwrap();
        adjust(goals, old, &change);
    }

    #[test]
    fn reads_goal_content() {
        let text = "f = {! suc n !} + {!  !} + ?";
        let goal = |start, end| Goal { id: 0, start, end };
        assert_eq!(goal(4, 15).content(text), "suc n");
        assert_eq!(goal(18, 24).content(text), "");
        assert_eq!(goal(27, 28).content(text), "");
    }

    #[test]
    fn follows_edits_before_inside_and_across_goals() {
        let old = "x = {!  !}";
        let mut goals = vec![Goal {
            id: 0,
            start: 4,
            end: 10,
        }];

        // Typing inside the hole grows it.
        edit(&mut goals, old, "x = {! y !}");
        assert_eq!(
            goals,
            vec![Goal {
                id: 0,
                start: 4,
                end: 11
            }]
        );

        // Typing before the hole shifts it.
        edit(&mut goals, "x = {! y !}", "xx = {! y !}");
        assert_eq!(
            goals,
            vec![Goal {
                id: 0,
                start: 5,
                end: 12
            }]
        );

        // Typing right after the hole leaves it alone.
        edit(&mut goals, "xx = {! y !}", "xx = {! y !} z");
        assert_eq!(
            goals,
            vec![Goal {
                id: 0,
                start: 5,
                end: 12
            }]
        );

        // Deleting a delimiter removes it.
        edit(&mut goals, "xx = {! y !} z", "xx = {! y } z");
        assert_eq!(goals, vec![]);
    }

    #[test]
    fn removes_a_goal_that_is_replaced() {
        let mut goals = vec![Goal {
            id: 3,
            start: 4,
            end: 5,
        }];
        edit(&mut goals, "x = ?", "x = zero");
        assert_eq!(goals, vec![]);
    }

    #[test]
    fn expands_lone_question_marks_only() {
        assert_eq!(expand_question_marks("suc ?"), "suc {!  !}");
        assert_eq!(expand_question_marks("(? , ?)"), "({!  !} , {!  !})");
        assert_eq!(expand_question_marks("_≟?_ x"), "_≟?_ x");
        assert_eq!(marker_offsets("suc {!  !} {!  !}"), vec![4, 11]);
    }

    /// Case split the first `{! … !}` in `text` into `clauses`.
    fn split(text: &str, variant: MakeCaseVariant, clauses: &[&str]) -> String {
        let chars: Vec<char> = text.chars().collect();
        let find = |pattern: [char; 2]| chars.windows(2).position(|w| w == pattern).unwrap();
        let goal = Goal {
            id: 0,
            start: find(['{', '!']),
            end: find(['!', '}']) + 2,
        };
        let clauses: Vec<String> = clauses.iter().map(|c| c.to_string()).collect();
        let (start, end, replacement) = case_split(text, &goal, variant, &clauses);
        chars[..start]
            .iter()
            .copied()
            .chain(replacement.chars())
            .chain(chars[end..].iter().copied())
            .collect()
    }

    #[test]
    fn case_split_replaces_the_line_of_a_function_clause() {
        // The clauses Agda 2.8.0 sends for `n + m = {! n !}`.
        let clauses = ["zero + m = ?", "suc n + m = ?"];
        assert_eq!(
            split(
                "_+_ : ℕ → ℕ → ℕ\nn + m = {! n !}\nx = y\n",
                MakeCaseVariant::Function,
                &clauses
            ),
            "_+_ : ℕ → ℕ → ℕ\nzero + m = {!  !}\nsuc n + m = {!  !}\nx = y\n"
        );
        // In a `where` block every clause keeps the indentation, and a
        // comment after the goal stays on the first clause (Emacs drops it).
        assert_eq!(
            split(
                "  where\n    n + m = {! n !}  -- todo\n",
                MakeCaseVariant::Function,
                &clauses
            ),
            "  where\n    zero + m = {!  !}  -- todo\n    suc n + m = {!  !}\n"
        );
        // Code after the goal is in Agda's clauses already, as Agda 2.8
        // sends them for `g n m = {! n !} + m`; `--` inside a name is no
        // comment.
        assert_eq!(
            split(
                "g n m = {! n !} + m x--y\n",
                MakeCaseVariant::Function,
                &["g zero m = ? + m x--y", "g (suc n) m = ? + m x--y"]
            ),
            "g zero m = {!  !} + m x--y\ng (suc n) m = {!  !} + m x--y\n"
        );
        // A Windows line end stays.
        assert_eq!(
            split("n + m = {! n !}\r\n", MakeCaseVariant::Function, &clauses),
            "zero + m = {!  !}\nsuc n + m = {!  !}\r\n"
        );
    }

    #[test]
    fn case_split_replaces_one_clause_of_an_extended_lambda() {
        // The clauses Agda 2.8.0 sends for `λ { x → {! x !} }`.
        let clauses = ["zero → ?", "(suc x) → ?"];
        assert_eq!(
            split(
                "f = λ { x → {! x !} }\n",
                MakeCaseVariant::ExtendedLambda,
                &clauses
            ),
            "f = λ { zero → {!  !} ; (suc x) → {!  !} }\n"
        );
        // Only the clause after the last `;`, and braces of implicit
        // arguments are skipped.
        assert_eq!(
            split(
                "f = λ { zero → zero ; {y} x → {! x !} }",
                MakeCaseVariant::ExtendedLambda,
                &["{y} zero → ?", "{y} (suc x) → ?"]
            ),
            "f = λ { zero → zero ; {y} zero → {!  !} ; {y} (suc x) → {!  !} }"
        );
        // In `λ where`, the clauses go on lines of their own.
        assert_eq!(
            split(
                "g = λ where\n  x → {! x !}\nh = g\n",
                MakeCaseVariant::ExtendedLambda,
                &clauses
            ),
            "g = λ where\n  zero → {!  !}\n  (suc x) → {!  !}\nh = g\n"
        );
    }

    /// Add a with-abstraction on the first goal in `text`.
    fn with(text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let find = |pattern: [char; 2]| chars.windows(2).position(|w| w == pattern).unwrap();
        let goal = Goal {
            id: 0,
            start: find(['{', '!']),
            end: find(['!', '}']) + 2,
        };
        let (start, end, replacement) = add_with(text, &goal)?;
        Some(
            chars[..start]
                .iter()
                .copied()
                .chain(replacement.chars())
                .chain(chars[end..].iter().copied())
                .collect(),
        )
    }

    #[test]
    fn adds_a_with_abstraction() {
        // The expression in the goal becomes the with-expression.
        assert_eq!(
            with("f : ℕ → ℕ\nf n = {! even n !}\nx = y\n").unwrap(),
            "f : ℕ → ℕ\nf n with even n\n... | w = {!  !}\nx = y\n"
        );
        // An empty goal gives a goal as with-expression; the indentation and
        // a comment stay, and `w`, used in the clause, is not taken.
        assert_eq!(
            with("  where\n    g w = {!  !}  -- todo\n").unwrap(),
            "  where\n    g w with {!  !}  -- todo\n    ... | w₁ = {!  !}\n"
        );
        // A nested with-clause.
        assert_eq!(
            with("... | x = {! y !}\n").unwrap(),
            "... | x with y\n... | w = {!  !}\n"
        );
        // Not when the goal is only part of the right-hand side.
        assert_eq!(with("f n = suc {! n !}\n"), None);
        assert_eq!(with("f n = {! n !} + 1\n"), None);
        assert_eq!(with("f = λ x → {! x !}\n"), None);
    }

    #[test]
    fn finds_the_word_in_a_goal() {
        let text = "f n = {! suc (N.n + m) !} x";
        let goal = Goal {
            id: 0,
            start: 6,
            end: 25,
        };
        // On, in and right after a name; a qualified name stays whole.
        assert_eq!(word_at(text, &goal, 9).as_deref(), Some("suc"));
        assert_eq!(word_at(text, &goal, 10).as_deref(), Some("suc"));
        assert_eq!(word_at(text, &goal, 12).as_deref(), Some("suc"));
        assert_eq!(word_at(text, &goal, 14).as_deref(), Some("N.n"));
        // Not on the delimiters, between words, or outside the goal.
        assert_eq!(word_at(text, &goal, 6), None);
        assert_eq!(word_at(text, &goal, 23), None);
        assert_eq!(word_at(text, &goal, 26), None);
        // A lone `?` has no text.
        let lone = Goal {
            id: 0,
            start: 4,
            end: 5,
        };
        assert_eq!(word_at("f = ?", &lone, 4), None);
    }
}
