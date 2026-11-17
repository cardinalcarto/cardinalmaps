//! Who's On First places from the WOF service (`wof_service/`, `GET /v1/places`), as the
//! app stores its responses with each region: the areas the index's documents are in, and
//! the areas' own documents.
//!
//! A region's box is fetched in pieces, and regions overlap, so a place can come in many
//! responses: as a place in the box (with its simplified boundary) in some, and as an
//! `ancestor_only` place (no geometry) in others. It's indexed if any response had it in
//! its box, with the geometry that response gave.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use airmail::language::Language;
use airmail::poi::{AdminBand, ToIndexPoi, iso_country_code};
use airmail::wof::{WofName, WofRecord, admin_document, containing, country};
use geo::{Coord, Geometry, LineString, MultiPolygon, Point, Polygon};
use serde::Deserialize;

/// The boxes to fetch a region's places in, `[west, south, east, north]`: its bounding box
/// split into a grid of boxes at most `side` degrees each way (the service caps a box's
/// area, and big responses are slow). A box across the antimeridian (`west > east`) is
/// split on it first.
#[must_use]
pub fn boxes(bbox: [f64; 4], side: f64) -> Vec<[f64; 4]> {
    let [west, south, east, north] = bbox;
    if west > east {
        let mut out = boxes([west, south, 180.0, north], side);
        out.extend(boxes([-180.0, south, east, north], side));
        return out;
    }
    let side = if side.is_finite() && side > 0.0 {
        side
    } else {
        1.0
    };
    let steps = |from: f64, to: f64| ((to - from) / side).ceil().max(1.0) as usize;
    let (nx, ny) = (steps(west, east), steps(south, north));
    let (dx, dy) = ((east - west) / nx as f64, (north - south) / ny as f64);
    let mut out = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let w = west + dx * i as f64;
            let s = south + dy * j as f64;
            // The last box ends exactly where the region does.
            let e = if i + 1 == nx { east } else { w + dx };
            let n = if j + 1 == ny { north } else { s + dy };
            out.push([w, s, e, n]);
        }
    }
    out
}

#[derive(Deserialize)]
struct Response {
    places: Vec<Place>,
}

/// A place as the service sends it (see `wof_service/README.md`).
#[derive(Deserialize)]
struct Place {
    id: u64,
    placetype: String,
    /// `[lang, tag, abbr, name]`.
    names: Vec<(String, String, bool, String)>,
    #[serde(default)]
    ancestors: Vec<u64>,
    country: Option<String>,
    population: Option<u64>,
    population_rank: Option<u32>,
    #[serde(default)]
    lang_x_spoken: Vec<String>,
    label: Option<[f64; 2]>,
    geometry: Option<serde_json::Value>,
    #[serde(default)]
    ancestor_only: bool,
}

/// Why a response couldn't be read.
#[derive(Debug, thiserror::Error)]
#[error("not a WOF service response: {0}")]
pub struct WofError(#[from] serde_json::Error);

/// The areas a feature is in, and what it's found by in them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Areas {
    /// Every area containing it, ancestors included.
    pub ids: Vec<u64>,
    /// Their names it's found by, by band.
    pub names: Vec<(AdminBand, String)>,
    /// Its country's ISO 3166-1 alpha-2 code.
    pub country_code: Option<String>,
    /// ISO 639-3 codes of the languages spoken there.
    pub languages: Vec<String>,
    /// The same, those airmail has street name rules for.
    pub street_languages: Vec<Language>,
}

/// The Who's On First places of the regions being indexed.
#[derive(Default)]
pub struct WofPlaces {
    records: HashMap<u64, WofRecord>,
    /// The places some response had in its box: those that are indexed.
    in_box: BTreeSet<u64>,
    /// [`WofPlaces::areas`] by the tile's areas, which many features share.
    memo: HashMap<Vec<(String, i64)>, Arc<Areas>>,
}

impl WofPlaces {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the places of one response (its JSON, uncompressed); returns how many it had.
    ///
    /// # Errors
    ///
    /// If it isn't a response of the service.
    pub fn add_response(&mut self, json: &[u8]) -> Result<usize, WofError> {
        let response: Response = serde_json::from_slice(json)?;
        let count = response.places.len();
        for place in response.places {
            let in_box = !place.ancestor_only;
            let known_in_box = self.in_box.contains(&place.id);
            if known_in_box && !in_box {
                continue;
            }
            if self.records.contains_key(&place.id) && !in_box {
                continue;
            }
            if in_box {
                self.in_box.insert(place.id);
            }
            self.records.insert(place.id, record(place));
        }
        self.memo.clear();
        Ok(count)
    }

