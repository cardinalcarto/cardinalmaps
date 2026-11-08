//! Spoken forms of street names and signs (`baldr::VerbalTextFormatter` and its US variants).
//!
//! Outside the US, mainline speaks names as written.
//! In the US it expands route designations ("I-5" becomes "Interstate 5")
//! and splits long numbers into pairs of digits ("1500" becomes "15 00"),
//! with extra rules for Texas and Colorado.
//!
//! Mainline uses `std::regex` (ECMAScript grammar) on bytes, with the "C" locale.
//! So character classes, word boundaries, and case-insensitivity are ASCII-only,
//! and we match on bytes with Unicode disabled to get the same behavior.
//! Every replacement inserts only ASCII text at ASCII boundaries,
//! so the result is valid UTF-8 whenever the input is.

use crate::error::OdinError;
use crate::markup_formatter::MarkupFormatter;
use crate::signs::Sign;
use crate::street_names::StreetName;
use regex::bytes::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

/// Which formatting rules apply, chosen by country and state (`VerbalTextFormatterFactory`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum VerbalTextFormatter {
    /// Names are spoken as written.
    #[default]
    Generic,
    /// US rules.
    Us,
    /// US rules plus Colorado state routes.
    UsColorado,
    /// US rules plus Texas farm and ranch to market roads.
    UsTexas,
}

/// A compiled pattern and its replacement (with `${n}` group references).
struct FindReplace {
    regex: Regex,
    replacement: &'static str,
}

impl FindReplace {
    fn new(pattern: &str, replacement: &'static str) -> Self {
        // The patterns are fixed and covered by tests, so a failure here is a programming error.
        #[expect(
            clippy::expect_used,
            reason = "the patterns are constants covered by tests"
        )]
        let regex = Regex::new(&format!("(?-u){pattern}")).expect("valid verbal formatter regex");
        Self { regex, replacement }
    }

    fn replace<'a>(&self, source: &'a [u8]) -> Cow<'a, [u8]> {
        self.regex.replace_all(source, self.replacement.as_bytes())
    }
}

/// Builds a list of case-insensitive find and replace pairs.
fn icase(pairs: &[(&str, &'static str)]) -> Vec<FindReplace> {
    pairs
        .iter()
        .map(|(pattern, replacement)| FindReplace::new(&format!("(?i){pattern}"), replacement))
        .collect()
}

/// The base formatter's number splitter, which only the tests use (see [`number_split`]).
#[cfg(test)]
static NUMBER_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| FindReplace::new(r"([^0-9]*)([0-9]+)([^0-9]*)", "").regex);
static US_NUMBER_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| FindReplace::new(r"(?i)([^0-9]*)([0-9]+)(st|nd|rd|th)?([^0-9]*)", "").regex);
static INTERSTATE: LazyLock<FindReplace> =
    LazyLock::new(|| FindReplace::new(r"(?i)(\bI)([ -])(H)?([0-9]{1,3})", "Interstate ${3}${4}"));
static US_HIGHWAY: LazyLock<FindReplace> =
    LazyLock::new(|| FindReplace::new(r"(?i)(\bUS)([ -])(Highway )?([0-9]{1,3})", "U.S. ${3}${4}"));
static LEADING_OH: LazyLock<FindReplace> =
    LazyLock::new(|| FindReplace::new(r"( )(0)([1-9])", "${1}o${3}"));
