//! Labelled queries for training the tagger, made from the index's places.
//!
//! Each example is a place written the way people search for it: its name
//! or address or a kind of place it is, then (sometimes) some of the areas
//! it's in, in local or English names, abbreviated or not, with or without
//! commas and words like "near", sometimes cut off mid-word as if still
//! being typed, sometimes with a typo, and seen from a focus near it, far
//! from it, or none. Every word is labelled by what it came from.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use airmail_tagger::features::Query;
use airmail_tagger::labels::Tag;
use airmail_tagger::text::{fold, words};
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{Rng, SeedableRng};
use tantivy::DocAddress;

use crate::index::{AirmailIndex, SearchHit};
use crate::poi::{AdminBand, Kind, WofPlace};

/// A labelled query.
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub query: Query,
    pub tags: Vec<Tag>,
}

/// How likely each part of a query is.
mod odds {
    /// No areas at all: `starbucks`, `1530 post alley`. Otherwise each band
    /// is in with its own odds.
    pub const NO_AREAS: f64 = 0.4;
    pub const NEIGHBOURHOOD: f64 = 0.25;
    pub const LOCALITY: f64 = 0.7;
    pub const COUNTY: f64 = 0.05;
    pub const REGION: f64 = 0.3;
    pub const COUNTRY: f64 = 0.12;
    /// A region or country by its abbreviation or code.
    pub const ABBREVIATION: f64 = 0.4;
    /// Areas by their name in a language of their country, where it has
    /// its own (see `local_languages`): `москва`, `roma`, `東京`.
    pub const LOCAL: f64 = 0.5;
    /// English rather than default names for areas, otherwise.
    pub const ENGLISH: f64 = 0.6;
    pub const COMMAS: f64 = 0.35;
    pub const CONNECTOR: f64 = 0.15;
    /// Of POIs without an address, a postcode for where: `pizza 98101`.
    pub const POI_POSTCODE: f64 = 0.05;
    /// Of venues without areas: `coffee near me`.
    pub const NEAR_HERE: f64 = 0.1;
    /// Of venues with areas, the most specific area before it, alone:
    /// `seattle starbucks`, `ballard fred meyer`.
    pub const AREAS_FIRST: f64 = 0.2;
    /// The locality, of places with a neighbourhood that's in: people
    /// naming a neighbourhood often leave its city out.
    pub const LOCALITY_WITH_NEIGHBOURHOOD: f64 = 0.4;
    /// A word with other forms in another of them: `street` as `st`,
    /// `north` as `n`, `saint` as `st`, and back.
    pub const OTHER_FORM: f64 = 0.35;
    /// A comma where commas don't usually go: after a venue, between a house
    /// number and its street.
    pub const STRAY_COMMA: f64 = 0.02;
    /// A POI's address with its name.
    pub const POI_ADDRESS: f64 = 0.1;
    /// A kind of place instead of a POI's name.
    pub const CATEGORY: f64 = 0.2;
    /// A postcode with an address.
    pub const POSTCODE: f64 = 0.2;
    pub const TYPING: f64 = 0.3;
    pub const TYPO: f64 = 0.04;
    pub const FOCUS_NEAR: f64 = 0.6;
    pub const FOCUS_FAR: f64 = 0.2;
}

/// Countries whose addresses put the house number after the street.
/// A country's main language, as libpostal's dictionaries name it, for
/// connector words.
/// The languages, as Who's On First's ISO 639-3 codes, that a country's
/// places are named in locally, where that isn't English (whose names are
/// `odds::ENGLISH`'s): the countries of most of the world's people and
/// places, not all.
fn local_languages(country: &str) -> &'static [&'static str] {
    match country.to_ascii_uppercase().as_str() {
        "RU" | "BY" | "KZ" | "KG" => &["rus"],
        "UA" => &["ukr"],
        "DE" | "AT" | "LI" => &["deu"],
        "CH" => &["deu", "fra", "ita"],
        "FR" | "LU" | "MC" | "SN" | "CI" | "ML" | "BF" | "NE" | "GN" | "CD" | "CG" | "CM" | "MG" | "HT" => &["fra"],
        "BE" => &["nld", "fra"],
        "NL" => &["nld"],
        "ES" | "MX" | "AR" | "CL" | "CO" | "PE" | "VE" | "UY" | "EC" | "BO" | "PY" | "CR" | "GT" | "CU" | "DO" | "HN" | "NI" | "SV"
        | "PA" => &["spa"],
        "IT" | "SM" | "VA" => &["ita"],
        "PT" | "BR" | "AO" | "MZ" => &["por"],
        "PL" => &["pol"],
        "SE" => &["swe"],
        "DK" => &["dan"],
        "NO" => &["nob", "nor"],
        "FI" => &["fin"],
        "IS" => &["isl"],
        "CZ" => &["ces"],
        "SK" => &["slk"],
        "HU" => &["hun"],
        "RO" | "MD" => &["ron"],
        "BG" => &["bul"],
        "GR" | "CY" => &["ell"],
        "TR" => &["tur"],
        "RS" => &["srp"],
        "HR" => &["hrv"],
        "SI" => &["slv"],
        "BA" => &["bos", "srp", "hrv"],
        "MK" => &["mkd"],
        "AL" => &["sqi"],
        "LT" => &["lit"],
        "LV" => &["lav"],
        "EE" => &["est"],
        "GE" => &["kat"],
        "AM" => &["hye"],
        "AZ" => &["aze"],
        "JP" => &["jpn"],
        "CN" | "TW" | "HK" | "MO" => &["zho"],
        "KR" => &["kor"],
        "TH" => &["tha"],
        "VN" => &["vie"],
        "ID" => &["ind"],
        "MY" => &["msa"],
        "KH" => &["khm"],
        "LA" => &["lao"],
        "MM" => &["mya"],
        "MN" => &["mon"],
        "IL" => &["heb"],
        "SA" | "AE" | "EG" | "MA" | "DZ" | "TN" | "LY" | "IQ" | "SY" | "JO" | "LB" | "YE" | "OM" | "KW" | "QA" | "BH" | "SD" | "PS" => &["ara"],
        "IR" | "AF" => &["fas"],
        "IN" => &["hin"],
        "PK" => &["urd"],
        "BD" => &["ben"],
        "NP" => &["nep"],
        "LK" => &["sin"],
        "ET" => &["amh"],
        _ => &[],
    }
}

