//! Exposing a feature to expression evaluation without copying more of it than needed.

use maplibre_expr::{EvaluationContext, Value};
use mlt_core::geo_types::{Coord, Geometry, LineString};
use mlt_core::{FeatureRef, ParsedLayer01, PropValueRef};

use super::TileId;
use crate::sprite::ImageAtlas;
use crate::style::{Dependencies, PropertyDeps};

/// Holds the [`EvaluationContext`] for the current feature of one MLT layer and refills it
/// for each feature, copying only what [`Dependencies`] says expressions will read.
pub(crate) struct FeatureContext {
    ctx: EvaluationContext,
    /// Each property to copy by its name and its slot in [`FeatureRef::iter_all_properties`].
    slots: Vec<(String, usize)>,
    id: bool,
    geometry_type: bool,
    geometry: bool,
    tile: TileId,
    extent: f64,
}

impl FeatureContext {
    pub fn new(layer: &ParsedLayer01<'_>, deps: &Dependencies, tile: TileId, zoom: f64) -> Self {
        let slots = match &deps.properties {
            PropertyDeps::None => Vec::new(),
            PropertyDeps::Keys(keys) => layer
                .iterate_prop_names()
                .enumerate()
                .map(|(slot, name)| (name.to_string(), slot))
                .filter(|(name, _)| keys.contains(name))
                .collect(),
            PropertyDeps::All => layer
                .iterate_prop_names()
                .enumerate()
                .map(|(slot, name)| (name.to_string(), slot))
                .collect(),
        };
        let mut ctx = EvaluationContext::new().with_zoom(zoom);
        ctx.canonical = Some((u32::from(tile.z), tile.x, tile.y));
        Self {
            ctx,
            slots,
            id: deps.id,
            // `within` and `distance` need the type to interpret the geometry.
            geometry_type: deps.geometry_type || deps.geometry,
            geometry: deps.geometry,
            tile,
            extent: f64::from(layer.extent().get()),
        }
    }

    pub fn context(&self) -> &EvaluationContext {
        &self.ctx
    }

    /// For evaluating at other zoom levels; whoever changes the zoom restores it.
    pub fn context_mut(&mut self) -> &mut EvaluationContext {
        &mut self.ctx
    }

    pub fn load(&mut self, feature: &FeatureRef<'_, '_>) {
        let feat = &mut self.ctx.feature;
        if !self.slots.is_empty() {
            let props = &mut feat.properties;
            // Slots are in ascending order, so a single pass over the values finds them all.
            let mut values = feature.iter_all_properties().enumerate();
            for (name, slot) in &self.slots {
                let value = values.find(|(i, _)| i == slot).and_then(|(_, v)| v);
                match value {
                    // Reuse the previous feature's entry, and its string buffer, if there is one.
                    Some(value) => match props.get_mut(name) {
                        Some(existing) => assign(existing, value),
                        None => {
                            props.insert(name.clone(), to_value(value));
                        }
                    },
                    None => {
                        props.remove(name);
                    }
                }
            }
        }
        if self.id {
            feat.id = feature.id().map(|id| Value::Number(id as f64));
        }
        let geometry = feature.geometry();
        if self.geometry_type {
            feat.geometry_type = Some(geometry_type_name(geometry).to_owned());
        }
        if self.geometry {
            load_geometry(feat, geometry, self.tile, self.extent);
        }
    }

    /// The id `promoteId` gives the loaded feature: its property `key`, which also becomes
    /// what `["id"]` reads.
    pub fn promote_id(&mut self, key: &str) -> Option<u64> {
        let feat = &mut self.ctx.feature;
        let value = feat.properties.get(key)?;
        let id = crate::state::feature_id(value);
        feat.id = Some(value.clone());
        id
    }
}

/// Tell `["image", name]` which images exist, when `deps` says an expression asks.
pub(crate) fn set_available_images(
    ctx: &mut EvaluationContext,
    deps: &Dependencies,
    images: Option<&ImageAtlas>,
) {
    if let Some(images) = images.filter(|_| deps.images) {
        ctx.available_images = images.names().map(str::to_owned).collect();
    }
}