static THOUSANDS: LazyLock<Vec<FindReplace>> = LazyLock::new(|| {
    vec![
        FindReplace::new(r"(^|[^0-9])([1-9]{1,2})(000$)", "${1}${2} thousand"),
        FindReplace::new(r"(?i)(^|[^0-9])([1-9]{1,2})(000th)", "${1}${2} thousandth"),
        FindReplace::new(r"(^|[^0-9])([1-9]{1,2})(000)( |-)", "${1}${2} thousand "),
        FindReplace::new(
            r"(^|[^0-9])([1-9]{1,2})(000)([^0-9])",
            "${1}${2} thousand ${4}",
        ),
    ]
});
static HUNDREDS: LazyLock<Vec<FindReplace>> = LazyLock::new(|| {
    vec![
        FindReplace::new(r"(^|[^0-9])([1-9]{1,2})(00$)", "${1}${2} hundred"),
        FindReplace::new(r"(?i)(^|[^0-9])([1-9]{1,2})(00th)", "${1}${2} hundredth"),
        FindReplace::new(r"(^|[^0-9])([1-9]{1,2})(00)( |-)", "${1}${2} hundred "),
        FindReplace::new(
            r"(^|[^0-9])([1-9]{1,2})(00)([^0-9])",
            "${1}${2} hundred ${4}",
        ),
    ]
});
static STATE_ROUTES: LazyLock<Vec<FindReplace>> = LazyLock::new(|| {
    icase(&[
        (r"(\bSR)([ -])?([0-9]{1,4})", "State Route ${3}"),
        (r"(\bSH)([ -])?([0-9]{1,4})", "State Highway ${3}"),
        (r"(\bCA)([ -])([0-9]{1,3})", "California ${3}"),
        (r"(\bTX)([ -])([0-9]{1,3})", "Texas ${3}"),
        (r"(\bFL)([ -])(A)?([0-9]{1,3})", "Florida ${3}${4}"),
        (r"(\bNY)([ -])([0-9]{1,3})", "New York ${3}"),
        (r"(\bIL)([ -])([0-9]{1,3})", "Illinois ${3}"),
        (r"(\bPA)([ -])([0-9]{1,3})", "Pennsylvania ${3}"),
        (r"(\bOH)([ -])([0-9]{1,3})", "Ohio ${3}"),
        (r"(\bGA)([ -])([0-9]{1,3})", "Georgia ${3}"),
        (r"(\bNC)([ -])([0-9]{1,3})", "North Carolina ${3}"),
        (r"(\bM)([ -])([0-9]{1,3})", "Michigan ${3}"),
        (r"(\bNJ)([ -])([0-9]{1,3})", "New Jersey ${3}"),
        (r"(\bVA)([ -])([0-9]{1,3})", "Virginia ${3}"),
        (r"(\bWA)([ -])([0-9]{1,3})", "Washington ${3}"),
        (r"(\bMA)([ -])([0-9]{1,3})", "Massachusetts ${3}"),
        (r"(\bAZ)([ -])([0-9]{1,3})", "Arizona ${3}"),
        (r"(\bIN)([ -])([0-9]{1,3})", "Indiana ${3}"),
        (r"(\bTN)([ -])([0-9]{1,3})", "Tennessee ${3}"),
        (r"(\bMO)([ -])([0-9]{1,3})", "Missouri ${3}"),
        (r"(\bMO)([ -])([[:alpha:]]{1,2}\b)", "Missouri ${3}"),
        (r"(\bMD)([ -])([0-9]{1,3})", "Maryland ${3}"),
        (r"(\bWI)([ -])([0-9]{1,3})", "Wisconsin ${3}"),
        (r"(\bMN)([ -])([0-9]{1,3})", "Minnesota ${3}"),
        (r"(\bAL)([ -])([0-9]{1,3})", "Alabama ${3}"),
        (r"(\bSC)([ -])([0-9]{1,3})", "South Carolina ${3}"),
        (r"(\bLA)([ -])([0-9]{1,4})", "Louisiana ${3}"),
        (r"(\bKY)([ -])([0-9]{1,4})", "Kentucky ${3}"),
        (r"(\bOR)([ -])([0-9]{1,3})", "Oregon ${3}"),
        (r"(\bOK)([ -])([0-9]{1,3})", "Oklahoma ${3}"),
        (r"(\bCT)([ -])([0-9]{1,3})", "Connecticut ${3}"),
        (r"(\bIA)([ -])([0-9]{1,3})", "Iowa ${3}"),
        (r"(\bMS)([ -])([0-9]{1,3})", "Mississippi ${3}"),
        (r"(\bAR)([ -])([0-9]{1,3})", "Arkansas ${3}"),
        (r"(\bUT)([ -])([0-9]{1,3})", "Utah ${3}"),
        (r"(\bKS)([ -])([0-9]{1,3})", "Kansas ${3}"),
        (r"(\bNV)([ -])([0-9]{1,3})", "Nevada ${3}"),
        (r"(\bNM)([ -])([0-9]{1,4})", "New Mexico ${3}"),
        (r"(\bNE)([ -])([0-9]{1,3})", "Nebraska ${3}"),
        (r"(\bWV)([ -])([0-9]{1,3})", "West Virginia ${3}"),
        (r"(\bID)([ -])([0-9]{1,3})", "Idaho ${3}"),
        (r"(\bHI)([ -])([0-9]{1,4})", "Hawaii ${3}"),
        (r"(\bME)([ -])([0-9]{1,3})", "Maine ${3}"),
        (r"(\bNH)([ -])([0-9]{1,3})", "New Hampshire ${3}"),
        (r"(\bRI)([ -])([0-9]{1,3})", "Rhode Island ${3}"),
        (r"(\bMT)([ -])([0-9]{1,3})", "Montana ${3}"),
        (r"(\bDE)([ -])([0-9]{1,3})", "Delaware ${3}"),
        (r"(\bSD)([ -])([0-9]{1,4})", "South Dakota ${3}"),
        (r"(\bND)([ -])([0-9]{1,4})", "North Dakota ${3}"),
        (r"(\bAK)([ -])([0-9]{1,3})", "Alaska ${3}"),
        (r"(\bDC)([ -])([0-9]{1,3})", "D C ${3}"),
        (r"(\bVT)([ -])([0-9]{1,3})", "Vermont ${3}"),
        (r"(\bWY)([ -])([0-9]{1,3})", "Wyoming ${3}"),
    ])
});
static COUNTY_ROUTES: LazyLock<Vec<FindReplace>> = LazyLock::new(|| {
    icase(&[
        (
            r"(\bCR)([0-9]{1,4})([[:alpha:]]{1,2})?\b",
            "County Route ${2}${3}",
        ),
        (
            r"(\bCR)([ -])([[:alpha:]]{1,2})?([0-9]{1,4})([[:alpha:]]{1,2})?\b",
            "County Route ${3}${4}${5}",
        ),
        (r"(\bCR)([ -])([[:alpha:]]{1,2})\b", "County Route ${3}"),
        (
            r"(\bC R)([0-9]{1,4})([[:alpha:]]{1,2})?\b",
            "County Route ${2}${3}",
        ),
        (
            r"(\bC R)([ -])([[:alpha:]]{1,2})?([0-9]{1,4})([[:alpha:]]{1,2})?\b",
            "County Route ${3}${4}${5}",
        ),
        (r"(\bC R)([ -])([[:alpha:]]{1,2})\b", "County Route ${3}"),
        (
            r"(\bCO)([ -])?([0-9]{1,4})([[:alpha:]]{1,2})?\b",
            "County Road ${3}${4}",
        ),
    ])
});
static COLORADO: LazyLock<FindReplace> =
    LazyLock::new(|| FindReplace::new(r"(?i)(\bCO)([ -])([0-9]{1,3})", "Colorado ${3}"));
