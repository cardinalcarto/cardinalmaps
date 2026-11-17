//! The languages street names are varied in (see `substitutions`).

/// A language that `substitutions::permute_road` knows, or that's spoken
/// where places are indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Language {
    Arabic,
    Catalan,
    Chinese,
    Danish,
    Dutch,
    English,
    Finnish,
    French,
    German,
    Greek,
    Hungarian,
    Italian,
    Portuguese,
    Romanian,
    Russian,
    Spanish,
    Swedish,
    Tamil,
    Turkish,
}

impl Language {
    pub const ALL: [Language; 19] = [
        Language::Arabic,
        Language::Catalan,
        Language::Chinese,
        Language::Danish,
        Language::Dutch,
        Language::English,
        Language::Finnish,
        Language::French,
        Language::German,
        Language::Greek,
        Language::Hungarian,
        Language::Italian,
        Language::Portuguese,
        Language::Romanian,
        Language::Russian,
        Language::Spanish,
        Language::Swedish,
        Language::Tamil,
        Language::Turkish,
    ];

    /// Its ISO 639-3 code, lowercase.
    #[must_use]
    pub fn iso_639_3(self) -> &'static str {
        match self {
            Language::Arabic => "ara",
            Language::Catalan => "cat",
            Language::Chinese => "zho",
            Language::Danish => "dan",
            Language::Dutch => "nld",
            Language::English => "eng",
            Language::Finnish => "fin",
            Language::French => "fra",
            Language::German => "deu",
            Language::Greek => "ell",
            Language::Hungarian => "hun",
            Language::Italian => "ita",
            Language::Portuguese => "por",
            Language::Romanian => "ron",
            Language::Russian => "rus",
            Language::Spanish => "spa",
            Language::Swedish => "swe",
            Language::Tamil => "tam",
            Language::Turkish => "tur",
        }
    }

    /// The language with an ISO 639-3 code, in any case, as Who's On First's
    /// `wof:lang_x_spoken` lists them, or `None` if it isn't one of these.
    #[must_use]
    pub fn from_iso_639_3(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.iso_639_3().eq_ignore_ascii_case(code))
    }

    /// The languages of ISO 639-3 codes, skipping the others.
    #[must_use]
    pub fn from_codes<S: AsRef<str>>(codes: &[S]) -> Vec<Self> {
        codes.iter().filter_map(|code| Self::from_iso_639_3(code.as_ref())).collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for language in Language::ALL {
            assert_eq!(Language::from_iso_639_3(language.iso_639_3()), Some(language));
        }
        assert_eq!(Language::from_iso_639_3("ENG"), Some(Language::English));
        assert_eq!(Language::from_iso_639_3("jpn"), None);
        assert_eq!(Language::from_codes(&["fra", "xxx", "deu"]), vec![Language::French, Language::German]);
    }
}
