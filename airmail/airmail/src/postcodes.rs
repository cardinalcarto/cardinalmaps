//! Postcodes, gathered from the addresses that have them.

use std::collections::HashMap;
use std::sync::Arc;

use crate::importance;
use crate::poi::{AdminBand, Kind, ToIndexPoi, postcode_key};
use geo::{Area, ConvexHull, Geometry, MultiPoint, Point};

/// The most points of a postcode's addresses kept for its shape, besides
/// its extremes.
const SAMPLE_POINTS: usize = 64;

/// The longest postcode, in characters: longer `addr:postcode` values are
/// mistakes.
const MAX_POSTCODE_LEN: usize = 12;

/// Where a postcode's addresses are.
struct Postcode {
    /// As written, preferring a form with a space (`M2M 1C8`).
    display: String,
    country: String,
    sample: Vec<Point<f64>>,
    seen: usize,
    /// Westernmost, easternmost, southernmost and northernmost.
    extremes: [Point<f64>; 4],
    /// Of the first address.
    admin_ids: Vec<u64>,
    admin_names: Arc<Vec<(AdminBand, String)>>,
}

/// Postcodes, gathered as addresses are indexed and indexed themselves once
/// they all are.
#[derive(Default)]
pub struct Postcodes {
    by_key: HashMap<String, Postcode>,
    /// Admin names by admin IDs, shared between postcodes in the same areas.
    admin_names: HashMap<Vec<u64>, Arc<Vec<(AdminBand, String)>>>,
}

impl Postcodes {
    /// Notes the postcodes of an address or POI.
    pub fn add(&mut self, poi: &ToIndexPoi) {
        let (Some(postcodes), Some(country)) = (&poi.postcode, &poi.country_code) else { return };
        if !matches!(poi.kind, Kind::Address | Kind::Poi) {
            return;
        }
        for code in postcodes.split(';').map(str::trim) {
            if code.is_empty()
                || code.chars().count() > MAX_POSTCODE_LEN
                || !code.chars().any(|c| c.is_ascii_digit())
                || !code.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-')
            {
                continue;
            }
            let key = postcode_key(country, code);
            let at = poi.center;
            match self.by_key.get_mut(&key) {
                Some(postcode) => {
                    postcode.seen += 1;
                    if postcode.sample.len() < SAMPLE_POINTS {
                        postcode.sample.push(at);
                    } else if postcode.seen.is_multiple_of(7) {
                        let i = postcode.seen / 7 % SAMPLE_POINTS;
                        postcode.sample[i] = at;
                    }
                    let [west, east, south, north] = &mut postcode.extremes;
                    if at.x() < west.x() {
                        *west = at;
                    }
                    if at.x() > east.x() {
                        *east = at;
                    }
                    if at.y() < south.y() {
                        *south = at;
                    }
                    if at.y() > north.y() {
                        *north = at;
                    }
                    if !postcode.display.contains(' ') && code.contains(' ') {
                        postcode.display = code.to_string();
                    }
                }
                None => {
                    let admin_names = self
                        .admin_names
                        .entry(poi.admin_ids.clone())
                        .or_insert_with(|| Arc::new(poi.admin_names.clone()))
                        .clone();
                    self.by_key.insert(
                        key,
                        Postcode {
                            display: code.to_string(),
                            country: country.clone(),
                            sample: vec![at],
                            seen: 1,
                            extremes: [at; 4],
                            admin_ids: poi.admin_ids.clone(),
                            admin_names,
                        },
                    );
                }
            }
        }
    }

    /// How many postcodes there are.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Each postcode as a document: shaped like where its addresses are, and
    /// found by its code as written and without spaces.
    pub fn into_pois(self) -> impl Iterator<Item = ToIndexPoi> {
        self.by_key.into_iter().filter_map(|(key, postcode)| {
            let mut points = postcode.sample;
            points.extend(postcode.extremes);
            let hull = MultiPoint(points.clone()).convex_hull();
            let geometry = if hull.exterior().0.len() >= 4 && hull.unsigned_area() > 0.0 {
                Geometry::Polygon(hull)
            } else {
                let n = points.len() as f64;
                Geometry::Point(Point::new(points.iter().map(|p| p.x()).sum::<f64>() / n, points.iter().map(|p| p.y()).sum::<f64>() / n))
            };
            let mut names = vec![postcode.display.clone()];
            let compact: String = postcode.display.chars().filter(|c| !c.is_whitespace()).collect();
            if compact != postcode.display {
                names.push(compact);
            }
            let tags = vec![
                ("postal_code".to_string(), postcode.display.clone()),
                ("addr:postcode".to_string(), postcode.display.clone()),
                ("cardinal:postcode_id".to_string(), key),
            ];
            let mut poi = ToIndexPoi::new(Kind::Postcode, names, geometry, tags)?;
            poi.postcode = Some(postcode.display);
            poi.admin_ids = postcode.admin_ids;
            poi.admin_names = (*postcode.admin_names).clone();
            poi.country_code = Some(postcode.country);
            poi.importance = importance::POSTCODE_IMPORTANCE;
            Some(poi)
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn address(postcode: &str, at: (f64, f64)) -> ToIndexPoi {
        let mut poi = ToIndexPoi::new(Kind::Address, vec![], Geometry::Point(Point::new(at.0, at.1)), vec![]).unwrap();
        poi.postcode = Some(postcode.into());
        poi.country_code = Some("CA".into());
        poi.admin_ids = vec![1, 2];
        poi.admin_names = vec![(AdminBand::Locality, "Toronto".into())];
        poi
    }

    #[test]
    fn addresses_make_postcodes() {
        let mut postcodes = Postcodes::default();
        postcodes.add(&address("M2M1C8", (-79.40, 43.78)));
        postcodes.add(&address("M2M 1C8", (-79.41, 43.79)));
        postcodes.add(&address("M2M 1C8", (-79.40, 43.79)));
        postcodes.add(&address("M2M 1C9;not a postcode", (-79.30, 43.70)));
        assert_eq!(postcodes.len(), 2);
        let mut pois: Vec<ToIndexPoi> = postcodes.into_pois().collect();
        pois.sort_by(|a, b| a.names.cmp(&b.names));
        let first = &pois[0];
        assert_eq!(first.kind, Kind::Postcode);
        assert_eq!(first.names, vec!["M2M 1C8", "M2M1C8"]);
        assert_eq!(first.tag("cardinal:postcode_id"), Some("CA/M2M1C8"));
        assert!(matches!(first.geometry, Geometry::Polygon(_)));
        assert_eq!(first.admin_names, vec![(AdminBand::Locality, "Toronto".to_string())]);
        assert!(matches!(pois[1].geometry, Geometry::Point(_)));
    }
}
