//! Drawing sparkle's buckets with Flutter GPU, as commands for the Dart side (see
//! [`crate::gpu`]). A port of `sparkle_harness`'s wgpu renderer, which it follows closely.
//!
//! Style layers are drawn bottom to top, each across every tile of its source, as MapLibre
//! does. Tiles are clipped with stencil masks: before the first layer of a source, each of
//! that source's tiles writes its own value into the stencil buffer over its area, and
//! layer pipelines only draw where the stencil matches the tile they draw. This keeps
//! features in a tile's buffer from spilling onto neighbours, at any bearing and pitch.
//!
//! Over terrain, on the globe or under a sky, the flat layers are rendered per tile into
//! textures draped over the terrain instead (see [`terrain`]), in runs ("stacks") of
//! consecutive flat layers so that 3D layers between them keep their place in the style's
//! order.
//!
//! The per-type drawing lives in [`layers`]; this module owns the targets, the frame, and
//! the masks.

pub mod layers;
pub mod paint;
pub mod terrain;
pub mod view;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::sync::Arc;

use sparkle::sprite::ImageAtlas;
use sparkle::state::FeatureStates;
use sparkle::style::StyleLayer;
use sparkle::symbol::placement::LabelAt;
use sparkle::terrain::TerrainTiles;
use sparkle::{Bucket, ProcessedTile, Style, TileId};
use sparkle_view::camera::{Camera, Placement};
use sparkle_view::map::{Terrain, TileUploader};

use self::layers::{DrawContext, LayerDraw, LayerRenderer, PrepareContext, TileDraw};
use self::terrain::{DRAPE_POOL, TerrainRenderer};
use self::view::View;
use crate::gpu::{
    Blend, Buffer, Compare, DepthStencilDesc, GpuInfo, Load, Pass, PassDesc, Pipeline,
    PipelineSpec, Sampler, StencilOp, StencilState, Store, Texture, TextureFormat, Topology,
    VertexBufferLayout, VertexFormat,
};

/// The GPU as layer renderers see it: the command stream, plus what they share.
pub struct Gpu {
    core: crate::gpu::Gpu,
    /// The style's sprites, if any were loaded.
    pub images: Option<ImageTexture>,
    /// The corners of the unit square, `[f32; 2]` each, in triangle strip order: tile
    /// quads, and full-screen draws as `position * 2 - 1`.
    pub quad: Buffer,
    /// Zeros for paint properties bound as uniforms (see [`paint`]).
    pub paint_zeros: Buffer,
}

impl Deref for Gpu {
    type Target = crate::gpu::Gpu;

    fn deref(&self) -> &crate::gpu::Gpu {
        &self.core
    }
}

impl Gpu {
    /// The vertex buffer layout of [`Self::quad`], read as `position`.
    pub fn quad_layout() -> VertexBufferLayout {
        VertexBufferLayout::packed(&[("position", VertexFormat::Float32x2)])
    }
}

/// A sprite atlas on the GPU, for layer types that draw icons or patterns.
pub struct ImageTexture {
    pub texture: Texture,
    /// In pixels.
    pub size: [u32; 2],
    /// Where each image is, for properties the renderer resolves itself; shared with the
    /// tile loader.
    pub atlas: Arc<ImageAtlas>,
}

/// One tile's buckets on the GPU, sorted by style layer.
pub struct GpuTile {
    buckets: Vec<GpuBucket>,
}

struct GpuBucket {
    style_layer: usize,
    extent: u32,
    data: Box<dyn Any + Send + Sync>,
    /// The bucket itself, kept when its paint reads feature or global state, to evaluate
    /// that paint again when the state changes.
    cpu: Option<Bucket>,
}

impl GpuTile {
    fn bucket(&self, style_layer: usize) -> Option<&GpuBucket> {
        let i = self
            .buckets
            .binary_search_by_key(&style_layer, |b| b.style_layer)
            .ok()?;
        Some(&self.buckets[i])
    }
}

/// The tiles of one source to draw this frame.
pub type SourceTiles<'a> = sparkle_view::map::SourceTiles<'a, GpuTile>;

