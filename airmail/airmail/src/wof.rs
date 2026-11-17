//! Who's On First places as documents: what the indexer reads from a
//! Who's On First database and a phone gets from the WOF service, and the
//! rules that make documents of them and give other places their areas.

use std::collections::{BTreeMap, BTreeSet};

use geo::{Geometry, Point};
use serde::{Deserialize, Serialize};

use crate::importance;
use crate::poi::{AdminBand, Kind, ToIndexPoi, WofPlace, is_containing_admin, iso_country_code};

/// Who's On First IDs of countries, whose spoken languages are the languages
/// of the places in them.
pub const COUNTRIES: [u64; 214] = [
    85632343, 85632573, 85632229, 85632529, 85632405, 85632773, 85632281, 85632715, 85632505,
    85632785, 85632793, 85632295, 85632717, 85632609, 85632491, 85632475, 85632997, 85632213,
    85633001, 85632167, 85632285, 85632247, 85632749, 85632623, 85633009, 85632339, 85632171,
    85632235, 85632395, 85632571, 85633041, 85632643, 85632391, 85632541, 85633051, 85632449,
    85633057, 85632245, 85632695, 85632519, 85632487, 85632675, 85632721, 85632441, 85632437,
    85633105, 85633111, 85632319, 85633121, 85632503, 85632713, 85632451, 85632261, 85633135,
    85632581, 85632217, 85632781, 85633129, 85632257, 85633143, 85632755, 85632431, 85633147,
    85632407, 85633159, 85632335, 85633163, 85632547, 85632189, 85633217, 85632603, 85632691,
    85632287, 85633171, 85632385, 85632757, 85632397, 85632483, 85632323, 85633229, 85632433,
    85633237, 85632203, 85633241, 85632315, 85632461, 85632469, 85632191, 85632361, 85633249,
    85633253, 85632593, 85632215, 85632425, 85632429, 85632329, 85632761, 85632359, 85632709,
    85632259, 85632551, 85632639, 85632231, 85632401, 85632307, 85632241, 85632533, 85632369,
    85633267, 85632313, 85632249, 85632173, 85633269, 85633275, 85633279, 85632627, 85632693,
    85633285, 85633287, 85632667, 85632223, 85632663, 85632373, 85632553, 85632181, 85632439,
    85632161, 85632679, 85633331, 85632357, 85632305, 85632383, 85633293, 85632739, 85632729,
    85632535, 85632269, 85632735, 85632599, 85633337, 85633341, 85632465, 85632747, 85633345,
    85632207, 85632179, 85632521, 85632347, 85632509, 85632659, 85633723, 85633739, 85633735,
    85632331, 85632355, 85632299, 85633745, 85633755, 85632685, 85632303, 85632253, 85632591,
    85632661, 85632751, 85633789, 85632605, 85633779, 85633769, 85632467, 85633763, 85632365,
    85632379, 85632443, 85632657, 85632765, 85632545, 85632185, 85632413, 85632635, 85632325,
    85632647, 85632293, 85632513, 85632583, 85632671, 85632703, 85632455, 85632393, 85632271,
    85632607, 85632403, 85632227, 85633805, 85632625, 102312305, 85633793, 85632511, 85632645,
    85632187, 85632569, 85632317, 85632763, 85632263, 85632681, 85633259, 1, 85632733, 1729945891,
    1729945893, 1729989201, 85632499, 85633813, 85632559, 85632243,
];

/// One of a place's names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WofName {
    /// ISO 639-3, or `und` for the default name.
    pub lang: String,
    /// `default`, `preferred`, `variant` or `colloquial`.
    pub tag: String,
    /// Whether it's an abbreviation.
    pub abbr: bool,
    pub name: String,
}

impl WofName {
    /// Whether the place is searched by this name. Colloquial names are
    /// nicknames, like Seattle's "Coffee Capital of the World", that would
    /// make every place there match their words.
    #[must_use]
    pub fn is_searchable(&self) -> bool {
        self.tag != "colloquial"
    }
}

/// What indexing needs of a Who's On First place, to index it or the places
/// in it.
#[derive(Debug, Clone, Default)]
pub struct WofRecord {
    pub id: u64,
    pub placetype: String,
    /// All its names, in every language.
    pub names: Vec<WofName>,
    /// The IDs of the places its hierarchy puts it in, in the database's
    /// order.
    pub ancestors: Vec<u64>,
    /// `wof:population`.
    pub population: Option<u64>,
    /// `wof:population_rank`.
    pub population_rank: Option<u32>,
    /// Its country code as Who's On First has it (`alpha2` or `wof:country`),
    /// unchecked.
    pub country: Option<String>,
    /// For countries, `wof:lang_x_spoken`'s ISO 639-3 codes.
    pub languages: Vec<String>,
    /// Its label point: the first of its `label_position`,
    /// `reversegeo_position` and `centroid`.
    pub label: Option<Point<f64>>,
    /// Its boundary, if it has one.
    pub boundary: Option<Geometry<f64>>,
}

