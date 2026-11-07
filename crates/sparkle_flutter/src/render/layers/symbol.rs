//! `symbol` layers: text and icon quads scaled and turned in the vertex shader (see
//! `sparkle::tile::SymbolBucket`). Each tile's icons are drawn before its text, and
//! distance fields draw their halos in a pass before their fill, as in MapLibre.
//!
//! Which labels show, and where glyphs along lines go, comes from
//! `sparkle::symbol::placement` once per frame, as one extra vertex value per quad corner
//! bound as frame data, with the triangles sorted for the frame when placement sorts them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use sparkle::style::{LayerKind, StyleLayer, SymbolLayer};
use sparkle::symbol::Labels;
use sparkle::symbol::placement::{
    BucketKey, DEFAULT_FADE_DURATION, DrawMode, LabelAt, PartAlignment, PlacementBucket, PlacementView,
    SymbolPlacer,
};
use sparkle::tile::{SymbolQuads, SymbolVertex};
use sparkle::{Bucket, BucketGeometry};

use super::{
    BucketData, DrawContext, LayerDraw, LayerRenderer, PrepareContext, TileDraw, translate_units,
};
use crate::gpu::{
    Buffer, Pipeline, PipelineSpec, Texture, TextureFormat, VertexBufferLayout, VertexFormat,
};
use crate::render::paint::{GpuPaint, UniformsMemo, bytes};
use crate::render::{Gpu, LINEAR};

/// A [`SymbolVertex`] as the shaders take it: floats only (integer attributes don't work
/// everywhere), packed into few inputs since the icon shader also takes 7 paint
/// properties.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct Vertex {
    /// `anchor` (xy) and `offset` (zw).
    anchor_offset: [f32; 4],
    /// `pixel_offset` (xy) and `tex` (zw).
    pixel_offset_tex: [f32; 4],
    /// `color`, as `0..1`.
    section_color: [f32; 4],
    /// `flags`, exactly: they're small integers.
    flags: f32,
}

impl From<&SymbolVertex> for Vertex {
    fn from(v: &SymbolVertex) -> Self {
        Self {
            anchor_offset: [v.anchor[0], v.anchor[1], v.offset[0], v.offset[1]],
            pixel_offset_tex: [v.pixel_offset[0], v.pixel_offset[1], v.tex[0], v.tex[1]],
            section_color: v.color.map(|c| f32::from(c) / 255.0),
            flags: v.flags as f32,
        }
    }
}

pub struct SymbolRenderer {
    /// By paint signature and whether it draws text.
    pipelines: HashMap<(u32, bool), Pipeline>,
    /// Made on the first frame, when it's known whether frames animate.
    placer: Option<SymbolPlacer>,
    start: Instant,
    animating: bool,
}

struct GpuQuads {
    vertices: Buffer,
    vertex_count: usize,
    /// The bucket's own triangle order, drawn unless placement sorts them by viewport `y`.
    triangles: Buffer,
    triangle_count: u32,
    paint: GpuPaint,
}

struct GpuSymbol {
    /// With the bucket's glyph atlas.
    text: Option<(GpuQuads, Texture)>,
    icons: Option<GpuQuads>,
    labels: Arc<Labels>,
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

/// The pipeline for quads with `paint`, from `pipelines` or made.
fn pipeline(
    pipelines: &mut HashMap<(u32, bool), Pipeline>,
    gpu: &Gpu,
    paint: &GpuPaint,
    is_text: bool,
) -> Pipeline {
    *pipelines
        .entry((paint.signature(), is_text))
        .or_insert_with(|| {
            let mut vertex_buffers = vec![
                VertexBufferLayout::packed(&[
                    ("anchor_offset", VertexFormat::Float32x4),
                    ("pixel_offset_tex", VertexFormat::Float32x4),
                    ("section_color", VertexFormat::Float32x4),
                    ("flags", VertexFormat::Float32),
                ]),
                VertexBufferLayout::packed(&[("placed", VertexFormat::Float32x4)]),
            ];
            vertex_buffers.extend(paint.vertex_buffers());
            gpu.pipeline(&PipelineSpec {
                program: if is_text { "SymbolText" } else { "SymbolIcon" },
                vertex_buffers,
                // Labels reach over tile edges, and belong to one tile only.
                clip: false,
                ..Default::default()
            })
        })
}

impl SymbolRenderer {
    pub fn new(_gpu: &Gpu) -> Self {
        Self {
            pipelines: HashMap::new(),
            placer: None,
            start: Instant::now(),
            animating: false,
        }
    }

