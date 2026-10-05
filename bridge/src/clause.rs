//! Adding a clause for a type signature, as Idris's "add clause" does:
//! `_+_ : ℕ → ℕ → ℕ` gets `n + m = {!  !}`. Agda has no command for it, so
//! the bridge reads the signature's text: the explicit arguments of the type
//! become pattern variables, named after their binders or after their types.

use crate::goals::GOAL_MARKER;

/// Words that start a line that is not a type signature to add a clause for.
const KEYWORDS: &[&str] = &[
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
    "where",
    "with",
    "rewrite",
    "let",
    "in",
    "...",
    "--",
    "{-",
];

/// Blocks whose signatures are not functions: constructors, fields,
/// postulates, generalised variables and primitives.
const BLOCKS: &[&str] = &[
    "data",
    "codata",
    "record",
    "field",
    "postulate",
    "variable",
    "primitive",
];

/// Characters that cannot be part of a name.
const RESERVED: &str = "(){}\";.@";

/// A type signature, perhaps over several lines.
#[derive(Debug, PartialEq, Eq)]
pub struct Signature {
    /// Its last line (0-based), after which the clauses go.
    pub last_line: usize,
    pub indent: String,
    pub names: Vec<String>,
    pub ty: String,
}

/// The type signature that starts on `line` of `text`, if it is one that
/// clauses can be added for.
pub fn signature_at(text: &str, line: usize) -> Option<Signature> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let first = *lines.get(line)?;
    let indent: String = first
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let rest = without_comment(first[indent.len()..].trim_end());
    let word = rest.split_whitespace().next()?;
    if KEYWORDS.contains(&word) {
        return None;
    }
    let colon = top_level_colon(rest)?;
    let names: Vec<String> = rest[..colon].split_whitespace().map(String::from).collect();
    if names.is_empty()
        || names
            .iter()
            .any(|name| name == "=" || name.contains(|c| RESERVED.contains(c)))
    {
        return None;
    }
    // Not a constructor, field, postulate or variable: look at the block the
    // line is in.
    let width = |l: &str| l.chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let depth = width(first);
    let block = lines[..line]
        .iter()
        .rev()
        .map(|l| l.trim_end())
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("--"))
        .find(|l| width(l) < depth);
    if let Some(block) = block {
        let word = block.split_whitespace().next().unwrap_or_default();
        if BLOCKS.contains(&word) {
            return None;
        }
    }
    // The type goes on over more indented lines.
    let mut ty = rest[colon + 1..].trim().to_string();
    let mut last_line = line;
    for (i, next) in lines.iter().enumerate().skip(line + 1) {
        if next.trim().is_empty() || width(next) <= depth {
            break;
        }
        ty.push(' ');
        ty.push_str(without_comment(next.trim()));
        last_line = i;
    }
    let ty = ty.trim().to_string();
    (!ty.is_empty()).then_some(Signature {
        last_line,
        indent,
        names,
        ty,
    })
}