fn language(country: Option<&str>) -> &'static str {
    match country.unwrap_or("") {
        "DE" | "AT" | "LI" => "de",
        "FR" | "BE" | "LU" | "MC" => "fr",
        "ES" | "MX" | "AR" | "CL" | "CO" | "PE" | "VE" | "UY" | "EC" | "BO" | "PY" | "CR" | "GT" | "CU" => "es",
        "IT" | "SM" => "it",
        "PT" | "BR" => "pt",
        "NL" => "nl",
        "PL" => "pl",
        "SE" => "sv",
        "DK" => "da",
        "NO" => "nb",
        "FI" => "fi",
        "CZ" => "cs",
        "RU" | "BY" | "KZ" => "ru",
        "UA" => "uk",
        "TR" => "tr",
        _ => "en",
    }
}

/// Words relating what's sought to where, by language.
/// Phrases for being near a place, by language: those that take a place
/// after them (`near`, `près de`), and those that stand alone (`near me`,
/// `près d'ici`).
struct Connectors {
    before_place: HashMap<&'static str, Vec<String>>,
    standalone: HashMap<&'static str, Vec<String>>,
}

/// The last words of `near` phrases that stand alone: me, here and nearby,
/// in the dictionaries' languages.
const STANDALONE_LAST_WORDS: &[&str] = &[
    "me", "here", "nearby", "men", "tuk", "наблизо", "nablizo", "mene", "ovdje", "ovde", "aquí", "aqui", "meu", "tady", "mig",
    "her", "hier", "mir", "gegend", "acá", "aca", "mi", "mì", "mulle", "siinkandis", "lahedal", "hemen", "gertu", "minua", "täällä",
    "taalla", "lähistöllä", "lahistolla", "lähellä", "lahella", "moi", "ici", "dici", "d'ici", "d'aquí", "d'aqui", "daquí", "là", "la", "alentours", "közelben",
    "kozelben", "közelemben", "kozelemben", "hér", "hérna", "herna", "mér", "mer", "qui", "vicinanze", "meg", "naerheten",
    "nærheten", "naerheden", "nærheden", "buurt", "mnie", "tutaj", "poblizu", "pobliżu", "mim", "daqui", "aici", "apropiere",
    "mine", "мной", "mnoy", "здесь", "zdes", "zdes'", "поблизости", "poblizosti", "tu", "mne", "okoli", "okolí", "tukaj",
    "här", "har", "narheten", "närheten", "тут", "tut", "мною", "mnoyu", "поблизу", "poblyzu", "nähe", "nahe", "naehe",
];

impl Connectors {
    fn new() -> Self {
        let (mut before_place, mut standalone) = (HashMap::new(), HashMap::new());
        for (language, contents) in airmail_tagger::dictionaries::of_type("near") {
            let (alone, before): (Vec<String>, Vec<String>) = contents
                .lines()
                .flat_map(|l| l.split('|'))
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .partition(|p| p.split_whitespace().last().is_some_and(|w| STANDALONE_LAST_WORDS.contains(&w)));
            before_place.insert(language, before);
            standalone.insert(language, alone);
        }
        Self { before_place, standalone }
    }

    /// One that takes a place after it.
    fn pick(&self, language: &str, rng: &mut StdRng) -> Option<&str> {
        Self::choose(&self.before_place, language, rng)
    }

    /// One that stands alone.
    fn pick_standalone(&self, language: &str, rng: &mut StdRng) -> Option<&str> {
        Self::choose(&self.standalone, language, rng)
    }

    fn choose<'a>(phrases: &'a HashMap<&'static str, Vec<String>>, language: &str, rng: &mut StdRng) -> Option<&'a str> {
        let phrases = phrases.get(language).filter(|p| !p.is_empty()).or_else(|| phrases.get("en"))?;
        phrases.choose(rng).map(String::as_str)
    }
}

