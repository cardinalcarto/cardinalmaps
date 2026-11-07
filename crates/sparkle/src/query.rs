//! Which features are drawn at a point, like MapLibre's `queryRenderedFeatures` for a
//! point, answered on the CPU from the tile's own geometry.
//!
//! Processing a tile keeps its source data in a [`FeatureIndex`] (the MLT bytes, or the
//! GeoJSON index the tile was cut from). A query decodes the layers that queryable style
//! layers draw from, applies their filters, and tests each feature's geometry the way the
//! layer type draws it: fills and extrusions contain the point, lines and circles are
//! within their width or radius of it, with `*-translate` applied. Features whose bounds
//! are farther from the point than any layer reaches are skipped before their filters are
//! evaluated. Symbols, heatmaps and raster layers aren't queryable this way.

use std::collections::BTreeMap;
use std::sync::Arc;

use maplibre_expr::{EvaluationContext, Value};
use mlt_core::geo_types::{Coord, Geometry, LineString, Polygon};
use mlt_core::{Decoder, Layer, LendingIterator, Parser};

use crate::Error;
use crate::geojson::GeoJsonIndex;
use crate::state::{FeatureStates, StateMap};
use crate::style::{Dependencies, LayerKind, PropertyDeps, PropertyValue, Style, StyleLayer};
use crate::tile::TileId;
use crate::tile::eval::{FeatureContext, geometry_type_name};

/// A processed tile's source data, kept for queries.
#[derive(Clone)]
pub struct FeatureIndex {
    tile: TileId,
    data: IndexData,
}

#[derive(Clone)]
enum IndexData {
    Mlt(Arc<[u8]>),
    GeoJson(Arc<GeoJsonIndex>),
}

impl std::fmt::Debug for FeatureIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match &self.data {
            IndexData::Mlt(data) => format!("MLT, {} bytes", data.len()),
            IndexData::GeoJson(_) => "GeoJSON".to_owned(),
        };
        write!(f, "FeatureIndex({:?}, {kind})", self.tile)
    }
}

/// Where to look, for [`FeatureIndex::query_point`].
#[derive(Debug, Clone)]
pub struct PointQuery {
    /// The point as a fraction of the tile: `[0, 0]` at its top left corner, `[1, 1]` at
    /// its bottom right.
    pub position: [f64; 2],
    /// Logical pixels the tile spans at the camera's zoom, to convert pixel widths.
    pub tile_size: f64,
    /// The camera's zoom, which widths and radii are evaluated at.
    pub zoom: f64,
    /// How many logical pixels away a feature may be and still count, as a touch target.
    pub tolerance: f64,
    /// Only query these layers (by id); every queryable layer when empty.
    pub layers: Vec<String>,
}

/// A feature found by a query.
#[derive(Debug, Clone, PartialEq)]
pub struct QueriedFeature {
    /// Index of the style layer that draws it.
    pub layer: usize,
    pub source: String,
    pub source_layer: Option<String>,
    pub id: Option<u64>,
    /// `"Point"`, `"LineString"` or `"Polygon"`.
    pub geometry_type: &'static str,
    pub properties: BTreeMap<String, Value>,
    /// Its feature state when queried.
    pub state: StateMap,
}

/// A style layer a query tests features against, with a tolerance in tile units.
struct Target<'a> {
    index: usize,
    layer: &'a StyleLayer,
}

impl FeatureIndex {
    pub(crate) fn mlt(tile: TileId, data: &[u8]) -> Self {
        Self {
            tile,
            data: IndexData::Mlt(data.into()),
        }
    }

    pub(crate) fn geojson(tile: TileId, index: Arc<GeoJsonIndex>) -> Self {
        Self {
            tile,
            data: IndexData::GeoJson(index),
        }
    }

    pub fn tile(&self) -> TileId {
        self.tile
    }