    /// How many places are known, and how many of them are indexed.
    #[must_use]
    pub fn len(&self) -> (usize, usize) {
        (self.records.len(), self.in_box.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn record(&self, id: u64) -> Option<&WofRecord> {
        self.records.get(&id)
    }

    /// The languages spoken in the country among `ids`, if there is one and it's known.
    fn languages<'a>(&self, ids: impl IntoIterator<Item = &'a u64>) -> (Option<u64>, Vec<String>) {
        let found = country(ids);
        let languages = found
            .and_then(|id| self.records.get(&id))
            .map(|c| c.languages.clone())
            .unwrap_or_default();
        (found, languages)
    }

    /// The areas a feature whose tile names the areas `tile_admins` (`(placetype, WOF ID)`,
    /// from `cardinal:admin:*`) is in: those and their ancestors, with the names it's found
    /// by there (see `airmail::wof::containing`).
    pub fn areas(&mut self, tile_admins: &[(String, i64)]) -> Arc<Areas> {
        if let Some(found) = self.memo.get(tile_admins) {
            return found.clone();
        }
        let mut ids = BTreeSet::new();
        for (_, id) in tile_admins {
            let Ok(id) = u64::try_from(*id) else { continue };
            ids.insert(id);
            if let Some(record) = self.records.get(&id) {
                ids.extend(record.ancestors.iter().copied());
            }
        }
        let (country_id, languages) = self.languages(&ids);
        let country_code = country_id
            .and_then(|id| self.records.get(&id))
            .and_then(|c| c.country.as_deref())
            .and_then(iso_country_code)
            .or_else(|| {
                // The smallest area's, if the country isn't known.
                ids.iter()
                    .filter_map(|id| self.records.get(id))
                    .find_map(|r| r.country.as_deref().and_then(iso_country_code))
            });
        let (ids, names) = containing(
            ids.iter().map(|id| {
                let record = self.records.get(id);
                (
                    *id,
                    record.map_or("", |r| r.placetype.as_str()),
                    record.map_or(&[][..], |r| r.names.as_slice()),
                )
            }),
            &languages,
        );
        let street_languages = languages
            .iter()
            .filter_map(|l| Language::from_iso_639_3(l))
            .collect();
        let areas = Arc::new(Areas {
            ids,
            names,
            country_code,
            languages,
            street_languages,
        });
        self.memo.insert(tile_admins.to_vec(), areas.clone());
        areas
    }

    /// The documents of the places in the boxes, in ID order.
    pub fn documents(&self) -> impl Iterator<Item = ToIndexPoi> + '_ {
        self.in_box.iter().filter_map(|id| {
            let record = self.records.get(id)?;
            let (_, languages) = self.languages(std::iter::once(id).chain(&record.ancestors));
            admin_document(record, |a| self.records.get(&a), &languages)
        })
    }
}

fn record(place: Place) -> WofRecord {
    WofRecord {
        id: place.id,
        placetype: place.placetype,
        names: place
            .names
            .into_iter()
            .map(|(lang, tag, abbr, name)| WofName {
                lang,
                tag,
                abbr,
                name,
            })
            .collect(),
        ancestors: place.ancestors,
        population: place.population,
        population_rank: place.population_rank,
        country: place.country,
        languages: place.lang_x_spoken,
        label: place.label.map(|[x, y]| Point::new(x, y)),
        boundary: place.geometry.as_ref().and_then(geometry),
    }
}

/// A GeoJSON Polygon or MultiPolygon, as the service sends boundaries.
fn geometry(json: &serde_json::Value) -> Option<Geometry<f64>> {
    let coordinates = json.get("coordinates")?;
    match json.get("type")?.as_str()? {
        "Polygon" => polygon(coordinates).map(Geometry::Polygon),
        "MultiPolygon" => {
            let polygons: Vec<Polygon<f64>> =
                coordinates.as_array()?.iter().filter_map(polygon).collect();
            (!polygons.is_empty()).then_some(Geometry::MultiPolygon(MultiPolygon(polygons)))
        }
        _ => None,
    }
}

fn polygon(json: &serde_json::Value) -> Option<Polygon<f64>> {
    let mut rings = json.as_array()?.iter().filter_map(ring);
    let exterior = rings.next()?;
    Some(Polygon::new(exterior, rings.collect()))
}