/// Words' other forms, from libpostal's dictionaries: `street` → `st`,
/// `north` → `n`, `saint` → `st`, and back. Single words only.
struct Forms(HashMap<String, Vec<String>>);

impl Forms {
    fn new(dictionaries: &[&str]) -> Self {
        let mut forms: HashMap<String, Vec<String>> = HashMap::new();
        for dictionary in dictionaries {
            for (_, contents) in airmail_tagger::dictionaries::of_type(dictionary) {
                for line in contents.lines() {
                    let all: Vec<String> =
                        line.split('|').map(|f| fold(f.trim()).trim_end_matches('.').to_string()).filter(|f| !f.is_empty() && !f.contains(' ')).collect();
                    for form in &all {
                        let others = forms.entry(form.clone()).or_default();
                        others.extend(all.iter().filter(|o| *o != form).cloned());
                    }
                }
            }
        }
        for others in forms.values_mut() {
            others.sort();
            others.dedup();
        }
        Self(forms)
    }

    /// A word in another form, sometimes.
    fn vary(&self, word: &str, rng: &mut StdRng) -> String {
        match self.0.get(word) {
            Some(others) if rng.random_bool(odds::OTHER_FORM) => others.choose(rng).cloned().unwrap_or_else(|| word.to_string()),
            _ => word.to_string(),
        }
    }
}

/// A part of a query: words and their tag.
#[derive(Debug, Clone)]
struct Part {
    /// As written, for the address templates.
    text: String,
    words: Vec<String>,
    tag: Tag,
}

fn part(text: &str, tag: Tag) -> Option<Part> {
    // As the index normalizes queries, so the tagger is trained on the words
    // it'll see.
    let words: Vec<String> = words(&crate::index::normalize_text(text)).into_iter().map(|(_, w)| w.to_string()).collect();
    (!words.is_empty()).then_some(Part { text: text.to_string(), words, tag })
}

