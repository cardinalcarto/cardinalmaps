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

use sparkle::paint::PaintArrays;
use sparkle::style::{LayerKind, StyleLayer};
use sparkle::tile::{FillExtrusionPattern, FillExtrusionVertex};
use sparkle::{Bucket, BucketGeometry};

use super::{DrawContext, LayerDraw, LayerRenderer, PrepareContext, translate_units};
use crate::render::gpu::{DEPTH_STENCIL_FORMAT, DepthMode, Gpu, PipelineSpec, UniformArena, View};

pub struct FillExtrusionRenderer {
    /// By paint signature, whether buckets have per-vertex patterns, and whether the
    /// pipeline only writes depth (into the map's depth buffer).
    pipelines: HashMap<(u32, bool, bool), wgpu::RenderPipeline>,
    composite: wgpu::RenderPipeline,
    pattern_layout: wgpu::BindGroupLayout,
    image_layout: wgpu::BindGroupLayout,
    /// Bound in place of the sprite atlas when there is none.
    empty_atlas: wgpu::TextureView,
    /// A no-pattern [`FillExtrusionPattern`], read once per instance by buckets without
    /// patterns so that every pipeline can use the same shader.
    no_pattern: wgpu::Buffer,
    targets: Option<Targets>,
    /// Whether a layer was drawn since the last `prepare`: the next one starts a frame and
    /// clears the shared depth buffer.
    frame_done: bool,
}

/// The offscreen images, at the viewport's size.
struct Targets {
    size: [u32; 2],
    /// Multisampled color, resolved into each layer's image (absent without multisampling).
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
    /// Each layer's resolved image, by style layer index.
    layers: HashMap<usize, LayerImage>,
}

struct LayerImage {
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
    /// Whether `prepare` drew it this frame.
    ready: bool,
}

struct GpuExtrusion {
    vertices: wgpu::Buffer,
    /// `None` when the bucket has no patterns.
    patterns: Option<wgpu::Buffer>,
    triangles: wgpu::Buffer,
    triangle_count: u32,
    paint_buffer: Option<wgpu::Buffer>,
    /// The layout of `paint_buffer`; its data isn't kept.
    paint: PaintArrays,
}

impl FillExtrusionRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let pattern_layout = gpu.texture_layout("fill-extrusion pattern", 1);
        let image_layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fill-extrusion image"),
                entries: &[texture_entry(0)],
            });
        let composite = gpu.create_pipeline(&PipelineSpec {
            label: "fill-extrusion composite",
            shader: include_str!("../../shaders/fill_extrusion_composite.wgsl"),
            clip: false,
            bind_group_layouts: &[&image_layout],
            ..Default::default()
        });
        let empty_atlas = gpu
            .rgba_texture("no sprites", 1, 1, &[0; 4])
            .create_view(&Default::default());
        let no_pattern = gpu.buffer(
            "no fill-extrusion pattern",
            bytemuck::bytes_of(&FillExtrusionPattern::default()),
            wgpu::BufferUsages::VERTEX,
        );
        Self {
            pipelines: HashMap::new(),
            composite,
            pattern_layout,
            image_layout,
            empty_atlas,
            no_pattern,
            targets: None,
            frame_done: true,
        }
    }

    fn pipeline(
        &mut self,
        gpu: &Gpu,
        paint: &PaintArrays,
        patterns: bool,
        depth_only: bool,
    ) -> &wgpu::RenderPipeline {
        let pattern_layout = &self.pattern_layout;
        self.pipelines
            .entry((paint.signature(), patterns, depth_only))
            .or_insert_with(|| {
                gpu.create_pipeline(&PipelineSpec {
                    label: if depth_only {
                        "fill-extrusion depth"
                    } else {
                        "fill-extrusion"
                    },
                    shader: include_str!("../../shaders/fill_extrusion.wgsl"),
                    vertex_buffers: &[
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<FillExtrusionVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            // Position and edge distance, then normal and top.
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                        },
                        wgpu::VertexBufferLayout {
                            array_stride: size_of::<FillExtrusionPattern>() as u64,
                            step_mode: if patterns {
                                wgpu::VertexStepMode::Vertex
                            } else {
                                wgpu::VertexStepMode::Instance
                            },
                            attributes: &wgpu::vertex_attr_array![2 => Float32x4, 3 => Float32x2],
                        },
                    ],
                    paint: Some(paint),
                    clip: false,
                    depth: DepthMode::TestWrite,
                    bind_group_layouts: &[pattern_layout],
                    write_mask: if depth_only {
                        wgpu::ColorWrites::empty()
                    } else {
                        wgpu::ColorWrites::ALL
                    },
                    ..Default::default()
                })
            })
    }

    /// The offscreen targets at the current viewport size, and the image for `layer`.
    fn targets(&mut self, gpu: &Gpu, size: [u32; 2], layer: usize) -> &mut Targets {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            self.targets = Some(Targets {
                size,
                msaa: (gpu.sample_count > 1).then(|| {
                    texture(
                        gpu,
                        "fill-extrusion msaa",
                        size,
                        gpu.sample_count,
                        gpu.format,
                    )
                }),
                depth: texture(
                    gpu,
                    "fill-extrusion depth",
                    size,
                    gpu.sample_count,
                    DEPTH_STENCIL_FORMAT,
                ),
                layers: HashMap::new(),
            });
        }
        let targets = self.targets.as_mut().expect("just created");
        targets.layers.entry(layer).or_insert_with(|| {
            let view = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("fill-extrusion layer"),
                size: extent(size),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = view.create_view(&Default::default());
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fill-extrusion layer"),
                layout: &self.image_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            LayerImage {
                view,
                group,
                ready: false,
            }
        });
        targets
    }
}

