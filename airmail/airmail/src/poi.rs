//! What gets indexed and what searches return.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use geo::{Geometry, Point};
use lingua::Language;
use serde::{Deserialize, Serialize};

use crate::categories::iso_639_3;
use crate::geometry;
use crate::substitutions::permute_road;

/// What a result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A named OSM feature.
    Poi,
    /// An OSM address without a name.
    Address,
    /// A Who's On First administrative area.
    Admin,
    /// A named OSM road: the ways with its name near each other.
    Street,
    /// A postcode, where the OSM addresses with it are.
    Postcode,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Poi, Kind::Address, Kind::Admin, Kind::Street, Kind::Postcode];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Poi => "poi",
            Kind::Address => "address",
            Kind::Admin => "admin",
            Kind::Street => "street",
            Kind::Postcode => "postcode",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsmType {
    Node,
    Way,
    Relation,
}

impl OsmType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            OsmType::Node => "node",
            OsmType::Way => "way",
            OsmType::Relation => "relation",
        }
    }
}

/// The OSM element a document comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsmRef {
    pub osm_type: OsmType,
    pub id: i64,
    pub version: Option<u32>,
}

impl OsmRef {
    /// Stored as `type/id/version`, with an empty version if unknown.
    #[must_use]
    pub fn encode(&self) -> String {
        let version = self.version.map(|v| v.to_string()).unwrap_or_default();
        format!("{}/{}/{version}", self.osm_type.as_str(), self.id)
    }

    /// # Errors
    /// If `s` isn't an encoded reference.
    pub fn decode(s: &str) -> Result<Self> {
        let mut parts = s.split('/');
        let osm_type = match parts.next() {
            Some("node") => OsmType::Node,
            Some("way") => OsmType::Way,
            Some("relation") => OsmType::Relation,
            _ => bail!("invalid OSM reference {s:?}"),
        };
        let id = parts.next().unwrap_or_default().parse()?;
        let version = match parts.next() {
            None | Some("") => None,
            Some(v) => Some(v.parse()?),
        };
        Ok(Self { osm_type, id, version })
    }
}

/// A Who's On First place, as stored with its document and shown in other
/// results' admins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WofPlace {
    pub id: u64,
    pub placetype: String,
    pub population: Option<u64>,
    /// The canonical name (`und` `default`).
    pub default_name: String,
    /// The preferred name in each language, by ISO 639-3 code.
    pub names: BTreeMap<String, String>,
    pub abbreviation: Option<String>,
    /// The ISO 3166-1 alpha-2 code of the country it's in, or its own code
    /// if it's a country.
    #[serde(default)]
    pub country: Option<String>,
    /// The area of its bounding box in square degrees, zero if it has no shape.
    #[serde(default)]
    pub size: f64,
}

impl WofPlace {
    /// Whether this is the one to show over `other`, of the same placetype: see
    /// [`one_per_placetype`]. An area of no known size never beats a known one.
    fn beats(&self, other: &WofPlace) -> bool {
        if self.size == other.size || self.size == 0.0 {
            return false;
        }
        other.size == 0.0
            || match self.placetype.as_str() {
                "locality" | "localadmin" => self.size > other.size,
                _ => self.size < other.size,
            }
    }

    /// The name in `lang` (a BCP 47 tag) if there is one, else the default.
    #[must_use]
    pub fn display_name(&self, lang: Option<&str>) -> &str {
        lang.and_then(iso_639_3)
            .and_then(|code| self.names.get(code))
            .unwrap_or(&self.default_name)
    }
}

/// An ISO 3166-1 alpha-2 country code from Who's On First's `wof:country`
/// (`alpha2` in SpatiaLite databases), uppercased, or `None` for its codes
/// for unknown and disputed places (`XX`, `XY`, `XZ` and the other
/// user-assigned codes). Kosovo's `XK` is kept, since it's in common use.
#[must_use]
pub fn iso_country_code(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    let [first, second] = *code.as_bytes() else {
        return None;
    };
    if !first.is_ascii_uppercase() || !second.is_ascii_uppercase() {
        return None;
    }
    let user_assigned = matches!(code.as_str(), "AA" | "ZZ") || (first == b'Q' && second >= b'M') || first == b'X';
    (!user_assigned || code == "XK").then_some(code)
}

/// A result's ID, as the API gives it: `poi:<id>`, `wof:<id>`,
/// `osm:<type>/<id>` or `postcode:<country>/<postcode>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceId {
    Poi(u64),
    Wof(u64),
    Osm(OsmType, i64),
    /// A postcode, as `<ISO country code>/<postcode without spaces>`, both
    /// uppercase: `CA/M2M1C8`.
    Postcode(String),
}