/// Formatted examples whose words couldn't all be traced back to their
/// parts, and were left out.
static UNALIGNED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many formatted examples were left out because their words couldn't
/// all be traced back to their parts.
pub fn unaligned() -> usize {
    UNALIGNED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Generates examples.
pub struct Generator {
    connectors: Connectors,
    road_forms: Forms,
    name_forms: Forms,
    rng: StdRng,
}

impl Generator {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            connectors: Connectors::new(),
            road_forms: Forms::new(&["street_types", "directionals", "synonyms", "personal_titles", "street_names"]),
            name_forms: Forms::new(&["synonyms", "personal_titles", "directionals"]),
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// Starts its random choices again from `seed`.
    pub fn reseed(&mut self, seed: u64) {
        self.rng = StdRng::seed_from_u64(seed);
    }

    /// An area's name as a query might write it.
    fn area_name(&mut self, area: &WofPlace) -> Option<String> {
        let band = AdminBand::of(&area.placetype)?;
        let abbreviates = matches!(band, AdminBand::Region | AdminBand::Country);
        if abbreviates && self.rng.random_bool(odds::ABBREVIATION) {
            // Countries mostly by ISO code (`us`, `deu`), else as Who's On
            // First abbreviates them.
            if band == AdminBand::Country
                && let Some(code) = &area.country
                && (area.abbreviation.is_none() || self.rng.random_bool(0.7))
            {
                let codes = crate::countries::codes(code);
                if let Some(code) = codes.choose(&mut self.rng) {
                    return Some(code.clone());
                }
            }
            if let Some(abbreviation) = &area.abbreviation {
                return Some(abbreviation.clone());
            }
        }
        // As libpostal trains on places' local, romanized and English names.
        let local: Vec<&String> =
            area.country.as_deref().map(local_languages).unwrap_or_default().iter().filter_map(|lang| area.names.get(*lang)).collect();
        if !local.is_empty() && self.rng.random_bool(odds::LOCAL) {
            return local.choose(&mut self.rng).map(|name| (*name).clone());
        }
        if self.rng.random_bool(odds::ENGLISH)
            && let Some(english) = area.names.get("eng")
        {
            return Some(english.clone());
        }
        Some(area.default_name.clone())
    }

    /// The areas containing a place, as parts, smallest first: some of them,
    /// sampled by band.
    fn areas(&mut self, hit: &SearchHit, admins: &HashMap<u64, WofPlace>) -> Vec<Part> {
        if self.rng.random_bool(odds::NO_AREAS) {
            return Vec::new();
        }
        let mut by_band: Vec<(AdminBand, &WofPlace)> = hit
            .admin_ids
            .iter()
            .filter_map(|id| admins.get(id))
            .filter_map(|a| AdminBand::of(&a.placetype).map(|b| (b, a)))
            .collect();
        by_band.sort_by_key(|(band, a)| (*band, a.id));
        by_band.dedup_by_key(|(band, _)| *band);
        let mut parts: Vec<Part> = Vec::new();
        for (band, area) in by_band {
            let odds = match band {
                AdminBand::Neighbourhood => odds::NEIGHBOURHOOD,
                AdminBand::Locality if parts.iter().any(|p| p.tag == Tag::Neighbourhood) => odds::LOCALITY_WITH_NEIGHBOURHOOD,
                AdminBand::Locality => odds::LOCALITY,
                AdminBand::County => odds::COUNTY,
                AdminBand::Region => odds::REGION,
                AdminBand::Country => odds::COUNTRY,
            };
            if !self.rng.random_bool(odds) {
                continue;
            }
            let tag = match band {
                AdminBand::Neighbourhood => Tag::Neighbourhood,
                AdminBand::Locality => Tag::Locality,
                AdminBand::County => Tag::County,
                AdminBand::Region => Tag::Region,
                AdminBand::Country => Tag::Country,
            };
            if let Some(name) = self.area_name(area) {
                parts.extend(part(&name, tag));
            }
        }
        parts
    }

    /// A venue's query: the venue, perhaps the most specific area before it
    /// (`ballard fred meyer`), else the areas after it, perhaps after a
    /// connector (`pizza near capitol hill`) or, with none, a standalone
    /// phrase (`coffee near me`); any postcode last (`pizza 98101`).
    fn venue_query(&mut self, subject: Vec<Part>, mut areas: Vec<Part>, postcode: Option<Part>, country: Option<&str>, commas: bool) -> Vec<(Part, bool)> {
        let mut parts: Vec<(Part, bool)> = Vec::new();
        let areas_first = !areas.is_empty() && self.rng.random_bool(odds::AREAS_FIRST);
        if areas_first {
            areas.truncate(1);
        }
        let connector = (!areas.is_empty() && !areas_first && self.rng.random_bool(odds::CONNECTOR))
            .then(|| self.connectors.pick(language(country), &mut self.rng).and_then(|c| part(c, Tag::Near)))
            .flatten();
        if areas_first {
            parts.extend(areas.drain(..).map(|a| (a, false)));
            parts.extend(subject.into_iter().map(|s| (s, false)));
        } else {
            parts.extend(subject.into_iter().map(|s| (s, false)));
            if let Some(connector) = connector {
                parts.push((connector, false));
            }
            let no_areas = areas.is_empty();
            parts.extend(areas.into_iter().map(|a| (a, commas)));
            if no_areas
                && postcode.is_none()
                && self.rng.random_bool(odds::NEAR_HERE)
                && let Some(near) = self.connectors.pick_standalone(language(country), &mut self.rng).and_then(|c| part(c, Tag::Near))
            {
                parts.push((near, false));
            }
        }
        parts.extend(postcode.map(|p| (p, false)));
        parts
    }

    /// An address's parts: its house number, road and any unit. The
    /// templates put them in the country's order.
    fn address(hit: &SearchHit) -> Vec<Part> {
        // The first of several: `24,26`, `12;12a`.
        let number = hit.tag("addr:housenumber").and_then(|n| part(n.split([';', ',']).next().unwrap_or(n), Tag::HouseNumber));
        let road = hit.tag("addr:street").and_then(|r| part(r, Tag::Road));
        let (Some(number), Some(road)) = (number, road) else { return Vec::new() };
        let mut parts = vec![number, road];
        parts.extend(hit.tag("addr:unit").and_then(|u| part(u, Tag::Unit)));
        parts
    }

    /// The parts written as the country writes an address (see
    /// `address_format`), each word of it tagged by the part it came from,
    /// with whether each part starts a section: after a line or a comma,
    /// if `commas`. A unit, which the templates leave out, goes after the
    /// road and number. `None` if the words can't all be traced back to
    /// parts (see `unaligned`).
    fn formatted(&mut self, parts: &[Part], country: Option<&str>, commas: bool) -> Option<Vec<(Part, bool)>> {
        let component = |tag: Tag| match tag {
            Tag::Venue => Some("house"),
            Tag::HouseNumber => Some("house_number"),
            Tag::Road => Some("road"),
            Tag::Postcode => Some("postcode"),
            Tag::Neighbourhood => Some("suburb"),
            Tag::Locality => Some("city"),
            Tag::County => Some("county"),
            Tag::Region => Some("state"),
            Tag::Country => Some("country"),
            Tag::Unit | Tag::Near | Tag::Other => None,
        };
        let mut components = BTreeMap::new();
        for p in parts {
            if let Some(key) = component(p.tag) {
                // A country by its code in lower case, which the templates'
                // rules don't spell out (`US` → `United States of America`).
                let text = if p.tag == Tag::Country && p.text.chars().count() <= 3 { p.text.to_lowercase() } else { p.text.clone() };
                // Commas within a value would split it across sections.
                let text = text.replace(',', " ");
                components.entry(key.to_string()).or_insert(text);
            }
        }
        if let Some(country) = country {
            components.insert("country_code".to_string(), country.to_lowercase());
        }
        let codes = self.rng.random_bool(odds::ABBREVIATION);
        let (text, components) = crate::address_format::Formatter::get().format_with_components(&components, Some(language(country)), codes);

        // Each component's words as the template had them, in the parts'
        // order, to find in the text.
        let tag_of = |key: &str| match key {
            "house" => Some(Tag::Venue),
            "house_number" => Some(Tag::HouseNumber),
            "road" => Some(Tag::Road),
            "postcode" => Some(Tag::Postcode),
            "suburb" | "neighbourhood" => Some(Tag::Neighbourhood),
            "city" => Some(Tag::Locality),
            "county" | "county_code" => Some(Tag::County),
            "state" | "state_code" => Some(Tag::Region),
            "country" => Some(Tag::Country),
            _ => None,
        };
        let order = |tag: Tag| parts.iter().position(|p| p.tag == tag).unwrap_or(usize::MAX);
        let mut pending: Vec<(Tag, Vec<String>)> = components
            .iter()
            .filter_map(|(key, value)| Some((tag_of(key)?, part(value, Tag::Other)?.words)))
            .collect();
        pending.sort_by_key(|(tag, _)| order(*tag));
        pending.dedup();
        let mut used = vec![false; pending.len()];

        let mut out: Vec<(Part, bool)> = Vec::new();
        let mut boundary = false;
        for line in text.lines() {
            for piece in line.split(',') {
                let Some(Part { words, .. }) = part(piece, Tag::Other) else { continue };
                let mut tags: Vec<Option<Tag>> = vec![None; words.len()];
                let mut i = 0;
                while i < words.len() {
                    let best = pending
                        .iter()
                        .enumerate()
                        .filter(|(k, (_, w))| !used[*k] && words[i..].starts_with(w))
                        .fold(None::<(usize, usize)>, |best, (k, (_, w))| match best {
                            Some((_, len)) if len >= w.len() => best,
                            _ => Some((k, w.len())),
                        });
                    match best {
                        Some((k, len)) => {
                            used[k] = true;
                            tags[i..i + len].fill(Some(pending[k].0));
                            i += len;
                        }
                        None => i += 1,
                    }
                }
                // Words a rule changed: those of the one component left that
                // has them, or the only one left.
                let mut start = 0;
                while start < words.len() {
                    if tags[start].is_some() {
                        start += 1;
                        continue;
                    }
                    let end = (start..words.len()).find(|&j| tags[j].is_some()).unwrap_or(words.len());
                    let run = &words[start..end];
                    let left: Vec<usize> = (0..pending.len()).filter(|k| !used[*k]).collect();
                    let k = left
                        .iter()
                        .copied()
                        .find(|k| pending[*k].1.windows(run.len()).any(|w| w == run))
                        .or_else(|| (left.len() == 1).then(|| left[0]));
                    // Else words a rule added to its neighbour's: `United
                    // States` written `United States of America`.
                    let neighbour = (start > 0).then(|| tags[start - 1]).flatten().or_else(|| tags.get(end).copied().flatten());
                    let tag = match (k, neighbour) {
                        (Some(k), _) => {
                            used[k] = true;
                            pending[k].0
                        }
                        (None, Some(tag)) if run.len() <= 3 => tag,
                        _ => {
                            if std::env::var_os("TAGGER_DEBUG_UNALIGNED").is_some() {
                                eprintln!("unaligned: {text:?} from {components:?}");
                            }
                            return None;
                        }
                    };
                    tags[start..end].fill(Some(tag));
                    start = end;
                }
                // Runs of one tag as parts.
                let mut first = true;
                let tagged: Vec<(Tag, &String)> = tags.iter().map(|t| t.expect("tagged")).zip(&words).collect();
                for run in tagged.chunk_by(|a, b| a.0 == b.0) {
                    let words: Vec<String> = run.iter().map(|(_, w)| (*w).clone()).collect();
                    let section = first && boundary && commas;
                    out.push((Part { text: words.join(" "), words, tag: run[0].0 }, section));
                    first = false;
                }
                boundary = true;
            }
        }
        if let Some(unit) = parts.iter().find(|p| p.tag == Tag::Unit) {
            let after = out.iter().rposition(|(p, _)| matches!(p.tag, Tag::Road | Tag::HouseNumber)).map_or(out.len(), |i| i + 1);
            out.insert(after, (unit.clone(), false));
        }
        Some(out)
    }

    /// A kind of place a POI is, as words someone might search for it by.
    fn category(hit: &SearchHit) -> Option<String> {
        let keys = ["cuisine", "amenity", "shop", "tourism", "leisure", "craft"];
        keys.iter().find_map(|key| {
            let value = hit.tag(key)?.split(';').next()?.trim();
            (!value.is_empty() && !matches!(value, "yes" | "no")).then(|| value.replace('_', " "))
        })
    }

    /// Examples of a place.
    pub fn examples(&mut self, hit: &SearchHit, admins: &HashMap<u64, WofPlace>, focus_elsewhere: [f64; 2]) -> Vec<Example> {
        let country = hit.wof.as_ref().and_then(|w| w.country.clone()).or_else(|| {
            hit.admin_ids.iter().filter_map(|id| admins.get(id)).find_map(|a| a.country.clone())
        });
        let country = country.as_deref();
        let mut subject: Vec<Part> = Vec::new();
        // Placed with the areas, as the country writes it (see
        // `place_postcode`).
        let mut postcode: Option<Part> = None;
        match hit.kind {
            Kind::Poi => {
                let english = self.rng.random_bool(0.2);
                let name = hit.tag(if english { "name:en" } else { "name" }).or_else(|| hit.tag("name"));
                let category = Self::category(hit).filter(|_| self.rng.random_bool(odds::CATEGORY));
                subject.extend(category.or(name.map(str::to_string)).and_then(|n| part(&n, Tag::Venue)));
                if self.rng.random_bool(odds::POI_ADDRESS) {
                    subject.extend(Self::address(hit));
                } else if self.rng.random_bool(odds::POI_POSTCODE) {
                    postcode = hit.tag("addr:postcode").and_then(|p| part(p, Tag::Postcode));
                }
            }
            Kind::Address => {
                subject = Self::address(hit);
                if self.rng.random_bool(odds::POSTCODE) {
                    postcode = hit.tag("addr:postcode").and_then(|p| part(p, Tag::Postcode));
                }
            }
            Kind::Street => subject.extend(hit.tag("name").and_then(|n| part(n, Tag::Road))),
            Kind::Postcode => subject.extend(hit.tag("postal_code").and_then(|p| part(p, Tag::Postcode))),
            Kind::Admin => {
                let wof = hit.wof.as_ref();
                let tag = match wof.and_then(|w| AdminBand::of(&w.placetype)) {
                    Some(AdminBand::Neighbourhood) => Tag::Neighbourhood,
                    Some(AdminBand::Locality) => Tag::Locality,
                    Some(AdminBand::County) => Tag::County,
                    Some(AdminBand::Region) => Tag::Region,
                    Some(AdminBand::Country) => Tag::Country,
                    None => return Vec::new(),
                };
                if let Some(name) = wof.and_then(|w| self.area_name(w)) {
                    subject.extend(part(&name, tag));
                }
            }
        }
        if subject.is_empty() {
            return Vec::new();
        }
        let mut areas = self.areas(hit, admins);
        if hit.kind == Kind::Admin {
            // An area's own band isn't its context.
            areas.retain(|a| a.tag != subject[0].tag);
        }
        // Nor is an area named as it is: Blakeslee Junction in Blakeslee
        // Junction.
        areas.retain(|a| subject.iter().all(|s| s.words != a.words));

        let commas = self.rng.random_bool(odds::COMMAS);
        // Addresses, streets, postcodes and areas as the country writes
        // them; venues, which the templates don't do, as people search.
        let venue = subject[0].tag == Tag::Venue && !subject.iter().any(|p| p.tag == Tag::HouseNumber);
        let parts = if venue {
            self.venue_query(subject, areas, postcode, country, commas)
        } else {
            let all: Vec<Part> = subject.into_iter().chain(postcode).chain(areas).collect();
            match self.formatted(&all, country, commas) {
                Some(parts) => parts,
                None => {
                    UNALIGNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Vec::new();
                }
            }
        };

        let mut query = Query::default();
        let mut tags = Vec::new();
        for (part, comma) in parts {
            let comma = comma || self.rng.random_bool(odds::STRAY_COMMA);
            let words: Vec<String> = match part.tag {
                Tag::Road => part.words.iter().map(|w| self.road_forms.vary(w, &mut self.rng)).collect(),
                Tag::HouseNumber | Tag::Postcode | Tag::Unit | Tag::Near => part.words,
                _ => part.words.iter().map(|w| self.name_forms.vary(w, &mut self.rng)).collect(),
            };
            for (i, word) in words.into_iter().enumerate() {
                let word = if word.chars().count() >= 5 && self.rng.random_bool(odds::TYPO) { typo(&word, &mut self.rng) } else { word };
                query.section_starts.push(comma && i == 0 && !query.words.is_empty());
                query.words.push(word);
                tags.push(part.tag);
            }
        }
        // Cut off as if still being typed.
        if self.rng.random_bool(odds::TYPING) {
            query.typing = true;
            if let Some(last) = query.words.last_mut() {
                let chars: Vec<char> = last.chars().collect();
                if chars.len() > 1 {
                    let keep = self.rng.random_range(1..=chars.len());
                    *last = chars[..keep].iter().collect();
                }
            }
        }
        let center = [hit.center.x(), hit.center.y()];
        let roll: f64 = self.rng.random();
        query.focus = if roll < odds::FOCUS_NEAR {
            // Within about 20 km.
            Some([center[0] + self.rng.random_range(-0.2..0.2), center[1] + self.rng.random_range(-0.15..0.15)])
        } else if roll < odds::FOCUS_NEAR + odds::FOCUS_FAR {
            Some(focus_elsewhere)
        } else {
            None
        };
        vec![Example { query, tags }]
    }
}

/// A word with one random edit: a deletion, a swap, a replacement or an
/// insertion.
fn typo(word: &str, rng: &mut StdRng) -> String {
    let mut chars: Vec<char> = word.chars().collect();
    let i = rng.random_range(1..chars.len());
    match rng.random_range(0..4) {
        0 => {
            chars.remove(i);
        }
        1 => chars.swap(i - 1, i),
        2 => chars[i] = chars[i - 1],
        _ => chars.insert(i, chars[i]),
    }
    chars.into_iter().collect()
}

/// How many times more examples to make of a kind of document than of
/// others: an index has few postcodes, and people often search for one
/// alone.
#[must_use]
pub fn copies(kind: Kind) -> usize {
    match kind {
        Kind::Postcode => 5,
        _ => 1,
    }
}

/// How many times more sparsely a document is sampled than POIs: an index
/// has an address for most buildings, which would otherwise swamp the
/// places people search for by name. (Areas are sampled apart: see
/// `AREA_SHARE`.) Transit stops are sampled sparsely too: they're mostly
/// named for the streets they're on ("13th Ave & E Pike St"), which would
/// teach that street names are venues, and seldom searched for. Stations are
/// kept.
fn sparsity(hit: &SearchHit) -> usize {
    match hit.kind {
        Kind::Address => 4,
        Kind::Poi if is_transit_stop(hit) => 20,
        Kind::Poi | Kind::Street | Kind::Postcode | Kind::Admin => 1,
    }
}

fn is_transit_stop(hit: &SearchHit) -> bool {
    hit.tag("highway") == Some("bus_stop")
        || matches!(hit.tag("public_transport"), Some("platform" | "stop_position"))
        || matches!(hit.tag("railway"), Some("tram_stop" | "platform"))
}

/// Areas, as a share of the other documents sampled: an index has every
/// area in the world, which taken by stride would be most of a regional
/// index's sample and hardly any of the planet's.
const AREA_SHARE: f64 = 0.25;

/// Every `stride`th document of the index but its areas, from `offset`, more
/// sparsely for some kinds (see `sparsity`); areas by importance (see `area_weight`), a
/// share of the rest (see `AREA_SHARE`); and the admins of them all, by ID:
/// one table, as most documents share their admins.
///
/// # Errors
/// If the index can't be read.
pub fn sample(index: &AirmailIndex, stride: u32, offset: u32) -> anyhow::Result<(Vec<SearchHit>, HashMap<u64, WofPlace>)> {
    let searcher = index.searcher()?;
    let segments = searcher.segment_readers();
    // Per segment, in parallel: its sampled non-admins and its admins.
    let per_segment = in_parallel(segments.len(), |ord| -> anyhow::Result<(Vec<SearchHit>, Vec<(DocAddress, f64)>)> {
        let segment = &segments[ord];
        let mut seen: HashMap<(Kind, usize), usize> = HashMap::new();
        let (mut sampled, mut areas) = (Vec::new(), Vec::new());
        let alive = segment.alive_bitset();
        let is_admin = segment.fast_fields().u64(crate::index::FIELD_IS_ADMIN)?;
        let importance = segment.fast_fields().f64(crate::index::FIELD_IMPORTANCE)?;
        for doc in 0..segment.max_doc() {
            if !alive.is_none_or(|bits| bits.is_alive(doc)) {
                continue;
            }
            if is_admin.first(doc) == Some(1) {
                areas.push((DocAddress::new(ord as u32, doc), area_weight(importance.first(doc).unwrap_or(0.0))));
                continue;
            }
            if doc % stride != offset % stride {
                continue;
            }
            let hit = index.hit(&searcher, DocAddress::new(ord as u32, doc), 0.0)?;
            let every = sparsity(&hit);
            let count = seen.entry((hit.kind, every)).or_default();
            *count += 1;
            if *count % every == 0 {
                sampled.push(without_shape(hit));
            }
        }
        Ok((sampled, areas))
    });
    let (mut sampled, mut areas) = (Vec::new(), Vec::new());
    for part in per_segment {
        let (hits, admins) = part?;
        sampled.extend(hits);
        areas.extend(admins);
    }
    // Systematic sampling by weight: an area is chosen each time the running
    // total of weights passes a multiple of the step, so a big city can be
    // chosen several times, each making different queries.
    let wanted = ((sampled.len() as f64 * AREA_SHARE) as usize).max(1);
    let weight = areas.iter().map(|(_, w)| w).sum::<f64>();
    let big = areas.iter().map(|(_, w)| w).filter(|w| **w >= 100.0).sum::<f64>();
    eprintln!("{} areas, {:.0}% of their samples of places of 100,000 or more", areas.len(), 100.0 * big / weight.max(1.0));
    let step = weight / wanted as f64;
    let mut next = step * ((f64::from(offset) * 0.618_034) % 1.0);
    let (mut total, mut chosen) = (0.0, Vec::new());
    for (address, weight) in areas {
        total += weight;
        while next < total {
            chosen.push(address);
            next += step;
        }
    }
    const PER_JOB: usize = 10_000;
    for hits in in_parallel(chosen.len().div_ceil(PER_JOB), |job| {
        chosen[job * PER_JOB..((job + 1) * PER_JOB).min(chosen.len())]
            .iter()
            .map(|address| index.hit(&searcher, *address, 0.0))
            .map(|hit| hit.map(without_shape))
            .collect::<anyhow::Result<Vec<_>>>()
    }) {
        sampled.extend(hits?);
    }
    // Their admins, looked up in parallel.
    let ids: Vec<u64> = sampled.iter().flat_map(|h| h.admin_ids.iter().copied()).collect::<BTreeSet<_>>().into_iter().collect();
    let mut admins = HashMap::new();
    for found in in_parallel(ids.len().div_ceil(PER_JOB), |job| {
        let ids: BTreeSet<u64> = ids[job * PER_JOB..((job + 1) * PER_JOB).min(ids.len())].iter().copied().collect();
        index.admins(&searcher, &ids)
    }) {
        admins.extend(found?);
    }
    Ok((sampled, admins))
}

/// How much an area is sampled, from its importance (see
/// `importance::admin_importance`, a log of its population): in proportion
/// to its population in thousands, as libpostal samples places, so a name's
/// examples mostly label it as its biggest place. At least 1, so every area
/// is seen, and at most 1000 (a city of about a million), so the biggest
/// don't crowd out the rest.
fn area_weight(importance: f64) -> f64 {
    let thousands_decades = (importance - 0.1) / 0.14;
    10f64.powf(thousands_decades).clamp(1.0, 1000.0)
}

/// Queries don't need shapes, which would take most of the memory.
fn without_shape(mut hit: SearchHit) -> SearchHit {
    hit.geometry = geo::Geometry::Point(hit.center);
    hit
}

/// `run(job)` for each of `jobs` jobs, on as many threads as there are
/// cores, with the results in job order.
pub fn in_parallel<R: Send>(jobs: usize, run: impl Fn(usize) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(jobs.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let job = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if job >= jobs {
                            return done;
                        }
                        done.push((job, run(job)));
                    }
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("a job panicked")).collect()
    });
    done.sort_by_key(|(job, _)| *job);
    done.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn typos_make_one_edit() {
        let mut rng = StdRng::seed_from_u64(1);
        for _ in 0..50 {
            let typo = typo("seattle", &mut rng);
            assert_ne!(typo, "seattle".repeat(2));
            assert!(typo.chars().count().abs_diff(7) <= 1);
        }
    }

    fn formatted(country: &str, parts: &[(&str, Tag)]) -> BTreeSet<String> {
        let mut generator = Generator::new(1);
        let parts: Vec<Part> = parts.iter().map(|(text, tag)| part(text, *tag).unwrap()).collect();
        (0..50)
            .map(|_| {
                let out = generator.formatted(&parts, Some(country), true).unwrap();
                out.iter()
                    .map(|(p, section)| format!("{}{}/{}", if *section { ", " } else { " " }, p.words.join(" "), p.tag.as_str()))
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn addresses_are_written_as_their_countries_write_them() {
        let us = formatted(
            "US",
            &[("1530", Tag::HouseNumber), ("Post Alley", Tag::Road), ("98101", Tag::Postcode), ("Seattle", Tag::Locality), ("Washington", Tag::Region)],
        );
        assert_eq!(
            us.into_iter().collect::<Vec<_>>(),
            [
                " 1530/house_number post alley/road, seattle/locality, wa/region 98101/postcode",
                " 1530/house_number post alley/road, seattle/locality, washington/region 98101/postcode",
            ]
        );
        let ch = formatted("CH", &[("1", Tag::HouseNumber), ("Bahnhofstrasse", Tag::Road), ("8001", Tag::Postcode), ("Zürich", Tag::Locality)]);
        assert_eq!(ch.into_iter().collect::<Vec<_>>(), [" bahnhofstrasse/road 1/house_number, 8001/postcode zurich/locality"]);
        // A unit after the road and number, which the templates leave out.
        let unit = formatted("US", &[("500", Tag::HouseNumber), ("Pine St", Tag::Road), ("2B", Tag::Unit), ("Seattle", Tag::Locality)]);
        assert_eq!(unit.into_iter().collect::<Vec<_>>(), [" 500/house_number pine st/road 2b/unit, seattle/locality"]);
    }

    #[test]
    fn connectors_and_street_types_come_from_the_dictionaries() {
        let mut rng = StdRng::seed_from_u64(1);
        let connectors = Connectors::new();
        assert!(connectors.pick("en", &mut rng).is_some());
        assert!(connectors.pick("xx", &mut rng).is_some());
        assert!(connectors.before_place["en"].contains(&"near".to_string()));
        assert!(!connectors.before_place["en"].contains(&"near me".to_string()));
        assert!(connectors.standalone["en"].contains(&"near me".to_string()));
        assert!(connectors.standalone["fr"].contains(&"près d'ici".to_string()));
        assert!(connectors.before_place["fr"].contains(&"près de".to_string()));
        let forms = Forms::new(&["street_types", "directionals", "personal_titles", "synonyms"]);
        assert!(forms.0["street"].contains(&"st".to_string()));
        assert!(forms.0["north"].contains(&"n".to_string()));
        assert!(forms.0["saint"].contains(&"st".to_string()));
        assert!(forms.0["mount"].contains(&"mt".to_string()));
    }
}
