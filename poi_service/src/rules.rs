//! Which OSM elements are POIs, and whether two versions of one element are
//! the same place.
//!
//! These rules decide when a POI gets a new identifier, so every deployment
//! must run exactly the same ones. Any change to their behaviour, including
//! upgrading the Unicode data they depend on, needs a new [`RULESET_VERSION`].
//! They mirror `poi_service/stable_ids/src/stable_ids/poi_tags.py`, which
//! measures them against real OSM edits.

use std::collections::{BTreeMap, HashMap, HashSet};

use icu_casemap::CaseMapper;
use icu_normalizer::DecomposingNormalizerBorrowed;
use icu_properties::props::{CanonicalCombiningClass, GeneralCategory, GeneralCategoryGroup};
use icu_properties::CodePointMapData;

/// Identifies the rules below. Deployments with different rulesets assign
/// different identifiers, so the database records which one it was built with.
pub const RULESET_VERSION: i32 = 2;

pub type Tags = BTreeMap<String, String>;

/// Keys whose presence makes a named element a POI: every key listed at
/// <https://wiki.openstreetmap.org/wiki/Top-level_tag> (fetched 2026-10-30)
/// except
/// - transportation infrastructure (`highway`, `railway`, `aeroway`,
///   `aerialway`, `route`, `junction`, `ford`, `noexit`, `traffic_calming`,
///   `traffic_sign`, `departures_board`, `area:highway`, `bridge:support`),
///   though `public_transport` stays, since stations and stops are places
///   people save;
/// - `addr:interpolation`, which describes address ranges rather than places;
/// - `type`, which on relations only says how to read the relation.
///
/// Must match `POI_KEYS` in `stable_ids/poi_tags.py`.
const POI_KEYS: [&str; 36] = [
    "advertising", "allotments", "amenity", "attraction", "barrier", "boundary", "building",
    "building:part", "cemetery", "club", "craft", "education", "emergency", "entrance", "geological",
    "golf", "healthcare", "historic", "indoor", "landcover", "landuse", "leisure", "man_made",
    "military", "natural", "office", "piste:type", "place", "playground", "power", "public_transport",
    "roller_coaster", "shop", "telecom", "tourism", "waterway",
];

/// Other names a place goes by. `old_name` is deliberately absent: mappers set
/// it to the previous business's name when a new one moves in.
const ALTERNATIVE_NAME_KEYS: [&str; 5] = ["short_name", "official_name", "alt_name", "loc_name", "int_name"];

/// How alike two names' characters must be to count as a spelling change.
const NAME_CHAR_SIMILARITY: f64 = 0.8;

/// Tags that make a named element a POI though their key doesn't, such as bus
/// stops mapped only as `highway=bus_stop` and trailheads. `ford` isn't here:
/// it's also a tag on ways, which would make any road with a ford a POI. Must match `POI_TAGS` in
/// `stable_ids/poi_tags.py`.
const POI_TAGS: [(&str, &str); 5] = [
    ("highway", "bus_stop"),
    ("highway", "emergency_access_point"),
    ("highway", "rest_area"),
    ("highway", "trailhead"),
    ("railway", "subway_entrance"),
];

/// POIs that, without a name or brand, are named by their operator (or the
/// other keys listed), as the planetiler fork labels them. Must match
/// `OPERATOR_NAMED` in `stable_ids/poi_tags.py`.
const OPERATOR_NAMED: [((&str, &str), &[&str]); 3] = [
    (("amenity", "atm"), &["operator", "network"]),
    (("amenity", "charging_station"), &["operator"]),
    (("amenity", "parcel_locker"), &["operator"]),
];

/// Every key in [`OPERATOR_NAMED`] that can name a POI.
const OPERATOR_NAME_KEYS: [&str; 2] = ["operator", "network"];

/// True if these tags describe a named POI.
pub fn is_poi<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> bool {
    let (mut named, mut poi) = (false, false);
    let mut operator_named = None;
    let mut operator_keys = [false; OPERATOR_NAME_KEYS.len()];
    for (key, value) in tags {
        named |= key == "name" || key == "brand";
        poi |= is_poi_tag(key, value);
        if let Some(i) = OPERATOR_NAMED.iter().position(|&(tag, _)| tag == (key, value)) {
            operator_named = Some(i);
        }
        if let Some(i) = OPERATOR_NAME_KEYS.iter().position(|&k| k == key) {
            operator_keys[i] = true;
        }
    }
    named |= operator_named.is_some_and(|i| {
        OPERATOR_NAMED[i].1.iter().any(|key| operator_keys[OPERATOR_NAME_KEYS.iter().position(|k| k == key).unwrap()])
    });
    named && poi
}

