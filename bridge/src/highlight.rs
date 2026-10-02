//! Agda's own highlighting, as LSP semantic tokens.
//!
//! While loading, Agda sends highlighting entries whose `atoms` say what a
//! stretch of text is. The bridge maps each atom to either a token type (a
//! kind of name, a keyword, a comment) or a modifier (a problem or a hole,
//! which `languages/agda/semantic_token_rules.json` shows as a background).
//!
//! Zed draws all tokens of one server as a single highlight layer, so where
//! two tokens overlap the inner one would cancel the outer one (`custom_highlights.rs`
//! in Zed). Agda's problems overlap names (a coverage problem covers a whole
//! clause), so the bridge flattens them into pieces that do not overlap, each
//! with the type of its name and the modifiers of every range covering it.

use tower_lsp_server::ls_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};

use crate::protocol::HighlightingEntry;
use crate::text::{Change, LineIndex};

/// Token types, in legend order. `punctuation`, `pragma` and `region` are not
/// standard LSP types; `semantic_token_rules.json` styles the first two, and
/// `region` (text that only has modifiers) gets only its modifiers' styles.
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "struct",
    "enumMember",
    "property",
    "function",
    "variable",
    "parameter",
    "macro",
    "keyword",
    "comment",
    "string",
    "number",
    "punctuation",
    "pragma",
    "region",
];

/// Modifiers, in legend order: modifier `i` is bit `1 << i`.
pub const TOKEN_MODIFIERS: &[&str] = &[
    "hole",
    "unsolvedMeta",
    "unsolvedConstraint",
    "terminationProblem",
    "positivityProblem",
    "coverageProblem",
    "confluenceProblem",
    "incompletePattern",
    "missingDefinition",
    "shadowingInTelescope",
    "catchallClause",
    "instanceProblem",
    "cosmeticProblem",
    "deadcode",
    "error",
    "errorWarning",
    "dottedPattern",
    "typeChecks",
];

/// Atoms that say what the text is, with their token type. When several
/// apply to the same text, the first in this list wins.
const KINDS: &[(&str, &str)] = &[
    ("bound", "variable"),
    ("generalizable", "variable"),
    ("argument", "parameter"),
    ("inductiveconstructor", "enumMember"),
    ("coinductiveconstructor", "enumMember"),
    ("field", "property"),
    ("function", "function"),
    ("postulate", "function"),
    ("macro", "macro"),
    ("primitive", "type"),
    ("primitivetype", "type"),
    ("datatype", "type"),
    ("record", "struct"),
    ("module", "namespace"),
    ("keyword", "keyword"),
    ("pragma", "pragma"),
    ("symbol", "punctuation"),
    ("comment", "comment"),
    ("markup", "comment"),
    ("string", "string"),
    ("number", "number"),
];

/// Atoms that decorate text rather than classify it, with their modifier.
const DECORATIONS: &[(&str, &str)] = &[
    ("hole", "hole"),
    ("unsolvedmeta", "unsolvedMeta"),
    ("unsolvedconstraint", "unsolvedConstraint"),
    ("terminationproblem", "terminationProblem"),
    ("positivityproblem", "positivityProblem"),
    ("coverageproblem", "coverageProblem"),
    ("confluenceproblem", "confluenceProblem"),
    ("incompletepattern", "incompletePattern"),
    ("missingdefinition", "missingDefinition"),
    ("shadowingintelescope", "shadowingInTelescope"),
    ("catchallclause", "catchallClause"),
    ("instanceproblem", "instanceProblem"),
    ("cosmeticproblem", "cosmeticProblem"),
    ("deadcode", "deadcode"),
    ("error", "error"),
    ("errorwarning", "errorWarning"),
    ("dottedpattern", "dottedPattern"),
    ("typechecks", "typeChecks"),
];

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES
            .iter()
            .map(|name| SemanticTokenType::new(name))
            .collect(),
        token_modifiers: TOKEN_MODIFIERS
            .iter()
            .map(|name| SemanticTokenModifier::new(name))
            .collect(),
    }
}

/// A highlighted range of 0-based char offsets: what it is (an index into
/// [`KINDS`], lower wins) and how it is decorated (modifier bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    kind: Option<usize>,
    modifiers: u32,
}

fn type_index(name: &str) -> u32 {
    TOKEN_TYPES
        .iter()
        .position(|t| *t == name)
        .expect("known token type") as u32
}

fn modifier_bit(name: &str) -> u32 {
    1 << TOKEN_MODIFIERS
        .iter()
        .position(|m| *m == name)
        .expect("known modifier")
}

