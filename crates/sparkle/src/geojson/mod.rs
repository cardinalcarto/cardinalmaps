//! GeoJSON sources, cut into tiles that go through the same processing as vector tiles.
//!
//! [`GeoJsonIndex`] does what MapLibre's GeoJSON worker does with geojson-vt and
//! supercluster: features are projected and simplified once and clipped to tiles on demand
//! (see [`tiler`]), and with `cluster` set, point features are clustered per zoom level
//! (see [`cluster`]). [`TileProcessor::process_geojson`](crate::TileProcessor::process_geojson)
//! turns a tile of it into buckets; nothing is encoded to MLT on the way.
//!
//! Every feature is in one source layer, which style layers don't name. As in MapLibre,
//! tiles are [`GeoJsonIndex::EXTENT`] units across.

mod cluster;
mod kdbush;
mod tiler;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use maplibre_expr::{EvaluationContext, Expr, Type, Value};
use mlt_core::geo_types::Geometry;
use serde_json::Value as Json;

pub use self::cluster::ClusterChild;
use self::cluster::{ClusterOptions, ClusterProperties, Supercluster};
use self::tiler::{Tiler, TilerOptions};
use crate::state::feature_id;
use crate::style::GeoJsonSource;
use crate::{Error, TileId};

/// A feature's properties, ready for expression evaluation.
pub type Properties = BTreeMap<String, Value>;

/// One feature of a GeoJSON tile, in tile coordinates.
#[derive(Debug, Clone)]
pub struct TileFeature {
    pub id: Option<u64>,
    pub geometry: Geometry<i32>,
    pub properties: Arc<Properties>,
    /// With `lineMetrics`, where this piece of a line starts and ends along the whole line,
    /// as fractions of its length (MapLibre's `mapbox_clip_start` and `mapbox_clip_end`).
    pub line_clip: Option<[f64; 2]>,
}

/// Geometry in `[longitude, latitude]` degrees, as GeoJSON has it.
#[derive(Debug, Clone)]
pub(crate) enum LngLatGeometry {
    Point([f64; 2]),
    MultiPoint(Vec<[f64; 2]>),
    LineString(Vec<[f64; 2]>),
    MultiLineString(Vec<Vec<[f64; 2]>>),
    Polygon(Vec<Vec<[f64; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
}

impl LngLatGeometry {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Point(_) | Self::MultiPoint(_) => "Point",
            Self::LineString(_) | Self::MultiLineString(_) => "LineString",
            Self::Polygon(_) | Self::MultiPolygon(_) => "Polygon",
        }
    }
}

struct SourceFeature {
    id: Option<u64>,
    properties: Arc<Properties>,
    geometry: LngLatGeometry,
}

/// A GeoJSON source's data, ready to be cut into tiles. Building it does all the work that
/// doesn't depend on the tile; [`Self::tile`] can then be called from any thread.
pub struct GeoJsonIndex {
    features: Vec<SourceFeature>,
    tiler: Tiler,
    clusters: Option<Supercluster>,
    max_zoom: u8,
}

impl GeoJsonIndex {
    /// Tile coordinates across a GeoJSON tile.
    pub const EXTENT: u32 = 8192;

