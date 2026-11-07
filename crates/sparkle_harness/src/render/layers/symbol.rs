//! `symbol` layers: text and icon quads scaled and turned in the vertex shader (see
//! `sparkle::tile::SymbolBucket`). Each tile's icons are drawn before its text, and
//! distance fields draw their halos in a pass before their fill, as in MapLibre.
//!
//! Which labels show, and where glyphs along lines go, comes from
//! `sparkle::symbol::placement` once per frame, as one extra vertex value per quad corner
//! in a buffer rewritten every frame.

use std::any::Any;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;


use sparkle::paint::PaintArrays;
use sparkle::style::{LayerKind, StyleLayer, SymbolLayer};
use sparkle::symbol::Labels;
use sparkle::symbol::placement::{
    BucketKey, DEFAULT_FADE_DURATION, DrawMode, PartAlignment, PlacedBucket, PlacementBucket,
    PlacementView, SymbolPlacer,
};
use sparkle::tile::{SymbolQuads, SymbolVertex};
use sparkle::{Bucket, BucketGeometry};

use super::{DrawContext, LayerDraw, LayerRenderer, PrepareContext, TileDraw, translate_units};
use crate::render::gpu::{Gpu, PipelineSpec};

/// Geometry attributes take locations 0..6 and the placed values 6, so paint attributes
/// start after them.
const PAINT_LOCATION: u32 = 7;

const TEXT_PRELUDE: &str = "
const IS_TEXT: bool = true;
fn layout_scale(p: PaintIn, flags: u32) -> f32 { return paint_size(p) / 24.0; }
";

const ICON_PRELUDE: &str = "
const IS_TEXT: bool = false;
// Icons fitted to their text are laid out in text units.
fn layout_scale(p: PaintIn, flags: u32) -> f32 {
    if (flags & 4u) != 0u { return paint_text_size(p) / 24.0; }
    return paint_size(p);
}
";

pub struct SymbolRenderer {
    /// By paint signature and whether it draws text.
    pipelines: HashMap<(u32, bool), wgpu::RenderPipeline>,
    texture_layout: wgpu::BindGroupLayout,
    /// The sprite atlas, made on first use.
    icon_group: Option<wgpu::BindGroup>,
    /// Made on the first frame, when it's known whether frames animate.
    placer: Option<SymbolPlacer>,
    start: Instant,
    animating: bool,
}

struct GpuQuads {
    vertices: wgpu::Buffer,
    /// [`PlacedBucket`] values, one per vertex, rewritten every frame.
    placed: wgpu::Buffer,
    triangles: wgpu::Buffer,
    triangle_count: u32,
    /// The bucket's own triangle order, to restore after drawing sorted by viewport `y`.
    default_triangles: Vec<u32>,
    paint_buffer: Option<wgpu::Buffer>,
    /// The layout of `paint_buffer`; its data isn't kept.
    paint: PaintArrays,
}

struct GpuSymbol {
    text: Option<(GpuQuads, wgpu::BindGroup)>,
    icons: Option<GpuQuads>,
    labels: Arc<Labels>,
    /// The [`PlacedBucket::triangles_version`] the triangle buffers hold.
    triangles_version: Cell<u64>,
}

/// How one kind of quad (text or icons) of a layer is drawn this frame.
struct Part {
    is_text: bool,
    alignment: PartAlignment,
    translate: [f32; 2],
    anchor: sparkle::style::TranslateAnchor,
    halo: bool,
}

/// The key placement knows a tile's bucket of a layer by.
fn bucket_key(layer: &LayerDraw<'_>, tile: &TileDraw<'_>) -> BucketKey {
    BucketKey {
        layer: layer.index,
        z: tile.placement.z,
        x: tile.placement.x,
        y: tile.placement.y,
    }
}

/// A layer's tiles, each once: an ancestor standing in for several missing tiles is
/// listed once for each.
fn unique_tiles<'a, 'b>(layer: &'b LayerDraw<'a>) -> impl Iterator<Item = &'b TileDraw<'a>> {
    let mut seen = HashSet::new();
    layer.tiles.iter().filter(move |t| seen.insert(t.placement))
}

