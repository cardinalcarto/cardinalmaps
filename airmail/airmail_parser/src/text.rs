//! Text utilities standing in for the npm packages upstream depends on:
//! `remove-accents` (accent folding for Who's On First names) and
//! `pluralize` (English plurals of place-name keywords).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::js;
use crate::resources;

/// Character → replacement table for accent folding (`resources/accents.tsv`).
///
/// Covers Latin-1 Supplement, Latin Extended-A/B and Latin Extended
/// Additional: letters whose canonical decomposition is ASCII plus combining
/// marks, and letters such as `ß`, `æ`, `ø`, `ł` that need explicit mappings.
/// This approximates the `remove-accents` npm package's character map.
static ACCENTS: LazyLock<HashMap<char, &'static str>> = LazyLock::new(|| {
    resources::file("accents.tsv")
        .expect("accents.tsv is embedded")
        .lines()
        .filter_map(|line| {
            let (from, to) = line.split_once('\t')?;
            let mut chars = from.chars();
            let c = chars.next()?;
            chars.next().is_none().then_some((c, to))
        })
        .collect()
});

/// Folds accented Latin letters to ASCII (`"zürich"` → `"zurich"`).
pub(crate) fn remove_accents(s: &str) -> std::borrow::Cow<'_, str> {
    if s.is_ascii() || !s.chars().any(|c| ACCENTS.contains_key(&c)) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match ACCENTS.get(&c) {
            Some(r) => out.push_str(r),
            None => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The normalizer upstream applies to Who's On First names and to phrases
/// looked up in them: trim, lowercase, fold accents, hyphens to spaces.
pub(crate) fn normalize_wof(s: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = js::trim(s);
    let needs_lower = trimmed.chars().any(|c| c.is_uppercase() || !c.is_ascii());
    let lowered: std::borrow::Cow<'_, str> = if needs_lower {
        std::borrow::Cow::Owned(trimmed.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(trimmed)
    };
    let folded = match remove_accents(&lowered) {
        std::borrow::Cow::Borrowed(_) => lowered,
        std::borrow::Cow::Owned(o) => std::borrow::Cow::Owned(o),
    };
    if folded.contains('-') {
        std::borrow::Cow::Owned(folded.replace('-', " "))
    } else {
        folded
    }
}

/// Pluralization rules, in the order the `pluralize` package registers them;
/// the last matching rule wins. The trailing `$0` rules are its uncountable
/// patterns, registered last so they take precedence.
const PLURAL_RULES: &[(&str, &str)] = &[
    (r"(?i)s?$", "s"),
    (r"(?i)[^\x00-\x7F]$", "$0"),
    (r"(?i)([^aeiou]ese)$", "$1"),
    (r"(?i)(ax|test)is$", "$1es"),
    (r"(?i)(alias|[^aou]us|t[lm]as|gas|ris)$", "$1es"),
    (r"(?i)(e[mn]u)s?$", "$1s"),
    (r"(?i)([^l]ias|[aeiou]las|[ejzr]as|[iu]am)$", "$1"),
    (
        r"(?i)(alumn|syllab|vir|radi|nucle|fung|cact|stimul|termin|bacill|foc|uter|loc|strat)(?:us|i)$",
        "$1i",
    ),
    (r"(?i)(alumn|alg|vertebr)(?:a|ae)$", "$1ae"),
    (r"(?i)(seraph|cherub)(?:im)?$", "$1im"),
    (r"(?i)(her|at|gr)o$", "$1oes"),
    (
        r"(?i)(agend|addend|millenni|dat|extrem|bacteri|desiderat|strat|candelabr|errat|ov|symposi|curricul|automat|quor)(?:a|um)$",
        "$1a",
    ),
    (
        r"(?i)(apheli|hyperbat|periheli|asyndet|noumen|phenomen|criteri|organ|prolegomen|hedr|automat)(?:a|on)$",
        "$1a",
    ),
    (r"(?i)sis$", "ses"),
    (r"(?i)(?:(kni|wi|li)fe|(ar|l|ea|eo|oa|hoo)f)$", "$1$2ves"),
    (r"(?i)([^aeiouy]|qu)y$", "$1ies"),
    (r"(?i)([^ch][ieo][ln])ey$", "$1ies"),
    (r"(?i)(x|ch|ss|sh|zz)$", "$1es"),
    (
        r"(?i)(matr|cod|mur|sil|vert|ind|append)(?:ix|ex)$",
        "$1ices",
    ),
    (r"(?i)(?-u:\b)((?:tit)?m|l)(?:ice|ouse)$", "$1ice"),
    (r"(?i)(pe)(?:rson|ople)$", "$1ople"),
    (r"(?i)(child)(?:ren)?$", "$1ren"),
    (r"(?i)eaux$", "$0"),
    (r"(?i)m[ae]n$", "men"),
    (r"(?i)^thou$", "you"),
    (r"(?i)pok[eé]mon$", "$0"),
    (r"(?i)[^aeiou]ese$", "$0"),
    (r"(?i)deer$", "$0"),
    (r"(?i)fish$", "$0"),
    (r"(?i)measles$", "$0"),
    (r"(?i)o[iu]s$", "$0"),
    (r"(?i)pox$", "$0"),
    (r"(?i)sheep$", "$0"),
];

const IRREGULAR: &[(&str, &str)] = &[
    ("i", "we"),
    ("me", "us"),
    ("he", "they"),
    ("she", "they"),
    ("them", "them"),
    ("myself", "ourselves"),
    ("yourself", "yourselves"),
    ("itself", "themselves"),
    ("herself", "themselves"),
    ("himself", "themselves"),
    ("themself", "themselves"),
    ("is", "are"),
    ("was", "were"),
    ("has", "have"),
    ("this", "these"),
    ("that", "those"),
    ("echo", "echoes"),
    ("dingo", "dingoes"),
    ("volcano", "volcanoes"),
    ("tornado", "tornadoes"),
    ("torpedo", "torpedoes"),
    ("genus", "genera"),
    ("viscus", "viscera"),
    ("stigma", "stigmata"),
    ("stoma", "stomata"),
    ("dogma", "dogmata"),
    ("lemma", "lemmata"),
    ("schema", "schemata"),
    ("anathema", "anathemata"),
    ("ox", "oxen"),
    ("axe", "axes"),
    ("die", "dice"),
    ("yes", "yeses"),
    ("foot", "feet"),
    ("eave", "eaves"),
    ("goose", "geese"),
    ("tooth", "teeth"),
    ("quiz", "quizzes"),
    ("human", "humans"),
    ("proof", "proofs"),
    ("carve", "carves"),
    ("valve", "valves"),
    ("looey", "looies"),
    ("thief", "thieves"),
    ("groove", "grooves"),
    ("pickaxe", "pickaxes"),
    ("passerby", "passersby"),
];

const UNCOUNTABLE: &[&str] = &[
    "adulthood",
    "advice",
    "agenda",
    "aid",
    "aircraft",
    "alcohol",
    "ammo",
    "analytics",
    "anime",
    "athletics",
    "audio",
    "bison",
    "blood",
    "bream",
    "buffalo",
    "butter",
    "carp",
    "cash",
    "chassis",
    "chess",
    "clothing",
    "cod",
    "commerce",
    "cooperation",
    "corps",
    "debris",
    "diabetes",
    "digestion",
    "elk",
    "energy",
    "equipment",
    "excretion",
    "expertise",
    "firmware",
    "flounder",
    "fun",
    "gallows",
    "garbage",
    "graffiti",
    "hardware",
    "headquarters",
    "health",
    "herpes",
    "highjinks",
    "homework",
    "housework",
    "information",
    "jeans",
    "justice",
    "kudos",
    "labour",
    "literature",
    "machinery",
    "mackerel",
    "mail",
    "media",
    "mews",
    "moose",
    "music",
    "mud",
    "manga",
    "news",
    "only",
    "personnel",
    "pike",
    "plankton",
    "pliers",
    "police",
    "pollution",
    "premises",
    "rain",
    "research",
    "rice",
    "salmon",
    "scissors",
    "series",
    "sewage",
    "shambles",
    "shrimp",
    "software",
    "staff",
    "species",
    "swine",
    "tennis",
    "traffic",
    "transportation",
    "trout",
    "tuna",
    "wealth",
    "welfare",
    "whiting",
    "wildebeest",
    "wildlife",
    "you",
];

static COMPILED_RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PLURAL_RULES
        .iter()
        .map(|(re, rep)| (Regex::new(re).expect("valid plural rule"), *rep))
        .collect()
});