/// True for keys that make a named element a POI whatever their value.
pub fn is_poi_key(key: &str) -> bool {
    POI_KEYS.binary_search(&key).is_ok()
}

/// True for keys that make a named element a POI only with some values.
pub fn is_poi_value_key(key: &str) -> bool {
    POI_TAGS.iter().any(|&(k, _)| k == key)
}

/// True for tags that make a named element a POI.
fn is_poi_tag(key: &str, value: &str) -> bool {
    is_poi_key(key) || POI_TAGS.contains(&(key, value))
}

/// `name` casefolded, without accents or punctuation.
pub fn normalize_name(name: &str) -> String {
    let decomposed = DecomposingNormalizerBorrowed::new_nfkd().normalize(name);
    let combining = CodePointMapData::<CanonicalCombiningClass>::new();
    let category = CodePointMapData::<GeneralCategory>::new();
    let alphanumeric = GeneralCategoryGroup::Letter.union(GeneralCategoryGroup::Number);
    let stripped: String = decomposed
        .chars()
        .filter(|&c| combining.get(c) == CanonicalCombiningClass::NotReordered)
        .map(|c| if alphanumeric.contains(category.get(c)) { c } else { ' ' })
        .collect();
    let folded = CaseMapper::new().fold_string(&stripped);
    folded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The name (or brand) of `tags`, normalized.
fn normalized_name(tags: &Tags) -> String {
    let name = tags.get("name").or_else(|| tags.get("brand")).map(String::as_str).or_else(|| operator_name(tags));
    normalize_name(name.unwrap_or(""))
}

/// The name an operator-named POI goes by without a name or brand.
fn operator_name(tags: &Tags) -> Option<&str> {
    let (_, keys) = OPERATOR_NAMED.iter().find(|&&((k, v), _)| tags.get(k).is_some_and(|value| value == v))?;
    keys.iter().find_map(|&key| tags.get(key)).map(String::as_str)
}

/// Normalized translations, transliterations and other names of a place.
fn alternative_names(tags: &Tags) -> HashSet<String> {
    tags.iter()
        .filter(|(k, _)| k.starts_with("name:") || ALTERNATIVE_NAME_KEYS.contains(&k.as_str()))
        .flat_map(|(_, v)| v.split(';'))
        .map(normalize_name)
        .filter(|name| !name.is_empty())
        .collect()
}

/// True if normalized name `nb` could be an edit of `na`.
///
/// Accepts spelling fixes ("starbuks" / "starbucks"), spacing ("7 eleven" /
/// "7eleven"), added or removed words ("cvs" / "cvs pharmacy"), and
/// mostly-shared words ("joe s pizza bar" / "joe s pizza grill"). Names
/// sharing only a generic word ("cafe milano" / "cafe deluxe") are not
/// similar.
fn similar_strings(na: &str, nb: &str) -> bool {
    let compact_a: Vec<char> = na.chars().filter(|&c| c != ' ').collect();
    let compact_b: Vec<char> = nb.chars().filter(|&c| c != ' ').collect();
    if contains(&compact_a, &compact_b) || contains(&compact_b, &compact_a) {
        return true;
    }
    let words_a: HashSet<&str> = na.split(' ').filter(|w| !w.is_empty()).collect();
    let words_b: HashSet<&str> = nb.split(' ').filter(|w| !w.is_empty()).collect();
    if words_a.intersection(&words_b).count() * 2 >= words_a.union(&words_b).count() {
        return true;
    }
    sequence_ratio(&compact_a, &compact_b) >= NAME_CHAR_SIMILARITY
}

fn contains(haystack: &[char], needle: &[char]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// True if `b`'s name could be an edit of `a`'s rather than another place's.
///
/// Besides `similar_strings` on the two names, a name also matches the other
/// version's translations and alternative names, so switching the name to
/// another language or script ("Asadaya Ryokan" / "浅田屋旅館") counts.
fn names_similar(a: &Tags, b: &Tags) -> bool {
    let (na, nb) = (normalized_name(a), normalized_name(b));
    similar_strings(&na, &nb)
        || alternative_names(b).iter().any(|alt| similar_strings(&na, alt))
        || alternative_names(a).iter().any(|alt| similar_strings(alt, &nb))
}

/// True if both versions have a brand and it switched to another one.
///
/// Matching `brand:wikidata` means the same brand however it's written
/// ("セブン-イレブン" / "7-ELEVEN"); otherwise the brands must be dissimilar
/// ("OMV" / "MOL", not "Total" / "TotalEnergies").
fn brand_changed(a: &Tags, b: &Tags) -> bool {
    let non_empty = |tags: &Tags, key: &str| tags.get(key).filter(|v| !v.is_empty()).cloned();
    let (Some(brand_a), Some(brand_b)) = (non_empty(a, "brand"), non_empty(b, "brand")) else {
        return false;
    };
    if let (Some(qid_a), Some(qid_b)) = (non_empty(a, "brand:wikidata"), non_empty(b, "brand:wikidata"))
        && qid_a == qid_b
    {
        return false;
    }
    !similar_strings(&normalize_name(&brand_a), &normalize_name(&brand_b))
}

/// True if two POI versions of one element describe the same place.
///
/// A brand change only counts when the name changed too; with the name kept,
/// it's nearly always a fix to a wrongly tagged brand.
///
/// A POI known only by its operator (see [`OPERATOR_NAMED`]) is the same
/// place as long as its operator is, so adding or removing a name, such as
/// "Ladesäule Kaiserdamm" on a charging station operated by Allego, doesn't
/// change its identity.
pub fn same_place(a: &Tags, b: &Tags) -> bool {
    let has_name = |tags: &Tags| tags.contains_key("name") || tags.contains_key("brand");
    if (!has_name(a) || !has_name(b))
        && let (Some(oa), Some(ob)) = (operator_name(a), operator_name(b))
        && similar_strings(&normalize_name(oa), &normalize_name(ob))
    {
        return true;
    }
    if !names_similar(a, b) {
        return false;
    }
    normalized_name(a) == normalized_name(b) || !brand_changed(a, b)
}

/// `difflib.SequenceMatcher(None, a, b).ratio()` from Python, including its
/// "autojunk" heuristic, so that names compare exactly as in the Python
/// tooling.
fn sequence_ratio(a: &[char], b: &[char]) -> f64 {
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    if b.len() >= 200 {
        let popular = b.len() / 100 + 1;
        b2j.retain(|_, js| js.len() <= popular);
    }
    let mut matched = 0;
    let mut queue = vec![(0, a.len(), 0, b.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest_match(a, b, &b2j, alo, ahi, blo, bhi);
        if k > 0 {
            matched += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    2.0 * matched as f64 / total as f64
}

/// `SequenceMatcher.find_longest_match` with no junk function.
#[allow(clippy::needless_range_loop)] // Kept index-based to mirror difflib.
fn longest_match(
    a: &[char],
    b: &[char],
    b2j: &HashMap<char, Vec<usize>>,
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> (usize, usize, usize) {
    let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
    let mut j2len: HashMap<usize, usize> = HashMap::new();
    for i in alo..ahi {
        let mut new_j2len = HashMap::new();
        for &j in b2j.get(&a[i]).map_or(&[][..], Vec::as_slice) {
            if j < blo {
                continue;
            }
            if j >= bhi {
                break;
            }
            let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
            new_j2len.insert(j, k);
            if k > bestsize {
                (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
            }
        }
        j2len = new_j2len;
    }
    // Extend over characters that autojunk dropped from `b2j`.
    while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
        (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
    }
    while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
        bestsize += 1;
    }
    (besti, bestj, bestsize)
}

/// Builds tags from key-value pairs.
#[cfg(test)]
pub fn tags<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Tags {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> Tags {
        tags([("amenity", "cafe"), ("name", name)])
    }

    #[test]
    fn normalizes_accents_punctuation_and_case() {
        assert_eq!(normalize_name("  Café  Straße-Ecke! "), "cafe strasse ecke");
        assert_eq!(normalize_name("7-ELEVEN"), "7 eleven");
    }

    #[test]
    fn similar_names_are_the_same_place() {
        assert!(same_place(&named("Starbuks"), &named("Starbucks")));
        assert!(same_place(&named("7-Eleven"), &named("7Eleven")));
        assert!(same_place(&named("CVS"), &named("CVS Pharmacy")));
        assert!(same_place(&named("Joe's Pizza Bar"), &named("Joe's Pizza Grill")));
    }

    #[test]
    fn unrelated_names_are_different_places() {
        assert!(!same_place(&named("Cafe Milano"), &named("Cafe Deluxe")));
        assert!(!same_place(&named("OXO"), &named("POP Bubble Tea")));
    }

    #[test]
    fn translations_count_as_the_same_name() {
        let a = tags([("tourism", "hotel"), ("name", "Asadaya Ryokan")]);
        let b = tags([("tourism", "hotel"), ("name", "浅田屋旅館"), ("name:en", "Asadaya Ryokan")]);
        assert!(same_place(&a, &b));
    }

    #[test]
    fn brand_switch_with_renamed_place_is_different() {
        let a = tags([("amenity", "fuel"), ("name", "OMV Šmarje-Sap"), ("brand", "OMV")]);
        let b = tags([("amenity", "fuel"), ("name", "MOL Šmarje-Sap"), ("brand", "MOL")]);
        assert!(!same_place(&a, &b));
        // With the name kept, a brand change is a tagging fix.
        let c = tags([("amenity", "fuel"), ("name", "OMV Šmarje-Sap"), ("brand", "MOL")]);
        assert!(same_place(&a, &c));
    }

    #[test]
    fn operator_named_pois_are_the_same_place_while_the_operator_is() {
        let charger = |pairs: &[(&'static str, &'static str)]| {
            tags([("amenity", "charging_station")].into_iter().chain(pairs.iter().copied()))
        };
        let allego = charger(&[("operator", "Allego")]);
        assert!(same_place(&allego, &charger(&[("operator", "Allego GmbH")])));
        // Naming it keeps its identity, even with a name unlike the operator.
        let named = charger(&[("operator", "Allego"), ("name", "Ladesäule Kaiserdamm")]);
        assert!(same_place(&allego, &named));
        assert!(same_place(&named, &allego));
        assert!(!same_place(&allego, &charger(&[("operator", "Ionity")])));
        assert!(!same_place(&named, &charger(&[("operator", "Ionity"), ("name", "Ionity Kaiserdamm")])));
    }

    #[test]
    fn poi_keys_are_sorted() {
        assert!(POI_KEYS.is_sorted());
    }

    #[test]
    fn poi_needs_name_and_poi_key() {
        let named = |key, value| is_poi([("name", "X"), (key, value)]);
        assert!(named("shop", "bakery"));
        assert!(is_poi([("brand", "X"), ("amenity", "cafe")]));
        assert!(named("natural", "peak"));
        assert!(named("public_transport", "platform"));
        assert!(named("highway", "bus_stop"));
        assert!(named("highway", "trailhead"));
        assert!(named("highway", "emergency_access_point"));
        assert!(named("highway", "rest_area"));
        assert!(!named("ford", "yes"));
        assert!(!named("highway", "residential"));
        assert!(!named("disused:shop", "bakery"));
        assert!(!named("addr:interpolation", "even"));
        assert!(!named("type", "multipolygon"));
        assert!(!is_poi([("shop", "bakery")]));
        assert!(named("railway", "subway_entrance"));
        assert!(!named("railway", "rail"));
    }

    #[test]
    fn some_pois_are_named_by_operator() {
        assert!(is_poi([("amenity", "charging_station"), ("operator", "Allego")]));
        assert!(is_poi([("amenity", "parcel_locker"), ("operator", "DHL")]));
        assert!(is_poi([("network", "Cash Group"), ("amenity", "atm")]));
        assert!(!is_poi([("amenity", "charging_station"), ("network", "X")]));
        assert!(!is_poi([("amenity", "waste_basket"), ("operator", "BSR")]));
        assert!(!is_poi([("power", "substation"), ("operator", "Stromnetz Berlin")]));
    }

    #[test]
    fn sequence_ratio_matches_python() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        // difflib.SequenceMatcher(None, "abcd", "bcde").ratio() == 0.75
        assert_eq!(sequence_ratio(&chars("abcd"), &chars("bcde")), 0.75);
        // difflib.SequenceMatcher(None, "starbucks", "starbuks").ratio()
        assert_eq!(sequence_ratio(&chars("starbucks"), &chars("starbuks")), 16.0 / 17.0);
        assert_eq!(sequence_ratio(&[], &[]), 1.0);
    }

    /// Compares against labels from the Python tooling. Generate a JSON Lines
    /// file of `{"a": tags, "b": tags, "same": bool}` with
    /// `stable_ids.poi_tags.same_place_label`, then run
    /// `RULES_PARITY_FILE=pairs.jsonl cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn matches_python_labels() {
        let path = std::env::var("RULES_PARITY_FILE").expect("RULES_PARITY_FILE");
        let (mut total, mut mismatches) = (0, 0);
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let pair: serde_json::Value = serde_json::from_str(line).unwrap();
            let a: Tags = serde_json::from_value(pair["a"].clone()).unwrap();
            let b: Tags = serde_json::from_value(pair["b"].clone()).unwrap();
            total += 1;
            if same_place(&a, &b) != pair["same"].as_bool().unwrap() {
                mismatches += 1;
                eprintln!("mismatch: {line}");
            }
        }
        assert_eq!(mismatches, 0, "{mismatches} of {total} pairs differ");
    }
}