    /// The properties of the feature with `id` in this tile, in `source_layer` for a
    /// vector source.
    pub fn properties(
        &self,
        style: &Style,
        source: &str,
        source_layer: Option<&str>,
        id: u64,
    ) -> Result<Option<BTreeMap<String, Value>>, Error> {
        match &self.data {
            IndexData::Mlt(data) => {
                let mut decoder = Decoder::default();
                for layer in Parser::default().parse_layers(data)? {
                    let Layer::Tag01(layer) = layer else { continue };
                    if Some(layer.name()) != source_layer {
                        continue;
                    }
                    let name = layer.name().to_owned();
                    let layer = layer.decode_all(&mut decoder)?;
                    let promote = style.source(source).and_then(|s| s.promote_id(&name));
                    let all = Dependencies {
                        properties: PropertyDeps::All,
                        ..Dependencies::default()
                    };
                    let zoom = f64::from(self.tile.z);
                    let mut ctx = FeatureContext::new(&layer, &all, self.tile, zoom);
                    let mut features = layer.iter_features();
                    while let Some(feature) = features.next() {
                        let feature = feature?;
                        let found = match promote {
                            Some(key) => {
                                ctx.load(&feature);
                                ctx.promote_id(key) == Some(id)
                            }
                            None => feature.id() == Some(id),
                        };
                        if found {
                            ctx.load(&feature);
                            return Ok(Some(ctx.context().feature.properties.clone()));
                        }
                    }
                }
                Ok(None)
            }
            IndexData::GeoJson(index) => Ok(index
                .tile(self.tile)
                .into_iter()
                .find(|f| f.id == Some(id))
                .map(|f| (*f.properties).clone())),
        }
    }

    /// The features of `source` drawn at a point of this tile, topmost first: by layer
    /// from the top of the style, then within a layer the last drawn first.
    pub fn query_point(
        &self,
        style: &Style,
        source: &str,
        query: &PointQuery,
        states: Option<&FeatureStates>,
    ) -> Result<Vec<QueriedFeature>, Error> {
        let targets = |source_layer: Option<&str>| -> Vec<Target<'_>> {
            style
                .layers
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    l.source.as_deref() == Some(source)
                        && (source_layer.is_none() || l.source_layer.as_deref() == source_layer)
                        && l.is_visible_at(query.zoom as f32)
                        && is_queryable(&l.kind)
                        && (query.layers.is_empty() || query.layers.contains(&l.id))
                })
                .map(|(index, layer)| Target { index, layer })
                .collect()
        };
        let mut hits = Vec::new();
        match &self.data {
            IndexData::Mlt(data) => {
                let mut decoder = Decoder::default();
                for layer in Parser::default().parse_layers(data)? {
                    let Layer::Tag01(layer) = layer else { continue };
                    let name = layer.name().to_owned();
                    let targets = targets(Some(&name));
                    if targets.is_empty() {
                        continue;
                    }
                    let extent = f64::from(layer.extent().get());
                    let layer = layer.decode_all(&mut decoder)?;
                    let promote = style.source(source).and_then(|s| s.promote_id(&name));
                    let mut deps = Dependencies::default();
                    targets
                        .iter()
                        .for_each(|t| deps.merge(&t.layer.feature_dependencies()));
                    if let Some(key) = promote {
                        deps.properties.insert(key);
                    }
                    deps.geometry_type = true;
                    let all = Dependencies {
                        properties: PropertyDeps::All,
                        ..Dependencies::default()
                    };
                    let mut ctx =
                        FeatureContext::new(&layer, &deps, self.tile, f64::from(self.tile.z));
                    let mut full = FeatureContext::new(&layer, &all, self.tile, query.zoom);
                    let tester = Tester::new(query, extent, states, source, Some(&name));
                    let reach = tester.max_reach(&targets);
                    let mut features = layer.iter_features();
                    let mut order = 0;
                    while let Some(feature) = features.next() {
                        let feature = feature?;
                        order += 1;
                        let geometry = feature.geometry();
                        if !tester.near(geometry, reach) {
                            continue;
                        }
                        ctx.load(&feature);
                        let id = match promote {
                            Some(key) => ctx.promote_id(key),
                            None => feature.id(),
                        };
                        let found = tester.test(&targets, ctx.context_mut(), id, geometry);
                        if found.is_empty() {
                            continue;
                        }
                        full.load(&feature);
                        let properties = full.context().feature.properties.clone();
                        for (index, state) in found {
                            hits.push((
                                index,
                                order,
                                QueriedFeature {
                                    layer: index,
                                    source: source.to_owned(),
                                    source_layer: Some(name.clone()),
                                    id,
                                    geometry_type: geometry_type_name(geometry),
                                    properties: properties.clone(),
                                    state,
                                },
                            ));
                        }
                    }
                }
            }
            IndexData::GeoJson(index) => {
                let targets = targets(None);
                if !targets.is_empty() {
                    let mut deps = Dependencies::default();
                    targets
                        .iter()
                        .for_each(|t| deps.merge(&t.layer.feature_dependencies()));
                    let tester =
                        Tester::new(query, f64::from(GeoJsonIndex::EXTENT), states, source, None);
                    let reach = tester.max_reach(&targets);
                    let mut ctx = EvaluationContext::new().with_zoom(f64::from(self.tile.z));
                    ctx.canonical = Some((u32::from(self.tile.z), self.tile.x, self.tile.y));
                    for (order, feature) in index.tile(self.tile).into_iter().enumerate() {
                        if !tester.near(&feature.geometry, reach) {
                            continue;
                        }
                        crate::tile::geojson::load(&mut ctx, &deps, &feature, self.tile);
                        for (layer, state) in
                            tester.test(&targets, &mut ctx, feature.id, &feature.geometry)
                        {
                            hits.push((
                                layer,
                                order,
                                QueriedFeature {
                                    layer,
                                    source: source.to_owned(),
                                    source_layer: None,
                                    id: feature.id,
                                    geometry_type: geometry_type_name(&feature.geometry),
                                    properties: (*feature.properties).clone(),
                                    state,
                                },
                            ));
                        }
                    }
                }
            }
        }
        hits.sort_by_key(|&(layer, order, _)| std::cmp::Reverse((layer, order)));
        Ok(hits.into_iter().map(|(_, _, f)| f).collect())
    }
}