    fn upload_quads(gpu: &Gpu, quads: &SymbolQuads) -> Option<GpuQuads> {
        if quads.vertices.is_empty() || quads.triangles.is_empty() {
            return None;
        }
        let vertices: Vec<Vertex> = quads.vertices.iter().map(Vertex::from).collect();
        Some(GpuQuads {
            vertices: gpu.buffer(bytemuck::cast_slice(&vertices)),
            vertex_count: vertices.len(),
            triangles: gpu.buffer(bytemuck::cast_slice(&quads.triangles)),
            triangle_count: quads.triangles.len() as u32,
            paint: GpuPaint::new(gpu, &quads.paint),
        })
    }
}

/// Draw one part of a tile's bucket with its placement this frame: `placed`, one value per
/// vertex, and the triangles placement `sorted`, if it did.
#[allow(clippy::too_many_arguments)]
fn draw_part(
    pipelines: &mut HashMap<(u32, bool), Pipeline>,
    memo: &mut UniformsMemo,
    cx: &mut DrawContext<'_, '_>,
    layer: &SymbolLayer,
    tile: &TileDraw<'_>,
    quads: &GpuQuads,
    atlas: (&Texture, [u32; 2]),
    part: &Part,
    placed: &[[f32; 4]],
    sorted: Option<&[u32]>,
) {
    if placed.len() != quads.vertex_count {
        // Not placed yet, so hidden.
        return;
    }
    let zoom = cx.view.camera.zoom;
    let mut u = cx.tile_uniforms(tile);
    u.paint = if part.is_text {
        memo.uniforms(&quads.paint.layout, &layer.text_attributes(), zoom)
    } else {
        memo.uniforms(&quads.paint.layout, &layer.icon_attributes(), zoom)
    };
    let flag = |b: bool| if b { 1.0 } else { 0.0 };
    let mode = part.alignment.draw_mode();
    // Placement already moved labels along lines by their translation.
    let [tx, ty] = match mode {
        DrawMode::Point => translate_units(part.translate, part.anchor, tile, cx.view),
        _ => [0.0; 2],
    };
    u.extra[1] = [tx, ty, cx.view.camera.camera_to_center() as f32, 0.0];
    let (texture, [width, height]) = atlas;
    u.extra[2] = [width as f32, height as f32, 0.0, 0.0];
    let pipeline = pipeline(pipelines, cx.gpu, &quads.paint, part.is_text);
    cx.pass.set_pipeline(&pipeline);
    cx.pass.set_texture("atlas", texture, LINEAR);
    cx.pass.set_vertex_buffer(0, quads.vertices.slice());
    cx.pass.set_vertex_buffer(1, bytes(placed));
    quads.paint.bind(cx.gpu, cx.pass, 2);
    let count = match sorted {
        Some(triangles) => {
            cx.pass.set_index_buffer(bytes(triangles), false);
            triangles.len() as u32
        }
        None => {
            cx.pass.set_index_buffer(quads.triangles.slice(), false);
            quads.triangle_count
        }
    };
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
        cx.bind_uniforms(&u, Some(tile));
        cx.pass.draw_indexed(count, 1);
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

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::Symbol(symbol) = &bucket.geometry else {
            return None;
        };
        let text = Self::upload_quads(gpu, &symbol.text).map(|quads| {
            let atlas = &symbol.glyphs;
            let texture = gpu.texture(
                [atlas.width, atlas.height],
                TextureFormat::R8,
                1,
                false,
                Some(&atlas.pixels),
            );
            (quads, texture)
        });
        let icons = Self::upload_quads(gpu, &symbol.icons);
        if text.is_none() && icons.is_none() {
            return None;
        }
        Some(Box::new(GpuSymbol {
            text,
            icons,
            labels: symbol.labels.clone(),
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
        for layer in layers {
            let LayerKind::Symbol(symbol) = &layer.layer.kind else {
                continue;
            };
            for tile in unique_tiles(layer) {
                let Some(bucket) = tile.bucket::<GpuSymbol>() else {
                    continue;
                };
                buckets.push(PlacementBucket {
                    key: bucket_key(layer, tile),
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
            }
        }
        let now = self.start.elapsed().as_secs_f64();
        self.animating = placer.update(now, &view, &buckets);
        log::debug!(
            "placed {} symbol buckets in {:.2} ms",
            buckets.len(),
            (self.start.elapsed().as_secs_f64() - now) * 1000.0
        );
    }

    fn animating(&self) -> bool {
        self.animating
    }

    fn labels_at(&self, point: [f32; 2], tolerance: f32) -> Vec<LabelAt> {
        self.placer
            .as_ref()
            .map_or_else(Vec::new, |p| p.labels_at(point, tolerance))
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        let LayerKind::Symbol(symbol) = &layer.layer.kind else {
            return;
        };
        let Some(placer) = &self.placer else {
            return;
        };
        let gpu = cx.gpu;
        let zoom = cx.view.camera.zoom;
        let p = &symbol.paint;
        let mut text_memo = UniformsMemo::default();
        let mut icon_memo = UniformsMemo::default();
        for tile in unique_tiles(layer) {
            let Some(bucket) = tile.bucket::<GpuSymbol>() else {
                continue;
            };
            // The values placement wrote for this frame.
            let Some(placed) = placer.bucket(&bucket_key(layer, tile)) else {
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
            if let (Some(quads), Some(images)) = (&bucket.icons, &gpu.images) {
                draw_part(
                    &mut self.pipelines,
                    &mut icon_memo,
                    cx,
                    symbol,
                    tile,
                    quads,
                    (&images.texture, images.size),
                    &icons,
                    &placed.icons,
                    placed.icon_triangles.as_deref(),
                );
            }
            if let Some((quads, glyphs)) = &bucket.text {
                draw_part(
                    &mut self.pipelines,
                    &mut text_memo,
                    cx,
                    symbol,
                    tile,
                    quads,
                    (glyphs, glyphs.size()),
                    &text,
                    &placed.text,
                    placed.text_triangles.as_deref(),
                );
            }
        }
    }
}
