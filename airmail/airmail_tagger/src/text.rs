//! How text is normalized and split into words, for indexing and searching
//! alike.
//!
//! Text keeps its script: Москва is indexed as `москва` and 渋谷 as `渋`,
//! `谷`, so native-script queries match precisely. It's also forgiving in the
//! ways people are when they search:
//!
//! - Case and the diacritics of Latin, Greek and Cyrillic letters are folded
//!   away ([`fold`]): `malmo` is Malmö, `lodz` Łódź, `ελλαδα` Ελλάδα. Marks
//!   are kept in other scripts, where they're vowels, not decorations.
//! - Names in other scripts are also indexed transliterated into Latin
//!   ([`transliteration`]), so `moskva` finds Москва. Transliteration is
//!   mechanical, and wrong for some languages (Japanese kanji get Chinese
//!   readings), so it's a variant a name is found by, not its name.
//! - German umlauts are also written out ([`umlaut_variant`]): Müller is
//!   `muller` and `mueller`.
//! - Apostrophes join a word (`mcdonalds`), and names are also indexed with
//!   them as spaces ([`apostrophe_variant`]), so `arsenal` finds Rue de
//!   l'Arsenal.
//!
//! Words are split at anything that isn't a letter or digit, and text in
//! scripts written without spaces between words (Chinese, Japanese, Thai,
//! Lao, Khmer, Myanmar) is split into single characters, which phrase
//! matches and exact names put back together ([`words`]).

use unicode_normalization::UnicodeNormalization;
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

/// Words longer than this, in bytes, aren't indexed: they're never typed.
const MAX_WORD_BYTES: usize = 64;

/// Whether a character's script is written without spaces between words.
fn unspaced(c: char) -> bool {
    matches!(
        c.script(),
        Script::Han
            | Script::Hiragana
            | Script::Katakana
            | Script::Thai
            | Script::Lao
            | Script::Khmer
            | Script::Myanmar
    )
}

/// Whether a character's diacritics are folded away.
fn folds_marks(c: char) -> bool {
    matches!(c.script(), Script::Latin | Script::Greek | Script::Cyrillic)
}

/// Letters that aren't a base letter with marks, folded as they're typed
/// without them.
fn fold_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'ß' => "ss",
        'ł' => "l",
        'ø' => "o",
        'æ' => "ae",
        'œ' => "oe",
        'đ' | 'ð' => "d",
        'þ' => "th",
        'ı' => "i",
        'ħ' => "h",
        'ŀ' => "l",
        'ŧ' => "t",
        'ŋ' => "n",
        'ſ' => "s",
        'ʼ' | '’' | '\'' | '`' => "",
        _ => return None,
    })
}

/// Text lowercased, compatibility-normalized (full-width letters, ligatures),
/// without the diacritics of Latin, Greek and Cyrillic letters, and without
/// apostrophes, which join a word rather than split it (`mcdonalds`).
#[must_use]
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        // Normalization leaves ASCII as it is, and its only letters that fold are the
        // apostrophes: the same result, without the normalizers' work.
        return text
            .bytes()
            .filter(|b| !matches!(b, b'\'' | b'`'))
            .map(|b| char::from(b.to_ascii_lowercase()))
            .collect();
    }
    fold_any(text)
}

/// [`fold`] of any text, ASCII or not.
fn fold_any(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut base_folds = false;
    for c in text.nfkd() {
        if unicode_normalization::char::is_combining_mark(c) {
            // A mark belongs to the letter before it.
            if !base_folds {
                out.push(c);
            }
            continue;
        }
        base_folds = folds_marks(c);
        for lower in c.to_lowercase() {
            match fold_letter(lower) {
                Some(replacement) => out.push_str(replacement),
                None => out.push(lower),
            }
        }
    }
    out.nfc().collect()
}

/// Whether text has letters outside the Latin script.
#[must_use]
pub fn has_non_latin(text: &str) -> bool {
    text.chars().any(|c| {
        c.is_alphabetic()
            && !matches!(
                c.script(),
                Script::Latin | Script::Common | Script::Inherited
            )
    })
}

/// `text` transliterated into Latin and folded, if it has letters in other
/// scripts.
#[must_use]
pub fn transliteration(text: &str) -> Option<String> {
    has_non_latin(text).then(|| fold(&deunicode::deunicode(text)))
}

/// `text` (folded or not) with German umlauts written out, if it has any:
/// `Müller` → `mueller`.
#[must_use]
pub fn umlaut_variant(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    if !lower.contains(['ä', 'ö', 'ü']) {
        return None;
    }
    let written: String = lower
        .chars()
        .flat_map(|c| match c {
            'ä' => vec!['a', 'e'],
            'ö' => vec!['o', 'e'],
            'ü' => vec!['u', 'e'],
            other => vec![other],
        })
        .collect();
    Some(fold(&written))
}

/// `text` with its apostrophes as spaces, if it has any: `l'arsenal` →
/// `l arsenal`. Before folding, which drops them.
#[must_use]
pub fn apostrophe_variant(text: &str) -> Option<String> {
    text.contains(['ʼ', '’', '\'', '`'])
        .then(|| fold(&text.replace(['ʼ', '’', '\'', '`'], " ")))
}