static FARM_TO_MARKET: LazyLock<FindReplace> = LazyLock::new(|| {
    FindReplace::new(
        r"(?i)(\bF[ -]?M)([ -])?([0-9]{1,4})",
        "Farm to Market Road ${3}",
    )
});
static RANCH_TO_MARKET: LazyLock<FindReplace> = LazyLock::new(|| {
    FindReplace::new(
        r"(?i)(\bR[ -]?M)([ -])?([0-9]{1,4})",
        "Ranch to Market Road ${3}",
    )
});

impl VerbalTextFormatter {
    /// The formatter for a country and state (`VerbalTextFormatterFactory::Create`).
    pub(crate) fn for_region(country_code: &str, state_code: &str) -> Self {
        match (country_code, state_code) {
            ("US", "TX") => Self::UsTexas,
            ("US", "CO") => Self::UsColorado,
            ("US", _) => Self::Us,
            _ => Self::Generic,
        }
    }

    /// The spoken form of a street name, using phoneme markup when it applies.
    pub(crate) fn format_street_name(
        self,
        street_name: &StreetName,
        markup: &MarkupFormatter,
    ) -> Result<String, OdinError> {
        if let Some(phoneme) =
            markup.format_phoneme(street_name.value(), street_name.pronunciation())?
        {
            return Ok(phoneme);
        }
        Ok(self.format(street_name.value()))
    }