/// Stencil values are 8 bits and 0 means "no tile".
const MAX_MASKED_TILES: usize = 255;
/// The deepest tiles layers are draped over terrain in.
const MAX_DRAPE_ZOOM: u8 = 22;

/// A frame's layers in drawing order: runs of flat layers draped together, and the 3D
/// layers between them, by index into the frame's layer draws.
enum Segment {
    Stack(Vec<usize>),
    Layers(Vec<usize>),
}

/// The frame's draws: `(renderer, source index, draw)`.
type Draws<'a> = Vec<(usize, Option<usize>, LayerDraw<'a>)>;

pub struct Renderer {
    gpu: Gpu,
    size: [u32; 2],
    msaa: Option<Texture>,
    depth_stencil: Texture,
    layers: Vec<Box<dyn LayerRenderer>>,
    /// Index into `layers` by layer type.
    by_type: HashMap<&'static str, usize>,
    mask: Pipeline,
    terrain: TerrainRenderer,
    /// Whether frames follow each other on screen, so transitions animate; a screenshot
    /// wants their end state at once.
    pub animate: bool,
}

impl Renderer {
    pub fn new(info: GpuInfo, size: [u32; 2]) -> Self {
        let core = crate::gpu::Gpu::new(info);
        let quad: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let gpu = Gpu {
            quad: core.buffer(bytemuck::cast_slice(&quad)),
            paint_zeros: core.buffer(&[0; paint::PAINT_ZEROS]),
            images: None,
            core,
        };
        let replace = StencilState {
            compare: Compare::Always,
            pass_op: StencilOp::Replace,
            read_mask: 0xff,
            write_mask: 0xff,
        };
        let mask = gpu.pipeline(&PipelineSpec {
            program: "Clip",
            vertex_buffers: vec![Gpu::quad_layout()],
            topology: Topology::TriangleStrip,
            blend: Blend::NoColor,
            stencil: Some(replace),
            ..Default::default()
        });
        let layers = layers::create(&gpu);
        let by_type = layers
            .iter()
            .enumerate()
            .map(|(i, l)| (l.layer_type(), i))
            .collect();
        let terrain = TerrainRenderer::new(&gpu);
        let mut renderer = Self {
            msaa: None,
            depth_stencil: gpu.screen(),
            terrain,
            gpu,
            size,
            layers,
            by_type,
            mask,
            animate: true,
        };
        renderer.resize(size);
        renderer
    }

    pub fn gpu(&self) -> &Gpu {
        &self.gpu
    }

    /// Make the style's sprites available to layer renderers.
    pub fn set_images(&mut self, atlas: &Arc<ImageAtlas>) {
        let texture = self
            .gpu
            .rgba_texture(atlas.width, atlas.height, &atlas.pixels);
        self.gpu.images = Some(ImageTexture {
            texture,
            size: [atlas.width, atlas.height],
            atlas: atlas.clone(),
        });
    }

    /// Draw frames of `size` physical pixels from now on.
    pub fn resize(&mut self, size: [u32; 2]) {
        let size = size.map(|v| v.max(1));
        if self.msaa.is_some() && self.size == size {
            return;
        }
        self.size = size;
        self.gpu.set_screen_size(size);
        let samples = self.gpu.info.sample_count;
        self.msaa = (samples > 1).then(|| self.gpu.target(size, TextureFormat::Screen, samples));
        self.depth_stencil = self.gpu.target(size, TextureFormat::DepthStencil, samples);
    }

    /// Forget what renderers keep per style layer, before drawing with a new style.
    pub fn style_changed(&mut self) {
        self.layers.iter_mut().for_each(|l| l.style_changed());
    }

    /// Whether a layer is animating, so the next frame needs drawing even if nothing else
    /// changes.
    pub fn animating(&self) -> bool {
        self.layers.iter().any(|l| l.animating())
    }

    /// The labels shown within `tolerance` of `point` (logical pixels), nearest first.
    pub fn labels_at(&self, point: [f32; 2], tolerance: f32) -> Vec<LabelAt> {
        self.layers
            .iter()
            .flat_map(|l| l.labels_at(point, tolerance))
            .collect()
    }

