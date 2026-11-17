//! Searching the device's index, with the query tagger and a categorical model when they're
//! downloaded, into results shaped like the geocoder server's (`airmail_service`'s
//! `Result`), so the app reads both the same way.
//!
//! A pointer result (a POI with a stable ID) has its names, center, importance and areas,
//! and says where its feature is (`pointer`: its POI ID and z14 tile); the app reads the
//! rest from the tile. Every other result comes with what it stored.

use std::collections::HashMap;
use std::path::Path;

use airmail::categories::CategoryModel;
use airmail::index::{AirmailIndex, SearchHit, SearchRequest};
use airmail::poi::{Kind, PlaceId, WofPlace, one_per_placetype, osm_display_name, postcode_key};
use anyhow::{Context, Result};
use geo::Geometry;
use serde_json::{Value, json};

/// What to search for, as the server's `/v1/search` takes it.
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub text: String,
    /// A BCP 47 tag, for names in that language.
    pub lang: Option<String>,
    pub prefix: bool,
    pub typos: bool,
    pub focus: Option<[f64; 2]>,
    pub viewport: Option<[f64; 4]>,
    pub limit: usize,
}

/// The index on the device, opened for searching.
pub struct DeviceIndex {
    index: AirmailIndex,
    model: Option<CategoryModel>,
}

impl DeviceIndex {
    /// Opens the index in `dir`, with the query tagger and the categorical model if given.
    ///
    /// # Errors
    ///
    /// If there's no index there, or a model can't be read.
    pub fn open(dir: &Path, tagger: Option<&Path>, categories: Option<&Path>) -> Result<Self> {
        let mut index = AirmailIndex::open_read_only(&dir.to_string_lossy())?;
        if let Some(path) = tagger {
            let file =
                std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
            let tagger = airmail_tagger::Tagger::read(&mut std::io::BufReader::new(file))
                .with_context(|| format!("reading {}", path.display()))?;
            index = index.with_tagger(tagger);
        }
        let model = categories.map(CategoryModel::open).transpose()?;
        // Opens every segment now rather than on the first search.
        index.searcher()?;
        Ok(Self { index, model })
    }

    /// How many documents it has.
    ///
    /// # Errors
    ///
    /// If it can't be read.
    pub fn documents(&self) -> Result<u64> {
        self.index.num_docs_blocking()
    }

    /// The categorical model's language, if one is loaded.
    #[must_use]
    pub fn category_language(&self) -> Option<&str> {
        self.model.as_ref().map(CategoryModel::language)
    }

    /// Results for `query`, most relevant first.
    ///
    /// # Errors
    ///
    /// If the search fails.
    pub fn search(&self, query: &Query) -> Result<Vec<Value>> {
        let request = SearchRequest {
            // A space at the end says the last word is finished.
            text: query.text.trim_start(),
            prefix: query.prefix,
            typos: query.typos,
            limit: query.limit.max(1),
            viewport: query.viewport,
            focus: query.focus,
            model: self.model.as_ref(),
            ..SearchRequest::default()
        };
        let results = self.index.search(&request)?;
        Ok(results
            .hits
            .iter()
            .map(|hit| result_json(hit, &results.admins, query.lang.as_deref()))
            .collect())
    }

    /// The place with the result ID `id`, or `None` if there's none (or `id` isn't one).
    ///
    /// # Errors
    ///
    /// If the lookup fails.
    pub fn place(&self, id: &str, lang: Option<&str>) -> Result<Option<Value>> {
        let Ok(id) = PlaceId::parse(id) else {
            return Ok(None);
        };
        let results = self.index.lookup_blocking(&id)?;
        Ok(results
            .hits
            .first()
            .map(|hit| result_json(hit, &results.admins, lang)))
    }
}

/// Rounds degrees to 7 decimal places, about a centimeter.
fn round(degrees: f64) -> f64 {
    (degrees * 1e7).round() / 1e7
}

fn position(c: geo::Coord<f64>) -> Value {
    json!([round(c.x), round(c.y)])
}

fn ring(line: &geo::LineString<f64>) -> Value {
    Value::Array(line.0.iter().map(|c| position(*c)).collect())
}

fn polygon(polygon: &geo::Polygon<f64>) -> Value {
    Value::Array(
        std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(ring)
            .collect(),
    )
}

fn geometry_json(geometry: &Geometry<f64>) -> Value {
    match geometry {
        Geometry::LineString(l) => json!({ "type": "LineString", "coordinates": ring(l) }),
        Geometry::MultiLineString(m) => {
            json!({ "type": "MultiLineString", "coordinates": m.0.iter().map(ring).collect::<Vec<_>>() })
        }
        Geometry::Polygon(p) => json!({ "type": "Polygon", "coordinates": polygon(p) }),
        Geometry::MultiPolygon(m) => {
            json!({ "type": "MultiPolygon", "coordinates": m.0.iter().map(polygon).collect::<Vec<_>>() })
        }
        other => {
            let p = airmail::geometry::representative_point(other)
                .unwrap_or_else(|| geo::Point::new(0.0, 0.0));
            json!({ "type": "Point", "coordinates": position(p.0) })
        }
    }
}

