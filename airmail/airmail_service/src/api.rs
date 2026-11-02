//! The HTTP API, as `openapi.yaml` describes it.

use std::collections::HashMap;
use std::sync::Arc;

use airmail::{
    categories::{CategoryModels, QueryCategories},
    index::{AirmailIndex, SearchHit, SearchRequest},
    poi::{Kind, WofPlace, osm_display_name, placetype_rank},
};
use axum::{
    Json,
    extract::{RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use geo::Geometry;
use serde::Serialize;
use serde_json::{Value, json};

use crate::error::Problem;

/// The API's description.
pub const OPENAPI: &str = include_str!("../openapi.yaml");

pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 50;

pub struct AppState {
    pub index: AirmailIndex,
    pub categories: CategoryModels,
    pub category_include_threshold: f32,
    pub schema_version: u32,
    pub documents: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryMode {
    Shape,
    Point,
}

/// `/v1/search` parameters, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchParams {
    pub q: String,
    pub lang: Option<String>,
    pub prefix: bool,
    pub typos: bool,
    pub bbox: Option<[f64; 4]>,
    pub within: Option<u64>,
    pub kinds: Vec<Kind>,
    pub tags: Vec<String>,
    pub limit: usize,
    pub geometry: GeometryMode,
    pub category_include_threshold: Option<f32>,
}

impl SearchParams {
    /// Parses a query string, reporting every invalid parameter.
    ///
    /// # Errors
    /// A 400 problem naming each missing or invalid parameter.
    pub fn parse(raw: &str) -> Result<Self, Problem> {
        let mut values: HashMap<String, String> = HashMap::new();
        let mut invalid: Vec<(String, String)> = Vec::new();
        for (name, value) in form_urlencoded::parse(raw.as_bytes()) {
            if values.insert(name.to_string(), value.to_string()).is_some() {
                invalid.push((name.to_string(), "given more than once".into()));
            }
        }
        let mut get = |name: &str| values.remove(name);
        let mut fail = |name: &str, reason: &str| invalid.push((name.to_string(), reason.to_string()));

        let q = get("q").unwrap_or_default();
        if q.trim().is_empty() {
            fail("q", "required, and not just whitespace");
        } else if q.chars().count() > 500 {
            fail("q", "longer than 500 characters");
        }

        let lang = get("lang");
        if let Some(lang) = &lang {
            let valid = lang.split('-').enumerate().all(|(i, part)| {
                let len_ok = if i == 0 { (2..=8).contains(&part.len()) } else { (1..=8).contains(&part.len()) };
                len_ok && part.chars().all(|c| c.is_ascii_alphanumeric()) && (i > 0 || part.chars().all(|c| c.is_ascii_alphabetic()))
            });
            if !valid {
                fail("lang", "not a BCP 47 language tag");
            }
        }

        let mut boolean = |name: &str| match get(name).as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                fail(name, "must be true or false");
                false
            }
        };
        let prefix = boolean("prefix");
        let typos = boolean("typos");

        let bbox = get("bbox").and_then(|value| match parse_bbox(&value) {
            Ok(bbox) => Some(bbox),
            Err(reason) => {
                fail("bbox", reason);
                None
            }
        });

        let within = get("within").and_then(|value| {
            let id = value.strip_prefix("wof:").and_then(|id| id.parse::<u64>().ok());
            if id.is_none() {
                fail("within", "must be wof:<id>");
            }
            id
        });

        let mut kinds = Vec::new();
        if let Some(value) = get("kind") {
            for part in value.split(',') {
                match Kind::parse(part) {
                    Some(kind) if !kinds.contains(&kind) => kinds.push(kind),
                    Some(_) => fail("kind", "lists a kind more than once"),
                    None => fail("kind", "must list poi, address or admin"),
                }
            }
        }

        let mut tags = Vec::new();
        if let Some(value) = get("tags") {
            for tag in value.split(',') {
                match tag.split_once('=') {
                    Some((k, v)) if !k.is_empty() && !v.is_empty() && !v.contains('=') => tags.push(tag.to_string()),
                    _ => fail("tags", "must list key=value tags"),
                }
            }
        }

        let limit = match get("limit") {
            None => DEFAULT_LIMIT,
            Some(value) => match value.parse::<usize>() {
                Ok(limit) if (1..=MAX_LIMIT).contains(&limit) => limit,
                _ => {
                    fail("limit", "must be an integer from 1 to 50");
                    DEFAULT_LIMIT
                }
            },
        };

        let geometry = match get("geometry").as_deref() {
            None | Some("shape") => GeometryMode::Shape,
            Some("point") => GeometryMode::Point,
            Some(_) => {
                fail("geometry", "must be shape or point");
                GeometryMode::Shape
            }
        };

        let category_include_threshold = get("category_include_threshold").and_then(|value| match value.parse::<f32>() {
            Ok(t) if (0.0..=1.0).contains(&t) => Some(t),
            _ => {
                fail("category_include_threshold", "must be a number from 0 to 1");
                None
            }
        });

        if !invalid.is_empty() {
            return Err(Problem::invalid_params(invalid));
        }
        Ok(Self { q, lang, prefix, typos, bbox, within, kinds, tags, limit, geometry, category_include_threshold })
    }
}