    /// Upload a processed tile, with the current feature state applied to buckets whose
    /// paint reads it.
    pub fn upload(
        &mut self,
        style: &Style,
        tile: ProcessedTile,
        states: &FeatureStates,
    ) -> GpuTile {
        let mut buckets = Vec::new();
        // Consecutive buckets to upload as one, and their renderer and merge key.
        let mut run: Vec<Bucket> = Vec::new();
        let mut run_key = None;
        for mut bucket in tile.buckets {
            let layer = &style.layers[bucket.style_layer];
            let Some(&r) = self.by_type.get(layer.kind.type_name()) else {
                continue;
            };
            let cpu = bucket.retains_features();
            let key = (!cpu)
                .then(|| self.layers[r].merge_key(layer, &bucket))
                .flatten()
                .map(|k| (r, k));
            let joins = key.is_some()
                && key == run_key
                && run.last().is_some_and(|prev| {
                    prev.extent == bucket.extent
                        && nothing_between(style, prev.style_layer, bucket.style_layer)
                });
            if !joins {
                self.upload_run(style, &mut run, &mut buckets);
            }
            run_key = key;
            if key.is_some() {
                run.push(bucket);
                continue;
            }
            if cpu && layer.source.as_ref().is_some_and(|s| states.has_source(s)) {
                bucket.update_feature_state(layer, states, None);
            }
            buckets.extend(self.upload_one(layer, r, bucket, cpu));
        }
        self.upload_run(style, &mut run, &mut buckets);
        GpuTile { buckets }
    }

    fn upload_one(
        &mut self,
        layer: &StyleLayer,
        r: usize,
        bucket: Bucket,
        cpu: bool,
    ) -> Option<GpuBucket> {
        let data = self.layers[r].upload(&self.gpu, layer, &bucket)?;
        Some(GpuBucket {
            style_layer: bucket.style_layer,
            extent: bucket.extent,
            data,
            cpu: cpu.then_some(bucket),
        })
    }

    /// Upload the buckets of `run` (taking them), as one if there are several.
    fn upload_run(&mut self, style: &Style, run: &mut Vec<Bucket>, out: &mut Vec<GpuBucket>) {
        let Some(first) = run.first() else {
            return;
        };
        let first = &style.layers[first.style_layer];
        let r = self.by_type[first.kind.type_name()];
        if run.len() == 1 {
            let bucket = run.pop().expect("one");
            out.extend(self.upload_one(first, r, bucket, false));
            return;
        }
        let pairs: Vec<(&StyleLayer, &Bucket)> = run
            .iter()
            .map(|b| (&style.layers[b.style_layer], b))
            .collect();
        let data = self.layers[r].upload_merged(&self.gpu, &pairs);
        out.extend(run.drain(..).zip(data).map(|(bucket, data)| GpuBucket {
            style_layer: bucket.style_layer,
            extent: bucket.extent,
            data,
            cpu: None,
        }));
    }

    /// Evaluate the paint of features `ids` (all for `None`) of `source_layer` again
    /// with their state in `states`, and upload what changed. Returns whether anything did.
    pub fn update_feature_state(
        &mut self,
        tile: &mut GpuTile,
        style: &Style,
        states: &FeatureStates,
        source_layer: Option<&str>,
        ids: Option<&[u64]>,
    ) -> bool {
        let mut changed = false;
        for bucket in &mut tile.buckets {
            let Some(cpu) = &mut bucket.cpu else { continue };
            let layer = &style.layers[bucket.style_layer];
            if layer.source_layer.as_deref() != source_layer
                || !cpu.update_feature_state(layer, states, ids)
            {
                continue;
            }
            changed = true;
            let Some(&r) = self.by_type.get(layer.kind.type_name()) else {
                continue;
            };
            let renderer = &mut self.layers[r];
            if !renderer.update_paint(&self.gpu, bucket.data.as_mut(), cpu) {
                // The renderer can't update paint alone; upload the whole bucket again.
                if let Some(data) = renderer.upload(&self.gpu, layer, cpu) {
                    bucket.data = data;
                }
            }
        }
        changed
    }