/// Spans from the highlighting Agda sent while loading. Entries for the same
/// range (Agda sends a name and its problem separately, and repeats entries)
/// are merged into one span.
pub fn from_highlighting(entries: &[HighlightingEntry]) -> Vec<Span> {
    let mut spans: Vec<Span> = entries
        .iter()
        .filter_map(|entry| {
            let [from, to] = entry.range;
            let atoms = || entry.atoms.iter().map(String::as_str);
            let kind = atoms()
                .filter_map(|atom| KINDS.iter().position(|(name, _)| *name == atom))
                .min();
            let modifiers = atoms()
                .filter_map(|atom| DECORATIONS.iter().find(|(name, _)| *name == atom))
                .fold(0, |bits, (_, modifier)| bits | modifier_bit(modifier));
            (from < to && (kind.is_some() || modifiers != 0)).then_some(Span {
                start: from - 1,
                end: to - 1,
                kind,
                modifiers,
            })
        })
        .collect();
    spans.sort_by_key(|span| (span.start, span.end));
    spans.dedup_by(|later, earlier| {
        let same = (later.start, later.end) == (earlier.start, earlier.end);
        if same {
            // Not `Option::min`: that would prefer `None`.
            earlier.kind = match (earlier.kind, later.kind) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            earlier.modifiers |= later.modifiers;
        }
        same
    });
    spans
}

/// Move spans along with one change to the document; spans the change
/// touches are dropped until the next load.
pub fn adjust(spans: &mut Vec<Span>, change: &Change) {
    spans.retain_mut(|span| match change.follow(span.start, span.end) {
        Some((start, end)) => {
            (span.start, span.end) = (start, end);
            true
        }
        None => false,
    });
}

/// Pieces that do not overlap: each with the highest-priority kind and all
/// modifiers of the spans covering it. Neighbouring pieces that look the
/// same are joined.
fn flatten(spans: &[Span]) -> Vec<(usize, usize, Option<usize>, u32)> {
    let mut events: Vec<(usize, bool, &Span)> = spans
        .iter()
        .flat_map(|span| [(span.start, true, span), (span.end, false, span)])
        .collect();
    events.sort_by_key(|(offset, _, _)| *offset);

    let mut kinds = vec![0u32; KINDS.len()];
    let mut modifiers = [0u32; 32];
    let mut pieces: Vec<(usize, usize, Option<usize>, u32)> = Vec::new();
    let mut previous = 0;
    for (offset, starts, span) in events {
        if offset > previous {
            let kind = kinds.iter().position(|&count| count > 0);
            let bits = (0..32)
                .filter(|&bit| modifiers[bit] > 0)
                .fold(0, |bits, bit| bits | 1 << bit);
            if kind.is_some() || bits != 0 {
                match pieces.last_mut() {
                    Some(last) if last.1 == previous && (last.2, last.3) == (kind, bits) => {
                        last.1 = offset;
                    }
                    _ => pieces.push((previous, offset, kind, bits)),
                }
            }
            previous = offset;
        }
        let step = |count: &mut u32| {
            if starts {
                *count += 1;
            } else {
                *count -= 1;
            }
        };
        if let Some(kind) = span.kind {
            step(&mut kinds[kind]);
        }
        for (bit, count) in modifiers.iter_mut().enumerate() {
            if span.modifiers & (1 << bit) != 0 {
                step(count);
            }
        }
    }
    pieces
}