impl FillExtrusionRenderer {
    /// Draw every tile's roofs and walls into `pass`: lit colors into the layer's image, or
    /// only depth into the map's depth buffer.
    fn draw_tiles(
        &mut self,
        pass: &mut wgpu::RenderPass<'_>,
        gpu: &Gpu,
        uniforms: &mut UniformArena,
        view: &View,
        layer: &LayerDraw<'_>,
        depth_only: bool,
    ) {
        let LayerKind::FillExtrusion(extrusion) = &layer.layer.kind else {
            return;
        };
        let paint = &extrusion.paint;
        let zoom = view.camera.zoom;
        let atlas = gpu.images.as_ref().map_or(&self.empty_atlas, |i| &i.view);
        let pattern_group = gpu.bind_textures(&self.pattern_layout, &[atlas], &gpu.linear_sampler);
        pass.set_bind_group(1, &pattern_group, &[]);

        let props = paint.attributes();
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
            u.paint = bucket.paint.uniforms(&props, zoom);
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
            u.extra[3][0] = tile.units_per_pixel * pattern_scale;

            let has_patterns = bucket.patterns.is_some();
            let pipeline = self
                .pipeline(gpu, &bucket.paint, has_patterns, depth_only)
                .clone();
            pass.set_pipeline(&pipeline);
            let (group, offset) = uniforms.push(gpu, &u);
            pass.set_bind_group(0, &group, &[offset]);
            pass.set_vertex_buffer(0, bucket.vertices.slice(..));
            pass.set_vertex_buffer(
                1,
                bucket
                    .patterns
                    .as_ref()
                    .unwrap_or(&self.no_pattern)
                    .slice(..),
            );
            if let Some(buffer) = &bucket.paint_buffer {
                pass.set_vertex_buffer(2, buffer.slice(..));
            }
            pass.set_index_buffer(bucket.triangles.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..bucket.triangle_count, 0, 0..1);
        }
    }
}

impl LayerRenderer for FillExtrusionRenderer {
    fn layer_type(&self) -> &'static str {
        "fill-extrusion"
    }

    fn upload(&mut self, gpu: &Gpu, _: &StyleLayer, bucket: &Bucket) -> Option<Box<dyn Any>> {
        let BucketGeometry::FillExtrusion(extrusion) = &bucket.geometry else {
            return None;
        };
        let vertex = wgpu::BufferUsages::VERTEX;
        let paint = extrusion.paint.layout();
        Some(Box::new(GpuExtrusion {
            vertices: gpu.buffer(
                "fill-extrusion vertices",
                bytemuck::cast_slice(&extrusion.vertices),
                vertex,
            ),
            patterns: (!extrusion.patterns.is_empty()).then(|| {
                gpu.buffer(
                    "fill-extrusion patterns",
                    bytemuck::cast_slice(&extrusion.patterns),
                    vertex,
                )
            }),
            triangles: gpu.buffer(
                "fill-extrusion triangles",
                bytemuck::cast_slice(&extrusion.triangles),
                wgpu::BufferUsages::INDEX,
            ),
            triangle_count: extrusion.triangles.len() as u32,
            paint_buffer: gpu.paint_buffer(&extrusion.paint),
            paint,
        }))
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
            Some(msaa) => (msaa, Some(&image.view)),
            None => (&image.view, None),
        };
        let mut pass = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fill-extrusion"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: if resolve.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: if first_in_frame {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        if first_in_frame && let Some(ground) = cx.ground {
            // Hills and the globe hide the buildings behind them.
            ground.draw_depth(gpu, &mut pass, cx.uniforms, cx.view);
        }
        self.draw_tiles(&mut pass, gpu, cx.uniforms, cx.view, layer, false);
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
        cx.bind_uniforms(&u);
        cx.pass.set_bind_group(1, &image.group, &[]);
        cx.pass.draw(0..3, 0..1);
        // The buildings' depth, for later layers that test against it.
        let gpu = cx.gpu;
        self.draw_tiles(cx.pass, gpu, cx.uniforms, cx.view, layer, true);
    }
}

fn extent([width, height]: [u32; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn texture(
    gpu: &Gpu,
    label: &str,
    size: [u32; 2],
    sample_count: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent(size),
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
