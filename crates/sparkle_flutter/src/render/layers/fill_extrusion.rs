//! `fill-extrusion` layers: 3D buildings.
//!
//! As in MapLibre, each layer is drawn in [`LayerRenderer::prepare`] into an offscreen
//! image with its own depth buffer, opaque, and then composited over the map at the
//! layer's opacity. Drawing straight into the map at less than full opacity would blend
//! every wall and roof behind the nearest one too.
//!
//! Over terrain and on the globe, the frame's first layer starts the depth buffer with the
//! ground's depth, so hills hide the buildings behind them.
//!
//! The depth buffer is shared by every extrusion layer of a frame, so a later layer's
//! buildings are hidden behind an earlier one's (but not the other way around, since the
//! earlier layer is composited first). After compositing, the buildings are drawn once
//! more into the map's own depth buffer, depth only, so that later layers that test depth
//! (`DepthMode::Test`) are hidden behind them.
//!
//! Tiles don't clip extrusions with the stencil masks, since walls rise above the tile's
//! footprint on screen; the shader cuts them at the tile's edges instead.

use std::any::Any;
use std::collections::HashMap;

use sparkle::style::{LayerKind, StyleLayer};
use sparkle::tile::{FillExtrusionPattern, FillExtrusionVertex};
use sparkle::{Bucket, BucketGeometry};

use super::{
    BucketData, DrawContext, LayerDraw, LayerRenderer, PrepareContext, bind_uniforms,
    translate_units,
};
use crate::gpu::{
    Blend, Buffer, DepthMode, DepthStencilDesc, Load, Pass, PassDesc, Pipeline, PipelineSpec,
    StepMode, Store, Texture, TextureFormat, Topology, VertexBufferLayout, VertexFormat,
};
use crate::render::paint::{GpuPaint, UniformsMemo};
use crate::render::view::View;
use crate::render::{Gpu, LINEAR, NEAREST};

pub struct FillExtrusionRenderer {
    /// By paint signature, whether buckets have per-vertex patterns, and whether the
    /// pipeline only writes depth (into the map's depth buffer).
    pipelines: HashMap<(u32, bool, bool), Pipeline>,
    composite: Pipeline,
    /// A no-pattern [`FillExtrusionPattern`], read once per instance by buckets without
    /// patterns so that every pipeline can use the same shader.
    no_pattern: Buffer,
    targets: Option<Targets>,
    /// Whether a layer was drawn since the last `prepare`: the next one starts a frame and
    /// clears the shared depth buffer.
    frame_done: bool,
}

/// The offscreen images, at the viewport's size.
struct Targets {
    size: [u32; 2],
    /// Multisampled color, resolved into each layer's image (absent without multisampling).
    msaa: Option<Texture>,
    depth: Texture,
    /// Each layer's resolved image, by style layer index.
    layers: HashMap<usize, LayerImage>,
}

struct LayerImage {
    texture: Texture,
    /// Whether `prepare` drew it this frame.
    ready: bool,
}

struct GpuExtrusion {
    vertices: Buffer,
    /// `None` when the bucket has no patterns.
    patterns: Option<Buffer>,
    triangles: Buffer,
    triangle_count: u32,
    paint: GpuPaint,
}