/// A postcode's key: `<country>/<postcode>`, uppercase, without spaces.
#[must_use]
pub fn postcode_key(country: &str, postcode: &str) -> String {
    let code: String = postcode.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    format!("{}/{code}", country.to_uppercase())
}

impl PlaceId {
    /// # Errors
    /// If `s` isn't a result ID.
    pub fn parse(s: &str) -> Result<Self> {
        let number = |n: &str| -> Option<u64> {
            (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.parse().ok()).flatten()
        };
        let parsed = if let Some(id) = s.strip_prefix("poi:") {
            number(id).map(PlaceId::Poi)
        } else if let Some(id) = s.strip_prefix("wof:") {
            number(id).map(PlaceId::Wof)
        } else if let Some((osm_type, id)) = s.strip_prefix("osm:").and_then(|rest| rest.split_once('/')) {
            let osm_type = match osm_type {
                "node" => Some(OsmType::Node),
                "way" => Some(OsmType::Way),
                "relation" => Some(OsmType::Relation),
                _ => None,
            };
            osm_type.zip(number(id).and_then(|id| i64::try_from(id).ok())).map(|(t, id)| PlaceId::Osm(t, id))
        } else if let Some((country, code)) = s.strip_prefix("postcode:").and_then(|rest| rest.split_once('/')) {
            let valid = country.len() == 2
                && country.bytes().all(|b| b.is_ascii_uppercase())
                && !code.is_empty()
                && code.chars().all(|c| c.is_alphanumeric() || c == '-');
            valid.then(|| PlaceId::Postcode(postcode_key(country, code)))
        } else {
            None
        };
        match parsed {
            Some(id) => Ok(id),
            None => bail!("invalid result ID {s:?}"),
        }
    }
}

impl std::fmt::Display for PlaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlaceId::Poi(id) => write!(f, "poi:{id}"),
            PlaceId::Wof(id) => write!(f, "wof:{id}"),
            PlaceId::Osm(osm_type, id) => write!(f, "osm:{}/{id}", osm_type.as_str()),
            PlaceId::Postcode(key) => write!(f, "postcode:{key}"),
        }
    }
}

/// Whether places of this placetype count as the administrative areas a
/// place is in. Continents, empires and seas contain places too, but nobody
/// gives them as part of an address.
#[must_use]
pub fn is_containing_admin(placetype: &str) -> bool {
    !matches!(
        placetype,
        "continent" | "empire" | "ocean" | "marinearea" | "planet" | "timezone" | "marketarea" | "campus"
    )
}

/// How specific a placetype is, smallest first, for ordering admins.
#[must_use]
pub fn placetype_rank(placetype: &str) -> u8 {
    match placetype {
        "microhood" => 0,
        "neighbourhood" => 1,
        "macrohood" => 2,
        "borough" => 3,
        "locality" => 4,
        "localadmin" => 5,
        "county" => 6,
        "macrocounty" => 7,
        "region" => 8,
        "macroregion" => 9,
        "disputed" => 10,
        "dependency" => 11,
        "country" => 12,
        "marinearea" => 13,
        "empire" => 14,
        "ocean" => 15,
        "continent" => 16,
        _ => 17,
    }
}

/// The areas to show for a place, smallest placetype first, from all those containing it.
///
/// A place can be in several areas of one placetype, as Who's On First's localities
/// include what are really neighbourhoods. Each placetype shows one: the largest for
/// localities and local admins, since that's the city in an address, and otherwise the
/// smallest, as the most specific. Ties go to the lower ID. Tiles choose by the same rule,
/// in `planetiler-openmaptiles`'s `WofAdmins`.
#[must_use]
pub fn one_per_placetype(mut places: Vec<&WofPlace>) -> Vec<&WofPlace> {
    places.sort_by_key(|place| (placetype_rank(&place.placetype), place.id));
    places.dedup_by_key(|place| place.id);
    let mut chosen: Vec<&WofPlace> = Vec::with_capacity(places.len());
    for place in places {
        match chosen.last_mut() {
            Some(last) if last.placetype == place.placetype => {
                if place.beats(last) {
                    *last = place;
                }
            }
            _ => chosen.push(place),
        }
    }
    chosen
}

