//! Renaming a name, which Agda's interaction protocol has no command for.
//!
//! The bridge renames every place whose link (from Agda's highlighting, see
//! `links.rs`) leads to the same definition site. This module decides what
//! each place becomes: Agda writes a name in full (`f`, `_+_`), qualified
//! (`N.suc`) or, for an operator, by its parts (`+` in `n + m`).

/// Characters that can never be part of an Agda name.
const RESERVED: &str = "(){}\";.@";

/// The parts of a name between its holes, with `None` for a hole:
/// `_+_` is `[None, Some("+"), None]` and `if_then_else_` has three parts.
fn layout(name: &str) -> Vec<Option<&str>> {
    name.split('_')
        .map(|part| (!part.is_empty()).then_some(part))
        .collect()
}

/// The full new name for renaming `old` to `new`, given that the rename was
/// asked on `occurrence` (the text of a place, perhaps qualified). Typing a
/// new name for one part of an operator renames that part: `⊕` on the `+`
/// of `n + m` makes `_+_` into `_⊕_`.
pub fn new_name(old: &str, new: &str, occurrence: &str) -> Result<String, String> {
    let new = new.trim();
    if new.is_empty()
        || new
            .chars()
            .any(|c| c.is_whitespace() || RESERVED.contains(c))
    {
        return Err(format!(
            "`{new}` is not a name: names have no spaces and none of {RESERVED}"
        ));
    }
    let old_layout = layout(old);
    if same_holes(&old_layout, &layout(new)) {
        return Ok(new.to_string());
    }
    // One part of an operator, named without underscores.
    let base = unqualified(occurrence);
    let slot = old_layout.iter().position(|part| *part == Some(base));
    if let (Some(slot), false, true) = (slot, new.contains('_'), old_layout.len() > 1) {
        let renamed: Vec<&str> = old_layout
            .iter()
            .enumerate()
            .map(|(i, part)| if i == slot { new } else { part.unwrap_or("") })
            .collect();
        return Ok(renamed.join("_"));
    }
    Err(format!(
        "`{new}` does not have the holes of `{old}`: an operator keeps its \
         underscores, as in `_+_` to `_⊕_`"
    ))
}

fn same_holes(a: &[Option<&str>], b: &[Option<&str>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.is_some() == b.is_some())
}

/// A name without its module qualifier: `suc` for `N.suc`.
pub fn unqualified(occurrence: &str) -> &str {
    occurrence.rsplit('.').next().unwrap_or(occurrence)
}

/// What one place becomes when `old` is renamed to `new`: the full name,
/// with its qualifier kept, or the matching part of an operator.
pub fn replacement(old: &str, new: &str, occurrence: &str) -> Option<String> {
    let base = unqualified(occurrence);
    let qualifier = &occurrence[..occurrence.len() - base.len()];
    if base == old {
        return Some(format!("{qualifier}{new}"));
    }
    let old_layout = layout(old);
    let new_layout = layout(new);
    let index = old_layout.iter().position(|part| *part == Some(base))?;
    new_layout[index].map(|part| format!("{qualifier}{part}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_and_completes_new_names() {
        assert_eq!(new_name("f", "g", "f").unwrap(), "g");
        assert_eq!(new_name("_+_", "_⊕_", "+").unwrap(), "_⊕_");
        // A new part typed on a part of the operator.
        assert_eq!(new_name("_+_", "⊕", "+").unwrap(), "_⊕_");
        assert_eq!(new_name("_+_", "⊕", "N.+").unwrap(), "_⊕_");
        assert_eq!(
            new_name("if_then_else_", "when", "if").unwrap(),
            "when_then_else_"
        );
        assert_eq!(
            new_name("if_then_else_", "otherwise", "else").unwrap(),
            "if_then_otherwise_"
        );
        // Holes cannot change, and names have no spaces or reserved
        // characters.
        assert!(new_name("f", "_f_", "f").is_err());
        assert!(new_name("_+_", "plus", "_+_").is_err());
        assert!(new_name("f", "a b", "f").is_err());
        assert!(new_name("f", "a.b", "f").is_err());
        assert!(new_name("f", "", "f").is_err());
    }

    #[test]
    fn replaces_full_qualified_and_partial_names() {
        assert_eq!(replacement("suc", "succ", "suc").unwrap(), "succ");
        assert_eq!(replacement("suc", "succ", "N.suc").unwrap(), "N.succ");
        assert_eq!(replacement("_+_", "_⊕_", "_+_").unwrap(), "_⊕_");
        assert_eq!(replacement("_+_", "_⊕_", "+").unwrap(), "⊕");
        assert_eq!(replacement("_+_", "_⊕_", "N.+").unwrap(), "N.⊕");
        assert_eq!(
            replacement("if_then_else_", "when_then_otherwise_", "else").unwrap(),
            "otherwise"
        );
        assert_eq!(replacement("suc", "succ", "zero"), None);
    }
}
