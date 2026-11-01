//! One renderer per style layer type, behind [`LayerRenderer`]. Ports of
//! `sparkle_harness`'s, each with a shader pair in `flutter/sparkle/shaders`.
//!
//! To add a layer type: write `<type>.rs` here implementing [`LayerRenderer`] (plus its
//! shaders, listed in `flutter/sparkle/sparkle.shaderbundle.json`), and add one line to
//! [`create`].

mod background;
mod circle;
mod color_relief;
mod dem;
mod fill;
mod fill_extrusion;
mod heatmap;
mod hillshade;
mod line;
mod pattern;
mod raster;
mod symbol;

use std::any::Any;

use sparkle::Bucket;
use sparkle::style::StyleLayer;
use sparkle_view::camera::Placement;

use super::Gpu;
use super::view::{DrawUniforms, TileGeo, View};
use crate::gpu::{Pass, Texture};

/// Whether layers of `layer_type` are flat on the map, so over terrain and on the globe
/// they're rendered into the tiles' drape textures rather than drawn in 3D.
pub fn is_draped(layer_type: &str) -> bool {
    matches!(
        layer_type,
        "background" | "fill" | "line" | "raster" | "hillshade" | "color-relief"
    )
}

/// Every layer renderer. The core picks one per style layer by [`LayerRenderer::layer_type`].
pub fn create(gpu: &Gpu) -> Vec<Box<dyn LayerRenderer>> {
    vec![
        Box::new(background::BackgroundRenderer::new(gpu)),
        Box::new(fill::FillRenderer::new(gpu)),
        Box::new(line::LineRenderer::new(gpu)),
        Box::new(symbol::SymbolRenderer::new(gpu)),
        Box::new(circle::CircleRenderer::new(gpu)),
        Box::new(heatmap::HeatmapRenderer::new(gpu)),
        Box::new(fill_extrusion::FillExtrusionRenderer::new(gpu)),
        Box::new(raster::RasterRenderer::new(gpu)),
        Box::new(hillshade::HillshadeRenderer::new(gpu)),
        Box::new(color_relief::ColorReliefRenderer::new(gpu)),
    ]
}

/// What a layer renderer keeps of an uploaded bucket.
pub type BucketData = Box<dyn Any + Send + Sync>;

/// Draws all style layers of one type. Renderers live behind the map's lock and are
/// moved between threads with it, so they and what they upload must be `Send`.
pub trait LayerRenderer: Send {
    /// The style layer `type` this draws.
    fn layer_type(&self) -> &'static str;

    /// Upload a bucket of one of this type's layers when its tile arrives. The result is
    /// handed back as [`TileDraw::bucket`]; `None` means there's nothing to draw.
    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<BucketData>;

    /// Whether `bucket` (which retains no features) can be merged into one upload with
    /// buckets of neighbouring layers of this type, and with which: those with the same
    /// key (see [`Self::upload_merged`]).
    fn merge_key(&self, _layer: &StyleLayer, _bucket: &Bucket) -> Option<usize> {
        None
    }

    /// Upload buckets of consecutive layers of one tile that [`Self::merge_key`] gave
    /// the same key, and that nothing else is drawn between in that tile, as one: what
    /// each bucket becomes, in order. [`Self::draw_group`] draws them as one when it can.
    fn upload_merged(
        &mut self,
        _gpu: &Gpu,
        _buckets: &[(&StyleLayer, &Bucket)],
    ) -> Vec<BucketData> {
        unreachable!("only called for layer types with a merge key")
    }

    /// Upload new paint attributes of `bucket`, which this type's [`Self::upload`] made
    /// `data` from, after feature state changed them (the geometry is the same). Returns
    /// false if this type can't, in which case the whole bucket is uploaded again.
    fn update_paint(
        &mut self,
        _gpu: &Gpu,
        _data: &mut (dyn Any + Send + Sync),
        _bucket: &Bucket,
    ) -> bool {
        false
    }

    /// The style changed: drop anything kept per style layer index, such as color ramps,
    /// since indices and properties may now mean something else.
    fn style_changed(&mut self) {}

    /// Work outside the main render pass before it starts, such as rendering to a texture
    /// that [`Self::draw`] then composites. Called for every visible layer of this type.
    fn prepare(&mut self, _cx: &mut PrepareContext<'_>, _layer: &LayerDraw<'_>) {}

    /// Like [`Self::prepare`], but once per frame with all visible layers of this type at
    /// once (bottom to top, possibly none), before any `prepare`: for work across layers,
    /// such as symbol collision detection.
    fn prepare_frame(&mut self, _cx: &mut PrepareContext<'_>, _layers: &[&LayerDraw<'_>]) {}

    /// Draw one layer into the main pass.
    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>);

    /// Draw consecutive layers of this type and the same source, in order.
    fn draw_group(&mut self, cx: &mut DrawContext<'_, '_>, layers: &[&LayerDraw<'_>]) {
        for layer in layers {
            self.draw(cx, layer);
        }
    }

    /// Whether frames drawn now would differ from frames drawn a moment later even with
    /// nothing else changing, as while tiles fade in.
    fn animating(&self) -> bool {
        false
    }
}

