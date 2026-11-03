//! Embedded dictionaries and the loaders that turn them into lookup tables,
//! ported from upstream's `resources/libpostal/libpostal.js`,
//! `resources/whosonfirst/whosonfirst.js` and `resources/helper.js`.
//!
//! Every file under `resources/` is compiled into the binary (see
//! `build.rs`). Upstream layers three sources per dictionary: the raw
//! libpostal / Who's On First file, then pelias' own overrides (`#` comments,
//! `!` removes entries), then user "custom" overrides (empty upstream, not
//! ported).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::js;

include!(concat!(env!("OUT_DIR"), "/embedded_resources.rs"));

/// The contents of an embedded resource, by path relative to `resources/`.
pub(crate) fn file(path: &str) -> Option<&'static str> {
    FILES
        .binary_search_by(|(p, _)| (*p).cmp(path))
        .ok()
        .map(|i| FILES[i].1)
}

/// Embedded files directly inside `dir` whose names match `pattern`, where
/// `pattern` may contain one `*` wildcard (upstream's `generateFilenames`).
fn matching_files(dir: &str, pattern: &str) -> Vec<&'static str> {
    let prefix = format!("{dir}/");
    FILES
        .iter()
        .filter_map(|(path, contents)| {
            let name = path.strip_prefix(&prefix)?;
            if name.contains('/') {
                return None;
            }
            let matches = match pattern.split_once('*') {
                Some((head, tail)) => {
                    name.len() >= head.len() + tail.len()
                        && name.starts_with(head)
                        && name.ends_with(tail)
                }
                None => name == pattern,
            };
            matches.then_some(*contents)
        })
        .collect()
}

/// Every language directory in upstream's libpostal dictionaries
/// (`libpostal.languages`), in directory order. Only the dictionaries the
/// classifiers use are embedded, so some of these have no files.
pub(crate) const LANGUAGES: &[&str] = &[
    "af", "all", "ar", "ast", "az", "be", "bg", "br", "bs", "ca", "cs", "cy", "da", "de", "el",
    "en", "es", "et", "eu", "fa", "fi", "fil", "fr", "ga", "gd", "gl", "gsw", "he", "hi", "hr",
    "hu", "id", "is", "it", "ja", "ka", "ko", "lb", "lt", "lv", "ms", "mt", "nb", "nl", "oc",
    "pap", "pl", "pt", "ro", "ru", "si", "sk", "sl", "sr", "sv", "th", "tr", "uk", "ur", "vi",
    "zh",
];

/// A set of languages, as bits indexing [`LANGUAGES`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LangSet(u64);

impl LangSet {
    pub(crate) fn of(lang: &str) -> Self {
        let i = LANGUAGES
            .iter()
            .position(|l| *l == lang)
            .expect("known language");
        LangSet(1 << i)
    }
    pub(crate) fn contains(self, lang: &str) -> bool {
        self.0 & Self::of(lang).0 != 0
    }
    fn insert(&mut self, other: LangSet) {
        self.0 |= other.0;
    }
    /// The language codes, in [`LANGUAGES`] order.
    pub(crate) fn codes(self) -> Vec<&'static str> {
        LANGUAGES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, l)| *l)
            .collect()
    }
}

/// A libpostal-style dictionary: normalized term → languages it came from.
pub(crate) type Dict = HashMap<Cow<'static, str>, LangSet>;

/// Options of upstream's libpostal `_normalize`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LoadOptions {
    /// Strip one trailing `.` (upstream `replace: [/\.$/, '']`).
    pub strip_final_period: bool,
    /// Drop terms shorter than this many UTF-16 units (0 = no minimum).
    pub minlength: usize,
    /// Lowercase terms.
    pub lowercase: bool,
}

fn normalize_cell(cell: &'static str, opts: LoadOptions) -> Option<Cow<'static, str>> {
    let mut value = js::trim(cell);
    if opts.strip_final_period {
        value = value.strip_suffix('.').unwrap_or(value);
    }
    if opts.minlength > 0 && js::len(value) < opts.minlength {
        return None;
    }
    let value: Cow<'static, str> = match opts.lowercase.then(|| value.to_lowercase()) {
        Some(lower) if lower != value => Cow::Owned(lower),
        _ => Cow::Borrowed(value),
    };
    (!value.is_empty()).then_some(value)
}

/// One operation of a pelias override file.
enum Override {
    Add(&'static str),
    Remove(&'static str),
}

/// Parses a pelias override file: `#` lines are comments, `!a|b` removes
/// entries, other lines add `|`-separated entries.
fn overrides(contents: &'static str) -> impl Iterator<Item = Override> {
    contents
        .split('\n')
        .filter(|row| !js::trim(row).starts_with('#'))
        .flat_map(|row| {
            let (remove, cells) = match row.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, row),
            };
            cells.split('|').map(move |cell| {
                if remove {
                    Override::Remove(cell)
                } else {
                    Override::Add(cell)
                }
            })
        })
}