impl WofRecord {
    /// How prominent it is, from its placetype and population.
    #[must_use]
    pub fn importance(&self) -> f64 {
        importance::admin_importance(&self.placetype, self.population, self.population_rank)
    }
}

/// The country among `ids`, the first in them that's one.
#[must_use]
pub fn country<'a>(ids: impl IntoIterator<Item = &'a u64>) -> Option<u64> {
    ids.into_iter().find(|id| COUNTRIES.contains(id)).copied()
}

/// The areas a place is in that count as containing it, of `areas` (each's
/// ID, placetype and names, in order), and the names it's found by in them:
/// every searchable name in `languages` (spoken where it is), English, and
/// the default names, by band.
#[must_use]
pub fn containing<'a>(areas: impl IntoIterator<Item = (u64, &'a str, &'a [WofName])>, languages: &[String]) -> (Vec<u64>, Vec<(AdminBand, String)>) {
    let mut ids = Vec::new();
    let mut names = BTreeSet::new();
    for (id, placetype, area_names) in areas {
        if !is_containing_admin(placetype) {
            continue;
        }
        ids.push(id);
        let Some(band) = AdminBand::of(placetype) else { continue };
        for name in area_names {
            if name.is_searchable() && (name.lang == "und" || name.lang == "eng" || languages.contains(&name.lang)) {
                names.insert((band, name.name.clone()));
            }
        }
    }
    (ids, names.into_iter().collect())
}

/// What results show of a place, from its names, or `None` if it has none.
#[must_use]
pub fn place(id: u64, placetype: &str, names: &[WofName], population: Option<u64>, country: Option<String>) -> Option<WofPlace> {
    let full = || names.iter().filter(|n| !n.abbr);
    let default_name = full()
        .find(|n| n.lang == "und" && n.tag == "default")
        .or_else(|| full().find(|n| n.lang == "eng" && n.tag == "preferred"))
        .or_else(|| full().next())?
        .name
        .clone();
    let mut preferred = BTreeMap::new();
    for name in full().filter(|n| n.tag == "preferred") {
        preferred.entry(name.lang.clone()).or_insert_with(|| name.name.clone());
    }
    let abbreviation = names
        .iter()
        .filter(|n| n.abbr)
        .min_by_key(|n| match n.lang.as_str() {
            "eng" => 0,
            "und" => 1,
            _ => 2,
        })
        .map(|n| n.name.clone());
    Some(WofPlace {
        id,
        placetype: placetype.to_string(),
        population,
        default_name,
        names: preferred,
        abbreviation,
        country,
        size: 0.0,
    })
}

/// A place as a document, or `None` if it has no name or no location.
/// `ancestor` gives the places in its hierarchy (`None` for one that isn't
/// known, which still counts as containing it), and `languages` are those
/// spoken in its country (see `country`).
pub fn admin_document<'a>(record: &WofRecord, ancestor: impl Fn(u64) -> Option<&'a WofRecord>, languages: &[String]) -> Option<ToIndexPoi> {
    place(record.id, &record.placetype, &record.names, None, None)?;
    // Its shape if it has one, else its label point.
    let shape = record.boundary.clone().or_else(|| record.label.map(Geometry::Point))?;

    // It's searched by the names of the places containing it in the
    // languages spoken there, English, and their default names.
    let ancestors: Vec<(u64, Option<&WofRecord>)> = record.ancestors.iter().map(|id| (*id, ancestor(*id))).collect();
    let (containing, admin_names) =
        containing(ancestors.iter().map(|(id, a)| (*id, a.map_or("", |a| a.placetype.as_str()), a.map_or(&[][..], |a| a.names.as_slice()))), languages);
    let city_importance = ancestors
        .iter()
        .filter_map(|(_, a)| *a)
        .filter(|a| a.placetype == "locality")
        .map(WofRecord::importance)
        .fold(0.0, f64::max);

    // Variant names (often historical) find a place, but aren't its name.
    let searchable = |variant: bool| -> BTreeSet<String> {
        record.names.iter().filter(|n| n.is_searchable() && (n.tag == "variant") == variant).map(|n| n.name.clone()).collect()
    };
    let mut own = searchable(false);
    let mut variants: Vec<String> = searchable(true).into_iter().filter(|n| !own.contains(n)).collect();
    if own.is_empty() {
        // A place known only by variants is known by them.
        own.extend(variants.drain(..));
    }
    let size = crate::geometry::bbox(&shape).map_or(0.0, |[west, south, east, north]| (east - west) * (north - south));
    let mut doc = ToIndexPoi::new(Kind::Admin, own.into_iter().collect(), shape, Vec::new())?;
    doc.variant_names = variants;
    if let Some(label) = record.label {
        doc.center = label;
    }
    doc.importance = record.importance();
    if importance::is_neighbourhood(&record.placetype) {
        doc.importance = importance::neighbourhood_importance(doc.importance, city_importance);
    }
    doc.admin_ids = containing;
    doc.admin_names = admin_names;
    let country = record.country.as_deref().and_then(iso_country_code);
    doc.country_code = country.clone();
    doc.wof = place(record.id, &record.placetype, &record.names, record.population, country).map(|place| WofPlace { size, ..place });
    Some(doc)
}

