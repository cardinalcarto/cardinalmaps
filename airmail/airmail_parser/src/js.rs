//! Small helpers reproducing JavaScript string semantics the upstream parser
//! relies on: UTF-16 lengths, the `\s` character class and `String.trim`.

/// Length of `s` in UTF-16 code units, i.e. JavaScript's `s.length`.
pub(crate) fn len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// JavaScript's `\s`: `WhiteSpace` plus `LineTerminator`. Unlike
/// [`char::is_whitespace`] this includes U+FEFF and excludes U+0085.
pub(crate) fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JavaScript's `String.prototype.trim`.
pub(crate) fn trim(s: &str) -> &str {
    s.trim_matches(is_whitespace)
}

/// Truncates `s` to at most `max` UTF-16 code units (on a char boundary),
/// like `s.slice(0, max)` for text without astral characters.
pub(crate) fn truncate(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &s[..i];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_and_lengths() {
        assert!(is_whitespace('\u{FEFF}'));
        assert!(!is_whitespace('\u{85}'));
        assert_eq!(len("zürich"), 6);
        assert_eq!(len("😀"), 2);
        assert_eq!(trim("\u{A0} a b \n"), "a b");
        assert_eq!(truncate("abcdef", 3), "abc");
    }
}
