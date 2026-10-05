//! Unicode input through completions, because a Zed extension cannot add an
//! input method: `\` followed by an abbreviation of Agda's Emacs input
//! method, as in `\to` for `→`, or `#` followed by Typst's notation: the name
//! of a symbol (`#arrow.r`), a math shorthand (`#->`) or an accent
//! (`#acute(e)` for `é`).
//!
//! The abbreviations are agda2-vscode's dump of Agda's `agda-input.el`, which
//! includes the TeX input method of Emacs (see `THIRD-PARTY-NOTICES.md`). The
//! Typst symbols come from the `codex` crate, which Typst itself uses; the
//! shorthands and accents from Typst's documentation.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::OnceLock;

use codex::{Def, Module};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// At most this many candidates are offered at once; typing more of the
/// name narrows them down.
pub const LIMIT: usize = 200;

/// How symbols are typed, from the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// `\` and Agda's abbreviations, which include LaTeX's names.
    pub latex: bool,
    /// `#` and Typst's notation.
    pub typst: bool,
    /// Whether a leader only counts at the start of a line or after
    /// whitespace, so that `{-#` or `x\y` stay as they are.
    pub only_after_whitespace: bool,
    /// Whether a space follows the symbol, unless one is already there
    /// (on by default).
    pub trailing_space: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            latex: true,
            typst: true,
            only_after_whitespace: false,
            trailing_space: true,
        }
    }
}

impl Options {
    /// Read `symbolInput` (`"both"`, `"latex"`, `"typst"` or `"none"`),
    /// `symbolTrailingSpace` and `symbolOnlyAfterWhitespace`. Values of the
    /// wrong kind keep their default and are reported.
    pub fn read(options: &Value) -> (Options, Vec<String>) {
        let mut result = Options::default();
        let mut problems = Vec::new();
        match &options["symbolInput"] {
            Value::Null => {}
            Value::String(mode) if mode == "both" => {}
            Value::String(mode) if mode == "latex" => result.typst = false,
            Value::String(mode) if mode == "typst" => result.latex = false,
            Value::String(mode) if mode == "none" => (result.latex, result.typst) = (false, false),
            other => problems.push(format!(
                "`symbolInput` must be \"both\", \"latex\", \"typst\" or \"none\", not {other}."
            )),
        }
        let mut flag = |key: &str, field: &mut bool| match &options[key] {
            Value::Null => {}
            Value::Bool(value) => *field = *value,
            other => problems.push(format!("`{key}` must be true or false, not {other}.")),
        };
        flag("symbolTrailingSpace", &mut result.trailing_space);
        flag(
            "symbolOnlyAfterWhitespace",
            &mut result.only_after_whitespace,
        );
        (result, problems)
    }

    /// Whether any symbol input is on.
    pub fn enabled(&self) -> bool {
        self.latex || self.typst
    }
}

/// A symbol to offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The name with its leader, as in `\to`, `#arrow.r` or `#->`.
    pub name: String,
    pub symbol: String,
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
    /// Whether a space should follow the symbol.
    pub space_after: bool,
}