    /// The spoken form of a sign, using phoneme markup when it applies.
    pub(crate) fn format_sign(
        self,
        sign: &Sign,
        markup: &MarkupFormatter,
    ) -> Result<String, OdinError> {
        if let Some(phoneme) = markup.format_phoneme(&sign.text, sign.pronunciation.as_ref())? {
            return Ok(phoneme);
        }
        Ok(self.format(&sign.text))
    }

    /// The spoken form of some text.
    pub(crate) fn format(self, text: &str) -> String {
        match self {
            Self::Generic => text.to_owned(),
            Self::Us | Self::UsColorado => to_string(&self.format_us(text.as_bytes())),
            Self::UsTexas => {
                let text = FARM_TO_MARKET.replace(text.as_bytes());
                let text = RANCH_TO_MARKET.replace(&text).into_owned();
                to_string(&self.format_us(&text))
            }
        }
    }

    /// `VerbalTextFormatterUs::Format`.
    fn format_us(self, text: &[u8]) -> Vec<u8> {
        let text = INTERSTATE.replace(text).into_owned();
        let text = US_HIGHWAY.replace(&text).into_owned();
        let text = self.process_states(&text);
        let text = process_first_change(&COUNTY_ROUTES, &text);
        let text = process_all(&THOUSANDS, &text);
        let text = process_all(&HUNDREDS, &text);
        let text = us_number_split(&text);
        LEADING_OH.replace(&text).into_owned()
    }

    fn process_states(self, text: &[u8]) -> Vec<u8> {
        if self == Self::UsColorado {
            let colorado = COLORADO.replace(text);
            if *colorado != *text {
                return colorado.into_owned();
            }
        }
        process_first_change(&STATE_ROUTES, text)
    }
}

/// Applies the first rule that changes the text, if any.
fn process_first_change(rules: &[FindReplace], text: &[u8]) -> Vec<u8> {
    for rule in rules {
        let replaced = rule.replace(text);
        if *replaced != *text {
            return replaced.into_owned();
        }
    }
    text.to_vec()
}

/// Applies every rule in turn.
fn process_all(rules: &[FindReplace], text: &[u8]) -> Vec<u8> {
    let mut text = text.to_vec();
    for rule in rules {
        text = rule.replace(&text).into_owned();
    }
    text
}

/// Inserts a space between every pair of digits counted from the right ("12345" becomes "1 23 45").
fn split_number(digits: &[u8], out: &mut Vec<u8>) {
    // Mainline inserts a space at index 2 (even lengths) or 1 (odd lengths),
    // then every 3 bytes after that, in the growing string.
    // That leaves the leading group with one or two digits and pairs after it.
    let first = if digits.len().is_multiple_of(2) { 2 } else { 1 };
    for (i, digit) in digits.iter().enumerate() {
        if i >= first && (i - first) % 2 == 0 {
            out.push(b' ');
        }
        out.push(*digit);
    }
}

/// The base formatter's `FormNumberSplitTts`, which US formatting overrides.
///
/// Mainline never calls it (the base `Format` does not split numbers),
/// but it documents the difference from [`us_number_split`].
#[cfg(test)]
fn number_split(source: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for captures in NUMBER_SPLIT.captures_iter(source) {
        if let Some(m) = captures.get(1) {
            out.extend_from_slice(m.as_bytes());
        }
        if let Some(m) = captures.get(2) {
            split_number(m.as_bytes(), &mut out);
        }
        if let Some(m) = captures.get(3) {
            out.extend_from_slice(m.as_bytes());
        }
    }
    if out.is_empty() { source.to_vec() } else { out }
}