/// A document to index.
#[derive(Debug, Clone)]
pub struct ToIndexPoi {
    pub kind: Kind,
    /// Every name it may be searched by.
    pub names: Vec<String>,
    /// Other names it may be found by that aren't its name, such as a place's
    /// historical names: matching one isn't matching its name exactly, so
    /// Seattle's Alki, once called New York, doesn't rival the city.
    pub variant_names: Vec<String>,
    pub house_number: Option<String>,
    pub road: Option<String>,
    pub unit: Option<String>,
    pub postcode: Option<String>,
    /// Names of the administrative areas containing it, with the band of
    /// each one's placetype, filled in by the indexer.
    pub admin_names: Vec<(AdminBand, String)>,
    /// Who's On First IDs of those areas.
    pub admin_ids: Vec<u64>,
    /// Its shape, already simplified.
    pub geometry: Geometry<f64>,
    pub center: Point<f64>,
    /// OSM tags, or nothing for Who's On First places.
    pub tags: Vec<(String, String)>,
    /// Languages spoken where it is, for street name variants.
    pub languages: Vec<Language>,
    /// The ISO 3166-1 alpha-2 code of the country it's in, filled in by the
    /// indexer.
    pub country_code: Option<String>,
    pub osm: Option<OsmRef>,
    pub wof: Option<WofPlace>,
    /// How prominent it is regardless of any query, from 0 to 1.
    pub importance: f64,
}

impl ToIndexPoi {
    /// A document with the shape `geometry`, simplified and normalized, or
    /// `None` if it has no usable shape.
    #[must_use]
    pub fn new(kind: Kind, names: Vec<String>, geometry: Geometry<f64>, tags: Vec<(String, String)>) -> Option<Self> {
        let geometry = geometry::simplify(geometry::normalize(geometry)?, geometry::MAX_VERTICES);
        let center = geometry::representative_point(&geometry)?;
        Some(Self {
            kind,
            names,
            variant_names: Vec::new(),
            house_number: None,
            road: None,
            unit: None,
            postcode: None,
            admin_names: Vec::new(),
            admin_ids: Vec::new(),
            geometry,
            center,
            tags,
            languages: Vec::new(),
            country_code: None,
            osm: None,
            wof: None,
            importance: 0.0,
        })
    }

    /// The tag's value, if it has the tag.
    #[must_use]
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// A document's searchable text, by field: its names, and the forms of its
/// road's name (`West 26th Street`, `W 26th St`, ...). Its house number,
/// unit, postcode and admins' names are in `poi`.
pub struct SchemafiedPoi {
    pub names: Vec<String>,
    pub roads: Vec<String>,
    pub poi: ToIndexPoi,
}

impl From<ToIndexPoi> for SchemafiedPoi {
    fn from(poi: ToIndexPoi) -> Self {
        let mut roads = Vec::new();
        if let Some(road) = &poi.road {
            // As written, whatever its permutations (which are transliterated).
            roads.push(road.clone());
            for lang in &poi.languages {
                roads.extend(permute_road(road, lang).unwrap_or_else(|_| vec![road.clone()]));
            }
        }
        roads.sort();
        roads.dedup();
        Self { names: poi.names.clone(), roads, poi }
    }
}

/// The placetypes of administrative areas, grouped the way queries name
/// them: `capitol hill, seattle, wa` is a neighbourhood, a locality and a
/// region, whether Who's On First calls the last a region or a macroregion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdminBand {
    /// Microhoods, neighbourhoods, macrohoods and boroughs.
    Neighbourhood,
    /// Localities and local admins.
    Locality,
    /// Counties and macrocounties.
    County,
    /// Regions and macroregions.
    Region,
    /// Countries and dependencies.
    Country,
}

impl AdminBand {
    pub const ALL: [AdminBand; 5] =
        [AdminBand::Neighbourhood, AdminBand::Locality, AdminBand::County, AdminBand::Region, AdminBand::Country];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AdminBand::Neighbourhood => "neighbourhood",
            AdminBand::Locality => "locality",
            AdminBand::County => "county",
            AdminBand::Region => "region",
            AdminBand::Country => "country",
        }
    }

    /// The band of a Who's On First placetype, or `None` for placetypes no
    /// query names a place by, like campuses and time zones.
    #[must_use]
    pub fn of(placetype: &str) -> Option<Self> {
        match placetype {
            "microhood" | "neighbourhood" | "macrohood" | "borough" => Some(AdminBand::Neighbourhood),
            "locality" | "localadmin" => Some(AdminBand::Locality),
            "county" | "macrocounty" => Some(AdminBand::County),
            "region" | "macroregion" => Some(AdminBand::Region),
            "country" | "dependency" | "disputed" => Some(AdminBand::Country),
            _ => None,
        }
    }
}