/// Name keys to show when a POI has no `name`, most preferred first (as the server).
const FALLBACK_NAME_KEYS: [&str; 4] = ["official_name", "alt_name", "short_name", "loc_name"];

/// A hit as the server's API gives it (`airmail_service/src/api.rs`, `result_json`, with
/// shapes), plus `pointer` for a pointer document. A document from the tiles has no POI
/// version or OSM version.
fn result_json(hit: &SearchHit, admins: &HashMap<u64, WofPlace>, lang: Option<&str>) -> Value {
    let name = match hit.kind {
        Kind::Admin => hit.wof.as_ref().map(|w| w.display_name(lang).to_string()),
        Kind::Postcode => hit.tag("postal_code").map(str::to_string),
        Kind::Poi | Kind::Street => osm_display_name(&hit.tags, lang)
            .or_else(|| FALLBACK_NAME_KEYS.iter().find_map(|key| hit.tag(key)))
            .or_else(|| {
                hit.tags
                    .iter()
                    .find(|(k, _)| k.contains("name"))
                    .map(|(_, v)| v.as_str())
            })
            .map(str::to_string),
        Kind::Address => None,
    };
    let address_parts = [
        ("housenumber", "addr:housenumber"),
        ("street", "addr:street"),
        ("unit", "addr:unit"),
        ("postcode", "addr:postcode"),
    ];
    let address = if address_parts.iter().any(|(_, key)| hit.tag(key).is_some()) {
        Value::Object(
            address_parts
                .iter()
                .map(|(field, key)| (field.to_string(), json!(hit.tag(key))))
                .collect(),
        )
    } else {
        Value::Null
    };
    let containing = one_per_placetype(
        hit.admin_ids
            .iter()
            .filter_map(|id| admins.get(id))
            .collect(),
    );
    let country_code = hit
        .wof
        .as_ref()
        .and_then(|wof| wof.country.clone())
        .or_else(|| containing.iter().find_map(|place| place.country.clone()));
    let center = position(hit.center.0);
    let id = if let Some(pointer) = &hit.pointer {
        format!("poi:{}", pointer.poi_id)
    } else if let Some((poi, _)) = hit.poi {
        format!("poi:{poi}")
    } else if let Some(wof) = &hit.wof {
        format!("wof:{}", wof.id)
    } else if let Some(osm) = &hit.osm {
        format!("osm:{}/{}", osm.osm_type.as_str(), osm.id)
    } else if let (Kind::Postcode, Some(country), Some(code)) =
        (hit.kind, &country_code, hit.tag("postal_code"))
    {
        PlaceId::Postcode(postcode_key(country, code)).to_string()
    } else {
        // A POI of the tiles with neither a stable ID nor an OSM element: found only by
        // search, so it's named by where it is.
        format!("point:{},{}", round(hit.center.x()), round(hit.center.y()))
    };
    let admins: Vec<Value> = containing
        .into_iter()
        .map(|place| {
            json!({
                "id": format!("wof:{}", place.id),
                "placetype": place.placetype,
                "name": place.display_name(lang),
                "abbreviation": place.abbreviation,
            })
        })
        .collect();
    let bbox = airmail::geometry::bbox(&hit.geometry)
        .map(|b| b.map(round))
        .unwrap_or([0.0; 4]);
    let tags: serde_json::Map<String, Value> = hit
        .tags
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    json!({
        "id": id,
        "kind": hit.kind,
        "name": name,
        "center": center,
        "address": address,
        "admins": admins,
        "country_code": country_code,
        "importance": hit.importance.clamp(0.0, 1.0),
        "score": hit.score.max(0.0),
        "poi": hit.poi.map(|(id, version)| json!({ "id": id, "version": version })),
        "osm": json!({
            "type": hit.osm.map(|o| o.osm_type.as_str()),
            "id": hit.osm.map(|o| o.id),
            "version": hit.osm.and_then(|o| o.version),
            "tags": tags,
        }),
        "wof": hit.wof.as_ref().map(|wof| json!({
            "id": wof.id,
            "placetype": wof.placetype,
            "population": wof.population,
            "abbreviation": wof.abbreviation,
        })),
        "pointer": hit.pointer.map(|p| json!({ "poi_id": p.poi_id, "tile": [p.tile.z, p.tile.x, p.tile.y] })),
        "feature": {
            "type": "Feature",
            "bbox": bbox,
            "geometry": geometry_json(&hit.geometry),
            "properties": null,
        },
    })
}