/// `VerbalTextFormatterUs::FormNumberSplitTts`: splits numbers, except ordinals like "21st".
fn us_number_split(source: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for captures in US_NUMBER_SPLIT.captures_iter(source) {
        if let Some(m) = captures.get(1) {
            out.extend_from_slice(m.as_bytes());
        }
        if let Some(number) = captures.get(2) {
            if let Some(suffix) = captures.get(3) {
                out.extend_from_slice(number.as_bytes());
                out.extend_from_slice(suffix.as_bytes());
            } else {
                split_number(number.as_bytes(), &mut out);
            }
        }
        if let Some(m) = captures.get(4) {
            out.extend_from_slice(m.as_bytes());
        }
    }
    if out.is_empty() { source.to_vec() } else { out }
}

fn to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(formatter: VerbalTextFormatter, cases: &[(&str, &str)]) {
        for (source, expected) in cases {
            assert_eq!(
                formatter.format(source),
                *expected,
                "{formatter:?} formatting {source:?}"
            );
        }
    }

    /// The cases from mainline's `test/verbal_text_formatter_us.cc` (`TestFormat`).
    const US_CASES: &[(&str, &str)] = &[
        ("905 203rd Street", "9 o5 203rd Street"),
        ("AL 233", "Alabama 2 33"),
        ("AR 107", "Arkansas 1 o7"),
        ("Co-17", "County Road 17"),
        ("Co 2000A", "County Road 2 thousand A"),
        ("Co 2000BAD", "Co 2 thousand BAD"),
        ("Co 2000", "County Road 2 thousand"),
        ("Co 200A", "County Road 2 hundred A"),
        ("Co 200BAD", "Co 2 hundred BAD"),
        ("Co 200", "County Road 2 hundred"),
        ("Co39", "County Road 39"),
        ("Co 7", "County Road 7"),
        ("CR 0", "County Route 0"),
        ("CR 1.00", "County Route 1.00"),
        ("CR 100", "County Route 1 hundred"),
        ("CR 100 S", "County Route 1 hundred S"),
        ("CR116", "County Route 1 16"),
        ("C R 13B", "County Route 13B"),
        ("CR 18VV", "County Route 18VV"),
        ("CR-4003", "County Route 40 o3"),
        ("C R 404", "County Route 4 o4"),
        ("CR 539", "County Route 5 39"),
        ("CR-852", "County Route 8 52"),
        ("Cr 8542 Marmot Trail", "County Route 85 42 Marmot Trail"),
        ("C R A13", "County Route A13"),
        ("CR A", "County Route A"),
        ("Crap", "Crap"),
        ("CR Ettinger", "CR Ettinger"),
        ("CR J29", "County Route J29"),
        ("CR JJ", "County Route JJ"),
        ("CT 14A", "Connecticut 14A"),
        ("i-205 East", "Interstate 2 o5 East"),
        ("I 270 Spur", "Interstate 2 70 Spur"),
        ("I 35E", "Interstate 35E"),
        ("I 35", "Interstate 35"),
        ("I 35W", "Interstate 35W"),
        ("I 495 West", "Interstate 4 95 West"),
        ("I 5", "Interstate 5"),
        ("I-695 West", "Interstate 6 95 West"),
        ("I-895", "Interstate 8 95"),
        ("I 95 South", "Interstate 95 South"),
        ("I H1", "Interstate H1"),
        ("North 28000th Avenue", "North 28 thousandth Avenue"),
        ("North 28000TH Avenue", "North 28 thousandth Avenue"),
        ("North 28000th", "North 28 thousandth"),
        ("North 2800th Avenue", "North 28 hundredth Avenue"),
        ("North 2800Th Avenue", "North 28 hundredth Avenue"),
        ("North 2800th", "North 28 hundredth"),
        ("PA 100", "Pennsylvania 1 hundred"),
        ("PA 1080", "Pennsylvania 10 80"),
        ("PA 23", "Pennsylvania 23"),
        ("PA 283", "Pennsylvania 2 83"),
        ("PA 4007", "Pennsylvania 40 o7"),
        ("Sh 1021", "State Highway 10 21"),
        ("SH 1021", "State Highway 10 21"),
        ("SH-1021", "State Highway 10 21"),
        ("SH3032", "State Highway 30 32"),
        ("Sr 1021", "State Route 10 21"),
        ("SR 1021", "State Route 10 21"),
        ("SR-1021", "State Route 10 21"),
        ("SR3032", "State Route 30 32"),
        ("T609", "T6 o9"),
        ("US 1", "U.S. 1"),
        ("US 202", "U.S. 2 o2"),
        ("US 220 North", "U.S. 2 20 North"),
        ("US-220 South", "U.S. 2 20 South"),
        ("US 22", "U.S. 22"),
        ("Us 422 Business Alternate", "U.S. 4 22 Business Alternate"),
        ("Us-522", "U.S. 5 22"),
        ("US Highway 222", "U.S. Highway 2 22"),
    ];

    #[test]
    fn us_matches_mainline_tests() {
        check(VerbalTextFormatter::Us, US_CASES);
    }

    // The distinctive cases from mainline's `test/verbal_text_formatter_us_co.cc`.
    #[test]
    fn colorado_matches_mainline_tests() {
        check(
            VerbalTextFormatter::UsColorado,
            &[
                ("Co 200", "Colorado 2 hundred"),
                ("Co 7", "Colorado 7"),
                ("CR-4003", "County Route 40 o3"),
                ("I 495 West", "Interstate 4 95 West"),
                ("PA 4007", "Pennsylvania 40 o7"),
                ("US 202", "U.S. 2 o2"),
            ],
        );
    }

    // The distinctive cases from mainline's `test/verbal_text_formatter_us_tx.cc`.
    #[test]
    fn texas_matches_mainline_tests() {
        check(
            VerbalTextFormatter::UsTexas,
            &[
                ("F M 1018", "Farm to Market Road 10 18"),
                ("F-M 1018", "Farm to Market Road 10 18"),
                ("F-M-1018", "Farm to Market Road 10 18"),
                ("FM 1018", "Farm to Market Road 10 18"),
                ("FM-1018", "Farm to Market Road 10 18"),
                ("FM1018", "Farm to Market Road 10 18"),
                ("R M 1018", "Ranch to Market Road 10 18"),
                ("R-M 1018", "Ranch to Market Road 10 18"),
                ("R-M-1018", "Ranch to Market Road 10 18"),
                ("RM 1018", "Ranch to Market Road 10 18"),
                ("RM-1018", "Ranch to Market Road 10 18"),
                ("RM1018", "Ranch to Market Road 10 18"),
                ("CR 539", "County Route 5 39"),
                ("SR-1021", "State Route 10 21"),
                ("T609", "T6 o9"),
            ],
        );
    }

    // The cases from mainline's `test/verbal_text_formatter.cc`.
    #[test]
    fn generic_is_unchanged() {
        check(
            VerbalTextFormatter::Generic,
            &[
                ("A104", "A104"),
                ("A320", "A320"),
                ("E411", "E411"),
                ("CG-2 12345", "CG-2 12345"),
            ],
        );
        // The base number splitter (which mainline never calls) splits ordinals too.
        assert_eq!(number_split(b"21st 12345"), b"21st 1 23 45".to_vec());
        assert_eq!(number_split(b"Main"), b"Main".to_vec());
    }

    #[test]
    fn non_ascii_text_survives() {
        check(
            VerbalTextFormatter::Us,
            &[("Café 1234 Straße", "Café 12 34 Straße")],
        );
        // Case folding is ASCII-only, as with `std::regex` in the "C" locale: "ſt" is not "st".
        check(VerbalTextFormatter::Us, &[("21ſt", "21ſt")]);
    }

    #[test]
    fn factory_picks_by_region() {
        assert_eq!(
            VerbalTextFormatter::for_region("US", "TX"),
            VerbalTextFormatter::UsTexas
        );
        assert_eq!(
            VerbalTextFormatter::for_region("US", "CO"),
            VerbalTextFormatter::UsColorado
        );
        assert_eq!(
            VerbalTextFormatter::for_region("US", "WA"),
            VerbalTextFormatter::Us
        );
        assert_eq!(
            VerbalTextFormatter::for_region("AD", ""),
            VerbalTextFormatter::Generic
        );
    }
}