impl FillExtrusionRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let composite = gpu.pipeline(&PipelineSpec {
            program: "FillExtrusionComposite",
            vertex_buffers: vec![Gpu::quad_layout()],
            topology: Topology::TriangleStrip,
            clip: false,
            ..Default::default()
        });
        Self {
            pipelines: HashMap::new(),
            composite,
            no_pattern: gpu.buffer(bytemuck::bytes_of(&FillExtrusionPattern::default())),
            targets: None,
            frame_done: true,
        }
    }

    fn pipeline(
        &mut self,
        gpu: &Gpu,
        paint: &GpuPaint,
        patterns: bool,
        depth_only: bool,
    ) -> Pipeline {
        *self
            .pipelines
            .entry((paint.signature(), patterns, depth_only))
            .or_insert_with(|| {
                // Position and edge distance, then normal and top.
                let vertex = VertexBufferLayout::packed(&[
                    ("pos_edge", VertexFormat::Float32x3),
                    ("normal_top", VertexFormat::Float32x4),
                ]);
                debug_assert_eq!(vertex.stride as usize, size_of::<FillExtrusionVertex>());
                let mut pattern = VertexBufferLayout::packed(&[
                    ("pattern_rect", VertexFormat::Float32x4),
                    ("pattern_size", VertexFormat::Float32x2),
                ]);
                debug_assert_eq!(pattern.stride as usize, size_of::<FillExtrusionPattern>());
                if !patterns {
                    pattern.step = StepMode::Instance;
                }
                let mut vertex_buffers = vec![vertex, pattern];
                vertex_buffers.extend(paint.vertex_buffers());
                gpu.pipeline(&PipelineSpec {
                    program: "FillExtrusion",
                    vertex_buffers,
                    clip: false,
                    depth: DepthMode::TestWrite,
                    blend: if depth_only {
                        Blend::NoColor
                    } else {
                        Blend::Premultiplied
                    },
                    ..Default::default()
                })
            })
    }

    /// The offscreen targets at the current viewport size, and the image for `layer`.
    fn targets(&mut self, gpu: &Gpu, size: [u32; 2], layer: usize) -> &mut Targets {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let samples = gpu.info.sample_count;
            self.targets = Some(Targets {
                size,
                msaa: (samples > 1).then(|| gpu.target(size, TextureFormat::Screen, samples)),
                depth: gpu.target(size, TextureFormat::DepthStencil, samples),
                layers: HashMap::new(),
            });
        }
        let targets = self.targets.as_mut().expect("just created");
        targets.layers.entry(layer).or_insert_with(|| LayerImage {
            texture: gpu.target(size, TextureFormat::Screen, 1),
            ready: false,
        });
        targets
    }

    /// Draw every tile's roofs and walls into `pass`: lit colors into the layer's image, or
    /// only depth into the map's depth buffer.
    fn draw_tiles(
        &mut self,
        pass: &mut Pass<'_>,
        gpu: &Gpu,
        view: &View,
        layer: &LayerDraw<'_>,
        depth_only: bool,
    ) {
        let LayerKind::FillExtrusion(extrusion) = &layer.layer.kind else {
            return;
        };
        let paint = &extrusion.paint;
        let zoom = view.camera.zoom;
        // Bound in place of the sprite atlas when there is none.
        let (atlas, atlas_size) = match &gpu.images {
            Some(i) => (&i.texture, i.size),
            None => (gpu.no_dem(), [1, 1]),
        };

        let props = paint.attributes();

        let mut memo = UniformsMemo::default();
        let translate = paint.translate.evaluate_at_zoom(zoom);
        let anchor = paint.translate_anchor.evaluate_at_zoom(zoom);
        let vertical_gradient = paint.vertical_gradient.evaluate_at_zoom(zoom);
        let light = &view.light;
        let pixels_per_meter = view.camera.pixels_per_meter() as f32;
        // Patterns keep their size between integer zoom levels, as MapLibre's do.
        let pattern_scale = (zoom - zoom.floor()).exp2() as f32;
        for tile in &layer.tiles {
            let Some(bucket) = tile.bucket::<GpuExtrusion>() else {
                continue;
            };
            let mut u = view.tile_uniforms(tile.matrix, &tile.geo);
            u.units_per_pixel = tile.units_per_pixel;
            u.extent = tile.extent as f32;
            u.paint = memo.uniforms(&bucket.paint.layout, &props, zoom);
            let [tx, ty] = translate_units(translate, anchor, tile, view);
            let [lx, ly, lz] = light.position;
            let [r, g, b] = light.color;
            u.extra[0] = [lx, ly, lz, light.intensity];
            u.extra[1] = [r, g, b, 1.0];
            u.extra[2] = [
                tx,
                ty,
                pixels_per_meter,
                f32::from(u8::from(vertical_gradient)),
            ];
            u.extra[3] = [
                tile.units_per_pixel * pattern_scale,
                atlas_size[0] as f32,
                atlas_size[1] as f32,
                0.0,
            ];

            let has_patterns = bucket.patterns.is_some();
            let pipeline = self.pipeline(gpu, &bucket.paint, has_patterns, depth_only);
            pass.set_pipeline(&pipeline);
            bind_uniforms(gpu, pass, &u, tile.geo.dem_texture.as_ref());
            pass.set_texture("pattern_atlas", atlas, LINEAR);
            pass.set_vertex_buffer(0, bucket.vertices.slice());
            pass.set_vertex_buffer(
                1,
                bucket.patterns.as_ref().unwrap_or(&self.no_pattern).slice(),
            );
            bucket.paint.bind(gpu, pass, 2);
            pass.set_index_buffer(bucket.triangles.slice(), false);
            pass.draw_indexed(bucket.triangle_count, 1);
        }
    }
}