    /// Index the source's data: inline GeoJSON, or the path of a local GeoJSON file (as
    /// its `data` URL, `file://` or not), relative to `base_dir` if it is relative.
    pub fn load(source: &GeoJsonSource, base_dir: Option<&Path>) -> Result<Self, Error> {
        match &source.data {
            Json::String(url) => {
                let path = Path::new(url.strip_prefix("file://").unwrap_or(url));
                let path = match base_dir {
                    Some(base) if path.is_relative() => base.join(path),
                    _ => path.to_owned(),
                };
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| Error::GeoJson(format!("reading {}: {e}", path.display())))?;
                Self::new(source, &serde_json::from_str(&text)?)
            }
            data => Self::new(source, data),
        }
    }

    /// Index GeoJSON `data` with the options of `source` (whose own `data` is ignored).
    pub fn new(source: &GeoJsonSource, data: &Json) -> Result<Self, Error> {
        let promote_id = promote_key(source.promote_id.as_ref());
        let filter = source.filter.as_ref().map(parse_filter).transpose()?;
        let mut features = Vec::new();
        // Parts of a GeometryCollection share their feature's properties, and its index.
        let mut index: Option<u64> = None;
        let mut last = std::ptr::null();
        parse_root(data, &mut |id, properties, geometry| {
            if Arc::as_ptr(&properties) != last {
                last = Arc::as_ptr(&properties);
                index = Some(index.map_or(0, |i| i + 1));
            }
            let id = if source.generate_id {
                index
            } else if let Some(key) = promote_id {
                properties.get(key).and_then(feature_id)
            } else {
                id.map(Value::from_json).as_ref().and_then(feature_id)
            };
            features.push(SourceFeature {
                id,
                properties,
                geometry,
            });
        })?;
        if let Some(filter) = &filter {
            let mut ctx = EvaluationContext::new().with_zoom(0.0);
            features.retain(|f| {
                ctx.feature.properties.clone_from(&f.properties);
                ctx.feature.id = f.id.map(|id| Value::Number(id as f64));
                ctx.feature.geometry_type = Some(f.geometry.type_name().to_owned());
                matches!(maplibre_expr::evaluate(filter, &ctx), Ok(Value::Bool(true)))
            });
        }

        let units = f64::from(Self::EXTENT) / 512.0;
        let is_point = |f: &SourceFeature| {
            matches!(
                f.geometry,
                LngLatGeometry::Point(_) | LngLatGeometry::MultiPoint(_)
            )
        };
        let clusters = if source.cluster {
            let properties = source
                .cluster_properties
                .as_ref()
                .map(ClusterProperties::parse)
                .transpose()?;
            let mut points = Vec::new();
            for f in features.iter().filter(|f| is_point(f)) {
                let positions = match &f.geometry {
                    LngLatGeometry::Point(p) => std::slice::from_ref(p),
                    LngLatGeometry::MultiPoint(ps) => ps.as_slice(),
                    _ => unreachable!(),
                };
                for &p in positions {
                    points.push((f.id, f.properties.clone(), lng_lat_to_world(p)));
                }
            }
            Some(Supercluster::new(
                points,
                ClusterOptions {
                    max_zoom: source
                        .cluster_max_zoom
                        .unwrap_or(source.max_zoom.saturating_sub(1)),
                    min_points: source.cluster_min_points.max(2),
                    radius: source.cluster_radius,
                    extent: Self::EXTENT,
                    properties,
                },
            ))
        } else {
            None
        };
        // With clustering, points only appear through the clusters.
        let tiled = features
            .iter()
            .enumerate()
            .filter(|(_, f)| clusters.is_none() || !is_point(f))
            .map(|(i, f)| (i, &f.geometry));
        let tiler = Tiler::new(
            tiled,
            TilerOptions {
                extent: Self::EXTENT,
                buffer: (f64::from(source.buffer) * units).round() as u32,
                tolerance: source.tolerance * units,
                max_zoom: source.max_zoom,
                line_metrics: source.line_metrics,
            },
        );
        Ok(Self {
            features,
            tiler,
            clusters,
            max_zoom: source.max_zoom,
        })
    }

    /// The source's `maxzoom`: deeper tiles are drawn magnified from this level's.
    pub fn max_zoom(&self) -> u8 {
        self.max_zoom
    }

    /// How many features the data has (after the source's `filter`).
    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    /// The features of one tile, in tile coordinates of [`Self::EXTENT`] units with a
    /// buffer around the tile.
    pub fn tile(&self, id: TileId) -> Vec<TileFeature> {
        let mut tiled = Vec::new();
        self.tiler.tile(id, &mut tiled);
        let mut out: Vec<TileFeature> = tiled
            .into_iter()
            .map(|(source, mut f)| {
                let feature = &self.features[source];
                f.id = feature.id;
                f.properties = feature.properties.clone();
                f
            })
            .collect();
        if let Some(clusters) = &self.clusters {
            clusters.tile(id, &mut out);
        }
        out
    }

    /// What a cluster (by its `cluster_id`) splits into one zoom level deeper, or `None`
    /// if there's no such cluster.
    pub fn cluster_children(&self, cluster_id: u64) -> Option<Vec<ClusterChild>> {
        self.clusters.as_ref()?.children(cluster_id)
    }

    /// The zoom level at which a cluster breaks apart into more than one feature.
    pub fn cluster_expansion_zoom(&self, cluster_id: u64) -> Option<u8> {
        self.clusters.as_ref()?.expansion_zoom(cluster_id)
    }
}

impl std::fmt::Debug for GeoJsonIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeoJsonIndex")
            .field("features", &self.features.len())
            .field("clustered", &self.clusters.is_some())
            .field("max_zoom", &self.max_zoom)
            .finish()
    }
}

/// The property `promoteId` names. GeoJSON sources have a single unnamed source layer, so
/// the per-layer object form uses its only entry.
fn promote_key(json: Option<&Json>) -> Option<&str> {
    match json? {
        Json::String(key) => Some(key),
        Json::Object(by_layer) => by_layer.values().next()?.as_str(),
        _ => None,
    }
}

