//! Making a helper function for a goal, as Idris's "make lemma" does. The
//! goal's text is the call, `aux n m`; Agda gives the helper's type
//! (`Cmd_helper_function`), and the bridge adds that signature, with a clause
//! for it, above the definition the goal is in, and puts the call in place of
//! the goal.

use crate::clause;

/// Words that cannot name a helper function.
const KEYWORDS: &[&str] = &[
    "let", "in", "where", "with", "rewrite", "λ", "\\", "∀", "forall", "→", "->", "=", "|", ":",
    "?", "_", "...",
];

/// Words that start a line which is not part of a definition: a block, an
/// import or a fixity. The helper never goes above one of these.
const BOUNDARIES: &[&str] = &[
    "data",
    "codata",
    "record",
    "module",
    "open",
    "import",
    "postulate",
    "field",
    "constructor",
    "pattern",
    "syntax",
    "infix",
    "infixl",
    "infixr",
    "variable",
    "primitive",
    "mutual",
    "abstract",
    "private",
    "instance",
    "macro",
    "opaque",
    "unfolding",
    "interleaved",
    "{-#",
];

/// The name of the helper function that `text` calls: its first word, when
/// that can be a name.
pub fn head(text: &str) -> Option<&str> {
    let text = text.trim_start();
    let end = text
        .find(|c: char| c.is_whitespace() || "(){}\";@".contains(c))
        .unwrap_or(text.len());
    let word = &text[..end];
    let name = !word.is_empty()
        && !word.contains('.')
        && !word.starts_with(|c: char| c.is_ascii_digit())
        && !KEYWORDS.contains(&word);
    name.then_some(word)
}

/// The call in `text` with the name Agda gave the helper, which differs when
/// the name was taken (`suc₁` for `suc`).
pub fn call(text: &str, name: &str) -> Option<String> {
    let text = text.trim();
    let old = head(text)?;
    Some(format!("{name}{}", &text[old.len()..]))
}

/// The line above which the helper for a goal on `line` goes, and that
/// line's indentation: the start of the definition the goal is in, which is
/// its type signature, or, without one, the goal's clause. Walking up from
/// the goal, a less indented line is the clause the goal's line continues,
/// unless it starts a block (such as `where`) that the definition is in.
pub fn place(text: &str, line: usize) -> (usize, String) {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let line = line.min(lines.len().saturating_sub(1));
    let width = |l: &str| l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let indent = |l: usize| -> String {
        lines[l]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    };
    let mut level = width(lines[line]);
    // The goal's clause: the first line at the definition's level that does
    // not start with `...`, which a with-clause does.
    let mut clause = None;
    let mut top = line;
    for l in (0..=line).rev() {
        let current = lines[l];
        let code = current.trim();
        if code.is_empty() || code.starts_with("--") {
            continue;
        }
        let depth = width(current);
        if depth > level {
            continue;
        }
        let word = code.split_whitespace().next().unwrap_or_default();
        let opens_block = depth < level && (code == "where" || code.ends_with(" where"));
        if BOUNDARIES.contains(&word) || opens_block {
            break;
        }
        if depth < level {
            level = depth;
            clause = None;
        }
        if let Some(signature) = clause::signature_at(text, l) {
            // Only the goal's own signature, not one of a definition above.
            let clause = clause.unwrap_or(top);
            let at = if defines(&signature.names, lines[clause]) {
                l
            } else {
                clause
            };
            return (at, indent(at));
        }
        if clause.is_none() && !code.starts_with("...") {
            clause = Some(l);
        }
        top = l;
    }
    let at = clause.unwrap_or(top);
    (at, indent(at))
}

/// Whether `line` is a clause (or the signature) of one of `names`: it has
/// the name, or every part of an operator (`+` for `_+_`), as a word.
fn defines(names: &[String], line: &str) -> bool {
    let words: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || "(){}".contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    names.iter().any(|name| {
        let mut parts = name.split('_').filter(|part| !part.is_empty()).peekable();
        parts.peek().is_some() && parts.all(|part| words.contains(&part))
    })
}

