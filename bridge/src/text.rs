//! Positions in a document, in the three units the bridge meets:
//!
//! - Agda: code points (`char`s), 1-based;
//! - LSP: lines and UTF-16 code units, 0-based (Zed only offers UTF-16);
//! - Zed task variables (`ZED_COLUMN`): UTF-8 bytes, 1-based.
//!
//! Internally the bridge stores every offset as a 0-based `char` index into the
//! document, which matches Agda's offsets minus one.

use tower_lsp_server::ls_types::Position;

/// Convert a 0-based char offset to an LSP position.
pub fn position_of(text: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut character = 0;
    for ch in text.chars().take(offset) {
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += ch.len_utf16() as u32;
        }
    }
    Position { line, character }
}

/// Convert an LSP position to a 0-based char offset, clamped to the line end.
pub fn offset_of(text: &str, position: Position) -> usize {
    let mut offset = 0;
    let mut line = 0;
    let mut units = 0;
    for ch in text.chars() {
        if line == position.line {
            if ch == '\n' || units >= position.character {
                return offset;
            }
            units += ch.len_utf16() as u32;
        } else if ch == '\n' {
            line += 1;
        }
        offset += 1;
    }
    offset
}

/// Convert a 1-based line and 1-based code point column (as in Agda's error
/// locations) to a 0-based char offset.
pub fn offset_of_line_col(text: &str, line: usize, col: usize) -> usize {
    let line_start = line_start_offset(text, line);
    line_start + col.saturating_sub(1)
}

/// Convert a 1-based row and 1-based UTF-8 byte column (as in Zed's
/// `ZED_ROW` and `ZED_COLUMN` task variables) to a 0-based char offset.
pub fn offset_of_row_byte_column(text: &str, row: usize, byte_column: usize) -> usize {
    let line_start = line_start_offset(text, row);
    let line = text.split('\n').nth(row.saturating_sub(1)).unwrap_or("");
    let byte = byte_column.saturating_sub(1).min(line.len());
    // A column inside a multibyte character counts as that character.
    let chars = line
        .char_indices()
        .take_while(|(index, _)| *index < byte)
        .count();
    line_start + chars
}

fn line_start_offset(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut seen = 1;
    for (offset, ch) in text.chars().enumerate() {
        if ch == '\n' {
            seen += 1;
            if seen == line {
                return offset + 1;
            }
        }
    }
    text.chars().count()
}

/// One contiguous edit, in char offsets: `old[start..old_end]` was replaced
/// by `new_len` chars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub old_end: usize,
    pub new_len: usize,
}

impl Change {
    pub fn delta(&self) -> isize {
        self.new_len as isize - (self.old_end - self.start) as isize
    }
}

/// The smallest single change that turns `old` into `new`, found from the
/// common prefix and suffix (like agda2-vscode's `computeSingleChange`).
/// Returns `None` when the texts are equal.
pub fn single_change(old: &str, new: &str) -> Option<Change> {
    if old == new {
        return None;
    }
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    Some(Change {
        start: prefix,
        old_end: old.len() - suffix,
        new_len: new.len() - suffix - prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "ab\nℕ 𝔹 x\nlast";

    #[test]
    fn converts_offsets_to_utf16_positions() {
        // `𝔹` lies outside the Basic Multilingual Plane: two UTF-16 units.
        assert_eq!(position_of(TEXT, 0), Position::new(0, 0));
        assert_eq!(position_of(TEXT, 3), Position::new(1, 0));
        assert_eq!(position_of(TEXT, 5), Position::new(1, 2)); // the 𝔹
        assert_eq!(position_of(TEXT, 7), Position::new(1, 5)); // the x
        assert_eq!(position_of(TEXT, 9), Position::new(2, 0));
    }

    #[test]
    fn converts_utf16_positions_back() {
        for offset in 0..TEXT.chars().count() {
            assert_eq!(offset_of(TEXT, position_of(TEXT, offset)), offset);
        }
        // Past the end of a line clamps to the newline.
        assert_eq!(offset_of(TEXT, Position::new(0, 99)), 2);
    }

    #[test]
    fn converts_agda_line_and_column() {
        assert_eq!(offset_of_line_col(TEXT, 1, 1), 0);
        assert_eq!(offset_of_line_col(TEXT, 2, 5), 7); // the x
        assert_eq!(offset_of_line_col(TEXT, 3, 1), 9);
    }

    #[test]
    fn converts_zed_byte_columns() {
        // Line 2 is "ℕ 𝔹 x": ℕ is 3 bytes, 𝔹 is 4 bytes, so x is at byte 9.
        assert_eq!(offset_of_row_byte_column(TEXT, 2, 1), 3);
        assert_eq!(offset_of_row_byte_column(TEXT, 2, 4), 4); // the space
        assert_eq!(offset_of_row_byte_column(TEXT, 2, 10), 7); // the x
    }

    #[test]
    fn finds_single_changes() {
        assert_eq!(single_change("abc", "abc"), None);
        assert_eq!(
            single_change("{!  !}", "{! x !}"),
            Some(Change {
                start: 3,
                old_end: 3,
                new_len: 1
            })
        );
        assert_eq!(
            single_change("a ℕ b", "a b"),
            Some(Change {
                start: 2,
                old_end: 4,
                new_len: 0
            })
        );
    }
}