/// Whether point queries test this layer type.
fn is_queryable(kind: &LayerKind) -> bool {
    matches!(
        kind,
        LayerKind::Fill(_)
            | LayerKind::Line(_)
            | LayerKind::Circle(_)
            | LayerKind::FillExtrusion(_)
    )
}

struct Tester<'a> {
    /// The point in tile units.
    point: [f64; 2],
    zoom: f64,
    units_per_pixel: f64,
    tolerance: f64,
    states: Option<&'a FeatureStates>,
    source: &'a str,
    source_layer: Option<&'a str>,
}

impl<'a> Tester<'a> {
    fn new(
        query: &PointQuery,
        extent: f64,
        states: Option<&'a FeatureStates>,
        source: &'a str,
        source_layer: Option<&'a str>,
    ) -> Self {
        Self {
            point: query.position.map(|c| c * extent),
            zoom: query.zoom,
            units_per_pixel: extent / query.tile_size,
            tolerance: query.tolerance,
            states,
            source,
            source_layer,
        }
    }

    /// How far from the point, in tile units, geometry can be for a layer of `kind` to draw
    /// over it, or `None` when that differs per feature.
    fn reach(&self, kind: &LayerKind) -> Option<f64> {
        let at_zoom = |p: &PropertyValue<f32>| {
            (!p.is_feature_dependent()).then(|| f64::from(p.evaluate_at_zoom(self.zoom)))
        };
        let offset = |p: &PropertyValue<[f32; 2]>| {
            (!p.is_feature_dependent()).then(|| {
                let [x, y] = p.evaluate_at_zoom(self.zoom);
                f64::from(x).hypot(f64::from(y))
            })
        };
        let pixels = match kind {
            LayerKind::Fill(l) => offset(&l.paint.translate)? + self.tolerance,
            LayerKind::FillExtrusion(l) => offset(&l.paint.translate)?,
            LayerKind::Line(l) => {
                let width = at_zoom(&l.paint.width)?;
                let gap = at_zoom(&l.paint.gap_width)?;
                let full = if gap > 0.0 { gap + 2.0 * width } else { width };
                offset(&l.paint.translate)? + full / 2.0 + self.tolerance
            }
            LayerKind::Circle(l) => {
                offset(&l.paint.translate)?
                    + at_zoom(&l.paint.radius)?
                    + at_zoom(&l.paint.stroke_width)?
                    + self.tolerance
            }
            _ => 0.0,
        };
        Some(pixels * self.units_per_pixel)
    }

