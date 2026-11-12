//! Turning one tile into GPU-ready geometry for each style layer that draws from it.
//!
//! Each layer type has its own module with its bucket type and a builder implementing
//! [`LayerBuilder`]; [`builder_for`] picks the builder for a style layer. Vector sources
//! go through MLT decoding and feed every feature to the builders of the layers that draw
//! the feature's source layer; raster and raster-dem sources are handed to
//! [`raster::process`] instead, and image sources to [`raster::process_image`]. GeoJSON
//! tiles come from a [`GeoJsonIndex`] and feed the same builders without being encoded
//! ([`TileProcessor::process_geojson`]).
//!
//! [`GeoJsonIndex`]: crate::geojson::GeoJsonIndex

mod circle;
mod color_relief;
pub(crate) mod eval;
mod fill;
mod fill_extrusion;
pub(crate) mod geojson;
mod heatmap;
mod hillshade;
mod line;
pub mod raster;
mod sort_key;
mod symbol;

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use maplibre_expr::EvaluationContext;
use mlt_core::geo_types::Geometry;
use mlt_core::{Decoder, Layer, LendingIterator, ParsedLayer01, Parser};

pub use circle::{CircleBucket, CircleVertex};
pub use color_relief::{ColorRamp, ColorReliefBucket};
pub use fill::{FillBucket, FillVertex};
pub use fill_extrusion::{
    ExtrusionRoofs, ExtrusionWalls, FillExtrusionBucket, FillExtrusionPattern, WallPoint,
};
pub use heatmap::{HeatmapBucket, HeatmapVertex};
pub use hillshade::{HillshadeBucket, HillshadeLight, LightSource, latitude_range};
pub use line::{LineBucket, LineVertex};
pub use raster::{IMAGE_EXTENT, RasterBucket, RasterImage, RasterVertex, image_source_tile};
pub use symbol::{SymbolBucket, SymbolQuads, SymbolVertex, symbol_flags};

use self::eval::FeatureContext;
use self::fill::PreTessellated;
use crate::Error;
use crate::dem::DemData;
use crate::glyphs::GlyphProvider;
use crate::query::FeatureIndex;
use crate::sprite::ImageAtlas;
use crate::style::{Dependencies, LayerKind, Source, Style, StyleLayer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn new(z: u8, x: u32, y: u32) -> Self {
        Self { z, x, y }
    }
}

/// Everything one tile contributes to the map.
#[derive(Debug, Clone)]
pub struct ProcessedTile {
    pub id: TileId,
    /// The zoom expressions were evaluated at: the tile's own, or a deeper one for a tile
    /// drawn magnified past its source's max zoom.
    pub zoom: u8,
    /// One bucket per style layer with something to draw, in style (draw) order.
    pub buckets: Vec<Bucket>,
    /// The tile's features, for [querying](crate::query) what is drawn where. `None` for
    /// raster tiles.
    pub features: Option<FeatureIndex>,
    /// The elevations of a tile of the style's terrain source, which the renderer drapes
    /// the map over whether or not a layer draws the source.
    pub dem: Option<Arc<DemData>>,
}

/// The geometry one style layer draws from one tile.
#[derive(Debug, Clone)]
pub struct Bucket {
    /// Index into [`Style::layers`].
    pub style_layer: usize,
    /// The tile coordinate that spans the tile's width, as positions are in `0..extent`
    /// (with features allowed to stray past the edges into a buffer).
    pub extent: u32,
    pub geometry: BucketGeometry,
}

/// A bucket's contents, one variant per layer type that draws from tiles.
#[derive(Debug, Clone)]
pub enum BucketGeometry {
    Fill(FillBucket),
    Line(LineBucket),
    Symbol(SymbolBucket),
    Circle(CircleBucket),
    Heatmap(HeatmapBucket),
    FillExtrusion(FillExtrusionBucket),
    Raster(RasterBucket),
    Hillshade(HillshadeBucket),
    ColorRelief(ColorReliefBucket),
}

/// Data from outside the tile that processing may need: sprite images (for icons and
/// patterns) and glyphs (for text). Both are optional; layer types that need a missing
/// one skip what they can't draw.
#[derive(Clone, Copy, Default)]
pub struct TileResources<'a> {
    pub images: Option<&'a ImageAtlas>,
    pub glyphs: Option<&'a dyn GlyphProvider>,
}

/// What a layer builder knows about the tile it builds for.
// Not every field is read by the layer types implemented so far.
#[allow(dead_code)]
pub(crate) struct BuildContext<'a> {
    pub tile: TileId,
    /// The zoom expressions are evaluated at.
    pub zoom: f32,
    pub extent: u32,
    pub resources: TileResources<'a>,
    /// The style source the tile is from.
    pub source: Option<&'a Source>,
}