/// Upstream `libpostal.load`: adds every `|`-separated term of
/// `<lang>/<filename>` for each language, then applies pelias' overrides.
pub(crate) fn load_libpostal(index: &mut Dict, langs: &[&str], filename: &str, opts: LoadOptions) {
    let add = |index: &mut Dict, lang: &str, cell: &'static str| {
        if let Some(value) = normalize_cell(cell, opts) {
            index.entry(value).or_default().insert(LangSet::of(lang));
        }
    };
    for lang in langs {
        if let Some(contents) = file(&format!("libpostal/{lang}/{filename}")) {
            for row in contents.split('\n') {
                for cell in row.split('|') {
                    add(index, lang, cell);
                }
            }
        }
    }
    for lang in langs {
        if let Some(contents) = file(&format!("pelias/libpostal/{lang}/{filename}")) {
            for op in overrides(contents) {
                match op {
                    Override::Add(cell) => add(index, lang, cell),
                    Override::Remove(cell) => {
                        if let Some(value) = normalize_cell(cell, opts) {
                            index.remove(&value);
                        }
                    }
                }
            }
        }
    }
}

/// Upstream `whosonfirst.load` with `{ minlength: 2, normalizer }`: the
/// normalized names in the matching files of `whosonfirst/<placetype>/`,
/// then pelias' overrides.
pub(crate) fn load_whosonfirst(
    set: &mut HashSet<Cow<'static, str>>,
    placetype: &str,
    patterns: &[&str],
) {
    let normalize = |cell: &'static str| -> Option<Cow<'static, str>> {
        let value = js::trim(cell);
        if js::len(value) < 2 {
            return None;
        }
        let value = crate::text::normalize_wof(value);
        (!value.is_empty()).then_some(value)
    };
    for pattern in patterns {
        for contents in matching_files(&format!("whosonfirst/{placetype}"), pattern) {
            for row in contents.split('\n') {
                for cell in row.split('|') {
                    if let Some(v) = normalize(cell) {
                        set.insert(v);
                    }
                }
            }
        }
    }
    for pattern in patterns {
        for contents in matching_files(&format!("pelias/whosonfirst/{placetype}"), pattern) {
            for op in overrides(contents) {
                match op {
                    Override::Add(cell) => {
                        if let Some(v) = normalize(cell) {
                            set.insert(v);
                        }
                    }
                    Override::Remove(cell) => {
                        if let Some(v) = normalize(cell) {
                            set.remove(&v);
                        }
                    }
                }
            }
        }
    }
}

/// Upstream `libpostal.generatePlurals`: adds the English plural of every
/// English term.
///
/// Upstream stores plurals with lodash `_.set(index, "<plural>.langs.en")`,
/// which treats dots as a property path, so a plural such as `ind.ests`
/// actually adds the key `ind`. That quirk is reproduced.
pub(crate) fn generate_plurals(index: &mut Dict) {
    let english: Vec<Cow<'static, str>> = index
        .iter()
        .filter(|(_, langs)| langs.contains("en"))
        .map(|(term, _)| term.clone())
        .collect();
    for term in english {
        let plural = crate::text::pluralize(&term);
        match plural.split_once('.') {
            None => index
                .entry(Cow::Owned(plural))
                .or_default()
                .insert(LangSet::of("en")),
            Some((head, _)) => {
                index.entry(Cow::Owned(head.to_string())).or_default();
            }
        }
    }
}

/// The postcode pattern (`zip`) from a chromium-i18n address metadata file.
pub(crate) fn chromium_zip_pattern(country: &str) -> Option<String> {
    let contents = file(&format!("chromium-i18n/{country}.json"))?;
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    value.get("zip")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_embedded_and_sorted() {
        assert!(FILES.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(file("libpostal/en/street_types.txt").is_some());
        assert!(file("whosonfirst/locality/name-eng_x_preferred.txt").is_some());
        assert_eq!(
            matching_files("whosonfirst/locality", "name-*_x_preferred.txt").len(),
            2
        );
    }

    #[test]
    fn libpostal_overrides_apply() {
        let mut d = Dict::new();
        load_libpostal(&mut d, &["en"], "street_types.txt", LoadOptions::default());
        assert!(d.contains_key("street"));
        assert!(d.contains_key("furlong"), "pelias addition");
        assert!(d["street"].contains("en"));
    }
}