impl LayerRenderer for FillExtrusionRenderer {
    fn layer_type(&self) -> &'static str {
        "fill-extrusion"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<BucketData> {
        let BucketGeometry::FillExtrusion(extrusion) = &bucket.geometry else {
            return None;
        };
        Some(Box::new(GpuExtrusion {
            vertices: gpu.buffer(bytemuck::cast_slice(&extrusion.vertices)),
            patterns: (!extrusion.patterns.is_empty())
                .then(|| gpu.buffer(bytemuck::cast_slice(&extrusion.patterns))),
            triangles: gpu.buffer(bytemuck::cast_slice(&extrusion.triangles)),
            triangle_count: extrusion.triangles.len() as u32,
            paint: GpuPaint::new(gpu, &extrusion.paint),
        }))
    }

    fn update_paint(
        &mut self,
        gpu: &Gpu,
        data: &mut (dyn Any + Send + Sync),
        bucket: &Bucket,
    ) -> bool {
        let (Some(data), BucketGeometry::FillExtrusion(extrusion)) =
            (data.downcast_mut::<GpuExtrusion>(), &bucket.geometry)
        else {
            return false;
        };
        data.paint.update(gpu, &extrusion.paint)
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>, layer: &LayerDraw<'_>) {
        let LayerKind::FillExtrusion(extrusion) = &layer.layer.kind else {
            return;
        };
        let paint = &extrusion.paint;
        let camera = &cx.view.camera;
        let zoom = camera.zoom;
        if paint.opacity.evaluate_at_zoom(zoom) <= 0.0 {
            return;
        }
        let first_in_frame = std::mem::replace(&mut self.frame_done, false);
        let gpu = cx.gpu;
        let size = camera.viewport.map(|v| (v as u32).max(1));
        let targets = self.targets(gpu, size, layer.index);
        if first_in_frame {
            targets.layers.values_mut().for_each(|l| l.ready = false);
        }
        let image = targets.layers.get_mut(&layer.index).expect("just created");
        image.ready = true;
        let (color, resolve) = match &targets.msaa {
            Some(msaa) => (msaa, Some(&image.texture)),
            None => (&image.texture, None),
        };
        let mut pass = gpu.begin_pass(&PassDesc {
            color,
            resolve,
            color_load: Load::Clear([0.0; 4]),
            color_store: if resolve.is_some() {
                Store::Resolve
            } else {
                Store::Store
            },
            depth_stencil: Some(DepthStencilDesc {
                texture: &targets.depth,
                depth_load: if first_in_frame {
                    Load::Clear(1.0)
                } else {
                    Load::Load
                },
                depth_store: Store::Store,
                stencil_load: Load::DontCare,
                stencil_store: Store::DontCare,
            }),
        });

        if first_in_frame && let Some(ground) = cx.ground {
            // Hills and the globe hide the buildings behind them.
            ground.draw_depth(gpu, &mut pass, cx.view);
        }
        self.draw_tiles(&mut pass, gpu, cx.view, layer, false);
    }

    fn draw(&mut self, cx: &mut DrawContext<'_, '_>, layer: &LayerDraw<'_>) {
        self.frame_done = true;
        let LayerKind::FillExtrusion(extrusion) = &layer.layer.kind else {
            return;
        };
        let Some(image) = self
            .targets
            .as_ref()
            .and_then(|t| t.layers.get(&layer.index))
            .filter(|i| i.ready)
        else {
            return;
        };
        let mut u = cx.view.uniforms();
        u.extra[0][0] = extrusion
            .paint
            .opacity
            .evaluate_at_zoom(cx.view.camera.zoom);
        cx.pass.set_pipeline(&self.composite);
        cx.bind_uniforms(&u, None);
        // Same size as the screen: each fragment takes its own pixel.
        cx.pass.set_texture("layer_image", &image.texture, NEAREST);
        cx.pass.set_vertex_buffer(0, cx.gpu.quad.slice());
        cx.pass.draw(4, 1);
        // The buildings' depth, for later layers that test against it.
        let gpu = cx.gpu;
        self.draw_tiles(cx.pass, gpu, cx.view, layer, true);
    }
}