#[cfg(test)]
mod test {
    use super::*;

    fn name(lang: &str, tag: &str, abbr: bool, name: &str) -> WofName {
        WofName { lang: lang.into(), tag: tag.into(), abbr, name: name.into() }
    }

    #[test]
    fn place_picks_default_preferred_and_abbreviation() {
        let names = [
            name("und", "default", false, "Zürich"),
            name("und", "default", true, "ZH"),
            name("eng", "preferred", false, "Zurich"),
            name("eng", "variant", false, "ZRH"),
            name("deu", "preferred", false, "Zürich"),
        ];
        let place = place(85682309, "region", &names, Some(1_500_000), Some("CH".into())).unwrap();
        assert_eq!(place.default_name, "Zürich");
        assert_eq!(place.names.get("eng").map(String::as_str), Some("Zurich"));
        assert_eq!(place.abbreviation.as_deref(), Some("ZH"));
        assert!(!place.names.values().any(|n| n == "ZRH"));
    }

    #[test]
    fn place_needs_a_name() {
        assert!(place(1, "locality", &[], None, None).is_none());
    }

    #[test]
    fn admin_documents_are_found_by_their_areas_in_the_languages_spoken_there() {
        let switzerland = WofRecord {
            id: 85633051,
            placetype: "country".into(),
            names: vec![name("und", "default", false, "Schweiz"), name("fra", "preferred", false, "Suisse"), name("jpn", "preferred", false, "スイス")],
            languages: vec!["deu".into(), "fra".into()],
            ..WofRecord::default()
        };
        let zurich = WofRecord {
            id: 101748463,
            placetype: "locality".into(),
            names: vec![name("und", "default", false, "Zürich"), name("eng", "variant", false, "Turicum"), name("eng", "colloquial", false, "Zurich City")],
            ancestors: vec![85633051, 999],
            population: Some(400_000),
            country: Some("ch".into()),
            label: Some(Point::new(8.54, 47.37)),
            boundary: Some(Geometry::Polygon(geo::Rect::new(geo::coord! { x: 8.45, y: 47.32 }, geo::coord! { x: 8.63, y: 47.43 }).to_polygon())),
            ..WofRecord::default()
        };
        let lookup = |id: u64| (id == switzerland.id).then_some(&switzerland);
        let languages = &lookup(country(std::iter::once(&zurich.id).chain(&zurich.ancestors)).unwrap()).unwrap().languages;
        let doc = admin_document(&zurich, lookup, languages).unwrap();
        assert_eq!(doc.names, vec!["Zürich"]);
        assert_eq!(doc.variant_names, vec!["Turicum"]);
        assert_eq!(doc.center, Point::new(8.54, 47.37));
        // An ancestor that isn't known still contains it.
        assert_eq!(doc.admin_ids, vec![85633051, 999]);
        assert_eq!(doc.admin_names, vec![(AdminBand::Country, "Schweiz".to_string()), (AdminBand::Country, "Suisse".to_string())]);
        assert_eq!(doc.country_code.as_deref(), Some("CH"));
        let wof = doc.wof.unwrap();
        assert_eq!((wof.id, wof.default_name.as_str(), wof.population), (101748463, "Zürich", Some(400_000)));
        assert!(wof.size > 0.0);
        assert!(doc.importance > 0.0);
    }
}
