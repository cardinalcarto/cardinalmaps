//! Classifications attached to spans (upstream `classification/*.js`).

use serde::Serialize;

macro_rules! kinds {
    ($($variant:ident => $name:literal, $label:literal, $public:literal;)*) => {
        /// Every classification upstream defines. Public ones (see
        /// [`Kind::is_public`]) can appear in parse solutions; private ones are
        /// intermediate features used by composite classifiers and solvers.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Kind {
            $(
                #[doc = concat!("`", $label, "` (upstream `", $name, "`)")]
                $variant,
            )*
        }

        impl Kind {
            /// Every kind.
            pub const ALL: &'static [Kind] = &[$(Kind::$variant),*];

            /// Upstream class name, e.g. `"HouseNumberClassification"`.
            pub fn class_name(self) -> &'static str {
                match self { $(Kind::$variant => $name,)* }
            }

            /// Upstream label, e.g. `"housenumber"`.
            pub fn label(self) -> &'static str {
                match self { $(Kind::$variant => $label,)* }
            }

            /// Whether upstream includes this classification in solutions.
            pub fn is_public(self) -> bool {
                match self { $(Kind::$variant => $public,)* }
            }
        }
    };
}

kinds! {
    Adjacent => "AdjacentClassification", "adjacent", false;
    Alpha => "AlphaClassification", "alpha", false;
    AlphaNumeric => "AlphaNumericClassification", "alphanumeric", false;
    Area => "AreaClassification", "area", false;
    Chain => "ChainClassification", "chain", false;
    Country => "CountryClassification", "country", true;
    Dependency => "DependencyClassification", "dependency", true;
    Directional => "DirectionalClassification", "directional", false;
    EndToken => "EndTokenClassification", "end_token", false;
    EndTokenSingleCharacter => "EndTokenSingleCharacterClassification", "end_token_single_character", false;
    GivenName => "GivenNameClassification", "given_name", false;
    HouseNumber => "HouseNumberClassification", "housenumber", true;
    Intersection => "IntersectionClassification", "intersection", false;
    Locality => "LocalityClassification", "locality", true;
    MiddleInitial => "MiddleInitialClassification", "middle_initial", false;
    MultiStreet => "MultiStreetClassification", "multistreet", false;
    Numeric => "NumericClassification", "numeric", false;
    Ordinal => "OrdinalClassification", "ordinal", false;
    Person => "PersonClassification", "person", false;
    PersonalSuffix => "PersonalSuffixClassification", "personal_suffix", false;
    PersonalTitle => "PersonalTitleClassification", "personal_title", false;
    Place => "PlaceClassification", "place", false;
    Postcode => "PostcodeClassification", "postcode", true;
    Punctuation => "PunctuationClassification", "punctuation", false;
    Region => "RegionClassification", "region", true;
    RoadType => "RoadTypeClassification", "road_type", false;
    SingleAlpha => "SingleAlphaClassification", "single_alpha", false;
    StartToken => "StartTokenClassification", "start_token", false;
    StopWord => "StopWordClassification", "stop_word", false;
    Street => "StreetClassification", "street", true;
    StreetName => "StreetNameClassification", "street_name", false;
    StreetPrefix => "StreetPrefixClassification", "street_prefix", false;
    StreetProperName => "StreetProperNameClassification", "street_proper_name", false;
    StreetSuffix => "StreetSuffixClassification", "street_suffix", false;
    Subdivision => "SubdivisionClassification", "subdivision", false;
    Surname => "SurnameClassification", "surname", false;
    Toponym => "ToponymClassification", "toponym", false;
    Unit => "UnitClassification", "unit", true;
    UnitType => "UnitTypeClassification", "unit_type", true;
    Venue => "VenueClassification", "venue", true;
}

/// A classification of a span with a confidence in `(0, 1]`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Classification {
    pub kind: Kind,
    pub confidence: f64,
    /// Upstream `meta.langs`: `None` when the classification has no `langs`
    /// metadata, otherwise the language keys in insertion order.
    pub langs: Option<Vec<String>>,
}

impl Classification {
    /// Upstream `new XClassification(confidence)`. As upstream
    /// (`confidence || 1.0`), a confidence of zero means 1.0.
    pub fn new(kind: Kind, confidence: f64) -> Self {
        Classification {
            kind,
            confidence: if confidence == 0.0 { 1.0 } else { confidence },
            langs: None,
        }
    }

    pub fn with_langs(kind: Kind, confidence: f64, langs: Vec<String>) -> Self {
        Classification {
            langs: Some(langs),
            ..Self::new(kind, confidence)
        }
    }
}