/// The clauses for `signature`, one per name, each on a line of its own.
pub fn clauses(signature: &Signature) -> String {
    let arguments = arguments(&signature.ty, &signature.names);
    signature
        .names
        .iter()
        .map(|name| {
            format!(
                "{}{} = {GOAL_MARKER}",
                signature.indent,
                left_hand_side(name, &arguments)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `name` applied to `arguments`, in mixfix form when an operator has a
/// hole for every argument: `n + m` for `_+_`.
fn left_hand_side(name: &str, arguments: &[String]) -> String {
    let holes = name.split('_').count() - 1;
    if name.contains('_') && holes == arguments.len() && holes > 0 {
        let mut arguments = arguments.iter();
        let parts: Vec<&str> = name
            .split('_')
            .enumerate()
            .flat_map(|(i, part)| {
                let hole = (i > 0)
                    .then(|| arguments.next().map(String::as_str))
                    .flatten();
                [hole, (!part.is_empty()).then_some(part)]
            })
            .flatten()
            .collect();
        return parts.join(" ");
    }
    std::iter::once(name)
        .chain(arguments.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The pattern variables for the explicit arguments of `ty`, named after
/// their binders, or else after their types; never one of `taken`.
fn arguments(ty: &str, taken: &[String]) -> Vec<String> {
    let segments = split_arrows(ty);
    let domains = &segments[..segments.len().saturating_sub(1)];
    // Explicit arguments, each with its binder's name if it has one.
    let mut explicit: Vec<(Option<String>, String)> = Vec::new();
    let mut used: Vec<String> = taken.to_vec();
    for domain in domains {
        let domain = domain.trim();
        let (binders, forall) = match domain
            .strip_prefix('∀')
            .or_else(|| domain.strip_prefix("forall"))
        {
            Some(rest) => (rest.trim(), true),
            None => (domain, false),
        };
        match telescope(binders, forall) {
            Some(groups) => {
                for (is_explicit, names, ty) in groups {
                    for name in &names {
                        if name != "_" {
                            used.push(name.clone());
                        }
                    }
                    if is_explicit {
                        for name in names {
                            explicit.push(((name != "_").then_some(name), ty.clone()));
                        }
                    }
                }
            }
            None => explicit.push((None, domain.to_string())),
        }
    }
    explicit
        .into_iter()
        .map(|(name, ty)| {
            let name = name.unwrap_or_else(|| fresh(&ty, &used));
            used.push(name.clone());
            name
        })
        .collect()
}

/// The binders of a telescope such as `{A : Set} (x y : A)`, or after `∀`
/// also bare names (`∀ x`): (explicit, names, type). `None` when `text` is
/// not a telescope but a type, such as `(A → B)`.
fn telescope(text: &str, forall: bool) -> Option<Vec<(bool, Vec<String>, String)>> {
    let mut groups = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() {
        let (open, close, explicit) = if rest.starts_with("{{") {
            ("{{", "}}", false)
        } else if rest.starts_with('⦃') {
            ("⦃", "⦄", false)
        } else if rest.starts_with('{') {
            ("{", "}", false)
        } else if rest.starts_with('(') {
            ("(", ")", true)
        } else if forall {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            groups.push((true, vec![rest[..end].to_string()], String::new()));
            rest = rest[end..].trim_start();
            continue;
        } else {
            return None;
        };
        let end = closing(rest, open, close)?;
        let inner = &rest[open.len()..end];
        match top_level_colon(inner) {
            Some(colon) => {
                let names = inner[..colon]
                    .split_whitespace()
                    .map(String::from)
                    .collect();
                groups.push((explicit, names, inner[colon + 1..].trim().to_string()));
            }
            // `{A}` binds `A` without a type; `(A → B)` is no binder.
            None if !explicit || forall => {
                let names = inner.split_whitespace().map(String::from).collect();
                groups.push((explicit, names, String::new()));
            }
            None => return None,
        }
        rest = rest[end + close.len()..].trim_start();
    }
    (!groups.is_empty()).then_some(groups)
}

/// The byte offset of the `close` that matches the `open` at the start.
fn closing(text: &str, open: &str, close: &str) -> Option<usize> {
    let mut depth = 0;
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(open) {
            depth += 1;
            i += open.len();
        } else if text[i..].starts_with(close) {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
            i += close.len();
        } else {
            i += text[i..].chars().next()?.len_utf8();
        }
    }
    None
}

/// A name for an argument of type `ty`, from the family its type suggests,
/// that is not in `used`.
fn fresh(ty: &str, used: &[String]) -> String {
    let ty = strip_parens(ty.trim());
    let head = ty.split_whitespace().next().unwrap_or("");
    let family: Vec<String> = if split_arrows(ty).len() > 1 {
        words(&["f", "g", "h"])
    } else if ty.contains('≡') {
        words(&["p", "q", "r"])
    } else {
        match head {
            "ℕ" | "Nat" => words(&["n", "m", "k", "l", "j"]),
            "𝔹" | "Bool" => words(&["b", "c"]),
            "List" | "Vec" | "Vector" => words(&["xs", "ys", "zs"]),
            "Fin" | "ℤ" | "Int" => words(&["i", "j", "k"]),
            "String" => words(&["s", "t"]),
            "Char" => words(&["c", "d"]),
            _ if head.starts_with("Set") || head.starts_with("Prop") || head == "Type" => {
                words(&["A", "B", "C"])
            }
            _ => match head.chars().next() {
                // A type variable such as `A`, or a type that is no name.
                Some(c) if c.is_uppercase() && head.chars().count() == 1 => words(&["x", "y", "z"]),
                Some(c) if c.is_alphabetic() => vec![c.to_lowercase().collect()],
                _ => words(&["x", "y", "z"]),
            },
        }
    };
    let free = |name: &String| !used.contains(name);
    if let Some(name) = family.iter().find(|name| free(name)) {
        return name.clone();
    }
    (1..)
        .map(|i| format!("{}{}", family[0], subscript(i)))
        .find(free)
        .expect("some subscript is free")
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn subscript(n: usize) -> String {
    n.to_string()
        .chars()
        .map(|d| char::from_u32('₀' as u32 + d.to_digit(10).unwrap()).unwrap())
        .collect()
}

fn strip_parens(ty: &str) -> &str {
    match ty.strip_prefix('(') {
        Some(inner) if closing(ty, "(", ")") == Some(ty.len() - 1) => {
            strip_parens(inner[..inner.len() - 1].trim())
        }
        _ => ty,
    }
}

/// `ty` split at its arrows outside brackets: the arguments and the result.
fn split_arrows(ty: &str) -> Vec<String> {
    let mut segments = vec![String::new()];
    let mut depth = 0i32;
    let chars: Vec<char> = ty.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let before = i.checked_sub(1).map(|j| chars[j]);
        let separate = |len: usize| {
            before.is_none_or(|b| b.is_whitespace() || "(){}".contains(b))
                && chars.get(i + len).is_none_or(|a| a.is_whitespace())
        };
        match c {
            '(' | '{' | '⦃' => depth += 1,
            ')' | '}' | '⦄' => depth -= 1,
            _ => {}
        }
        if depth == 0 && c == '→' && separate(1) {
            segments.push(String::new());
        } else if depth == 0 && c == '-' && chars.get(i + 1) == Some(&'>') && separate(2) {
            segments.push(String::new());
            i += 1;
        } else {
            segments.last_mut().unwrap().push(c);
        }
        i += 1;
    }
    segments.into_iter().map(|s| s.trim().to_string()).collect()
}

/// The byte offset of the first `:` outside brackets with whitespace on
/// both sides.
fn top_level_colon(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    for (k, &(i, c)) in bytes.iter().enumerate() {
        match c {
            '(' | '{' | '⦃' => depth += 1,
            ')' | '}' | '⦄' => depth -= 1,
            ':' if depth == 0 => {
                let before = k.checked_sub(1).map(|j| bytes[j].1);
                let after = bytes.get(k + 1).map(|&(_, c)| c);
                if before.is_some_and(char::is_whitespace) && after.is_none_or(char::is_whitespace)
                {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `line` without a `--` comment that starts at whitespace.
fn without_comment(line: &str) -> &str {
    let mut previous = None;
    for (i, c) in line.char_indices() {
        if c == '-' && line[i..].starts_with("--") && previous.is_none_or(char::is_whitespace) {
            return line[..i].trim_end();
        }
        previous = Some(c);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clauses for the signature on the first line of `text`.
    fn add(text: &str) -> Option<String> {
        signature_at(text, 0).map(|signature| clauses(&signature))
    }

    #[test]
    fn names_arguments_after_their_types() {
        assert_eq!(add("_+_ : ℕ → ℕ → ℕ").unwrap(), "n + m = {!  !}");
        assert_eq!(add("not : 𝔹 → 𝔹").unwrap(), "not b = {!  !}");
        assert_eq!(
            add("map : {A B : Set} → (A → B) → List A → List B").unwrap(),
            "map f xs = {!  !}"
        );
        assert_eq!(
            add("zipWith : (A → B → C) → List A → List B → List C").unwrap(),
            "zipWith f xs ys = {!  !}"
        );
        assert_eq!(add("sym : x ≡ y → y ≡ x").unwrap(), "sym p = {!  !}");
        assert_eq!(add("id : {A : Set} → A → A").unwrap(), "id x = {!  !}");
        assert_eq!(add("const : A -> B -> A").unwrap(), "const x y = {!  !}");
        assert_eq!(
            add("if_then_else_ : 𝔹 → A → A → A").unwrap(),
            "if b then x else y = {!  !}"
        );
        assert_eq!(add("two : ℕ").unwrap(), "two = {!  !}");
        assert_eq!(add("depth : Tree A → ℕ").unwrap(), "depth t = {!  !}");
        // Many arguments of one type.
        assert_eq!(
            add("f : ℕ → ℕ → ℕ → ℕ → ℕ → ℕ → ℕ").unwrap(),
            "f n m k l j n₁ = {!  !}"
        );
    }

    #[test]
    fn keeps_the_names_of_binders() {
        assert_eq!(
            add("replicate : (n : ℕ) → A → Vec A n").unwrap(),
            "replicate n x = {!  !}"
        );
        assert_eq!(
            add("lookup : ∀ {n} (xs : Vec A n) (i : Fin n) → A").unwrap(),
            "lookup xs i = {!  !}"
        );
        assert_eq!(
            add("+-comm : ∀ m n → m + n ≡ n + m").unwrap(),
            "+-comm m n = {!  !}"
        );
        // An implicit `n` makes the next ℕ argument `m`.
        assert_eq!(add("g : {n : ℕ} → ℕ → ℕ").unwrap(), "g m = {!  !}");
        // A function named like an argument does not lend its name.
        assert_eq!(add("n : ℕ → ℕ").unwrap(), "n m = {!  !}");
    }

    #[test]
    fn finds_signatures_and_where_clauses_go() {
        // Over several lines, with the indentation of a `where` block.
        let text = "  where\n    go : ℕ\n       → ℕ  -- comment\n    go = x";
        let signature = signature_at(text, 1).unwrap();
        assert_eq!(signature.last_line, 2);
        assert_eq!(clauses(&signature), "    go n = {!  !}");
        // One clause per name.
        assert_eq!(add("f g : ℕ → ℕ").unwrap(), "f n = {!  !}\ng n = {!  !}");
        // Not for constructors, fields, postulates or other lines.
        assert_eq!(signature_at("data T : Set where\n  c : T", 1), None);
        assert_eq!(
            signature_at("record R : Set where\n  field\n    x : ℕ", 2),
            None
        );
        assert_eq!(signature_at("postulate\n  P : Set", 1), None);
        assert_eq!(signature_at("data T : Set where", 0), None);
        assert_eq!(signature_at("f n = g (x : A)", 0), None);
        assert_eq!(signature_at("-- f : ℕ", 0), None);
        assert_eq!(signature_at("f = λ x → x", 0), None);
    }
}