fn parse_filter(json: &Json) -> Result<Expr, Error> {
    let invalid = |e: String| Error::GeoJson(format!("filter: {e}"));
    let expr = maplibre_expr::parse_filter(json).map_err(|e| invalid(e.to_string()))?;
    maplibre_expr::typecheck(&expr, Some(&Type::Boolean), false).map_err(|e| invalid(e.to_string()))
}

fn lng_lat_to_world([lng, lat]: [f64; 2]) -> [f64; 2] {
    let sin = lat.to_radians().sin();
    let y = 0.5 - 0.25 * ((1.0 + sin) / (1.0 - sin)).ln() / std::f64::consts::PI;
    [lng / 360.0 + 0.5, y.clamp(0.0, 1.0)]
}

type Sink<'a> = dyn FnMut(Option<&Json>, Arc<Properties>, LngLatGeometry) + 'a;

/// Every feature of a FeatureCollection, Feature or bare geometry.
fn parse_root(json: &Json, out: &mut Sink<'_>) -> Result<(), Error> {
    match json.get("type").and_then(Json::as_str) {
        Some("FeatureCollection") => {
            let features = json
                .get("features")
                .and_then(Json::as_array)
                .ok_or_else(|| Error::GeoJson("FeatureCollection without features".into()))?;
            for feature in features {
                parse_feature(feature, out)?;
            }
            Ok(())
        }
        Some("Feature") => parse_feature(json, out),
        Some(_) => {
            let properties = Arc::new(Properties::new());
            parse_geometry(json, &mut |g| out(None, properties.clone(), g))
        }
        None => Err(Error::GeoJson("not a GeoJSON object".into())),
    }
}

fn parse_feature(json: &Json, out: &mut Sink<'_>) -> Result<(), Error> {
    let properties: Properties = match json.get("properties") {
        Some(Json::Object(p)) => p
            .iter()
            .map(|(k, v)| (k.clone(), Value::from_json(v)))
            .collect(),
        _ => Properties::new(),
    };
    let properties = Arc::new(properties);
    let id = json.get("id");
    match json.get("geometry") {
        // A feature without geometry has nothing to draw.
        None | Some(Json::Null) => Ok(()),
        Some(geometry) => parse_geometry(geometry, &mut |g| out(id, properties.clone(), g)),
    }
}

/// Parse a geometry; a GeometryCollection gives each of its members.
fn parse_geometry(json: &Json, out: &mut dyn FnMut(LngLatGeometry)) -> Result<(), Error> {
    let ty = json.get("type").and_then(Json::as_str).unwrap_or_default();
    if ty == "GeometryCollection" {
        for g in json
            .get("geometries")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            parse_geometry(g, out)?;
        }
        return Ok(());
    }
    let coordinates = json
        .get("coordinates")
        .ok_or_else(|| Error::GeoJson(format!("{ty} without coordinates")))?;
    let invalid = || Error::GeoJson(format!("invalid {ty} coordinates"));
    let geometry = match ty {
        "Point" => LngLatGeometry::Point(position(coordinates).ok_or_else(invalid)?),
        "MultiPoint" => LngLatGeometry::MultiPoint(positions(coordinates).ok_or_else(invalid)?),
        "LineString" => LngLatGeometry::LineString(positions(coordinates).ok_or_else(invalid)?),
        "MultiLineString" => {
            LngLatGeometry::MultiLineString(nested(coordinates, positions).ok_or_else(invalid)?)
        }
        "Polygon" => LngLatGeometry::Polygon(nested(coordinates, positions).ok_or_else(invalid)?),
        "MultiPolygon" => LngLatGeometry::MultiPolygon(
            nested(coordinates, |p| nested(p, positions)).ok_or_else(invalid)?,
        ),
        other => return Err(Error::GeoJson(format!("unknown geometry type {other:?}"))),
    };
    out(geometry);
    Ok(())
}