/// The LSP encoding of the spans in `text`: one token per piece and line,
/// with positions relative to the previous token.
pub fn tokens(spans: &[Span], text: &str) -> Vec<SemanticToken> {
    let index = LineIndex::new(text);
    let mut tokens = Vec::new();
    let (mut previous_line, mut previous_start) = (0, 0);
    for (start, end, kind, modifiers) in flatten(spans) {
        let token_type = match kind {
            Some(kind) => type_index(KINDS[kind].1),
            None => type_index("region"),
        };
        let end = end.min(index.len());
        let mut from = start;
        while from < end {
            let position = index.position(from);
            let line_end = index.line_end(position.line);
            let to = end.min(line_end);
            if to > from {
                let length = index.position(to).character - position.character;
                let delta_line = position.line - previous_line;
                let delta_start = if delta_line == 0 {
                    position.character - previous_start
                } else {
                    position.character
                };
                tokens.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length,
                    token_type,
                    token_modifiers_bitset: modifiers,
                });
                (previous_line, previous_start) = (position.line, position.character);
            }
            from = index.next_line_start(position.line);
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(range: [usize; 2], atoms: &[&str]) -> HighlightingEntry {
        HighlightingEntry {
            range,
            atoms: atoms.iter().map(|atom| atom.to_string()).collect(),
            definition_site: None,
        }
    }

    /// Tokens as (line, character, length, type, modifiers), absolute.
    fn decode(tokens: &[SemanticToken]) -> Vec<(u32, u32, u32, &'static str, Vec<&'static str>)> {
        let (mut line, mut character) = (0, 0);
        tokens
            .iter()
            .map(|token| {
                line += token.delta_line;
                character = if token.delta_line == 0 {
                    character + token.delta_start
                } else {
                    token.delta_start
                };
                let modifiers = (0..TOKEN_MODIFIERS.len())
                    .filter(|bit| token.token_modifiers_bitset & (1 << bit) != 0)
                    .map(|bit| TOKEN_MODIFIERS[bit])
                    .collect();
                (
                    line,
                    character,
                    token.length,
                    TOKEN_TYPES[token.token_type as usize],
                    modifiers,
                )
            })
            .collect()
    }

    #[test]
    fn rules_file_names_only_types_and_modifiers_of_the_legend() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../languages/agda/semantic_token_rules.json"
        );
        let text: String = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let rules: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
        for rule in &rules {
            if let Some(token_type) = rule["token_type"].as_str() {
                assert!(TOKEN_TYPES.contains(&token_type), "{token_type}");
            }
            for modifier in rule["token_modifiers"].as_array().into_iter().flatten() {
                let modifier = modifier.as_str().unwrap();
                assert!(TOKEN_MODIFIERS.contains(&modifier), "{modifier}");
            }
        }
        // Every decoration has a rule, so none is invisible.
        for (_, modifier) in DECORATIONS {
            assert!(
                rules
                    .iter()
                    .any(|rule| rule["token_modifiers"][0] == *modifier),
                "no rule for {modifier}"
            );
        }
    }

    #[test]
    fn merges_a_name_with_its_problem() {
        // Agda sends `loop` once as a function and once as a termination problem.
        let text = "loop n = loop n";
        let spans = from_highlighting(&[
            entry([1, 5], &["function"]),
            entry([1, 5], &["terminationproblem"]),
            entry([6, 7], &["bound"]),
            entry([1, 5], &["function"]),
        ]);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            decode(&tokens(&spans, text)),
            [
                (0, 0, 4, "function", vec!["terminationProblem"]),
                (0, 5, 1, "variable", vec![]),
            ]
        );
    }

    #[test]
    fn flattens_a_problem_over_several_names() {
        // `coverageproblem` covers the clause `partial zero`.
        let text = "partial zero = zero";
        let spans = from_highlighting(&[
            entry([1, 13], &["coverageproblem"]),
            entry([1, 8], &["function"]),
            entry([9, 13], &["inductiveconstructor"]),
            entry([14, 15], &["symbol"]),
        ]);
        assert_eq!(
            decode(&tokens(&spans, text)),
            [
                (0, 0, 7, "function", vec!["coverageProblem"]),
                (0, 7, 1, "region", vec!["coverageProblem"]),
                (0, 8, 4, "enumMember", vec!["coverageProblem"]),
                (0, 13, 1, "punctuation", vec![]),
            ]
        );
    }

    #[test]
    fn splits_at_line_ends_and_counts_utf16() {
        // `𝔹` takes two UTF-16 code units; the dead clause spans two lines.
        let text = "data 𝔹 : Set\ndead x =\n  x";
        let spans = from_highlighting(&[
            entry([1, 5], &["keyword"]),
            entry([6, 7], &["datatype"]),
            entry([8, 9], &["symbol"]),
            entry([14, 26], &["deadcode"]),
        ]);
        assert_eq!(
            decode(&tokens(&spans, text)),
            [
                (0, 0, 4, "keyword", vec![]),
                (0, 5, 2, "type", vec![]),
                (0, 8, 1, "punctuation", vec![]),
                (1, 0, 8, "region", vec!["deadcode"]),
                (2, 0, 3, "region", vec!["deadcode"]),
            ]
        );
    }

    #[test]
    fn follows_edits() {
        let mut spans =
            from_highlighting(&[entry([1, 5], &["keyword"]), entry([6, 7], &["datatype"])]);
        // Typing inside the keyword drops it; the name after it moves along.
        adjust(
            &mut spans,
            &Change {
                start: 2,
                old_end: 2,
                new_len: 1,
            },
        );
        assert_eq!(
            decode(&tokens(&spans, "datta ℕ")),
            [(0, 6, 1, "type", vec![])]
        );
    }
}