fn parse_bbox(value: &str) -> Result<[f64; 4], &'static str> {
    let numbers: Vec<f64> = value
        .split(',')
        .map(|n| n.trim().parse::<f64>().map_err(|_| "must be four numbers"))
        .collect::<Result<_, _>>()?;
    let [min_lon, min_lat, max_lon, max_lat] = numbers[..] else {
        return Err("must be four numbers: min_lon,min_lat,max_lon,max_lat");
    };
    if !numbers.iter().all(|n| n.is_finite()) {
        return Err("must be four numbers");
    }
    if !(-180.0..=180.0).contains(&min_lon) || !(-180.0..=180.0).contains(&max_lon) {
        return Err("longitudes must be from -180 to 180");
    }
    if !(-90.0..=90.0).contains(&min_lat) || !(-90.0..=90.0).contains(&max_lat) {
        return Err("latitudes must be from -90 to 90");
    }
    if min_lat > max_lat {
        return Err("min_lat must not be greater than max_lat");
    }
    Ok([min_lon, min_lat, max_lon, max_lat])
}

pub async fn search(RawQuery(raw): RawQuery, State(state): State<Arc<AppState>>) -> Response {
    let params = match SearchParams::parse(raw.as_deref().unwrap_or_default()) {
        Ok(params) => params,
        Err(problem) => return problem.into_response(),
    };
    match run_search(&params, &state).await {
        Ok(body) => Json(body).into_response(),
        Err(e) => Problem::internal(&e).into_response(),
    }
}

async fn run_search(params: &SearchParams, state: &AppState) -> anyhow::Result<Value> {
    let lang = params.lang.as_deref();
    // The model normalizes on its own and keeps accents, so it gets the raw query.
    let model = lang.and_then(|lang| state.categories.get(lang));
    let categories = model.map(|model| model.understand(&params.q));
    let threshold = params.category_include_threshold.unwrap_or(state.category_include_threshold);
    let request = SearchRequest {
        text: params.q.trim(),
        prefix: params.prefix,
        typos: params.typos,
        tags: params.tags.clone(),
        categories: categories.as_ref().map(|c| (c, c.mode(threshold))),
        bbox: params.bbox,
        within: params.within,
        kinds: params.kinds.clone(),
        limit: params.limit,
    };
    let results = state.index.search(&request).await?;

    let kinds = if params.kinds.is_empty() { Kind::ALL.to_vec() } else { params.kinds.clone() };
    let query = json!({
        "text": params.q,
        "lang": params.lang,
        "prefix": params.prefix,
        "typos": params.typos,
        "bbox": params.bbox,
        "within": params.within.map(|id| format!("wof:{id}")),
        "kind": kinds,
        "tags": params.tags,
        "limit": params.limit,
        "geometry": params.geometry,
        "category_language": model.map(|m| m.language()),
        "categories": categories.as_ref().map(categories_json),
    });
    let results: Vec<Value> =
        results.hits.iter().map(|hit| result_json(hit, &results.admins, lang, params.geometry)).collect();
    Ok(json!({ "query": query, "results": results }))
}

fn categories_json(categories: &QueryCategories) -> Value {
    let scored = |values: &[(String, f32)]| -> Vec<Value> {
        values.iter().map(|(value, p)| json!({ "value": value, "confidence": p.clamp(0.0, 1.0) })).collect()
    };
    json!({
        "categories": scored(&categories.categories),
        "food": categories.food.clamp(0.0, 1.0),
        "cuisines": scored(&categories.cuisines),
        "diets": scored(&categories.diets),
    })
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
    Value::Array(std::iter::once(polygon.exterior()).chain(polygon.interiors()).map(ring).collect())
}

/// A geometry as GeoJSON. Stored geometries are only these kinds.
fn geometry_json(geometry: &Geometry<f64>) -> Value {
    match geometry {
        Geometry::Point(p) => json!({ "type": "Point", "coordinates": position(p.0) }),
        Geometry::LineString(l) => json!({ "type": "LineString", "coordinates": ring(l) }),
        Geometry::Polygon(p) => json!({ "type": "Polygon", "coordinates": polygon(p) }),
        Geometry::MultiPolygon(m) => {
            json!({ "type": "MultiPolygon", "coordinates": m.0.iter().map(polygon).collect::<Vec<_>>() })
        }
        other => geometry_json(&Geometry::Point(
            airmail::geometry::representative_point(other).unwrap_or_else(|| geo::Point::new(0.0, 0.0)),
        )),
    }
}

/// Name keys to show when a POI has no `name`, most preferred first.
const FALLBACK_NAME_KEYS: [&str; 4] = ["official_name", "alt_name", "short_name", "loc_name"];