/// The display name of an OSM element in `lang` (a BCP 47 tag): its
/// `name:<lang>` tag if it has one, else `name`.
#[must_use]
pub fn osm_display_name<'a>(tags: &'a [(String, String)], lang: Option<&str>) -> Option<&'a str> {
    let get = |key: &str| tags.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    if let Some(lang) = lang {
        let primary = lang.split(['-', '_']).next().unwrap_or_default().to_lowercase();
        let two_letter = match primary.len() {
            3 => isolang::Language::from_639_3(&primary).and_then(|l| l.to_639_1()).map(str::to_string),
            _ => Some(primary.clone()),
        };
        for code in two_letter.iter().chain(std::iter::once(&primary)) {
            if let Some(name) = get(&format!("name:{code}")) {
                return Some(name);
            }
        }
    }
    get("name")
}

#[cfg(test)]
mod test {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn osm_ref_round_trips() {
        for r in [
            OsmRef { osm_type: OsmType::Relation, id: 5843919, version: Some(31) },
            OsmRef { osm_type: OsmType::Node, id: 1, version: None },
        ] {
            assert_eq!(OsmRef::decode(&r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn osm_display_name_prefers_the_language() {
        let t = tags(&[("name", "Genève"), ("name:de", "Genf"), ("name:it", "Ginevra")]);
        assert_eq!(osm_display_name(&t, Some("de-CH")), Some("Genf"));
        assert_eq!(osm_display_name(&t, Some("ita")), Some("Ginevra"));
        assert_eq!(osm_display_name(&t, Some("fr")), Some("Genève"));
        assert_eq!(osm_display_name(&t, None), Some("Genève"));
    }

    #[test]
    fn one_area_of_each_placetype_by_its_rule() {
        let area = |id, placetype: &str, size| WofPlace {
            id,
            placetype: placetype.into(),
            population: None,
            default_name: format!("{id}"),
            names: BTreeMap::new(),
            abbreviation: None,
            country: None,
            size,
        };
        let places = [
            area(1, "country", 100.0),
            // The city, and what Who's On First also calls a locality in it, whose ID is
            // lower in one case and higher in the other.
            area(2, "locality", 1.0),
            area(3, "locality", 0.01),
            area(4, "neighbourhood", 0.05),
            area(5, "neighbourhood", 0.001),
            area(6, "localadmin", 2.0),
            area(7, "localadmin", 3.0),
            area(8, "county", 5.0),
            area(9, "county", 5.0),
            // No shape, so not chosen over one with.
            area(10, "microhood", 0.0),
            area(11, "microhood", 0.1),
        ];
        let ids = |places: Vec<&WofPlace>| -> Vec<u64> {
            one_per_placetype(places).into_iter().map(|place| place.id).collect()
        };
        assert_eq!(ids(places.iter().collect()), [11, 5, 2, 7, 8, 1]);
        assert_eq!(ids(places.iter().rev().collect()), [11, 5, 2, 7, 8, 1]);
        assert_eq!(ids(vec![&places[1], &places[1]]), [2]);
        assert!(ids(Vec::new()).is_empty());
    }

    #[test]
    fn wof_display_name_falls_back_to_the_default() {
        let place = WofPlace {
            id: 101748463,
            placetype: "locality".into(),
            population: Some(400_028),
            default_name: "Zürich".into(),
            names: [("eng".to_string(), "Zurich".to_string())].into(),
            abbreviation: None,
            country: Some("CH".into()),
            size: 0.0,
        };
        assert_eq!(place.display_name(Some("en-US")), "Zurich");
        assert_eq!(place.display_name(Some("ja")), "Zürich");
        assert_eq!(place.display_name(None), "Zürich");
    }

    #[test]
    fn country_codes_are_iso_or_none() {
        assert_eq!(iso_country_code("ch").as_deref(), Some("CH"));
        assert_eq!(iso_country_code("US").as_deref(), Some("US"));
        assert_eq!(iso_country_code("XK").as_deref(), Some("XK"));
        for unknown in ["XX", "XY", "XZ", "QO", "", "USA", "-1", "1A"] {
            assert_eq!(iso_country_code(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn place_ids_round_trip() {
        for id in ["poi:51234", "wof:101730401", "osm:node/2470190871", "osm:way/1", "osm:relation/5843919", "postcode:CA/M2M1C8", "postcode:US/10010"] {
            assert_eq!(PlaceId::parse(id).unwrap().to_string(), id);
        }
        for bad in ["", "poi:", "poi:-1", "poi:1x", "wof:+5", "osm:node", "osm:area/1", "osm:node/", "node/1", "POI:1", "postcode:", "postcode:CA/", "postcode:ca/M2M", "postcode:CAN/M2M", "postcode:US/1 2"] {
            assert!(PlaceId::parse(bad).is_err(), "{bad}");
        }
    }
}
