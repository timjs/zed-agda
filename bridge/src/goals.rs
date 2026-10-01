//! Goals (Agda's interaction points) and how they follow edits.
//!
//! Agda reports goal ranges for the text it loaded. Between loads the user
//! keeps typing, so the bridge moves the ranges along with every change:
//! edits before a goal shift it, edits strictly inside `{! … !}` grow or
//! shrink it, and edits that touch its delimiters remove it (the same rule as
//! agda2-vscode's `adjustRangeContaining`).

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
}