/// The helper's declaration, to insert at the start of a line: Agda's
/// signature and a clause for it, with `indent` before every line, and a
/// blank line after them.
pub fn declaration(signature: &str, indent: &str) -> String {
    let signature = signature
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    match clause::signature_at(&signature, 0) {
        Some(parsed) => format!("{signature}\n{}\n\n", clause::clauses(&parsed)),
        None => format!("{signature}\n\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_helpers_name_and_renames_the_call() {
        assert_eq!(head("aux n m"), Some("aux"));
        assert_eq!(head(" lemma (suc n) "), Some("lemma"));
        assert_eq!(head("help{A}"), Some("help"));
        assert_eq!(head("+-comm′ n"), Some("+-comm′"));
        for text in [
            "",
            "(aux n)",
            "λ x → x",
            "1 + n",
            "N.suc n",
            "? n",
            "let x = n in x",
        ] {
            assert_eq!(head(text), None, "{text}");
        }
        assert_eq!(call(" suc n ", "suc₁").unwrap(), "suc₁ n");
        assert_eq!(call("aux", "aux").unwrap(), "aux");
    }

    #[test]
    fn goes_above_the_definition_the_goal_is_in() {
        let text = "\
module M where

data ℕ : Set where
  zero : ℕ
  suc  : ℕ → ℕ

_+_ : ℕ → ℕ → ℕ
zero  + m = m
suc n + m =
  suc {! aux n m !}

double : ℕ → ℕ
double n = twice n
  where
    twice : ℕ → ℕ
    twice k = {! go k !}

half : ℕ → ℕ
half n = h n
  where
    other : ℕ
    other = zero

    h = λ k → {! go k !}

long : ℕ →
       ℕ
long n = {! aux n !}
";
        // The signature of `_+_`, past its other clause and from the line
        // the clause goes on to.
        assert_eq!(place(text, 9), (6, String::new()));
        // In a `where` block, at its indentation.
        assert_eq!(place(text, 15), (14, "    ".to_string()));
        // Without a signature, the goal's clause, below a definition that has
        // one.
        assert_eq!(place(text, 23), (23, "    ".to_string()));
        // A signature over two lines.
        assert_eq!(place(text, 27), (25, String::new()));
        // Never above a `data` block or the module.
        assert_eq!(
            place("module M where\nx = {! aux !}\n", 1),
            (1, String::new())
        );
        assert_eq!(
            place("data D : Set where\n  c : D\nx = {! aux !}\n", 2),
            (2, String::new())
        );
        // Past with-clauses, and a `where` after the goal itself.
        let with = "f : ℕ → ℕ\nf n with n\n... | zero = zero\n... | suc k = {! aux k !}\n";
        assert_eq!(place(with, 3), (0, String::new()));
        let own = "f : ℕ → ℕ\nf n = {! aux n !} where\n  g : ℕ\n  g = zero\n";
        assert_eq!(place(own, 1), (0, String::new()));
        // A signature of another definition: the clause itself.
        assert_eq!(
            place("f : ℕ\nf = zero\ng = {! aux !}\n", 2),
            (2, String::new())
        );
    }

    #[test]
    fn declares_the_helper_with_a_clause() {
        assert_eq!(
            declaration("aux : (n m : ℕ) → ℕ", ""),
            "aux : (n m : ℕ) → ℕ\naux n m = {!  !}\n\n"
        );
        assert_eq!(
            declaration("help : ∀ {A} (x : A) → A", "    "),
            "    help : ∀ {A} (x : A) → A\n    help x = {!  !}\n\n"
        );
        assert_eq!(
            declaration("lemma : ∀ n → (n + zero) ≡ n → _ ≡ suc n", ""),
            "lemma : ∀ n → (n + zero) ≡ n → _ ≡ suc n\nlemma n p = {!  !}\n\n"
        );
        assert_eq!(declaration("aux : ℕ", ""), "aux : ℕ\naux = {!  !}\n\n");
        // Agda breaks a long type over lines indented under the name.
        assert_eq!(
            declaration(
                "helper : ∀ a b\n           (p : a ≡ b) →\n         b ≡ a",
                "  "
            ),
            "  helper : ∀ a b\n             (p : a ≡ b) →\n           b ≡ a\n  helper a b p = {!  !}\n\n"
        );
    }
}