/// The `pluralize` package's `restoreCase`.
fn restore_case(word: &str, token: &str) -> String {
    if word == token {
        return token.to_string();
    }
    if word == word.to_lowercase() {
        return token.to_lowercase();
    }
    if word == word.to_uppercase() {
        return token.to_uppercase();
    }
    let mut chars = word.chars();
    if let Some(first) = chars.next()
        && first.to_uppercase().eq(std::iter::once(first))
    {
        let mut t = token.chars();
        return match t.next() {
            Some(c) => c
                .to_uppercase()
                .chain(t.as_str().to_lowercase().chars())
                .collect(),
            None => String::new(),
        };
    }
    token.to_lowercase()
}

/// English plural of `word`, reproducing the `pluralize` package (v8) for
/// the dictionary entries the place classifier feeds it.
pub(crate) fn pluralize(word: &str) -> String {
    let token = word.to_lowercase();
    if IRREGULAR.iter().any(|(_, plural)| *plural == token) {
        return restore_case(word, &token);
    }
    if let Some((_, plural)) = IRREGULAR.iter().find(|(single, _)| *single == token) {
        return restore_case(word, plural);
    }
    if token.is_empty() || UNCOUNTABLE.contains(&token.as_str()) {
        return word.to_string();
    }
    for (re, replacement) in COMPILED_RULES.iter().rev() {
        let Some(caps) = re.captures(word) else {
            continue;
        };
        let whole = caps.get(0).expect("group 0");
        // interpolate $n with capture groups (missing groups become '')
        let mut result = String::new();
        let mut rest = *replacement;
        while let Some(pos) = rest.find('$') {
            result.push_str(&rest[..pos]);
            let digits: String = rest[pos + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .take(2)
                .collect();
            if digits.is_empty() {
                result.push('$');
                rest = &rest[pos + 1..];
                continue;
            }
            let n: usize = digits.parse().expect("digits");
            result.push_str(caps.get(n).map_or("", |m| m.as_str()));
            rest = &rest[pos + 1 + digits.len()..];
        }
        result.push_str(rest);
        let restored = if whole.as_str().is_empty() {
            // `word[index - 1]`: the character before the (empty) match
            let before = word[..whole.start()]
                .chars()
                .next_back()
                .map(String::from)
                .unwrap_or_default();
            restore_case(&before, &result)
        } else {
            restore_case(whole.as_str(), &result)
        };
        return format!(
            "{}{}{}",
            &word[..whole.start()],
            restored,
            &word[whole.end()..]
        );
    }
    word.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals() {
        for (s, p) in [
            ("park", "parks"),
            ("church", "churches"),
            ("university", "universities"),
            ("library", "libraries"),
            ("campus", "campuses"),
            ("ind.est", "ind.ests"),
            ("cafe", "cafes"),
            ("bus", "buses"),
            ("fish", "fish"),
            ("person", "people"),
            ("shelf", "shelves"),
            ("abbey", "abbeys"),
            ("market hall", "market halls"),
            ("parks", "parks"),
        ] {
            assert_eq!(pluralize(s), p, "{s}");
        }
    }

    #[test]
    fn accents() {
        assert_eq!(remove_accents("zürich"), "zurich");
        assert_eq!(remove_accents("straße"), "strasse");
        assert_eq!(remove_accents("łódź"), "lodz");
        assert_eq!(remove_accents("東京"), "東京");
        assert_eq!(normalize_wof(" Saint-Étienne "), "saint etienne");
    }
}