    /// Rebuild the paint attributes of the tile's bucket of layer `index` after a style
    /// change. Returns false when the bucket can't do that, so the tile must be processed
    /// again.
    pub fn rebuild_paint(
        &mut self,
        tile: &mut GpuTile,
        style: &Style,
        index: usize,
        states: &FeatureStates,
    ) -> bool {
        let Ok(i) = tile.buckets.binary_search_by_key(&index, |b| b.style_layer) else {
            // Nothing drawn from this tile for the layer, which a change of paint alone
            // can't alter.
            return true;
        };
        let bucket = &mut tile.buckets[i];
        let layer = &style.layers[index];
        let Some(cpu) = &mut bucket.cpu else {
            return false;
        };
        if !cpu.rebuild_paint(layer, states) {
            return false;
        }
        let Some(&r) = self.by_type.get(layer.kind.type_name()) else {
            return true;
        };
        if let Some(data) = self.layers[r].upload(&self.gpu, layer, cpu) {
            bucket.data = data;
        }
        true
    }

    /// Record a whole frame. The commands are then taken with [`crate::gpu::Gpu::take`].
    pub fn render(
        &mut self,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        terrain: Option<Terrain<'_>>,
    ) {
        let view = View::scene(
            camera,
            &style.light,
            terrain.map(|t| t.exaggeration),
            style.sky.as_ref(),
        );
        let tiles = terrain.map(|t| t.tiles);
        let drape_tiles = if view.is_3d() {
            let range = |id: TileId| {
                let (lo, hi) = tiles?.min_max(id)?;
                let e = terrain?.exaggeration;
                Some((f64::from(lo) * e, f64::from(hi) * e))
            };
            let drape = camera.covering_tiles_with(0, MAX_DRAPE_ZOOM, 512, &range);

            if let Some(tiles) = tiles {
                let wanted = drape
                    .iter()
                    .chain(
                        sources
                            .iter()
                            .flat_map(|s| s.tiles.iter().flat_map(|t| [&t.placement, &t.clip])),
                    )
                    .filter_map(|p| tiles.find(p.id()))
                    .collect();
                self.terrain.update_dems(&self.gpu, tiles, &wanted);
            }
            Some(drape)
        } else {
            None
        };
        let draws = self.layer_draws(style, &view, sources, tiles, &|_| true);

        let animate = self.animate;
        let ground = drape_tiles.as_deref().map(|placements| terrain::Ground {
            renderer: &self.terrain,
            tiles,
            placements,
        });
        // Draped layers are drawn in their tiles' textures, not prepared for the screen.
        let prepared = |draw: &LayerDraw<'_>| {
            drape_tiles.is_none() || !layers::is_draped(draw.layer.kind.type_name())
        };
        for (r, renderer) in self.layers.iter_mut().enumerate() {
            let mine: Vec<&LayerDraw<'_>> = draws
                .iter()
                .filter(|(dr, _, d)| *dr == r && prepared(d))
                .map(|(_, _, d)| d)
                .collect();
            let mut cx = PrepareContext {
                gpu: &self.gpu,
                view: &view,
                animate,
                ground: ground.as_ref(),
            };
            renderer.prepare_frame(&mut cx, &mine);
        }
        for (r, _, draw) in draws.iter().filter(|(_, _, d)| prepared(d)) {
            let mut cx = PrepareContext {
                gpu: &self.gpu,
                view: &view,
                animate,
                ground: ground.as_ref(),
            };
            self.layers[*r].prepare(&mut cx, draw);
        }

