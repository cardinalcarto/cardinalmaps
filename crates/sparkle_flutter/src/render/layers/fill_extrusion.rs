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
//!
//! Each tile draws its roofs, then its walls: instanced, a quad from each
//! [`WallPoint`] to the next, read from the same buffer bound twice, the second time one
//! point on.

use std::any::Any;
use std::collections::HashMap;

use sparkle::style::{LayerKind, StyleLayer};
use sparkle::tile::{FillExtrusionPattern, WallPoint};
use sparkle::{Bucket, BucketGeometry};

use super::{
    BucketData, DrawContext, LayerDraw, LayerRenderer, PrepareContext, bind_uniforms,
    translate_units,
};
use crate::gpu::{
    Blend, Buffer, DepthMode, DepthStencilDesc, Load, Pass, PassDesc, Pipeline, PipelineSpec,
    Slice, StepMode, Store, Texture, TextureFormat, Topology, VertexBufferLayout, VertexFormat,
};
use crate::render::indices::GpuIndices;
use crate::render::paint::{GpuPaint, UniformsMemo};
use crate::render::view::View;
use crate::render::{Gpu, LINEAR, NEAREST};

pub struct FillExtrusionRenderer {
    /// By whether they draw walls, paint signature, whether buckets have patterns, and
    /// whether the pipeline only writes depth (into the map's depth buffer).
    pipelines: HashMap<(bool, u32, bool, bool), Pipeline>,
    /// The corners of a wall's quad, two triangles, as `fill_extrusion_wall.vert` reads
    /// them: 1 at the next point's end (x) and at the top (y).
    wall_corners: Buffer,
    composite: Pipeline,
    /// No-pattern [`FillExtrusionPattern`]s, read by buckets without patterns so that
    /// their pipelines can use the same shaders: the first once per instance by roofs, one
    /// per corner by walls.
    no_pattern: Buffer,
    /// Zero edge distances, one per corner, read by walls without patterns.
    no_edges: Buffer,
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
    /// `None` when the bucket has no roofs.
    roofs: Option<GpuRoofs>,
    /// `None` when the bucket has no walls.
    walls: Option<GpuWalls>,
}

struct GpuRoofs {
    vertices: Buffer,
    /// `None` when the bucket has no patterns.
    patterns: Option<Buffer>,
    triangles: GpuIndices,
    paint: GpuPaint,
}

