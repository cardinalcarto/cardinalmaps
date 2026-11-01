//! One renderer per style layer type, behind [`LayerRenderer`].
//!
//! To add a layer type: write `<type>.rs` here implementing [`LayerRenderer`] (plus a
//! `shaders/<type>.wgsl`), and add one line to [`create`]. Types that are listed but not
//! implemented yet have a module whose renderer draws nothing.

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

use super::gpu::{DrawUniforms, Gpu, TileGeo, UniformArena, View};
use crate::camera::Placement;

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

/// Draws all style layers of one type.
pub trait LayerRenderer {
    /// The style layer `type` this draws.
    fn layer_type(&self) -> &'static str;

    /// Upload a bucket of one of this type's layers when its tile arrives. The result is
    /// handed back as [`TileDraw::bucket`]; `None` means there's nothing to draw.
    fn upload(&mut self, gpu: &Gpu, layer: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>>;

    /// Upload new paint attributes of `bucket`, which this type's [`Self::upload`] made
    /// `data` from, after feature state changed them (the geometry is the same). Returns
    /// false if this type can't, in which case the whole bucket is uploaded again.
    fn update_paint(&mut self, _gpu: &Gpu, _data: &mut dyn Any, _bucket: &Bucket) -> bool {
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
    pub bucket: &'a dyn Any,
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

/// What [`LayerRenderer::draw`] draws with.
pub struct DrawContext<'a, 'p> {
    pub gpu: &'a Gpu,
    pub pass: &'a mut wgpu::RenderPass<'p>,
    pub uniforms: &'a mut UniformArena,
    pub view: &'a View,
}

impl DrawContext<'_, '_> {
    /// Store `u` for this draw and bind it as group 0.
    pub fn bind_uniforms(&mut self, u: &DrawUniforms) {
        let (group, offset) = self.uniforms.push(self.gpu, u);
        self.pass.set_bind_group(0, &group, &[offset]);
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

/// What [`LayerRenderer::prepare`] works with: the frame's encoder, before the main pass.
pub struct PrepareContext<'a> {
    pub gpu: &'a Gpu,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub uniforms: &'a mut UniformArena,
    pub view: &'a View,
    /// Whether frames follow each other on screen, so transitions animate; offscreen
    /// screenshots show their end state at once.
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
    let [x, y] = translate;
    let [x, y] = match anchor {
        sparkle::style::TranslateAnchor::Map => [x, y],
        sparkle::style::TranslateAnchor::Viewport => {
            let (sin, cos) = (view.camera.bearing.to_radians() as f32).sin_cos();
            [x * cos - y * sin, x * sin + y * cos]
        }
    };
    [x * tile.units_per_pixel, y * tile.units_per_pixel]
}