/// One tile to draw a layer from.
pub struct TileDraw<'a> {
    /// Where the tile's geometry is: the tile itself, or an ancestor standing in for it.
    pub placement: Placement,
    pub extent: u32,
    /// What [`LayerRenderer::upload`] returned for this tile's bucket.
    pub bucket: &'a (dyn Any + Send + Sync),
    /// Tile coordinates to clip space.
    pub matrix: [[f32; 4]; 4],
    /// Tile units per logical pixel at the camera's zoom.
    pub units_per_pixel: f32,
    /// The stencil value inside this tile's clip mask; pipelines with `clip` only draw
    /// where the stencil equals it, so set it with `pass.set_stencil_reference`.
    pub stencil_ref: u32,
    /// Where the tile is on terrain and the globe, for shaders that place geometry
    /// themselves (`to_clip` and friends).
    pub geo: TileGeo,
    /// The terrain under the tile, for the CPU.
    pub elevation: Option<sparkle::terrain::ElevationSampler>,
}

impl TileDraw<'_> {
    /// The bucket as the type this layer type's `upload` made.
    pub fn bucket<T: 'static>(&self) -> Option<&T> {
        self.bucket.downcast_ref()
    }
}

/// One style layer to draw, with every visible tile that has a bucket for it.
pub struct LayerDraw<'a> {
    /// Index into the style's layers.
    pub index: usize,
    pub layer: &'a StyleLayer,
    /// Empty for layers without a source.
    pub tiles: Vec<TileDraw<'a>>,
}

/// Bind `u` as the `Draw` block, and `dem` (or a placeholder) as the DEM texture that
/// `common.glsl`'s `elevation` samples.
pub fn bind_uniforms(gpu: &Gpu, pass: &mut Pass<'_>, u: &DrawUniforms, dem: Option<&Texture>) {
    pass.set_uniforms(bytemuck::bytes_of(u));
    pass.set_texture("dem", dem.unwrap_or(gpu.no_dem()), super::NEAREST);
}

/// What [`LayerRenderer::draw`] draws with.
pub struct DrawContext<'a, 'p> {
    pub gpu: &'a Gpu,
    pub pass: &'a mut Pass<'p>,
    pub view: &'a View,
}

impl DrawContext<'_, '_> {
    /// Bind `u` for the next draws, with the DEM of `tile` (if any) for its elevations.
    pub fn bind_uniforms(&mut self, u: &DrawUniforms, tile: Option<&TileDraw<'_>>) {
        let dem = tile.and_then(|t| t.geo.dem_texture.as_ref());
        bind_uniforms(self.gpu, self.pass, u, dem);
    }

    /// The uniforms placing `tile`, with no paint values or extras yet.
    pub fn tile_uniforms(&self, tile: &TileDraw<'_>) -> DrawUniforms {
        DrawUniforms {
            units_per_pixel: tile.units_per_pixel,
            extent: tile.extent as f32,
            ..self.view.tile_uniforms(tile.matrix, &tile.geo)
        }
    }
}

/// What [`LayerRenderer::prepare`] works with, before the main pass: passes of its own
/// are begun with [`crate::gpu::Gpu::begin_pass`].
pub struct PrepareContext<'a> {
    pub gpu: &'a Gpu,
    pub view: &'a View,
    /// Whether frames follow each other on screen, so transitions animate; screenshots
    /// show their end state at once.
    pub animate: bool,
    /// The terrain or globe as drawn this frame, for offscreen passes of 3D layers that it
    /// must hide.
    pub ground: Option<&'a super::terrain::Ground<'a>>,
}

/// A `*-translate` in pixels as tile units for `tile`, turned with the map unless the
/// `*-translate-anchor` is `viewport`.
pub fn translate_units(
    translate: [f32; 2],
    anchor: sparkle::style::TranslateAnchor,
    tile: &TileDraw<'_>,
    view: &View,
) -> [f32; 2] {
    translate_pixels(translate, anchor, view).map(|v| v * tile.units_per_pixel)
}

/// A `*-translate` in pixels along the map's axes: turned with it unless the
/// `*-translate-anchor` is `viewport`.
pub fn translate_pixels(
    translate: [f32; 2],
    anchor: sparkle::style::TranslateAnchor,
    view: &View,
) -> [f32; 2] {
    let [x, y] = translate;
    match anchor {
        sparkle::style::TranslateAnchor::Map => [x, y],
        sparkle::style::TranslateAnchor::Viewport => {
            let (sin, cos) = (view.camera.bearing.to_radians() as f32).sin_cos();
            [x * cos - y * sin, x * sin + y * cos]
        }
    }
}