        match &drape_tiles {
            None => {
                let mut pass = main_pass(
                    &self.gpu,
                    self.msaa.as_ref(),
                    &self.depth_stencil,
                    true,
                    true,
                );
                let all: Vec<usize> = (0..draws.len()).collect();
                draw_layers(
                    &mut self.layers,
                    &self.gpu,
                    &self.mask,
                    None,
                    &mut pass,
                    &view,
                    sources,
                    &draws,
                    &all,
                );
            }
            Some(drape_tiles) => {
                {
                    let mut pass = main_pass(
                        &self.gpu,
                        self.msaa.as_ref(),
                        &self.depth_stencil,
                        true,
                        false,
                    );
                    self.terrain.draw_sky(&self.gpu, &mut pass, &view);
                }
                for segment in segments(&draws) {
                    match segment {
                        Segment::Stack(stack) => {
                            for chunk in drape_tiles.chunks(DRAPE_POOL) {
                                for (slot, &tile) in chunk.iter().enumerate() {
                                    self.render_drape(
                                        style, camera, sources, &draws, &stack, tile, slot,
                                    );
                                }
                                let mut pass = main_pass(
                                    &self.gpu,
                                    self.msaa.as_ref(),
                                    &self.depth_stencil,
                                    false,
                                    false,
                                );
                                for (slot, &tile) in chunk.iter().enumerate() {
                                    self.terrain
                                        .draw_tile(&self.gpu, &mut pass, &view, tiles, tile, slot);
                                }
                            }
                        }
                        Segment::Layers(run) => {
                            let mut pass = main_pass(
                                &self.gpu,
                                self.msaa.as_ref(),
                                &self.depth_stencil,
                                false,
                                false,
                            );
                            draw_layers(
                                &mut self.layers,
                                &self.gpu,
                                &self.mask,
                                Some((&self.terrain, tiles)),
                                &mut pass,
                                &view,
                                sources,
                                &draws,
                                &run,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Render the layers of a stack (indices into `draws`) for drape tile `tile` into pool
    /// texture `slot`.
    #[allow(clippy::too_many_arguments)]
    fn render_drape(
        &mut self,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        draws: &Draws<'_>,
        stack: &[usize],
        tile: Placement,
        slot: usize,
    ) {
        // Looking straight down at the tile, which then fills the viewport exactly. Pixel
        // sizes are those of the camera's zoom, so lines are as wide as on screen.
        let size = (1024.0 * camera.pixel_ratio).clamp(512.0, 2048.0).round();
        let tile_size = camera.tile_world_size(tile.z);
        let n = f64::from(tile.z).exp2();
        let mut flat = Camera::new([size; 2], size / tile_size);
        flat.center = [(tile.x as f64 + 0.5) / n, (f64::from(tile.y) + 0.5) / n];
        flat.zoom = camera.zoom;
        let view = View::new(&flat, &style.light);
        let overlaps = |p: Placement| {
            let z = p.z.min(tile.z);
            p.ancestor(p.z - z) == tile.ancestor(tile.z - z)
        };
        let local: Vec<SourceTiles<'_>> = sources
            .iter()
            .map(|s| SourceTiles {
                source: s.source,
                tiles: s
                    .tiles
                    .iter()
                    .filter(|t| overlaps(t.clip))
                    .copied()
                    .collect(),
            })
            .collect();
        let layers: HashSet<usize> = stack.iter().map(|&i| draws[i].2.index).collect();
        let local_draws = self.layer_draws(style, &view, &local, None, &|i| layers.contains(&i));
        let mut pass = self.terrain.begin_drape(&self.gpu, size as u32, slot);
        let all: Vec<usize> = (0..local_draws.len()).collect();
        draw_layers(
            &mut self.layers,
            &self.gpu,
            &self.mask,
            None,
            &mut pass,
            &view,
            &local,
            &local_draws,
            &all,
        );
    }

    /// Every visible layer with its renderer, source (index into `sources`) and tiles,
    /// for the layers `include` accepts (by style index).
    fn layer_draws<'a>(
        &self,
        style: &'a Style,
        view: &View,
        sources: &'a [SourceTiles<'a>],
        terrain: Option<&TerrainTiles>,
        include: &dyn Fn(usize) -> bool,
    ) -> Draws<'a> {
        let camera = &view.camera;
        let mut draws = Vec::new();
        for (index, layer) in style.layers.iter().enumerate() {
            if !include(index) || !layer.is_visible_at(camera.zoom as f32) {
                continue;
            }
            let Some(&r) = self.by_type.get(layer.kind.type_name()) else {
                continue;
            };
            let Some(source_id) = &layer.source else {
                draws.push((
                    r,
                    None,
                    LayerDraw {
                        index,
                        layer,
                        tiles: Vec::new(),
                    },
                ));
                continue;
            };
            let Some(s) = sources.iter().position(|s| s.source == source_id) else {
                continue;
            };
            let tiles: Vec<_> = sources[s]
                .tiles
                .iter()
                .take(MAX_MASKED_TILES)
                .enumerate()
                .filter_map(|(k, t)| {
                    let bucket = t.tile.bucket(index)?;
                    let units_per_pixel =
                        f64::from(bucket.extent) / camera.tile_world_size(t.placement.z);
                    let (geo, elevation) =
                        self.terrain
                            .geo(terrain, view.z_per_meter, t.placement, bucket.extent);
                    Some(TileDraw {
                        placement: t.placement,
                        extent: bucket.extent,
                        bucket: bucket.data.as_ref(),
                        matrix: camera.tile_matrix(
                            &view.view_projection,
                            t.placement,
                            bucket.extent,
                        ),
                        units_per_pixel: units_per_pixel as f32,
                        stencil_ref: k as u32 + 1,
                        geo,
                        elevation,
                    })
                })
                .collect();
            if !tiles.is_empty() {
                draws.push((
                    r,
                    Some(s),
                    LayerDraw {
                        index,
                        layer,
                        tiles,
                    },
                ));
            }
        }
        draws
    }
}

impl TileUploader for Renderer {
    type Tile = GpuTile;

    fn upload(&mut self, style: &Style, tile: ProcessedTile, states: &FeatureStates) -> GpuTile {
        Renderer::upload(self, style, tile, states)
    }

    fn style_changed(&mut self) {
        Renderer::style_changed(self);
    }

    fn rebuild_paint(
        &mut self,
        tile: &mut GpuTile,
        style: &Style,
        index: usize,
        states: &FeatureStates,
    ) -> bool {
        Renderer::rebuild_paint(self, tile, style, index, states)
    }

    fn update_feature_state(
        &mut self,
        tile: &mut GpuTile,
        style: &Style,
        states: &FeatureStates,
        source_layer: Option<&str>,
        ids: Option<&[u64]>,
    ) -> bool {
        Renderer::update_feature_state(self, tile, style, states, source_layer, ids)
    }
}

/// Whether a tile's buckets of style layers `a` and `b` (with none between) may be drawn
/// as one: whether whatever the layers between draw in other tiles stays out of this one.
/// That holds for layers of the same source that draw only in their tiles, which have no
/// bucket here; layers of other sources draw over every tile, and some layer types (such
/// as `fill-extrusion`, composited over the screen) draw outside their tiles.
fn nothing_between(style: &Style, a: usize, b: usize) -> bool {
    let source = &style.layers[a].source;
    style.layers[a + 1..b].iter().all(|layer| {
        layer.source.is_some()
            && layer.source == *source
            && matches!(
                layer.kind.type_name(),
                "fill" | "line" | "circle" | "symbol"
            )
    })
}

/// A pass over the whole frame, clearing it first or adding to what's drawn. Unless it's
/// the `only` pass of the frame, it keeps everything for later passes to add to; the
/// only one keeps just the (resolved) color, so that a tiling GPU needn't write the
/// multisampled color and the depth and stencil out to memory.
fn main_pass<'g>(
    gpu: &'g Gpu,
    msaa: Option<&Texture>,
    depth_stencil: &Texture,
    clear: bool,
    only: bool,
) -> Pass<'g> {
    let screen = gpu.screen();
    let (color, resolve) = match msaa {
        Some(msaa) => (msaa, Some(&screen)),
        None => (&screen, None),
    };
    gpu.begin_pass(&PassDesc {
        color,
        resolve,
        // An opaque white page under the map, as in a browser.
        color_load: if clear {
            Load::Clear([1.0; 4])
        } else {
            Load::Load
        },
        // Later passes of the frame add to the multisampled image; each resolves
        // what's there so far.
        color_store: match (resolve.is_some(), only) {
            (true, true) => Store::Resolve,
            (true, false) => Store::StoreAndResolve,
            (false, _) => Store::Store,
        },
        depth_stencil: Some(DepthStencilDesc {
            texture: depth_stencil,
            depth_load: if clear { Load::Clear(1.0) } else { Load::Load },
            depth_store: if only { Store::DontCare } else { Store::Store },
            stencil_load: if clear { Load::Clear(0) } else { Load::Load },
            stencil_store: if only { Store::DontCare } else { Store::Store },
        }),
    })
}

/// Draw `which` of `draws` in order, each source's tile masks before its first layer.
/// With `terrain`, masks follow the terrain and the globe.
#[allow(clippy::too_many_arguments)]
fn draw_layers(
    layers: &mut [Box<dyn LayerRenderer>],
    gpu: &Gpu,
    mask: &Pipeline,
    terrain: Option<(&TerrainRenderer, Option<&TerrainTiles>)>,
    pass: &mut Pass<'_>,
    view: &View,
    sources: &[SourceTiles<'_>],
    draws: &Draws<'_>,
    which: &[usize],
) {
    let mut masked: Option<usize> = None;
    // Runs of draws by the same renderer from the same source, which it draws together.
    for run in which.chunk_by(|&a, &b| draws[a].0 == draws[b].0 && draws[a].1 == draws[b].1) {
        let (r, source, _) = &draws[run[0]];
        if let Some(source) = *source
            && masked != Some(source)
        {
            draw_masks(
                gpu,
                mask,
                terrain,
                pass,
                view,
                &sources[source],
                masked.is_some(),
            );
            masked = Some(source);
        }
        let group: Vec<&LayerDraw<'_>> = run.iter().map(|&i| &draws[i].2).collect();
        let mut cx = DrawContext { gpu, pass, view };
        layers[*r].draw_group(&mut cx, &group);
    }
}

/// The frame's draws as stacks of draped layers and the 3D layers between them.
fn segments(draws: &Draws<'_>) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    for (i, (_, _, draw)) in draws.iter().enumerate() {
        let draped = layers::is_draped(draw.layer.kind.type_name());
        match (out.last_mut(), draped) {
            (Some(Segment::Stack(stack)), true) => stack.push(i),
            (Some(Segment::Layers(run)), false) => run.push(i),
            (_, true) => out.push(Segment::Stack(vec![i])),
            (_, false) => out.push(Segment::Layers(vec![i])),
        }
    }
    out
}

/// Write each tile's stencil value over its clip area, after clearing earlier masks. With
/// `terrain`, over the terrain as drawn.
fn draw_masks(
    gpu: &Gpu,
    mask: &Pipeline,
    terrain: Option<(&TerrainRenderer, Option<&TerrainTiles>)>,
    pass: &mut Pass<'_>,
    view: &View,
    source: &SourceTiles<'_>,
    clear: bool,
) {
    let mut u = view.uniforms();
    u.extent = 1.0;
    let bind = |pass: &mut Pass<'_>| {
        pass.set_pipeline(mask);
        pass.set_vertex_buffer(0, gpu.quad.slice());
    };
    if clear {
        bind(pass);
        // The unit square to the whole of clip space, flat.
        u.matrix = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, -1.0, 0.0, 1.0],
        ];
        u.globe = [0.0; 4];
        layers::bind_uniforms(gpu, pass, &u, None);
        pass.set_stencil_reference(0);
        pass.draw(4, 1);
    }
    if source.tiles.len() > MAX_MASKED_TILES {
        log::warn!(
            "{} tiles of {} to clip; only the first {MAX_MASKED_TILES} are drawn",
            source.tiles.len(),
            source.source
        );
    }
    for (k, t) in source.tiles.iter().take(MAX_MASKED_TILES).enumerate() {
        if let Some((renderer, tiles)) = terrain {
            renderer.draw_mask(gpu, pass, view, tiles, t.clip, k as u32 + 1);
            continue;
        }
        bind(pass);
        u.matrix = view.camera.tile_matrix(&view.view_projection, t.clip, 1);
        layers::bind_uniforms(gpu, pass, &u, None);
        pass.set_stencil_reference(k as u32 + 1);
        pass.draw(4, 1);
    }
}

/// How layer renderers sample textures they draw with.
pub const LINEAR: Sampler = Sampler::LINEAR;
pub const NEAREST: Sampler = Sampler::NEAREST;
