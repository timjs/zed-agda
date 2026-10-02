//! Unicode input through completions, because a Zed extension cannot add an
//! input method: `\` followed by an abbreviation of Agda's Emacs input
//! method, as in `\to` for `→`, or `#` followed by the name of a Typst
//! symbol, as in `#arrow.r` for `→`.
//!
//! The abbreviations are agda2-vscode's dump of Agda's `agda-input.el`, which
//! includes the TeX input method of Emacs (see `THIRD-PARTY-NOTICES.md`). The
//! Typst symbols come from the `codex` crate, which Typst itself uses.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::OnceLock;

use codex::{Def, Module};

/// At most this many candidates are offered at once; typing more of the
/// name narrows them down.
pub const LIMIT: usize = 200;

/// A symbol to offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The name with its leader, as in `\to` or `#arrow.r`.
    pub name: String,
    pub symbol: &'static str,
}

/// The candidates for what is being typed before the cursor.
#[derive(Debug, PartialEq, Eq)]
pub struct Completions {
    /// Where the leader is, in UTF-16 units from the start of the line.
    pub start: u32,
    /// The best candidates first.
    pub candidates: Vec<Candidate>,
    /// Whether more candidates matched than were kept.
    pub truncated: bool,
}

/// The non-word characters that can continue an abbreviation or a Typst
/// name. Zed only asks again for completions after a word character or one
/// of these, so they must all be trigger characters, or typing `\->` would
/// close the menu at `-`.
pub fn trigger_characters() -> Vec<String> {
    (b'!'..=b'~')
        .map(char::from)
        .filter(|c| c.is_ascii_punctuation() && *c != '_')
        .map(String::from)
        .collect()
}

/// The candidates for the input that ends at UTF-16 `column` of `line`, if
/// an abbreviation or a Typst name is being typed there.
pub fn complete(line: &str, column: u32) -> Option<Completions> {
    let before = prefix_utf16(line, column)?;
    let candidate = |leader: char| {
        move |(name, symbol): (&str, &'static str)| Candidate {
            name: format!("{leader}{name}"),
            symbol,
        }
    };
    let (typed, mut candidates): (&str, Vec<Candidate>) =
        if let Some(typed) = abbreviation_at(before) {
            let found = abbreviations(typed);
            (typed, found.into_iter().map(candidate('\\')).collect())
        } else {
            let typed = symbol_name_at(before)?;
            let found = symbols(typed);
            let found = found.iter().map(|(name, symbol)| (name.as_str(), *symbol));
            (typed, found.map(candidate('#')).collect())
        };
    if candidates.is_empty() {
        return None;
    }
    let truncated = candidates.len() > LIMIT;
    candidates.truncate(LIMIT);
    let leader_length = 1 + typed.encode_utf16().count() as u32;
    Some(Completions {
        start: column - leader_length,
        candidates,
        truncated,
    })
}

/// The part of `line` before UTF-16 `column`, if the column is on a
/// character boundary within the line.
fn prefix_utf16(line: &str, column: u32) -> Option<&str> {
    let mut units = 0;
    for (byte, c) in line.char_indices() {
        if units == column {
            return Some(&line[..byte]);
        }
        units += c.len_utf16() as u32;
    }
    (units == column).then_some(line)
}

/// The abbreviation typed after the last `\` of `before`, if no whitespace
/// follows that `\`.
fn abbreviation_at(before: &str) -> Option<&str> {
    let start = before.rfind(|c: char| c == '\\' || c.is_whitespace())?;
    before[start..]
        .starts_with('\\')
        .then(|| &before[start + 1..])
}

/// The Typst name typed after a `#` at the start of `before` or after
/// whitespace; elsewhere `#` belongs to Agda, as in the pragma `{-#`.
fn symbol_name_at(before: &str) -> Option<&str> {
    let start = before
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .filter(|&i| before[i..].starts_with('#'))?;
    let ok = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    ok.then(|| &before[start + 1..])
}

/// Whether `symbol` can be seen and is worth typing: not plain ASCII, and
/// without spaces or invisible characters, which only confuse in source code.
fn useful(symbol: &str) -> bool {
    !symbol.is_ascii()
        && !symbol.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
        })
}

/// Agda's abbreviations and their symbols, the first symbol being the usual
/// one. Abbreviations with `\` or spaces are left out: they are TeX accent
/// sequences such as `"\'I`, and the leader search would cut them anyway.
fn abbreviation_table() -> &'static BTreeMap<String, Vec<String>> {
    static TABLE: OnceLock<BTreeMap<String, Vec<String>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: BTreeMap<String, Vec<String>> =
            serde_json::from_str(include_str!("abbreviations.json"))
                .expect("abbreviations.json is valid");
        table.retain(|abbreviation, symbols| {
            symbols.retain(|symbol| useful(symbol));
            !symbols.is_empty() && !abbreviation.contains(|c: char| c == '\\' || c.is_whitespace())
        });
        table
    })
}

