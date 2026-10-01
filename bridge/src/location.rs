//! Source locations in Agda's messages, such as `/x/A.agda:7.5-8: error: …`.
//!
//! Agda 2.8 writes `line.col`, older versions write `line,col`. A range is
//! either `L.C-C2` (one line) or `L.C-L2.C2`; columns count code points and
//! the end is exclusive.

/// A range with 1-based lines and columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

/// The location at the start of `message`, if it is in `file`.
pub fn span_in_file(message: &str, file: &str) -> Option<Span> {
    let rest = message.strip_prefix(file)?.strip_prefix(':')?;
    let range = rest.split(|c: char| c == ':' || c.is_whitespace()).next()?;
    parse_range(range)
}

/// The message without its leading `file:range: ` location, when it has one.
pub fn without_location<'a>(message: &'a str, file: &str) -> &'a str {
    let Some(rest) = message
        .strip_prefix(file)
        .and_then(|rest| rest.strip_prefix(':'))
    else {
        return message;
    };
    match rest.split_once(": ") {
        Some((range, text)) if parse_range(range).is_some() => text,
        _ => message,
    }
}

fn parse_range(range: &str) -> Option<Span> {
    let (from, to) = range.split_once('-').unwrap_or((range, ""));
    let start = parse_point(from)?;
    let end = if to.is_empty() {
        (start.0, start.1 + 1)
    } else if let Some(end) = parse_point(to) {
        end
    } else {
        (start.0, to.parse().ok()?)
    };
    Some(Span { start, end })
}

fn parse_point(point: &str) -> Option<(usize, usize)> {
    let (line, col) = point.split_once(['.', ','])?;
    Some((line.parse().ok()?, col.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agda_2_8_locations() {
        let message = "/x/Bad.agda:7.5-8: error: [UnequalTerms]\nSet₁ !=< ℕ";
        assert_eq!(
            span_in_file(message, "/x/Bad.agda"),
            Some(Span {
                start: (7, 5),
                end: (7, 8)
            })
        );
        assert_eq!(
            span_in_file("/x/A.agda:3.1-4.6: warning", "/x/A.agda"),
            Some(Span {
                start: (3, 1),
                end: (4, 6)
            })
        );
    }

    #[test]
    fn parses_older_comma_locations() {
        assert_eq!(
            span_in_file("/x/A.agda:10,5-15\nmessage", "/x/A.agda"),
            Some(Span {
                start: (10, 5),
                end: (10, 15)
            })
        );
    }

    #[test]
    fn strips_the_location_prefix() {
        let message = "/x/Bad.agda:7.5-8: error: [UnequalTerms]\nSet₁ !=< ℕ";
        assert_eq!(
            without_location(message, "/x/Bad.agda"),
            "error: [UnequalTerms]\nSet₁ !=< ℕ"
        );
        assert_eq!(without_location("plain", "/x/Bad.agda"), "plain");
    }

    #[test]
    fn ignores_other_files_and_relative_locations() {
        assert_eq!(span_in_file("/x/B.agda:1.1-2: error", "/x/A.agda"), None);
        assert_eq!(
            span_in_file("1.1-5: error: [CannotApply]", "/x/A.agda"),
            None
        );
    }
}