fn position(json: &Json) -> Option<[f64; 2]> {
    let a = json.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn positions(json: &Json) -> Option<Vec<[f64; 2]>> {
    json.as_array()?.iter().map(position).collect()
}

fn nested<T>(json: &Json, item: impl Fn(&Json) -> Option<T>) -> Option<Vec<T>> {
    json.as_array()?.iter().map(item).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::{Source, Style};

    fn source(options: Json) -> GeoJsonSource {
        let mut json = json!({"type": "geojson", "data": ""});
        json.as_object_mut()
            .unwrap()
            .extend(options.as_object().unwrap().clone());
        let style = Style::from_json(&json!({"sources": {"g": json}, "layers": []})).unwrap();
        let Source::GeoJson(source) = &style.sources["g"] else {
            panic!()
        };
        source.clone()
    }

    fn collection() -> Json {
        json!({"type": "FeatureCollection", "features": [
            {"type": "Feature", "id": 7, "properties": {"name": "a", "key": "x9"},
             "geometry": {"type": "Point", "coordinates": [-122.33, 47.6]}},
            {"type": "Feature", "id": "12", "properties": {"name": "b", "key": 5},
             "geometry": {"type": "LineString", "coordinates": [[-122.4, 47.6], [-122.3, 47.65]]}},
            {"type": "Feature", "properties": {"name": "c"},
             "geometry": {"type": "GeometryCollection", "geometries": [
                {"type": "Point", "coordinates": [-122.3, 47.6]},
                {"type": "Polygon", "coordinates": [[[-122.4, 47.5], [-122.3, 47.5], [-122.3, 47.6], [-122.4, 47.5]]]}
             ]}},
            {"type": "Feature", "properties": {}, "geometry": null},
        ]})
    }

    #[test]
    fn ids() {
        let index = GeoJsonIndex::new(&source(json!({})), &collection()).unwrap();
        let ids: Vec<_> = index.features.iter().map(|f| f.id).collect();
        assert_eq!(ids, [Some(7), Some(12), None, None]);

        let index = GeoJsonIndex::new(&source(json!({"generateId": true})), &collection()).unwrap();
        let ids: Vec<_> = index.features.iter().map(|f| f.id).collect();
        // Both parts of the GeometryCollection are the same feature.
        assert_eq!(ids, [Some(0), Some(1), Some(2), Some(2)]);

        let index = GeoJsonIndex::new(&source(json!({"promoteId": "key"})), &collection()).unwrap();
        assert_eq!(index.features[1].id, Some(5));
        assert_eq!(
            index.features[0].id,
            feature_id(&Value::String("x9".into()))
        );
        assert_eq!(index.features[2].id, None);
    }

    #[test]
    fn filter_drops_features() {
        let index = GeoJsonIndex::new(
            &source(json!({"filter": ["==", ["geometry-type"], "Point"]})),
            &collection(),
        )
        .unwrap();
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn tiles_carry_properties() {
        let index = GeoJsonIndex::new(&source(json!({})), &collection()).unwrap();
        // Seattle at zoom 10.
        let features = index.tile(TileId::new(10, 163, 357));
        let names: Vec<_> = features
            .iter()
            .map(|f| f.properties["name"].to_string())
            .collect();
        assert_eq!(names, ["a", "b", "c", "c"]);
        assert!(matches!(features[3].geometry, Geometry::Polygon(_)));
        assert!(index.tile(TileId::new(10, 0, 0)).is_empty());
    }

    #[test]
    fn clustered_points() {
        let points: Vec<Json> = (0..20)
            .map(|i| {
                json!({"type": "Feature", "properties": {"n": i},
                       "geometry": {"type": "Point", "coordinates": [-122.3 + f64::from(i) * 1e-4, 47.6]}})
            })
            .chain([json!({"type": "Feature", "properties": {},
                    "geometry": {"type": "LineString", "coordinates": [[-122.4, 47.6], [-122.3, 47.6]]}})])
            .collect();
        let data = json!({"type": "FeatureCollection", "features": points});
        let index = GeoJsonIndex::new(
            &source(json!({"cluster": true, "clusterProperties": {"total": ["+", ["get", "n"]]}})),
            &data,
        )
        .unwrap();
        let features = index.tile(TileId::new(8, 40, 89));
        let cluster = features
            .iter()
            .find(|f| f.properties.contains_key("cluster"))
            .expect("a cluster");
        assert_eq!(cluster.properties["point_count"], Value::Number(20.0));
        assert_eq!(cluster.properties["total"], Value::Number(190.0));
        assert!(
            features
                .iter()
                .any(|f| matches!(f.geometry, Geometry::LineString(_))),
            "lines are tiled alongside clusters"
        );
        let id = cluster.id.unwrap();
        assert!(index.cluster_expansion_zoom(id).unwrap() > 8);
    }

    #[test]
    fn loads_files_relative_to_base() {
        let dir = std::env::temp_dir().join(format!("sparkle-geojson-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("data.geojson"), collection().to_string()).unwrap();
        let mut s = source(json!({}));
        s.data = json!("data.geojson");
        let index = GeoJsonIndex::load(&s, Some(&dir)).unwrap();
        assert_eq!(index.len(), 4);
        s.data = json!("missing.geojson");
        assert!(GeoJsonIndex::load(&s, Some(&dir)).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