/// The abbreviations that start with `typed` and their symbols: the
/// abbreviation itself first, then shorter ones before longer ones.
fn abbreviations(typed: &str) -> Vec<(&'static str, &'static str)> {
    let mut found: Vec<(&str, &str)> = abbreviation_table()
        .range::<str, _>((Bound::Included(typed), Bound::Unbounded))
        .take_while(|(abbreviation, _)| abbreviation.starts_with(typed))
        .flat_map(|(abbreviation, symbols)| {
            symbols
                .iter()
                .map(move |symbol| (abbreviation.as_str(), symbol.as_str()))
        })
        .collect();
    // Stable, so abbreviations of one length stay in alphabetical order and
    // the symbols of one abbreviation in their own order.
    found.sort_by_key(|(abbreviation, _)| (*abbreviation != typed, abbreviation.len()));
    found
}

/// One variant of a Typst symbol.
struct Variant {
    /// The module path and the symbol, as in `arrow` or `gender.female`.
    name: String,
    /// Its modifiers, in codex's order.
    modifiers: Vec<&'static str>,
    symbol: &'static str,
}

impl Variant {
    fn path(&self) -> String {
        std::iter::once(self.name.as_str())
            .chain(self.modifiers.iter().copied())
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// Every variant of every symbol in Typst's `sym` module, in codex's order,
/// without deprecated names.
fn symbol_table() -> &'static [Variant] {
    fn walk(module: Module, prefix: &str, variants: &mut Vec<Variant>) {
        for (name, binding) in module.iter() {
            if binding.deprecation.is_some() {
                continue;
            }
            let name = format!("{prefix}{name}");
            match binding.def {
                Def::Module(module) => walk(module, &format!("{name}."), variants),
                Def::Symbol(symbol) => {
                    for (modifiers, symbol, deprecation) in symbol.variants() {
                        if deprecation.is_none() && useful(symbol) {
                            variants.push(Variant {
                                name: name.clone(),
                                modifiers: modifiers.into_iter().collect(),
                                symbol,
                            });
                        }
                    }
                }
            }
        }
    }
    static TABLE: OnceLock<Vec<Variant>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut variants = Vec::new();
        walk(codex::SYM, "", &mut variants);
        variants
    })
}