/// One feature, as the builders of the layers drawing it see it.
pub(crate) struct FeatureInput<'a, 'b> {
    /// Evaluates expressions for this feature at the tile's zoom. Only the properties some
    /// target layer reads are loaded.
    pub ctx: &'a mut EvaluationContext,
    pub id: Option<u64>,
    pub geometry: &'a Geometry<i32>,
    /// Triangles the tile stores for a polygon feature, when some builder asked for them
    /// with [`LayerBuilder::wants_triangles`].
    pub pre_tessellated: Option<PreTessellated<'b>>,
    /// For a line of a GeoJSON source with `lineMetrics`: where this piece of the line
    /// starts and ends along the whole line, as fractions of its length, so `line-progress`
    /// can be computed from distances along the piece.
    pub line_clip: Option<[f64; 2]>,
}

/// Builds one style layer's bucket for one tile. Implemented once per layer type.
pub(crate) trait LayerBuilder {
    /// Whether the tile's stored polygon triangulation is useful to this builder.
    fn wants_triangles(&self) -> bool {
        false
    }

    /// Add a feature that passed the layer's filter.
    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>);

    /// The finished bucket, or `None` if nothing was added. Scratch buffers go back to
    /// `scratch` for the next tile.
    fn finish(self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry>;
}

/// The builder for `layer`, or `None` for layer types that don't draw vector features
/// (or aren't implemented yet).
fn builder_for<'a>(
    layer: &'a StyleLayer,
    cx: &BuildContext<'a>,
    scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    match &layer.kind {
        LayerKind::Fill(l) => fill::builder(l, cx, scratch),
        LayerKind::Line(l) => line::builder(l, cx, scratch),
        LayerKind::Symbol(l) => symbol::builder(l, cx, scratch),
        LayerKind::Circle(l) => circle::builder(l, cx, scratch),
        LayerKind::Heatmap(l) => heatmap::builder(l, cx, scratch),
        LayerKind::FillExtrusion(l) => fill_extrusion::builder(l, cx, scratch),
        LayerKind::Background(_)
        | LayerKind::Raster(_)
        | LayerKind::Hillshade(_)
        | LayerKind::ColorRelief(_)
        | LayerKind::Unsupported(_) => None,
    }
}

/// Scratch values (tessellators, buffers) kept between tiles, one of each type, so
/// builders can borrow them without each layer type needing a field here.
#[derive(Default)]
pub(crate) struct Scratch {
    values: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl Scratch {
    /// The stored `T`, or a new one.
    pub fn take<T: Default + Send + 'static>(&mut self) -> T {
        self.values
            .remove(&TypeId::of::<T>())
            .and_then(|v| v.downcast().ok())
            .map_or_else(T::default, |v| *v)
    }

    pub fn put<T: Send + 'static>(&mut self, value: T) {
        self.values.insert(TypeId::of::<T>(), Box::new(value));
    }
}

/// Reusable state for processing tiles. Keep one per worker thread to avoid reallocating
/// scratch buffers for every tile.
#[derive(Default)]
pub struct TileProcessor {
    scratch: Scratch,
}