/// The words of folded text, with their byte offsets: runs of letters and
/// digits, with each character (grapheme) of scripts written without spaces
/// a word of its own.
#[must_use]
pub fn words(text: &str) -> Vec<(usize, &str)> {
    if text.is_ascii() {
        return AsciiWords::new(text).collect();
    }
    words_any(text)
}

/// The [`words`] of ASCII text, one by one. Every ASCII character is a grapheme of its own
/// (but "\r\n", which isn't a word either) and none is unspaced: words are runs of letters
/// and digits.
#[derive(Clone, Debug)]
pub struct AsciiWords<'a> {
    text: &'a str,
    next: usize,
}

impl<'a> AsciiWords<'a> {
    /// The words of `text`, which must be ASCII.
    #[must_use]
    pub fn new(text: &'a str) -> Self {
        debug_assert!(text.is_ascii());
        Self { text, next: 0 }
    }
}

impl<'a> Iterator for AsciiWords<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.text.as_bytes();
        loop {
            let start = self.next
                + bytes[self.next..]
                    .iter()
                    .position(u8::is_ascii_alphanumeric)?;
            let end = bytes[start..]
                .iter()
                .position(|b| !b.is_ascii_alphanumeric())
                .map_or(bytes.len(), |n| start + n);
            self.next = end;
            if end - start <= MAX_WORD_BYTES {
                return Some((start, &self.text[start..end]));
            }
        }
    }
}

/// [`words`] of any text, ASCII or not.
fn words_any(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (i, grapheme) in text.grapheme_indices(true) {
        let first = grapheme.chars().next().unwrap_or(' ');
        let alphanumeric = first.is_alphanumeric();
        if alphanumeric && !unspaced(first) {
            start.get_or_insert(i);
            continue;
        }
        if let Some(s) = start.take() {
            words.push((s, &text[s..i]));
        }
        if alphanumeric {
            words.push((i, grapheme));
        }
    }
    if let Some(s) = start {
        words.push((s, &text[s..]));
    }
    words.retain(|(_, w)| w.len() <= MAX_WORD_BYTES);
    words
}

#[cfg(test)]
mod test {
    use super::*;

    fn word_list(text: &str) -> Vec<&str> {
        words(text).into_iter().map(|(_, w)| w).collect()
    }

    #[test]
    fn ascii_takes_a_shortcut_to_the_same_result() {
        let mut texts: Vec<String> = (0u8..128).map(|b| char::from(b).to_string()).collect();
        texts.extend(
            [
                "McDonald's Rd\r\nNE",
                "1530 Post Alley",
                "  a--b__c  ",
                "Rue de l'Arsenal, 40",
                "`back`ticks`",
                "ALL CAPS 54TH",
                "\t\ttabs\x7f\x00nul",
                "",
            ]
            .map(str::to_string),
        );
        // Every pair of ASCII characters, around letters.
        for a in 0u8..128 {
            for b in 0u8..128 {
                texts.push(format!("x{}{}Y", char::from(a), char::from(b)));
            }
        }
        texts.push("a".repeat(65) + " b " + &"c".repeat(64));
        for text in &texts {
            assert_eq!(fold(text), fold_any(text), "{text:?}");
            assert_eq!(words(text), words_any(text), "{text:?}");
        }
    }

    #[test]
    fn folding_keeps_scripts_and_drops_decoration() {
        assert_eq!(fold("Malmö"), "malmo");
        assert_eq!(fold("Łódź"), "lodz");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(fold("Ελλάδα"), "ελλαδα");
        assert_eq!(fold("Москва"), "москва");
        assert_eq!(fold("Ёлки"), "елки");
        assert_eq!(fold("McDonald’s"), "mcdonalds");
        assert_eq!(fold("ＴＯＫＹＯ"), "tokyo");
        assert_eq!(fold("Trường Chinh"), "truong chinh");
        assert_eq!(fold("İstanbul"), "istanbul");
        // Vowel signs are letters in Indic and Thai scripts.
        assert_eq!(fold("दिल्ली"), "दिल्ली");
        assert_eq!(fold("เชียงใหม่"), "เชียงใหม่");
    }

    #[test]
    fn other_scripts_are_also_transliterated() {
        assert_eq!(transliteration("Москва").as_deref(), Some("moskva"));
        assert_eq!(transliteration("Ελλάδα").as_deref(), Some("ellada"));
        assert_eq!(transliteration("Malmö"), None);
        assert_eq!(umlaut_variant("Müller").as_deref(), Some("mueller"));
        assert_eq!(umlaut_variant("Mueller"), None);
    }

    #[test]
    fn unspaced_scripts_are_split_into_characters() {
        assert_eq!(
            word_list("渋谷区 shibuya"),
            vec!["渋", "谷", "区", "shibuya"]
        );
        assert_eq!(
            word_list("東京都渋谷区2-1"),
            vec!["東", "京", "都", "渋", "谷", "区", "2", "1"]
        );
        assert_eq!(
            word_list("เชียงใหม่"),
            "เชียงใหม่".graphemes(true).collect::<Vec<_>>()
        );
        assert_eq!(word_list("เชียงใหม่").len(), 7);
        assert_eq!(word_list("독서당로39길 22"), vec!["독서당로39길", "22"]);
        assert_eq!(
            word_list(&fold("Rue de l'Arsenal, 40")),
            vec!["rue", "de", "larsenal", "40"]
        );
        assert_eq!(
            apostrophe_variant("Rue de l'Arsenal").as_deref(),
            Some("rue de l arsenal")
        );
    }
}