    /// The farthest any of `targets` [reaches](Self::reach).
    fn max_reach(&self, targets: &[Target<'_>]) -> Option<f64> {
        targets
            .iter()
            .try_fold(0.0, |max: f64, t| Some(max.max(self.reach(&t.layer.kind)?)))
    }

    /// Whether `geometry`'s bounds are within `reach` of the point; always when `reach` is
    /// `None`.
    fn near(&self, geometry: &Geometry<i32>, reach: Option<f64>) -> bool {
        let (Some(reach), Some([min, max])) = (reach, bounds(geometry)) else {
            return true;
        };
        let [x, y] = self.point;
        x >= min[0] - reach && x <= max[0] + reach && y >= min[1] - reach && y <= max[1] + reach
    }

    /// The targets (by style index) that draw the feature in `ctx` over the point, each
    /// with the feature's state.
    fn test(
        &self,
        targets: &[Target<'_>],
        ctx: &mut EvaluationContext,
        id: Option<u64>,
        geometry: &Geometry<i32>,
    ) -> Vec<(usize, StateMap)> {
        let mut found = Vec::new();
        let state = id
            .and_then(|id| self.states?.get(self.source, self.source_layer, id))
            .cloned()
            .unwrap_or_default();
        let tile_zoom = ctx.zoom;
        for target in targets {
            // Filters are evaluated as when the tile was processed.
            ctx.zoom = tile_zoom;
            if !target.layer.filter.matches(ctx) {
                continue;
            }
            ctx.zoom = Some(self.zoom);
            ctx.feature.state.clone_from(&state);
            let hit = self.hits(&target.layer.kind, ctx, geometry);
            ctx.feature.state.clear();
            if hit {
                found.push((target.index, state.clone()));
            }
        }
        ctx.zoom = tile_zoom;
        found
    }

    fn hits(&self, kind: &LayerKind, ctx: &EvaluationContext, geometry: &Geometry<i32>) -> bool {
        let u = self.units_per_pixel;
        let shifted = |translate: [f32; 2]| {
            [
                self.point[0] - f64::from(translate[0]) * u,
                self.point[1] - f64::from(translate[1]) * u,
            ]
        };
        let tolerance = self.tolerance * u;
        match kind {
            LayerKind::Fill(l) => {
                let p = shifted(l.paint.translate.evaluate(ctx));
                polygons_contain(geometry, p)
                    || (tolerance > 0.0 && distance(geometry, p) <= tolerance)
            }
            LayerKind::FillExtrusion(l) => {
                polygons_contain(geometry, shifted(l.paint.translate.evaluate(ctx)))
            }
            LayerKind::Line(l) => {
                let p = shifted(l.paint.translate.evaluate(ctx));
                let width = f64::from(l.paint.width.evaluate(ctx));
                let gap = f64::from(l.paint.gap_width.evaluate(ctx));
                // As MapLibre: a cased line with a gap spans the gap and both lines.
                let full = if gap > 0.0 { gap + 2.0 * width } else { width };
                distance(geometry, p) <= full / 2.0 * u + tolerance
            }
            LayerKind::Circle(l) => {
                let p = shifted(l.paint.translate.evaluate(ctx));
                let radius =
                    f64::from(l.paint.radius.evaluate(ctx) + l.paint.stroke_width.evaluate(ctx));
                point_distance(geometry, p) <= radius * u + tolerance
            }
            _ => false,
        }
    }
}

fn to_f64(c: Coord<i32>) -> [f64; 2] {
    [f64::from(c.x), f64::from(c.y)]
}

/// The corners `[min, max]` of the box around `geometry`, or `None` for an empty or
/// unusual geometry.
fn bounds(geometry: &Geometry<i32>) -> Option<[[f64; 2]; 2]> {
    let coords: Box<dyn Iterator<Item = &Coord<i32>>> = match geometry {
        Geometry::Point(p) => Box::new(std::iter::once(&p.0)),
        Geometry::MultiPoint(mp) => Box::new(mp.0.iter().map(|p| &p.0)),
        Geometry::LineString(l) => Box::new(l.0.iter()),
        Geometry::MultiLineString(ml) => Box::new(ml.0.iter().flat_map(|l| &l.0)),
        // Holes are inside the exterior.
        Geometry::Polygon(p) => Box::new(p.exterior().0.iter()),
        Geometry::MultiPolygon(mp) => Box::new(mp.0.iter().flat_map(|p| &p.exterior().0)),
        _ => return None,
    };
    coords.map(|&c| to_f64(c)).fold(None, |b, p| {
        let [min, max] = b.unwrap_or([p, p]);
        Some([
            [min[0].min(p[0]), min[1].min(p[1])],
            [max[0].max(p[0]), max[1].max(p[1])],
        ])
    })
}

fn rings(p: &Polygon<i32>) -> impl Iterator<Item = &LineString<i32>> {
    std::iter::once(p.exterior()).chain(p.interiors())
}

/// Whether a (multi)polygon contains `p`, holes excluded.
fn polygons_contain(geometry: &Geometry<i32>, p: [f64; 2]) -> bool {
    let contains = |polygon: &Polygon<i32>| {
        // Even-odd over every ring handles holes.
        let mut inside = false;
        for ring in rings(polygon) {
            for w in ring.0.windows(2) {
                let ([ax, ay], [bx, by]) = (to_f64(w[0]), to_f64(w[1]));
                if (ay > p[1]) != (by > p[1]) && p[0] < (bx - ax) * (p[1] - ay) / (by - ay) + ax {
                    inside = !inside;
                }
            }
        }
        inside
    };
    match geometry {
        Geometry::Polygon(polygon) => contains(polygon),
        Geometry::MultiPolygon(mp) => mp.0.iter().any(contains),
        _ => false,
    }
}

/// The distance from `p` to the nearest line, or ring edge, of `geometry`.
fn distance(geometry: &Geometry<i32>, p: [f64; 2]) -> f64 {
    let line = |l: &LineString<i32>| -> f64 {
        match l.0.as_slice() {
            [only] => length(sub(to_f64(*only), p)),
            coords => coords
                .windows(2)
                .map(|w| segment_distance(p, to_f64(w[0]), to_f64(w[1])))
                .fold(f64::INFINITY, f64::min),
        }
    };
    match geometry {
        Geometry::LineString(l) => line(l),
        Geometry::MultiLineString(ml) => ml.0.iter().map(line).fold(f64::INFINITY, f64::min),
        Geometry::Polygon(polygon) => rings(polygon).map(line).fold(f64::INFINITY, f64::min),
        Geometry::MultiPolygon(mp) => {
            mp.0.iter()
                .flat_map(rings)
                .map(line)
                .fold(f64::INFINITY, f64::min)
        }
        _ => f64::INFINITY,
    }
}

/// The distance from `p` to the nearest point of `geometry`.
fn point_distance(geometry: &Geometry<i32>, p: [f64; 2]) -> f64 {
    match geometry {
        Geometry::Point(point) => length(sub(to_f64(point.0), p)),
        Geometry::MultiPoint(mp) => {
            mp.0.iter()
                .map(|point| length(sub(to_f64(point.0), p)))
                .fold(f64::INFINITY, f64::min)
        }
        _ => f64::INFINITY,
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn length(v: [f64; 2]) -> f64 {
    v[0].hypot(v[1])
}

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = sub(b, a);
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    };
    length(sub(p, [a[0] + ab[0] * t, a[1] + ab[1] * t]))
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::polygon;

    use super::*;

    #[test]
    fn polygon_with_hole() {
        let p = Geometry::Polygon(polygon!(
            exterior: [(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)],
            interiors: [[(x: 2, y: 2), (x: 4, y: 2), (x: 4, y: 4), (x: 2, y: 4), (x: 2, y: 2)]],
        ));
        assert!(polygons_contain(&p, [5.0, 5.0]));
        assert!(!polygons_contain(&p, [3.0, 3.0]));
        assert!(!polygons_contain(&p, [11.0, 5.0]));
        assert_eq!(distance(&p, [12.0, 5.0]), 2.0);
        assert_eq!(distance(&p, [3.0, 3.0]), 1.0);
    }

    #[test]
    fn segments() {
        assert_eq!(segment_distance([5.0, 3.0], [0.0, 0.0], [10.0, 0.0]), 3.0);
        assert_eq!(segment_distance([13.0, 4.0], [0.0, 0.0], [10.0, 0.0]), 5.0);
        assert_eq!(
            segment_distance([1.0, 1.0], [0.0, 0.0], [0.0, 0.0]),
            2f64.sqrt()
        );
    }
}
