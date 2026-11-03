//! Spellings of the same name that tokenizing alone doesn't equate.
//!
//! Text is normalized the same way when it's indexed and when it's searched
//! for ([`expand_abbreviations`]), and names and addresses are also indexed
//! in other forms they're written in ([`variants`]): `54 street` finds 54th
//! Street, and `ackermann strasse`, `ackermannstr` and `Ackermannstraße` are
//! all Ackermannstraße.

/// German street suffixes written joined to the name: `Grolmanstraße`,
/// `Clayallee`. Also Dutch `-weg`. After transliteration, so `ß` is `ss`.
const COMPOUND_SUFFIXES: &[&str] = &["strasse", "gasse", "platz", "allee", "weg", "damm"];

/// The shortest name a compound's suffix is split from.
const MIN_COMPOUND_STEM: usize = 3;

/// `text` (transliterated) with abbreviated German street suffixes written
/// out: `Grolmanstr.` is `Grolmanstrasse`. Not `Str.` on its own, which may
/// be English for Street.
#[must_use]
pub fn expand_abbreviations(text: &str) -> String {
    if !text.to_ascii_lowercase().contains("str") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let bare = word.strip_suffix('.').unwrap_or(word);
        let lower = bare.to_ascii_lowercase();
        let stem = lower.strip_suffix("str");
        match stem {
            Some(stem) if stem.chars().all(char::is_alphabetic) && stem.len() >= MIN_COMPOUND_STEM => {
                out.push_str(&bare[..stem.len()]);
                out.push_str("strasse");
            }
            _ => out.push_str(word),
        }
    }
    out
}

/// Other forms of normalized `text` to index beside it: ordinals as plain
/// numbers (`54th` as `54`), and compound street names with their suffix
/// split off (`grolmanstrasse` as `grolman strasse`). Empty if there are
/// none.
#[must_use]
pub fn variants(text: &str) -> Vec<String> {
    let mut changed = false;
    let words: Vec<String> = text
        .split_whitespace()
        .map(|word| {
            let lower = word.to_lowercase();
            if let Some(number) = ordinal_number(&lower) {
                changed = true;
                return number.to_string();
            }
            for suffix in COMPOUND_SUFFIXES {
                if let Some(stem) = lower.strip_suffix(suffix)
                    && stem.chars().count() >= MIN_COMPOUND_STEM
                    && stem.chars().all(char::is_alphabetic)
                {
                    changed = true;
                    return format!("{stem} {suffix}");
                }
            }
            word.to_string()
        })
        .collect();
    if changed { vec![words.join(" ")] } else { Vec::new() }
}

/// The number of an ordinal like `54th` or `1st`.
fn ordinal_number(word: &str) -> Option<&str> {
    let digits = word.find(|c: char| !c.is_ascii_digit())?;
    (digits > 0 && matches!(&word[digits..], "st" | "nd" | "rd" | "th")).then(|| &word[..digits])
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn abbreviated_street_suffixes_are_written_out() {
        assert_eq!(expand_abbreviations("Grolmanstr. 1"), "Grolmanstrasse 1");
        assert_eq!(expand_abbreviations("grolmanstr"), "grolmanstrasse");
        assert_eq!(expand_abbreviations("Ackermann Str."), "Ackermann Str.");
        assert_eq!(expand_abbreviations("198 Baker Str"), "198 Baker Str");
        assert_eq!(expand_abbreviations("Ackermannstrasse"), "Ackermannstrasse");
        // Not words that just end in the letters.
        assert_eq!(expand_abbreviations("Ostr 5"), "Ostr 5");
        assert_eq!(expand_abbreviations("1str"), "1str");
    }

    #[test]
    fn ordinals_and_compounds_have_variants() {
        assert_eq!(variants("West 54th Street"), vec!["West 54 Street"]);
        assert_eq!(variants("1st Avenue"), vec!["1 Avenue"]);
        assert_eq!(variants("grolmanstrasse 1"), vec!["grolman strasse 1"]);
        assert_eq!(variants("Alexanderplatz"), vec!["alexander platz"]);
        assert_eq!(variants("Vanadiumweg 11C"), vec!["vanadium weg 11C"]);
        assert!(variants("Main Street").is_empty());
        assert!(variants("Weg 3").is_empty());
        assert!(variants("Pike Place").is_empty());
        assert!(variants("16 Ave").is_empty());
    }
}