fn ring(json: &serde_json::Value) -> Option<LineString<f64>> {
    let coords: Vec<Coord<f64>> = json
        .as_array()?
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            Some(Coord {
                x: p.first()?.as_f64()?,
                y: p.get(1)?.as_f64()?,
            })
        })
        .collect();
    (coords.len() >= 4).then_some(LineString(coords))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seattle in Washington in the US, as two overlapping boxes might send them.
    fn responses() -> (String, String) {
        let us = r#"{"id": 85633793, "placetype": "country",
            "names": [["und", "default", false, "United States"], ["eng", "preferred", true, "US"],
                      ["spa", "preferred", false, "Estados Unidos"]],
            "ancestors": [102191575], "country": "US", "lang_x_spoken": ["eng", "spa"],
            "label": [-97.0, 39.0], "geometry": null, "ancestor_only": true}"#;
        let wa = r#"{"id": 85688623, "placetype": "region",
            "names": [["und", "default", false, "Washington"], ["eng", "preferred", true, "WA"]],
            "ancestors": [85633793, 102191575], "country": "US", "label": [-120.5, 47.4],
            "geometry": null, "ancestor_only": true}"#;
        let seattle = |geometry: &str, ancestor_only: bool| {
            format!(
                r#"{{"id": 101730401, "placetype": "locality",
                "names": [["und", "default", false, "Seattle"], ["eng", "colloquial", false, "Emerald City"],
                          ["spa", "preferred", false, "Seattle"], ["jpn", "preferred", false, "シアトル"]],
                "ancestors": [102086191, 85688623, 85633793, 102191575], "country": "US",
                "population": 737015, "label": [-122.33, 47.61], "geometry": {geometry},
                "ancestor_only": {ancestor_only}}}"#
            )
        };
        let square = r#"{"type": "Polygon", "coordinates": [[[-122.44, 47.49], [-122.23, 47.49], [-122.23, 47.74], [-122.44, 47.74], [-122.44, 47.49]]]}"#;
        (
            format!(r#"{{"places": [{us}, {wa}, {}]}}"#, seattle(square, false)),
            format!(r#"{{"places": [{us}, {wa}, {}]}}"#, seattle("null", true)),
        )
    }

    #[test]
    fn a_place_in_any_box_is_indexed_with_its_boundary() {
        let (first, second) = responses();
        // Either order.
        for order in [[&first, &second], [&second, &first]] {
            let mut places = WofPlaces::new();
            for json in order {
                assert_eq!(places.add_response(json.as_bytes()).unwrap(), 3);
            }
            assert_eq!(places.len(), (3, 1));
            let docs: Vec<ToIndexPoi> = places.documents().collect();
            assert_eq!(docs.len(), 1);
            let seattle = &docs[0];
            assert_eq!(seattle.names, vec!["Seattle", "シアトル"]);
            assert!(matches!(seattle.geometry, Geometry::Polygon(_)));
            assert_eq!(seattle.country_code.as_deref(), Some("US"));
            // Found by its region and country in English, Spanish and their default names.
            assert!(
                seattle
                    .admin_names
                    .contains(&(AdminBand::Country, "Estados Unidos".into()))
            );
            assert!(
                seattle
                    .admin_names
                    .contains(&(AdminBand::Region, "WA".into()))
            );
        }
    }

    #[test]
    fn tile_areas_expand_through_ancestors() {
        let (first, _) = responses();
        let mut places = WofPlaces::new();
        places.add_response(first.as_bytes()).unwrap();
        let areas = places.areas(&[("locality".into(), 101_730_401)]);
        // The county and the continent aren't in the response, so they count (the service
        // sends every ancestor; one it lacks may contain the place).
        assert_eq!(
            areas.ids,
            vec![
                85_633_793,
                85_688_623,
                101_730_401,
                102_086_191,
                102_191_575
            ]
        );
        assert_eq!(areas.country_code.as_deref(), Some("US"));
        assert_eq!(areas.languages, vec!["eng", "spa"]);
        assert!(
            areas
                .names
                .contains(&(AdminBand::Locality, "Seattle".into()))
        );
        assert!(
            !areas
                .names
                .iter()
                .any(|(_, n)| n == "Emerald City" || n == "シアトル")
        );
        assert!(Arc::ptr_eq(
            &areas,
            &places.areas(&[("locality".into(), 101_730_401)])
        ));
        // A tile area that isn't known still counts.
        assert_eq!(places.areas(&[("county".into(), 5)]).ids, vec![5]);
    }

    #[test]
    fn regions_split_into_small_boxes() {
        // Washington, 7.9 by 3.5 degrees.
        let wa = boxes([-124.85, 45.54, -116.91, 49.01], 3.0);
        assert_eq!(wa.len(), 6);
        assert!(wa.iter().all(|[w, s, e, n]| e - w <= 3.0 && n - s <= 3.0));
        assert_eq!(wa[0][0], -124.85);
        assert_eq!(wa[5][2], -116.91);
        assert_eq!(wa[5][3], 49.01);
        let area: f64 = wa.iter().map(|[w, s, e, n]| (e - w) * (n - s)).sum();
        assert!((area - 7.94 * 3.47).abs() < 1e-9);
        assert_eq!(boxes([1.0, 2.0, 1.5, 2.5], 3.0), vec![[1.0, 2.0, 1.5, 2.5]]);
        // Across the antimeridian: Fiji.
        let fiji = boxes([176.0, -20.0, -178.0, -15.0], 3.0);
        assert!(fiji.iter().all(|[w, _, e, _]| w < e));
        assert_eq!(fiji.len(), 2 * 2 + 2);
    }

    #[test]
    fn junk_is_an_error() {
        assert!(
            WofPlaces::new()
                .add_response(b"{\"error\": \"too big\"}")
                .is_err()
        );
    }
}
