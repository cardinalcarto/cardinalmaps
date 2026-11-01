//! Processing tiles of GeoJSON sources: the tiler's features go to the same layer builders
//! as decoded MLT features.

use std::sync::Arc;

use maplibre_expr::{EvaluationContext, Value};

use super::eval::{geometry_type_name, load_geometry, set_available_images};
use super::{
    BuildContext, ProcessedTile, TileId, TileProcessor, TileResources, add_feature, builder_for,
};
use crate::geojson::{GeoJsonIndex, TileFeature};
use crate::query::FeatureIndex;
use crate::style::{Dependencies, PropertyDeps, Style};

/// The properties geojson-vt adds to line pieces with `lineMetrics`, which MapLibre's
/// line layers read for `line-progress`.
const CLIP_START: &str = "mapbox_clip_start";
const CLIP_END: &str = "mapbox_clip_end";

impl TileProcessor {
    /// Process tile `id` of the GeoJSON source `source`, whose data `index` holds.
    pub fn process_geojson(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        index: &Arc<GeoJsonIndex>,
        resources: TileResources<'_>,
    ) -> ProcessedTile {
        self.process_geojson_at(style, source, id, id.z, index, resources)
    }

    /// [`Self::process_geojson`], evaluating expressions at `zoom` as
    /// [`Self::process_at`] does.
    pub fn process_geojson_at(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        zoom: u8,
        index: &Arc<GeoJsonIndex>,
        resources: TileResources<'_>,
    ) -> ProcessedTile {
        let tile_zoom = zoom.max(id.z);
        let zoom = f32::from(tile_zoom);
        let cx = BuildContext {
            tile: id,
            zoom,
            extent: GeoJsonIndex::EXTENT,
            resources,
            source: style.source(source),
        };
        let mut builders = Vec::new();
        for (i, layer) in style.layers.iter().enumerate() {
            if layer.source.as_deref() == Some(source)
                && layer.is_visible_at(zoom)
                && let Some(builder) = builder_for(layer, &cx, &mut self.scratch)
            {
                builders.push((i, layer, builder));
            }
        }
        let mut buckets = Vec::new();
        if !builders.is_empty() {
            let mut deps = Dependencies::default();
            builders
                .iter()
                .for_each(|(_, l, _)| deps.merge(&l.feature_dependencies()));
            let mut ctx = EvaluationContext::new().with_zoom(f64::from(zoom));
            ctx.canonical = Some((u32::from(id.z), id.x, id.y));
            set_available_images(&mut ctx, &deps, resources.images);
            for feature in index.tile(id) {
                load(&mut ctx, &deps, &feature, id);
                add_feature(
                    &mut builders,
                    &mut ctx,
                    feature.id,
                    &feature.geometry,
                    None,
                    feature.line_clip,
                );
            }
            self.finish_builders(builders, GeoJsonIndex::EXTENT, &mut buckets);
        }
        ProcessedTile {
            id,
            zoom: tile_zoom,
            buckets,
            features: Some(FeatureIndex::geojson(id, index.clone())),
            dem: None,
        }
    }
}

/// Make `feature` the one `ctx` evaluates, copying what `deps` says expressions read.
pub(crate) fn load(
    ctx: &mut EvaluationContext,
    deps: &Dependencies,
    feature: &TileFeature,
    tile: TileId,
) {
    let feat = &mut ctx.feature;
    match &deps.properties {
        PropertyDeps::None => {}
        PropertyDeps::All => feat.properties.clone_from(&feature.properties),
        PropertyDeps::Keys(keys) => {
            for key in keys {
                match feature.properties.get(key) {
                    Some(value) => {
                        feat.properties.insert(key.clone(), value.clone());
                    }
                    None => {
                        feat.properties.remove(key);
                    }
                }
            }
        }
    }
    if let Some([start, end]) = feature.line_clip {
        feat.properties
            .insert(CLIP_START.into(), Value::Number(start));
        feat.properties.insert(CLIP_END.into(), Value::Number(end));
    }
    feat.id = feature.id.map(|id| Value::Number(id as f64));
    feat.geometry_type = Some(geometry_type_name(&feature.geometry).to_owned());
    if deps.geometry {
        load_geometry(
            feat,
            &feature.geometry,
            tile,
            f64::from(GeoJsonIndex::EXTENT),
        );
    }
}