struct GpuWalls {
    points: Buffer,
    /// The patterns and edge distances; `None` when the bucket has no patterns.
    patterns: Option<(Buffer, Buffer)>,
    instances: u32,
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
        // In the order MapLibre's two triangles take a wall's four vertices.
        let corners: [[f32; 2]; 6] = [[1., 0.], [0., 0.], [1., 1.], [1., 1.], [0., 0.], [0., 1.]];
        Self {
            pipelines: HashMap::new(),
            wall_corners: gpu.buffer(bytemuck::cast_slice(&corners)),
            composite,
            no_pattern: gpu.buffer(bytemuck::cast_slice(&[FillExtrusionPattern::default(); 6])),
            no_edges: gpu.buffer(bytemuck::cast_slice(&[0f32; 6])),
            targets: None,
            frame_done: true,
        }
    }

    fn pipeline(
        &mut self,
        gpu: &Gpu,
        walls: bool,
        paint: &GpuPaint,
        patterns: bool,
        depth_only: bool,
    ) -> Pipeline {
        *self
            .pipelines
            .entry((walls, paint.signature(), patterns, depth_only))
            .or_insert_with(|| {
                let mut pattern = VertexBufferLayout::packed(&[
                    ("pattern_rect", VertexFormat::Float32x4),
                    ("pattern_size", VertexFormat::Float32x2),
                ]);
                debug_assert_eq!(pattern.stride as usize, size_of::<FillExtrusionPattern>());
                // Per roof vertex or per wall; without patterns, the zeros of
                // `no_pattern`, read by vertex index on walls (each of the six corners
                // reads its own).
                if walls == patterns {
                    pattern.step = StepMode::Instance;
                }
                let paint = if walls {
                    paint.instance_vertex_buffers()
                } else {
                    paint.vertex_buffers()
                };
                let mut vertex_buffers = if walls {
                    // Position and normal: this point's, then the next one's position.
                    let mut point =
                        VertexBufferLayout::packed(&[("pos_normal", VertexFormat::Float32x4)]);
                    debug_assert_eq!(point.stride as usize, size_of::<WallPoint>());
                    point.step = StepMode::Instance;
                    let mut next =
                        VertexBufferLayout::packed(&[("next_pos", VertexFormat::Float32x2)]);
                    next.stride = point.stride;
                    next.step = StepMode::Instance;
                    let corner = VertexBufferLayout::packed(&[("corner", VertexFormat::Float32x2)]);
                    // Edge distances, as patterns are: per wall, or the zeros of
                    // `no_edges` by vertex index.
                    let edges = |name| {
                        let mut layout =
                            VertexBufferLayout::packed(&[(name, VertexFormat::Float32)]);
                        if patterns {
                            layout.step = StepMode::Instance;
                        }
                        layout
                    };
                    vec![
                        point,
                        next,
                        corner,
                        pattern,
                        edges("edge"),
                        edges("next_edge"),
                    ]
                } else {
                    let vertex = VertexBufferLayout::packed(&[("pos", VertexFormat::Float32x2)]);
                    vec![vertex, pattern]
                };
                vertex_buffers.extend(paint);
                gpu.pipeline(&PipelineSpec {
                    program: if walls {
                        "FillExtrusionWall"
                    } else {
                        "FillExtrusion"
                    },
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

        let (roof_props, wall_props) = (paint.roof_attributes(), paint.attributes());

        let (mut roof_memo, mut wall_memo) = (UniformsMemo::default(), UniformsMemo::default());
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

            if let Some(roofs) = &bucket.roofs {
                let patterns = roofs.patterns.as_ref();
                let pipeline =
                    self.pipeline(gpu, false, &roofs.paint, patterns.is_some(), depth_only);
                pass.set_pipeline(&pipeline);
                u.paint = roof_memo.uniforms(&roofs.paint.layout, &roof_props, zoom);
                bind_uniforms(gpu, pass, &u, tile.geo.dem_texture.as_ref());
                pass.set_texture("pattern_atlas", atlas, LINEAR);
                let no_pattern = &self.no_pattern;
                roofs.triangles.draw(pass, |pass: &mut Pass<'_>, first| {
                    pass.set_vertex_buffer(
                        0,
                        roofs.vertices.slice_from(first, size_of::<[f32; 2]>()),
                    );
                    let patterns = match patterns {
                        Some(p) => p.slice_from(first, size_of::<FillExtrusionPattern>()),
                        None => no_pattern.slice(),
                    };
                    pass.set_vertex_buffer(1, patterns);
                    roofs.paint.bind_from(gpu, pass, 2, first);
                });
            }
            if let Some(walls) = &bucket.walls {
                let patterns = walls.patterns.as_ref();
                let pipeline =
                    self.pipeline(gpu, true, &walls.paint, patterns.is_some(), depth_only);
                pass.set_pipeline(&pipeline);
                u.paint = wall_memo.uniforms(&walls.paint.layout, &wall_props, zoom);
                bind_uniforms(gpu, pass, &u, tile.geo.dem_texture.as_ref());
                pass.set_texture("pattern_atlas", atlas, LINEAR);
                let stride = size_of::<WallPoint>() as u32;
                let size = (walls.instances + 1) * stride;
                pass.set_vertex_buffer(0, Slice::Buffer(&walls.points, 0, size));
                pass.set_vertex_buffer(1, Slice::Buffer(&walls.points, stride, size - stride));
                pass.set_vertex_buffer(2, self.wall_corners.slice());
                match patterns {
                    Some((patterns, edges)) => {
                        pass.set_vertex_buffer(3, patterns.slice());
                        let size = (walls.instances + 1) * 4;
                        pass.set_vertex_buffer(4, Slice::Buffer(edges, 0, size));
                        pass.set_vertex_buffer(5, Slice::Buffer(edges, 4, size - 4));
                    }
                    None => {
                        pass.set_vertex_buffer(3, self.no_pattern.slice());
                        pass.set_vertex_buffer(4, self.no_edges.slice());
                        pass.set_vertex_buffer(5, self.no_edges.slice());
                    }
                }
                walls.paint.bind(gpu, pass, 6);
                pass.draw(6, walls.instances);
            }
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
        let roofs = &extrusion.roofs;
        let walls = &extrusion.walls;
        let patterns = |p: &[FillExtrusionPattern]| {
            (!p.is_empty()).then(|| gpu.buffer(bytemuck::cast_slice(p)))
        };
        Some(Box::new(GpuExtrusion {
            roofs: (!roofs.triangles.is_empty()).then(|| GpuRoofs {
                vertices: gpu.buffer(bytemuck::cast_slice(&roofs.vertices)),
                patterns: patterns(&roofs.patterns),
                triangles: GpuIndices::new(gpu, &roofs.triangles, 3),
                paint: GpuPaint::new(gpu, &roofs.paint),
            }),
            walls: (walls.instances() > 0).then(|| GpuWalls {
                points: gpu.buffer(bytemuck::cast_slice(&walls.points)),
                patterns: patterns(&walls.patterns)
                    .map(|p| (p, gpu.buffer(bytemuck::cast_slice(&walls.edge_distances)))),
                instances: walls.instances() as u32,
                paint: GpuPaint::new(gpu, &walls.paint),
            }),
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
        let roofs = data
            .roofs
            .as_mut()
            .is_none_or(|r| r.paint.update(gpu, &extrusion.roofs.paint));
        let walls = data
            .walls
            .as_mut()
            .is_none_or(|w| w.paint.update(gpu, &extrusion.walls.paint));
        roofs && walls
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