impl SymbolRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let texture_layout = gpu.texture_layout("symbol atlas", 1);
        Self {
            pipelines: HashMap::new(),
            texture_layout,
            icon_group: None,
            placer: None,
            start: Instant::now(),
            animating: false,
        }
    }

    fn bind_group(&self, gpu: &Gpu, view: &wgpu::TextureView) -> wgpu::BindGroup {
        gpu.bind_textures(&self.texture_layout, &[view], &gpu.linear_sampler)
    }

    fn pipeline(&mut self, gpu: &Gpu, paint: &PaintArrays, is_text: bool) -> &wgpu::RenderPipeline {
        let layout = &self.texture_layout;
        self.pipelines
            .entry((paint.signature(), is_text))
            .or_insert_with(|| {
                let prelude = if is_text { TEXT_PRELUDE } else { ICON_PRELUDE };
                let shader = format!("{prelude}\n{}", include_str!("../../shaders/symbol.wgsl"));
                gpu.create_pipeline(&PipelineSpec {
                    label: if is_text {
                        "symbol text"
                    } else {
                        "symbol icons"
                    },
                    shader: &shader,
                    vertex_buffers: &[
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<SymbolVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x2, 1 => Float32x2, 2 => Float32x2, 3 => Float32x2,
                                4 => Unorm8x4, 5 => Uint32
                            ],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<[f32; 4]>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![6 => Float32x4],
                        },
                    ],
                    paint: Some(paint),
                    paint_location: PAINT_LOCATION,
                    // Labels reach over tile edges, and belong to one tile only.
                    clip: false,
                    bind_group_layouts: &[layout],
                    ..Default::default()
                })
            })
    }

    fn upload_quads(gpu: &Gpu, quads: &SymbolQuads) -> Option<GpuQuads> {
        if quads.vertices.is_empty() {
            return None;
        }
        let paint = quads.paint.layout();
        // Hidden until placement says otherwise.
        let placed = vec![[0f32; 4]; quads.vertices.len()];
        Some(GpuQuads {
            vertices: gpu.buffer(
                "symbol vertices",
                bytemuck::cast_slice(&quads.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            placed: gpu.buffer(
                "symbol placement",
                bytemuck::cast_slice(&placed),
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            ),
            triangles: gpu.buffer(
                "symbol triangles",
                bytemuck::cast_slice(&quads.triangles),
                wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            ),
            triangle_count: quads.triangles.len() as u32,
            default_triangles: quads.triangles.clone(),
            paint_buffer: gpu.paint_buffer(&quads.paint),
            paint,
        })
    }

    /// Upload a bucket's placement for this frame.
    fn write_placed(gpu: &Gpu, bucket: &GpuSymbol, placed: &PlacedBucket) {
        let sorted = bucket.triangles_version.get() != placed.triangles_version;
        let write = |quads: &GpuQuads, values: &[[f32; 4]], triangles: &Option<Vec<u32>>| {
            if !values.is_empty() {
                gpu.queue
                    .write_buffer(&quads.placed, 0, bytemuck::cast_slice(values));
            }
            if sorted {
                let triangles = triangles.as_deref().unwrap_or(&quads.default_triangles);
                gpu.queue
                    .write_buffer(&quads.triangles, 0, bytemuck::cast_slice(triangles));
            }
        };
        if let Some((quads, _)) = &bucket.text {
            write(quads, &placed.text, &placed.text_triangles);
        }
        if let Some(quads) = &bucket.icons {
            write(quads, &placed.icons, &placed.icon_triangles);
        }
        bucket.triangles_version.set(placed.triangles_version);
    }

    fn draw_part(
        &mut self,
        cx: &mut DrawContext<'_, '_>,
        layer: &SymbolLayer,
        tile: &TileDraw<'_>,
        quads: &GpuQuads,
        group: &wgpu::BindGroup,
        part: &Part,
    ) {
        let zoom = cx.view.camera.zoom;
        let mut u = cx.tile_uniforms(tile);
        u.paint = if part.is_text {
            quads.paint.uniforms(&layer.text_attributes(), zoom)
        } else {
            quads.paint.uniforms(&layer.icon_attributes(), zoom)
        };
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let mode = part.alignment.draw_mode();
        // Placement already moved labels along lines by their translation.
        let [tx, ty] = match mode {
            DrawMode::Point => translate_units(part.translate, part.anchor, tile, cx.view),
            _ => [0.0; 2],
        };
        u.extra[1] = [tx, ty, cx.view.camera.camera_to_center() as f32, 0.0];
        let pipeline = self.pipeline(cx.gpu, &quads.paint, part.is_text).clone();
        cx.pass.set_pipeline(&pipeline);
        cx.pass.set_bind_group(1, group, &[]);
        cx.pass.set_vertex_buffer(0, quads.vertices.slice(..));
        cx.pass.set_vertex_buffer(1, quads.placed.slice(..));
        if let Some(buffer) = &quads.paint_buffer {
            cx.pass.set_vertex_buffer(2, buffer.slice(..));
        }
        cx.pass
            .set_index_buffer(quads.triangles.slice(..), wgpu::IndexFormat::Uint32);
        let passes: &[bool] = if part.halo { &[true, false] } else { &[false] };
        let mode = match mode {
            DrawMode::Point => 0.0,
            DrawMode::LineInTile => 1.0,
            DrawMode::LineOnScreen => 2.0,
        };
        for &halo in passes {
            u.extra[0] = [
                flag(halo),
                flag(part.alignment.pitch_with_map),
                flag(part.alignment.rotate_with_map),
                mode,
            ];
            cx.bind_uniforms(&u);
            cx.pass.draw_indexed(0..quads.triangle_count, 0, 0..1);
        }
    }
}

/// Whether a halo pass may draw anything.
fn has_halo(width: &sparkle::style::PropertyValue<f32>) -> bool {
    width.as_constant().is_none_or(|&w| w > 0.0)
}

impl LayerRenderer for SymbolRenderer {
    fn layer_type(&self) -> &'static str {
        "symbol"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::Symbol(symbol) = &bucket.geometry else {
            return None;
        };
        let text = Self::upload_quads(gpu, &symbol.text).map(|quads| {
            let atlas = &symbol.glyphs;
            let texture = gpu.texture_with_data(
                &wgpu::TextureDescriptor {
                    label: Some("glyphs"),
                    size: wgpu::Extent3d {
                        width: atlas.width,
                        height: atlas.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                &atlas.pixels,
            );
            let group = self.bind_group(gpu, &texture.create_view(&Default::default()));
            (quads, group)
        });
        let icons = Self::upload_quads(gpu, &symbol.icons);
        if icons.is_some()
            && self.icon_group.is_none()
            && let Some(images) = &gpu.images
        {
            self.icon_group = Some(self.bind_group(gpu, &images.view));
        }
        if text.is_none() && icons.is_none() {
            return None;
        }
        Some(Box::new(GpuSymbol {
            text,
            icons,
            labels: symbol.labels.clone(),
            triangles_version: Cell::new(0),
        }))
    }

    fn prepare_frame(&mut self, cx: &mut PrepareContext<'_>, layers: &[&LayerDraw<'_>]) {
        let placer = self.placer.get_or_insert_with(|| {
            SymbolPlacer::new(if cx.animate {
                DEFAULT_FADE_DURATION
            } else {
                0.0
            })
        });
        let camera = &cx.view.camera;
        let view = PlacementView {
            viewport: camera.logical_size().map(|v| v as f32),
            zoom: camera.zoom,
            bearing: camera.bearing.to_radians() as f32,
            camera_to_center: camera.camera_to_center() as f32,
        };
        let mut buckets = Vec::new();
        let mut uploaded = Vec::new();
        for layer in layers {
            let LayerKind::Symbol(symbol) = &layer.layer.kind else {
                continue;
            };
            for tile in unique_tiles(layer) {
                let Some(bucket) = tile.bucket::<GpuSymbol>() else {
                    continue;
                };
                let key = bucket_key(layer, tile);
                buckets.push(PlacementBucket {
                    key,
                    layer: symbol,
                    labels: &bucket.labels,
                    projection: camera.tile_projection(
                        tile.matrix,
                        tile.placement,
                        tile.extent,
                        tile.elevation.clone(),
                    ),
                    units_per_pixel: tile.units_per_pixel,
                });
                uploaded.push((key, bucket));
            }
        }
        let now = self.start.elapsed().as_secs_f64();
        self.animating = placer.update(now, &view, &buckets);
        log::debug!(
            "placed {} symbol buckets in {:.2} ms",
            buckets.len(),
            (self.start.elapsed().as_secs_f64() - now) * 1000.0
        );
        for (key, bucket) in uploaded {
            if let Some(placed) = placer.bucket(&key) {
                Self::write_placed(cx.gpu, bucket, placed);
            }
        }
    }

    fn animating(&self) -> bool {
        self.animating
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Symbol(symbol) = &layer.layer.kind else {
            return;
        };
        let zoom = cx.view.camera.zoom;
        let p = &symbol.paint;
        for tile in unique_tiles(layer) {
            let Some(bucket) = tile.bucket::<GpuSymbol>() else {
                continue;
            };
            let labels = &bucket.labels;
            let text = Part {
                is_text: true,
                alignment: PartAlignment::text(symbol, zoom, labels.text_along_line),
                translate: p.text_translate.evaluate_at_zoom(zoom),
                anchor: p.text_translate_anchor.evaluate_at_zoom(zoom),
                halo: has_halo(&p.text_halo_width),
            };
            let icons = Part {
                is_text: false,
                alignment: PartAlignment::icon(symbol, zoom, labels.icons_along_line),
                translate: p.icon_translate.evaluate_at_zoom(zoom),
                anchor: p.icon_translate_anchor.evaluate_at_zoom(zoom),
                halo: has_halo(&p.icon_halo_width),
            };
            if let (Some(quads), Some(group)) = (&bucket.icons, self.icon_group.clone()) {
                self.draw_part(cx, symbol, tile, quads, &group, &icons);
            }
            if let Some((quads, group)) = &bucket.text {
                self.draw_part(cx, symbol, tile, quads, group, &text);
            }
        }
    }
}