impl Completions {
    /// The text that replaces the leader and what was typed after it.
    pub fn insertion(&self, candidate: &Candidate) -> String {
        match self.space_after {
            true => format!("{} ", candidate.symbol),
            false => candidate.symbol.clone(),
        }
    }
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
pub fn complete(line: &str, column: u32, options: &Options) -> Option<Completions> {
    let before = prefix_utf16(line, column)?;
    let candidates = |leader: char, found: Vec<(String, String)>| -> Vec<Candidate> {
        found
            .into_iter()
            .map(|(name, symbol)| Candidate {
                name: format!("{leader}{name}"),
                symbol,
            })
            .collect()
    };
    // An abbreviation first, so `\#` stays Agda's `♯`; when it has no
    // candidates, a Typst name may still follow a `#` in it.
    let latex = options
        .latex
        .then(|| typed_after('\\', before, options))
        .flatten()
        .map(|typed| {
            let found = abbreviations(typed)
                .into_iter()
                .map(|(name, symbol)| (name.to_string(), symbol.to_string()))
                .collect();
            (typed, candidates('\\', found))
        })
        .filter(|(_, found)| !found.is_empty());
    let (typed, mut candidates) = latex.or_else(|| {
        let typed = options
            .typst
            .then(|| typed_after('#', before, options))
            .flatten()?;
        Some((typed, candidates('#', typst(typed))))
    })?;
    if candidates.is_empty() {
        return None;
    }
    let truncated = candidates.len() > LIMIT;
    candidates.truncate(LIMIT);
    let leader_length = 1 + typed.encode_utf16().count() as u32;
    let next = line[before.len()..].chars().next();
    Some(Completions {
        start: column - leader_length,
        candidates,
        truncated,
        space_after: options.trailing_space && next.is_none_or(|c| !c.is_whitespace()),
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

/// What was typed after the last `leader` of `before`, if no whitespace
/// follows it, and, with `only_after_whitespace`, if whitespace or the start
/// of the line comes before it.
fn typed_after<'a>(leader: char, before: &'a str, options: &Options) -> Option<&'a str> {
    let start = before.rfind(|c: char| c == leader || c.is_whitespace())?;
    if !before[start..].starts_with(leader) {
        return None;
    }
    let after_whitespace = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (after_whitespace || !options.only_after_whitespace).then(|| &before[start + 1..])
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

/// Typst's math shorthands, from the list in its documentation of symbols.
const SHORTHANDS: &[(&str, &str)] = &[
    ("...", "…"),
    ("-", "−"),
    ("*", "∗"),
    ("~", "∼"),
    ("!=", "≠"),
    (":=", "≔"),
    ("::=", "⩴"),
    ("=:", "≕"),
    ("<<", "≪"),
    ("<<<", "⋘"),
    (">>", "≫"),
    (">>>", "⋙"),
    ("<=", "≤"),
    (">=", "≥"),
    ("->", "→"),
    ("-->", "⟶"),
    ("|->", "↦"),
    (">->", "↣"),
    ("->>", "↠"),
    ("<-", "←"),
    ("<--", "⟵"),
    ("<-<", "↢"),
    ("<<-", "↞"),
    ("<->", "↔"),
    ("<-->", "⟷"),
    ("~>", "⇝"),
    ("~~>", "⟿"),
    ("<~", "⇜"),
    ("<~~", "⬳"),
    ("=>", "⇒"),
    ("|=>", "⤇"),
    ("==>", "⟹"),
    ("<==", "⟸"),
    ("<=>", "⇔"),
    ("<==>", "⟺"),
    ("[|", "⟦"),
    ("|]", "⟧"),
    ("||", "‖"),
];

/// Typst's accents, from its documentation of `accent`, as the combining
/// characters Typst puts over the base.
const ACCENTS: &[(&str, char)] = &[
    ("grave", '\u{300}'),
    ("acute", '\u{301}'),
    ("hat", '\u{302}'),
    ("tilde", '\u{303}'),
    ("macron", '\u{304}'),
    ("dash", '\u{305}'),
    ("breve", '\u{306}'),
    ("dot", '\u{307}'),
    ("dot.double", '\u{308}'),
    ("diaer", '\u{308}'),
    ("dot.triple", '\u{20DB}'),
    ("dot.quad", '\u{20DC}'),
    ("circle", '\u{30A}'),
    ("acute.double", '\u{30B}'),
    ("caron", '\u{30C}'),
    ("arrow", '\u{20D7}'),
    ("arrow.l", '\u{20D6}'),
    ("arrow.l.r", '\u{20E1}'),
    ("harpoon", '\u{20D1}'),
    ("harpoon.lt", '\u{20D0}'),
];

/// The candidates for what was typed after `#`: an accent, shorthands that
/// start with it, and symbol names.
fn typst(typed: &str) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = accented(typed).into_iter().collect();
    let mut shorthands: Vec<&(&str, &str)> = SHORTHANDS
        .iter()
        .filter(|(shorthand, _)| shorthand.starts_with(typed))
        .collect();
    shorthands.sort_by_key(|(shorthand, _)| (*shorthand != typed, shorthand.len()));
    found.extend(
        shorthands
            .into_iter()
            .map(|(shorthand, symbol)| (shorthand.to_string(), symbol.to_string())),
    );
    if typed.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        found.extend(
            symbols(typed)
                .into_iter()
                .map(|(name, symbol)| (name, symbol.to_string())),
        );
    }
    found
}

/// An accent as Typst writes it in math, `acute(e)`, perhaps without its
/// closing parenthesis yet, and around a letter, a symbol name or another
/// accent (`macron(diaer(u))` for `ǖ`). The result is composed into one
/// character where Unicode has one.
fn accented(typed: &str) -> Option<(String, String)> {
    let (name, rest) = typed.split_once('(')?;
    let mark = ACCENTS.iter().find(|(accent, _)| *accent == name)?.1;
    let inner = rest.strip_suffix(')').unwrap_or(rest);
    let (inner_name, base) = if inner.contains('(') {
        accented(inner)?
    } else {
        accent_base(inner)?
    };
    let symbol: String = base.chars().chain([mark]).nfc().collect();
    Some((format!("{name}({inner_name})"), symbol))
}

/// The base of an accent: one character, or the name of a Typst symbol.
fn accent_base(inner: &str) -> Option<(String, String)> {
    let mut chars = inner.chars();
    match (chars.next(), chars.next()) {
        (None, _) => None,
        (Some(c), None) if !c.is_whitespace() => Some((inner.to_string(), inner.to_string())),
        _ => symbol_table()
            .iter()
            .find(|variant| variant.path() == inner)
            .map(|variant| (inner.to_string(), variant.symbol.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Completion with the default options.
    fn complete(line: &str, column: u32) -> Option<Completions> {
        super::complete(line, column, &Options::default())
    }

    fn names(completions: &Completions) -> Vec<(&str, &str)> {
        completions
            .candidates
            .iter()
            .map(|candidate| (candidate.name.as_str(), candidate.symbol.as_str()))
            .collect()
    }

    fn pair(name: &str, symbol: &str) -> (String, String) {
        (name.to_string(), symbol.to_string())
    }

    fn end(line: &str) -> u32 {
        line.encode_utf16().count() as u32
    }

    /// The first candidate for `line` with the cursor at its end.
    fn first(line: &str) -> (String, String) {
        let found = complete(line, end(line)).unwrap();
        let candidate = &found.candidates[0];
        (candidate.name.clone(), candidate.symbol.clone())
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
        assert_eq!(first("\\->"), pair("\\->", "→"));
        assert_eq!(first("\\=="), pair("\\==", "≡"));
        assert_eq!(first("\\bN"), pair("\\bN", "ℕ"));
        assert_eq!(first("\\Gl"), pair("\\Gl", "λ"));
        assert_eq!(first("\\_1"), pair("\\_1", "₁"));
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

        assert_eq!(first("#NN"), pair("#NN", "ℕ"));
        assert_eq!(first("#alph"), pair("#alpha", "α"));
        // Modifiers in any order, the last one partly typed.
        assert_eq!(first("#arrow.long.r"), pair("#arrow.r.long", "⟶"));
        assert_eq!(first("#arrow.r.doub"), pair("#arrow.r.double", "⇒"));
        // A symbol in a nested module.
        assert_eq!(first("#gender.fem"), pair("#gender.female", "♀\u{FE0E}"));
        // Without modifiers, Typst takes the first variant; it comes first.
        assert_eq!(first("#arrow"), pair("#arrow.r", "→"));
    }

    #[test]
    fn typst_shorthands_and_accents_after_a_hash() {
        // Math shorthands, the exact one first.
        let found = complete("f : A #->", 9).unwrap();
        assert_eq!(found.start, 6);
        assert_eq!(names(&found)[..2], [("#->", "→"), ("#->>", "↠")]);
        assert_eq!(first("#=>"), pair("#=>", "⇒"));
        assert_eq!(first("#[|"), pair("#[|", "⟦"));
        assert_eq!(first("#!="), pair("#!=", "≠"));
        // `-` alone is the minus sign, and `...` the ellipsis.
        assert_eq!(first("#-"), pair("#-", "−"));
        assert_eq!(first("#..."), pair("#...", "…"));
        // Accents, with or without the closing parenthesis, composed where
        // Unicode has one character.
        assert_eq!(first("#acute(e)"), pair("#acute(e)", "é"));
        assert_eq!(first("#diaer(o"), pair("#diaer(o)", "ö"));
        assert_eq!(first("#macron(diaer(U))"), pair("#macron(diaer(U))", "Ǖ"));
        assert_eq!(first("#hat(alpha)"), pair("#hat(alpha)", "α\u{302}"));
        assert_eq!(first("#arrow(x)"), pair("#arrow(x)", "x\u{20D7}"));
        // An unknown accent or base gives nothing.
        assert_eq!(complete("#acute(zz)", 10), None);
        assert_eq!(complete("#sharp(e)", 9), None);
    }

    #[test]
    fn options_choose_the_leaders_and_where_they_count() {
        let both = Options::default();
        // By default both leaders count anywhere, also in a pragma.
        assert!(super::complete("x#NN", 4, &both).is_some());
        assert!(super::complete("x\\to", 4, &both).is_some());
        assert!(super::complete("{-#", 3, &both).is_some());
        // Only after whitespace: for both leaders alike.
        let spaced = Options {
            only_after_whitespace: true,
            ..both
        };
        assert_eq!(super::complete("x#NN", 4, &spaced), None);
        assert_eq!(super::complete("x\\to", 4, &spaced), None);
        assert_eq!(super::complete("{-#", 3, &spaced), None);
        assert!(super::complete("a #NN", 5, &spaced).is_some());
        assert!(super::complete("\\to", 3, &spaced).is_some());
        // One leader only, or none.
        let latex = Options {
            typst: false,
            ..both
        };
        let typst = Options {
            latex: false,
            ..both
        };
        let none = Options {
            latex: false,
            typst: false,
            ..both
        };
        assert!(super::complete("\\to", 3, &latex).is_some());
        assert_eq!(super::complete("#NN", 3, &latex), None);
        assert_eq!(super::complete("\\to", 3, &typst), None);
        assert!(super::complete("#NN", 3, &typst).is_some());
        assert_eq!(super::complete("\\to", 3, &none), None);
        // `\#` stays Agda's sharp, and a Typst name may follow a `#` in a
        // failed abbreviation.
        assert_eq!(first("\\#"), pair("\\#", "♯"));
        assert_eq!(first("\\zz#NN"), pair("#NN", "ℕ"));
    }

    #[test]
    fn a_space_follows_by_default_unless_already_there() {
        let found = complete("a \\to", 5).unwrap();
        assert_eq!(found.insertion(&found.candidates[0]), "→ ");
        let found = complete("a \\to b", 5).unwrap();
        assert_eq!(found.insertion(&found.candidates[0]), "→");
        let options = Options {
            trailing_space: false,
            ..Options::default()
        };
        let found = super::complete("a \\to", 5, &options).unwrap();
        assert_eq!(found.insertion(&found.candidates[0]), "→");
    }

    #[test]
    fn reads_the_settings() {
        use serde_json::json;
        let read = |value| Options::read(&value);
        assert_eq!(read(json!({})), (Options::default(), vec![]));
        let (options, problems) = read(json!({
            "symbolInput": "typst",
            "symbolTrailingSpace": false,
            "symbolOnlyAfterWhitespace": true,
        }));
        assert!(problems.is_empty());
        assert_eq!(
            options,
            Options {
                latex: false,
                typst: true,
                only_after_whitespace: true,
                trailing_space: false,
            }
        );
        assert!(!read(json!({ "symbolInput": "none" })).0.enabled());
        let (options, problems) = read(json!({ "symbolInput": "tex", "symbolTrailingSpace": 1 }));
        assert_eq!(options, Options::default());
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn invisible_and_ascii_symbols_are_left_out() {
        // `\,` is a narrow no-break space in Emacs; `#paren.l` is `(`.
        assert!(
            complete("\\,", 2)
                .is_none_or(|found| found.candidates.iter().all(|c| useful(&c.symbol)))
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