fn result_json(hit: &SearchHit, admins: &HashMap<u64, WofPlace>, lang: Option<&str>, mode: GeometryMode) -> Value {
    let id = if let Some((poi, _)) = hit.poi {
        format!("poi:{poi}")
    } else if let Some(wof) = &hit.wof {
        format!("wof:{}", wof.id)
    } else if let Some(osm) = &hit.osm {
        format!("osm:{}/{}", osm.osm_type.as_str(), osm.id)
    } else {
        String::new()
    };
    let name = match hit.kind {
        Kind::Admin => hit.wof.as_ref().map(|w| w.display_name(lang).to_string()),
        Kind::Poi => osm_display_name(&hit.tags, lang)
            .or_else(|| FALLBACK_NAME_KEYS.iter().find_map(|key| hit.tag(key)))
            .or_else(|| hit.tags.iter().find(|(k, _)| k.contains("name")).map(|(_, v)| v.as_str()))
            .map(str::to_string),
        Kind::Address => None,
    };
    let address_parts = [("housenumber", "addr:housenumber"), ("street", "addr:street"), ("unit", "addr:unit"), ("postcode", "addr:postcode")];
    let address = if address_parts.iter().any(|(_, key)| hit.tag(key).is_some()) {
        Value::Object(address_parts.iter().map(|(field, key)| (field.to_string(), json!(hit.tag(key)))).collect())
    } else {
        Value::Null
    };
    let mut containing: Vec<&WofPlace> = hit.admin_ids.iter().filter_map(|id| admins.get(id)).collect();
    containing.sort_by_key(|place| (placetype_rank(&place.placetype), place.id));
    containing.dedup_by_key(|place| place.id);
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
    let center = position(hit.center.0);
    let (geometry, bbox) = match mode {
        GeometryMode::Shape => (
            geometry_json(&hit.geometry),
            airmail::geometry::bbox(&hit.geometry).map(|b| b.map(round)).unwrap_or([0.0; 4]),
        ),
        GeometryMode::Point => {
            let (x, y) = (round(hit.center.x()), round(hit.center.y()));
            (json!({ "type": "Point", "coordinates": [x, y] }), [x, y, x, y])
        }
    };
    let tags: serde_json::Map<String, Value> = hit.tags.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({
        "id": id,
        "kind": hit.kind,
        "name": name,
        "center": center,
        "address": address,
        "admins": admins,
        "importance": hit.importance.clamp(0.0, 1.0),
        "score": hit.score.max(0.0),
        "poi": hit.poi.map(|(id, version)| json!({ "id": id, "version": version })),
        "osm": hit.osm.map(|osm| json!({
            "type": osm.osm_type.as_str(),
            "id": osm.id,
            "version": osm.version,
            "tags": tags,
        })),
        "wof": hit.wof.as_ref().map(|wof| json!({
            "id": wof.id,
            "placetype": wof.placetype,
            "population": wof.population,
        })),
        "feature": {
            "type": "Feature",
            "bbox": bbox,
            "geometry": geometry,
            "properties": null,
        },
    })
}

pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "index_schema_version": state.schema_version,
        "documents": state.documents,
        "category_languages": state.categories.languages(),
    }))
}

pub async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/yaml")], OPENAPI)
}

pub async fn not_found() -> impl IntoResponse {
    Problem::new(StatusCode::NOT_FOUND, "Not Found", None)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parses_defaults() {
        let params = SearchParams::parse("q=seattle").unwrap();
        assert_eq!(params.q, "seattle");
        assert!(!params.prefix && !params.typos);
        assert_eq!(params.limit, DEFAULT_LIMIT);
        assert_eq!(params.geometry, GeometryMode::Shape);
        assert!(params.kinds.is_empty());
    }

    #[test]
    fn parses_everything() {
        let params = SearchParams::parse(
            "q=pho&lang=en-US&prefix=true&typos=true&bbox=-122.4,47.5,-122.2,47.7&within=wof:101730401\
             &kind=poi,admin&tags=cuisine%3Dvietnamese&limit=50&geometry=point&category_include_threshold=0.5",
        )
        .unwrap();
        assert_eq!(params.bbox, Some([-122.4, 47.5, -122.2, 47.7]));
        assert_eq!(params.within, Some(101_730_401));
        assert_eq!(params.kinds, vec![Kind::Poi, Kind::Admin]);
        assert_eq!(params.tags, vec!["cuisine=vietnamese"]);
        assert_eq!(params.limit, 50);
        assert_eq!(params.geometry, GeometryMode::Point);
    }

    #[test]
    fn reports_every_invalid_parameter() {
        let problem = SearchParams::parse("q=&limit=0&bbox=1,2,3&within=4&prefix=yes&kind=place").unwrap_err();
        let names: Vec<&str> = problem.invalid_params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["q", "prefix", "bbox", "within", "kind", "limit"]);
    }

    #[test]
    fn accepts_boxes_across_the_antimeridian() {
        assert!(SearchParams::parse("q=fiji&bbox=177,-19,-178,-16").is_ok());
        assert!(SearchParams::parse("q=x&bbox=0,10,1,5").is_err());
    }
}