impl TileProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a tile of the style source named `source`, without images or glyphs.
    pub fn process(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        data: &[u8],
    ) -> Result<ProcessedTile, Error> {
        self.process_with(style, source, id, data, TileResources::default())
    }

    /// Process a tile of the style source named `source`: MLT for vector sources, an
    /// encoded image for raster and raster-dem ones, and the whole image for an image
    /// source (placed relative to `id`, normally [`image_source_tile`]).
    ///
    /// Expressions are evaluated at the tile's zoom level. MLT layers that no visible style
    /// layer draws from are skipped without being decoded.
    pub fn process_with(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        data: &[u8],
        resources: TileResources<'_>,
    ) -> Result<ProcessedTile, Error> {
        self.process_at(style, source, id, id.z, data, resources)
    }

    /// [`Self::process_with`], evaluating expressions (filters, layer zoom ranges and
    /// layout) at `zoom` rather than the tile's own: for tiles drawn magnified past their
    /// source's max zoom, as MapLibre lays out overscaled tiles. Not below `id.z`.
    pub fn process_at(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        zoom: u8,
        data: &[u8],
        resources: TileResources<'_>,
    ) -> Result<ProcessedTile, Error> {
        let zoom = zoom.max(id.z);
        let mut buckets = Vec::new();
        let mut dem = None;
        let features = match style.source(source) {
            Some(Source::Raster(_) | Source::RasterDem(_)) => {
                dem = raster::process(style, source, id, data, &mut buckets)?;
                None
            }
            Some(Source::Image(image)) => {
                raster::process_image(style, source, image, id, data, &mut buckets)?;
                None
            }
            // Vector tiles, and anything else that was converted to MLT.
            _ => {
                buckets = self.process_mlt(style, source, id, zoom, data, resources)?;
                Some(FeatureIndex::mlt(id, data))
            }
        };
        buckets.sort_by_key(|b| b.style_layer);
        Ok(ProcessedTile {
            id,
            zoom,
            buckets,
            features,
            dem,
        })
    }

    fn process_mlt(
        &mut self,
        style: &Style,
        source: &str,
        id: TileId,
        zoom: u8,
        data: &[u8],
        resources: TileResources<'_>,
    ) -> Result<Vec<Bucket>, Error> {
        let zoom = f32::from(zoom);
        let mut buckets = Vec::new();
        let mut decoder = Decoder::default();
        for layer in Parser::default().parse_layers(data)? {
            let Layer::Tag01(layer) = layer else { continue };
            let targets: Vec<_> = style
                .layers
                .iter()
                .enumerate()
                .filter(|(_, l)| {
                    l.source.as_deref() == Some(source)
                        && l.source_layer.as_deref() == Some(layer.name())
                        && l.is_visible_at(zoom)
                })
                .collect();
            if targets.is_empty() {
                continue;
            }
            let cx = BuildContext {
                tile: id,
                zoom,
                extent: layer.extent().get(),
                resources,
                source: style.source(source),
            };
            let mut builders = Vec::new();
            for &(index, style_layer) in &targets {
                if let Some(builder) = builder_for(style_layer, &cx, &mut self.scratch) {
                    builders.push((index, style_layer, builder));
                }
            }
            if builders.is_empty() {
                continue;
            }
            let layer_name = layer.name().to_owned();
            let layer = layer.decode_all(&mut decoder)?;
            let promote_id = style.source(source).and_then(|s| s.promote_id(&layer_name));
            self.process_layer(
                &layer,
                &mut builders,
                id,
                zoom,
                promote_id,
                resources.images,
            )?;
            self.finish_builders(builders, cx.extent, &mut buckets);
        }
        Ok(buckets)
    }

    /// Finish each builder's bucket.
    fn finish_builders(
        &mut self,
        builders: Vec<Builder<'_>>,
        extent: u32,
        buckets: &mut Vec<Bucket>,
    ) {
        for (index, _, builder) in builders {
            if let Some(geometry) = builder.finish(&mut self.scratch) {
                buckets.push(Bucket {
                    style_layer: index,
                    extent,
                    geometry,
                });
            }
        }
    }

    fn process_layer(
        &mut self,
        layer: &ParsedLayer01<'_>,
        builders: &mut [Builder<'_>],
        id: TileId,
        zoom: f32,
        promote_id: Option<&str>,
        images: Option<&ImageAtlas>,
    ) -> Result<(), Error> {
        let mut deps = Dependencies::default();
        builders
            .iter()
            .for_each(|(_, l, _)| deps.merge(&l.feature_dependencies()));
        if let Some(key) = promote_id {
            deps.properties.insert(key);
        }
        let mut ctx = FeatureContext::new(layer, &deps, id, f64::from(zoom));
        eval::set_available_images(ctx.context_mut(), &deps, images);
        let needs_feature = deps.is_feature_dependent();

        let wants_triangles = builders.iter().any(|(_, _, b)| b.wants_triangles());
        let stored = layer.geometry_values();
        let stored_triangles = match (stored.index_buffer(), stored.triangle_offsets()) {
            (Some(indices), Some(offsets)) if wants_triangles => Some((indices, offsets)),
            _ => None,
        };

        let mut features = layer.iter_features();
        let mut index = 0;
        // Stored triangles are listed per polygon feature, so count those separately.
        let mut polygon_index = 0;
        while let Some(feature) = features.next() {
            let feature = feature?;
            let feature_index = index;
            index += 1;
            let geometry = feature.geometry();
            let is_polygon = matches!(geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_));
            let pre_tessellated = if is_polygon {
                polygon_index += 1;
                stored_triangles.and_then(|(indices, offsets)| {
                    let run = offsets.get(polygon_index - 1)?..offsets.get(polygon_index)?;
                    let indices = indices.get(*run.start as usize * 3..*run.end as usize * 3)?;
                    let vertices = stored.vertex_range(feature_index).ok()?;
                    Some(PreTessellated {
                        indices,
                        first_vertex: u32::try_from(vertices.start).ok()?,
                        vertex_count: vertices.len(),
                    })
                })
            } else {
                None
            };

            if needs_feature {
                ctx.load(&feature);
            }
            let id = match promote_id {
                Some(key) => ctx.promote_id(key),
                None => feature.id(),
            };
            add_feature(
                builders,
                ctx.context_mut(),
                id,
                geometry,
                pre_tessellated,
                None,
            );
        }
        Ok(())
    }
}

/// A style layer's builder, with the layer and its index in the style.
type Builder<'a> = (usize, &'a StyleLayer, Box<dyn LayerBuilder + 'a>);

/// Give a feature to the builders of every layer whose filter it passes.
fn add_feature(
    builders: &mut [Builder<'_>],
    ctx: &mut EvaluationContext,
    id: Option<u64>,
    geometry: &Geometry<i32>,
    pre_tessellated: Option<PreTessellated<'_>>,
    line_clip: Option<[f64; 2]>,
) {
    for (_, style_layer, builder) in builders.iter_mut() {
        if !style_layer.filter.matches(ctx) {
            continue;
        }
        builder.add_feature(&mut FeatureInput {
            ctx,
            id,
            geometry,
            pre_tessellated,
            line_clip,
        });
    }
}
