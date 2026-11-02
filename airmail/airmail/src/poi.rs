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
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Poi, Kind::Address, Kind::Admin];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Poi => "poi",
            Kind::Address => "address",
            Kind::Admin => "admin",
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
}

impl WofPlace {
    /// The name in `lang` (a BCP 47 tag) if there is one, else the default.
    #[must_use]
    pub fn display_name(&self, lang: Option<&str>) -> &str {
        lang.and_then(iso_639_3)
            .and_then(|code| self.names.get(code))
            .unwrap_or(&self.default_name)
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

/// A document to index.
#[derive(Debug, Clone)]
pub struct ToIndexPoi {
    pub kind: Kind,
    /// Every name it may be searched by.
    pub names: Vec<String>,
    pub house_number: Option<String>,
    pub road: Option<String>,
    pub unit: Option<String>,
    pub postcode: Option<String>,
    /// Names of the administrative areas containing it, filled in by the
    /// indexer.
    pub admin_names: Vec<String>,
    /// Who's On First IDs of those areas.
    pub admin_ids: Vec<u64>,
    /// Its shape, already simplified.
    pub geometry: Geometry<f64>,
    pub center: Point<f64>,
    /// OSM tags, or nothing for Who's On First places.
    pub tags: Vec<(String, String)>,
    /// Languages spoken where it is, for street name variants.
    pub languages: Vec<Language>,
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

/// A document's searchable text, by field.
pub struct SchemafiedPoi {
    pub names: Vec<String>,
    pub address: Vec<String>,
    pub admin_names: Vec<String>,
    pub poi: ToIndexPoi,
}

impl From<ToIndexPoi> for SchemafiedPoi {
    fn from(poi: ToIndexPoi) -> Self {
        let mut address = Vec::new();
        address.extend(poi.house_number.clone());
        if let Some(road) = &poi.road {
            if poi.languages.is_empty() {
                address.push(road.clone());
            }
            for lang in &poi.languages {
                address.extend(permute_road(road, lang).unwrap_or_else(|_| vec![road.clone()]));
            }
        }
        address.extend(poi.unit.clone());
        address.extend(poi.postcode.clone());
        address.dedup();
        Self { names: poi.names.clone(), address, admin_names: poi.admin_names.clone(), poi }
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
    fn wof_display_name_falls_back_to_the_default() {
        let place = WofPlace {
            id: 101748463,
            placetype: "locality".into(),
            population: Some(400_028),
            default_name: "Zürich".into(),
            names: [("eng".to_string(), "Zurich".to_string())].into(),
            abbreviation: None,
        };
        assert_eq!(place.display_name(Some("en-US")), "Zurich");
        assert_eq!(place.display_name(Some("ja")), "Zürich");
        assert_eq!(place.display_name(None), "Zürich");
    }
}