/// Set the feature's `geometry` to `geometry` in longitude and latitude, for `within` and
/// `distance`.
pub(crate) fn load_geometry(
    feat: &mut maplibre_expr::Feature,
    geometry: &Geometry<i32>,
    tile: TileId,
    extent: f64,
) {
    feat.geometry.clear();
    let to_lng_lat = |line: &LineString<i32>| {
        line.0
            .iter()
            .map(|&c| tile_to_lng_lat(tile, extent, c))
            .collect()
    };
    match geometry {
        Geometry::Point(p) => feat.geometry.push(vec![tile_to_lng_lat(tile, extent, p.0)]),
        Geometry::MultiPoint(mp) => feat.geometry.push(
            mp.0.iter()
                .map(|p| tile_to_lng_lat(tile, extent, p.0))
                .collect(),
        ),
        Geometry::LineString(l) => feat.geometry.push(to_lng_lat(l)),
        Geometry::MultiLineString(ml) => feat.geometry.extend(ml.0.iter().map(to_lng_lat)),
        Geometry::Polygon(p) => feat.geometry.extend(rings(p).map(to_lng_lat)),
        Geometry::MultiPolygon(mp) => {
            feat.geometry
                .extend(mp.0.iter().flat_map(rings).map(to_lng_lat));
        }
        _ => {}
    }
}

fn rings<T: mlt_core::geo_types::CoordNum>(
    p: &mlt_core::geo_types::Polygon<T>,
) -> impl Iterator<Item = &LineString<T>> {
    std::iter::once(p.exterior()).chain(p.interiors())
}

/// The name MapLibre gives a geometry in `["geometry-type"]` and legacy `$type` filters.
pub(crate) fn geometry_type_name(geometry: &Geometry<i32>) -> &'static str {
    match geometry {
        Geometry::Point(_) | Geometry::MultiPoint(_) => "Point",
        Geometry::LineString(_) | Geometry::MultiLineString(_) | Geometry::Line(_) => "LineString",
        _ => "Polygon",
    }
}

fn tile_to_lng_lat(tile: TileId, extent: f64, c: Coord<i32>) -> (f64, f64) {
    let size = f64::from(tile.z).exp2();
    let x = (f64::from(tile.x) + f64::from(c.x) / extent) / size;
    let y = (f64::from(tile.y) + f64::from(c.y) / extent) / size;
    let lng = x * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y))
        .sinh()
        .atan()
        .to_degrees();
    (lng, lat)
}

fn to_value(value: PropValueRef<'_>) -> Value {
    match value {
        PropValueRef::Bool(v) => Value::Bool(v),
        PropValueRef::I8(v) => Value::Number(v.into()),
        PropValueRef::U8(v) => Value::Number(v.into()),
        PropValueRef::I32(v) => Value::Number(v.into()),
        PropValueRef::U32(v) => Value::Number(v.into()),
        PropValueRef::I64(v) => Value::Number(v as f64),
        PropValueRef::U64(v) => Value::Number(v as f64),
        PropValueRef::F32(v) => Value::Number(v.into()),
        PropValueRef::F64(v) => Value::Number(v),
        PropValueRef::Str(v) => Value::String(v.to_owned()),
    }
}

fn assign(slot: &mut Value, value: PropValueRef<'_>) {
    match (slot, value) {
        (Value::String(s), PropValueRef::Str(v)) => {
            s.clear();
            s.push_str(v);
        }
        (slot, value) => *slot = to_value(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_corners_to_lng_lat() {
        let tile = TileId::new(1, 1, 0);
        let (lng, lat) = tile_to_lng_lat(tile, 4096.0, Coord { x: 0, y: 4096 });
        assert!((lng - 0.0).abs() < 1e-9);
        assert!(lat.abs() < 1e-9);
        let (lng, lat) = tile_to_lng_lat(tile, 4096.0, Coord { x: 4096, y: 0 });
        assert!((lng - 180.0).abs() < 1e-9);
        assert!((lat - 85.051_128_78).abs() < 1e-6);
    }
}