/// The Typst symbols that match `typed`: a name, perhaps partly typed, or a
/// name and modifiers in any order, the last perhaps partly typed, as in
/// `arrow.long.r` or `arrow.r.dou`. The exact match comes first, then
/// shorter names and variants with fewer modifiers.
fn symbols(typed: &str) -> Vec<(String, &'static str)> {
    let parts: Vec<&str> = typed.split('.').collect();
    let mut found: Vec<(bool, &Variant)> = symbol_table()
        .iter()
        .filter_map(|variant| {
            let name: Vec<&str> = variant.name.split('.').collect();
            if parts.len() <= name.len() {
                // Still typing the name.
                let (last, complete) = parts.split_last()?;
                let matches =
                    name[..complete.len()] == *complete && name[complete.len()].starts_with(last);
                let exact = parts == name && variant.modifiers.is_empty();
                return matches.then_some((exact, variant));
            }
            let (typed_name, modifiers) = parts.split_at(name.len());
            if typed_name != name {
                return None;
            }
            let (last, complete) = modifiers.split_last()?;
            let matches = complete.iter().all(|m| variant.modifiers.contains(m))
                && (last.is_empty()
                    || variant
                        .modifiers
                        .iter()
                        .any(|m| !complete.contains(m) && m.starts_with(last)));
            let exact = variant.modifiers.len() == modifiers.len()
                && modifiers.iter().all(|m| variant.modifiers.contains(m));
            matches.then_some((exact, variant))
        })
        .collect();
    found.sort_by_key(|(exact, variant)| (!exact, variant.name.len(), variant.modifiers.len()));
    found
        .into_iter()
        .map(|(_, variant)| (variant.path(), variant.symbol))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(completions: &Completions) -> Vec<(&str, &str)> {
        completions
            .candidates
            .iter()
            .map(|candidate| (candidate.name.as_str(), candidate.symbol))
            .collect()
    }

    fn end(line: &str) -> u32 {
        line.encode_utf16().count() as u32
    }

    /// The first candidate for `line` with the cursor at its end.
    fn first(line: &str) -> (String, &'static str) {
        let found = complete(line, end(line)).unwrap();
        let candidate = &found.candidates[0];
        (candidate.name.clone(), candidate.symbol)
    }

    #[test]
    fn abbreviations_after_a_backslash() {
        let line = "id : A \\to";
        let found = complete(line, end(line)).unwrap();
        assert_eq!(found.start, 7);
        assert!(!found.truncated);
        // The abbreviation itself first, then longer ones.
        assert_eq!(names(&found)[0], ("\\to", "→"));
        assert!(names(&found).contains(&("\\top", "⊤")));
        assert!(found.candidates.iter().all(|c| c.name.starts_with("\\to")));

        // Abbreviations with punctuation, case and several symbols.
        assert_eq!(first("\\->"), ("\\->".to_string(), "→"));
        assert_eq!(first("\\=="), ("\\==".to_string(), "≡"));
        assert_eq!(first("\\bN"), ("\\bN".to_string(), "ℕ"));
        assert_eq!(first("\\Gl"), ("\\Gl".to_string(), "λ"));
        assert_eq!(first("\\_1"), ("\\_1".to_string(), "₁"));
        let l = complete("\\l", 2).unwrap();
        assert_eq!(names(&l)[..2], [("\\l", "←"), ("\\l", "⇐")]);
    }

    #[test]
    fn the_leader_is_found_on_the_line() {
        // Columns are UTF-16 units: `𝔹` takes two.
        let line = "f : 𝔹 \\all";
        assert_eq!(complete(line, end(line)).unwrap().start, 7);
        // Halfway through the abbreviation, only what is before the cursor
        // counts; `\a` is not an abbreviation, so the shortest longer ones
        // come first.
        let found = complete(line, 9).unwrap();
        assert_eq!(found.start, 7);
        assert_eq!(names(&found)[0], ("\\aa", "å"));
        assert!(names(&found).contains(&("\\all", "∀")));
        // A space ends the abbreviation, so a lambda is left alone.
        assert_eq!(complete("\\x x", 4), None);
        // Nothing matches.
        assert_eq!(complete("\\zzz", 4), None);
        // A column beyond the line, or inside `𝔹`, is not answered.
        assert_eq!(complete("\\to", 9), None);
        assert_eq!(complete("𝔹\\to", 1), None);
        // Only the leader: everything, in parts.
        let all = complete("\\", 1).unwrap();
        assert!(all.truncated);
        assert_eq!(all.candidates.len(), LIMIT);
    }

    #[test]
    fn typst_names_after_a_hash() {
        let line = "f : A #arrow.r";
        let found = complete(line, end(line)).unwrap();
        assert_eq!(found.start, 6);
        assert_eq!(names(&found)[0], ("#arrow.r", "→"));
        assert!(names(&found).contains(&("#arrow.r.long", "⟶")));
        assert!(
            names(&found)
                .iter()
                .all(|(name, _)| name.starts_with("#arrow."))
        );

        assert_eq!(first("#NN"), ("#NN".to_string(), "ℕ"));
        assert_eq!(first("#alph"), ("#alpha".to_string(), "α"));
        // Modifiers in any order, the last one partly typed.
        assert_eq!(first("#arrow.long.r"), ("#arrow.r.long".to_string(), "⟶"));
        assert_eq!(first("#arrow.r.doub"), ("#arrow.r.double".to_string(), "⇒"));
        // A symbol in a nested module.
        assert_eq!(
            first("#gender.fem"),
            ("#gender.female".to_string(), "♀\u{FE0E}")
        );
        // Without modifiers, Typst takes the first variant; it comes first.
        assert_eq!(first("#arrow"), ("#arrow.r".to_string(), "→"));
    }

    #[test]
    fn hash_only_after_whitespace() {
        // Agda's own `#`, in pragmas and names, is left alone.
        assert_eq!(complete("{-#", 3), None);
        assert_eq!(complete("x#y", 3), None);
        // The `#` that closes a pragma follows a space, so it opens the menu,
        // but the `-` after it closes it again.
        assert!(complete("{-# OPTIONS --safe #", 20).is_some());
        assert_eq!(complete("{-# OPTIONS --safe #-", 21), None);
        assert!(complete("# x", 1).is_some());
        assert!(complete("a #", 3).is_some());
        // Typst names only have letters and dots.
        assert_eq!(complete("a #-", 4), None);
    }

    #[test]
    fn invisible_and_ascii_symbols_are_left_out() {
        // `\,` is a narrow no-break space in Emacs; `#paren.l` is `(`.
        assert!(
            complete("\\,", 2)
                .is_none_or(|found| found.candidates.iter().all(|c| useful(c.symbol)))
        );
        assert!(abbreviation_table().values().flatten().all(|s| useful(s)));
        assert!(symbol_table().iter().all(|v| useful(v.symbol)));
        assert!(!symbol_table().iter().any(|v| v.name == "space"));
        assert!(!abbreviation_table().contains_key("\"\\'I"));
    }

    #[test]
    fn trigger_characters_cover_all_abbreviations() {
        let triggers = trigger_characters();
        for abbreviation in abbreviation_table().keys() {
            for c in abbreviation.chars() {
                assert!(
                    c.is_alphanumeric() || c == '_' || triggers.contains(&c.to_string()),
                    "{c:?} in {abbreviation:?}"
                );
            }
        }
        assert!(triggers.contains(&"\\".to_string()) && triggers.contains(&"#".to_string()));
        assert!(triggers.contains(&".".to_string()));
    }
}
