//! Drawing sparkle's buckets with wgpu.
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

pub mod gpu;
pub mod indices;
pub mod layers;
mod paint;
pub mod terrain;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use sparkle::sprite::ImageAtlas;
use sparkle::state::FeatureStates;
use sparkle::terrain::TerrainTiles;
use sparkle::{Bucket, ProcessedTile, Style, TileId};
use winit::window::Window;

use self::gpu::{DEPTH_STENCIL_FORMAT, Gpu, ImageTexture, PipelineSpec, UniformArena, View};
use self::layers::{DrawContext, LayerDraw, LayerRenderer, PrepareContext, TileDraw};
use self::terrain::{DRAPE_POOL, TerrainRenderer};
use crate::camera::{Camera, Placement};

/// One tile's buckets on the GPU, sorted by style layer.
pub struct GpuTile {
    buckets: Vec<GpuBucket>,
    /// What its buffers and textures took when uploaded.
    bytes: usize,
}

struct GpuBucket {
    style_layer: usize,
    extent: u32,
    data: Box<dyn Any>,
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

pub use sparkle_view::map::Terrain;

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

enum Target {
    Window {
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    /// Rendering only for screenshots.
    Offscreen,
}

pub struct Renderer {
    gpu: Gpu,
    target: Target,
    size: [u32; 2],
    msaa: Option<wgpu::TextureView>,
    depth_stencil: wgpu::TextureView,
    layers: Vec<Box<dyn LayerRenderer>>,
    /// Index into `layers` by layer type.
    by_type: HashMap<&'static str, usize>,
    uniforms: UniformArena,
    mask: wgpu::RenderPipeline,
    terrain: TerrainRenderer,
}

impl Renderer {
    /// Render into a window.
    pub async fn windowed(instance: &wgpu::Instance, window: Arc<Window>) -> Result<Self> {
        let surface = instance.create_surface(window.clone())?;
        let adapter = request_adapter(instance, Some(&surface)).await?;
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // Style colors are sRGB values blended as they are, as MapLibre does, so the surface
        // must not convert them again.
        log::debug!("surface formats: {:?}", caps.formats);
        let format = [
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8Unorm,
        ]
        .into_iter()
        .find(|f| caps.formats.contains(f))
        .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
        };
        let (device, queue) = request_device(&adapter).await?;
        surface.configure(&device, &config);
        let size = [config.width, config.height];
        Ok(Self::new(
            &adapter,
            device,
            queue,
            format,
            size,
            Target::Window {
                window,
                surface,
                config,
            },
        ))
    }

    /// Render without a window, for screenshots only.
    pub async fn headless(instance: &wgpu::Instance, width: u32, height: u32) -> Result<Self> {
        let adapter = request_adapter(instance, None).await?;
        let (device, queue) = request_device(&adapter).await?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        Ok(Self::new(
            &adapter,
            device,
            queue,
            format,
            [width.max(1), height.max(1)],
            Target::Offscreen,
        ))
    }

    fn new(
        adapter: &wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        target: Target,
    ) -> Self {
        let msaa_ok = |f| {
            adapter
                .get_texture_format_features(f)
                .flags
                .sample_count_supported(4)
        };
        let sample_count = if msaa_ok(format) && msaa_ok(DEPTH_STENCIL_FORMAT) {
            4
        } else {
            1
        };
        let gpu = Gpu::new(device, queue, format, sample_count);
        let replace = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Replace,
        };
        let mask = gpu.create_pipeline(&PipelineSpec {
            label: "tile mask",
            shader: include_str!("../shaders/clip.wgsl"),
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            write_mask: wgpu::ColorWrites::empty(),
            blend: None,
            stencil: Some(wgpu::StencilState {
                front: replace,
                back: replace,
                read_mask: 0xff,
                write_mask: 0xff,
            }),
            ..Default::default()
        });
        let layers = layers::create(&gpu);
        let by_type = layers
            .iter()
            .enumerate()
            .map(|(i, l)| (l.layer_type(), i))
            .collect();
        let uniforms = UniformArena::new(&gpu);
        let depth_stencil = create_depth_stencil(&gpu, size);
        let terrain = TerrainRenderer::new(&gpu);
        let mut renderer = Self {
            terrain,
            msaa: create_msaa(&gpu, size),
            depth_stencil,
            gpu,
            target,
            size,
            layers,
            by_type,
            uniforms,
            mask,
        };
        renderer.resize(size[0], size[1]);
        renderer
    }

    /// Make the style's sprites available to layer renderers.
    pub fn set_images(&mut self, atlas: &Arc<ImageAtlas>) {
        let texture = self
            .gpu
            .rgba_texture("sprites", atlas.width, atlas.height, &atlas.pixels);
        self.gpu.images = Some(ImageTexture {
            view: texture.create_view(&Default::default()),
            size: [atlas.width, atlas.height],
            atlas: atlas.clone(),
        });
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.size = [width.max(1), height.max(1)];
        if let Target::Window {
            surface, config, ..
        } = &mut self.target
        {
            config.width = self.size[0];
            config.height = self.size[1];
            surface.configure(&self.gpu.device, config);
        }
        self.msaa = create_msaa(&self.gpu, self.size);
        self.depth_stencil = create_depth_stencil(&self.gpu, self.size);
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

    /// Upload a processed tile, with the current feature state applied to buckets whose
    /// paint reads it.
    pub fn upload(
        &mut self,
        style: &Style,
        tile: ProcessedTile,
        states: &FeatureStates,
    ) -> GpuTile {
        let before = self.gpu.created_bytes();
        let buckets = tile
            .buckets
            .into_iter()
            .filter_map(|mut bucket| {
                let layer = &style.layers[bucket.style_layer];
                let &r = self.by_type.get(layer.kind.type_name())?;
                let cpu = bucket.retains_features();
                if cpu && layer.source.as_ref().is_some_and(|s| states.has_source(s)) {
                    bucket.update_feature_state(layer, states, None);
                }
                let data = self.layers[r].upload(&self.gpu, layer, &bucket)?;
                Some(GpuBucket {
                    style_layer: bucket.style_layer,
                    extent: bucket.extent,
                    data,
                    cpu: cpu.then_some(bucket),
                })
            })
            .collect();
        GpuTile {
            buckets,
            bytes: self.gpu.created_bytes() - before,
        }
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

    /// Draw a frame to the window.
    pub fn render(
        &mut self,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        terrain: Option<Terrain<'_>>,
    ) {
        let Target::Window {
            window,
            surface,
            config,
        } = &self.target
        else {
            return;
        };
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                // Draw this frame anyway; reconfiguring now would invalidate it.
                window.request_redraw();
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&self.gpu.device, config);
                window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => return,
        };
        let window = window.clone();
        let view = frame.texture.create_view(&Default::default());
        let encoder = self.encode(&view, style, camera, sources, terrain);
        self.gpu.queue.submit([encoder.finish()]);
        window.pre_present_notify();
        self.gpu.queue.present(frame);
    }

    /// Draw a frame like [`Self::render`] does, but into a PNG file.
    pub fn screenshot(
        &mut self,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        terrain: Option<Terrain<'_>>,
        path: &Path,
    ) -> Result<()> {
        let [width, height] = self.size;
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screenshot"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.gpu.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row_bytes = width * 4;
        let padded_row = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: u64::from(padded_row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let view = texture.create_view(&Default::default());
        let mut encoder = self.encode(&view, style, camera, sources, terrain);
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: None,
                },
            },
            size,
        );
        self.gpu.queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |result| {
            if let Err(err) = result {
                log::error!("reading back the screenshot: {err}");
            }
        });
        self.gpu.device.poll(wgpu::PollType::wait_indefinitely())?;

        let mapped = readback.get_mapped_range(..)?;
        let bgra = matches!(
            self.gpu.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
        for row in mapped.chunks(padded_row as usize) {
            for px in row[..row_bytes as usize].chunks(4) {
                let [r, g, b] = if bgra {
                    [px[2], px[1], px[0]]
                } else {
                    [px[0], px[1], px[2]]
                };
                pixels.extend([r, g, b, 255]);
            }
        }
        let file =
            std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }

    /// Encode a whole frame into `target`.
    fn encode(
        &mut self,
        target: &wgpu::TextureView,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        terrain: Option<Terrain<'_>>,
    ) -> wgpu::CommandEncoder {
        let view = View::scene(
            camera,
            &style.light,
            terrain.map(|t| t.exaggeration),
            style.sky.as_ref(),
        );
        let tiles = terrain.map(|t| t.tiles);
        self.uniforms.reset();
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
                self.terrain
                    .update_atlas(&self.gpu, &mut self.uniforms, tiles, &wanted);
            }
            Some(drape)
        } else {
            None
        };
        let draws = self.layer_draws(style, &view, sources, tiles, &|_| true);

        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let animate = matches!(self.target, Target::Window { .. });
        let ground = drape_tiles.as_deref().map(|placements| terrain::Ground {
            renderer: &self.terrain,
            tiles,
            placements,
        });
        // Draped layers are drawn in their tiles' textures, not prepared for the screen.
        let prepared = |_: usize, draw: &LayerDraw<'_>| {
            drape_tiles.is_none() || !layers::is_draped(draw.layer.kind.type_name())
        };
        for (r, renderer) in self.layers.iter_mut().enumerate() {
            let mine: Vec<&LayerDraw<'_>> = draws
                .iter()
                .filter(|(dr, _, d)| *dr == r && prepared(r, d))
                .map(|(_, _, d)| d)
                .collect();
            let mut cx = PrepareContext {
                gpu: &self.gpu,
                encoder: &mut encoder,
                uniforms: &mut self.uniforms,
                view: &view,
                animate,
                ground: ground.as_ref(),
            };
            renderer.prepare_frame(&mut cx, &mine);
        }
        for (r, _, draw) in draws.iter().filter(|(r, _, d)| prepared(*r, d)) {
            let mut cx = PrepareContext {
                gpu: &self.gpu,
                encoder: &mut encoder,
                uniforms: &mut self.uniforms,
                view: &view,
                animate,
                ground: ground.as_ref(),
            };
            self.layers[*r].prepare(&mut cx, draw);
        }

        match &drape_tiles {
            None => {
                let mut pass = self.main_pass(&mut encoder, target, true);
                let all: Vec<usize> = (0..draws.len()).collect();
                Self::draw_layers(
                    &mut self.layers,
                    &self.gpu,
                    &mut self.uniforms,
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
                    let mut pass = self.main_pass(&mut encoder, target, true);
                    self.terrain
                        .draw_sky(&self.gpu, &mut pass, &mut self.uniforms, &view);
                }
                for segment in segments(&draws) {
                    match segment {
                        Segment::Stack(stack) => {
                            for chunk in drape_tiles.chunks(DRAPE_POOL) {
                                for (slot, &tile) in chunk.iter().enumerate() {
                                    self.render_drape(
                                        &mut encoder,
                                        style,
                                        camera,
                                        sources,
                                        &draws,
                                        &stack,
                                        tile,
                                        slot,
                                    );
                                }
                                let mut pass = self.main_pass(&mut encoder, target, false);
                                for (slot, &tile) in chunk.iter().enumerate() {
                                    self.terrain.draw_tile(
                                        &self.gpu,
                                        &mut pass,
                                        &mut self.uniforms,
                                        &view,
                                        tiles,
                                        tile,
                                        slot,
                                    );
                                }
                            }
                        }
                        Segment::Layers(run) => {
                            let mut pass = self.main_pass(&mut encoder, target, false);
                            Self::draw_layers(
                                &mut self.layers,
                                &self.gpu,
                                &mut self.uniforms,
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
        self.uniforms.flush(&self.gpu.queue);
        encoder
    }

    /// A pass over the whole frame, clearing it first or adding to what's drawn.
    fn main_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clear: bool,
    ) -> wgpu::RenderPass<'e> {
        let (color, resolve) = match &self.msaa {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        let load = |value| {
            if clear {
                wgpu::LoadOp::Clear(value)
            } else {
                wgpu::LoadOp::Load
            }
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    // An opaque white page under the map, as in a browser.
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::WHITE)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    // Later passes of the frame add to the multisampled image; each resolves
                    // what's there so far.
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_stencil,
                depth_ops: Some(wgpu::Operations {
                    load: load(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Draw `which` of `draws` in order, each source's tile masks before its first layer.
    /// With `terrain`, masks follow the terrain and the globe.
    #[allow(clippy::too_many_arguments)]
    fn draw_layers(
        layers: &mut [Box<dyn LayerRenderer>],
        gpu: &Gpu,
        uniforms: &mut UniformArena,
        mask: &wgpu::RenderPipeline,
        terrain: Option<(&TerrainRenderer, Option<&TerrainTiles>)>,
        pass: &mut wgpu::RenderPass<'_>,
        view: &View,
        sources: &[SourceTiles<'_>],
        draws: &[(usize, Option<usize>, LayerDraw<'_>)],
        which: &[usize],
    ) {
        let mut masked: Option<usize> = None;
        for &i in which {
            let (r, source, draw) = &draws[i];
            if let Some(source) = *source
                && masked != Some(source)
            {
                draw_masks(
                    gpu,
                    uniforms,
                    mask,
                    terrain,
                    pass,
                    view,
                    &sources[source],
                    masked.is_some(),
                );
                masked = Some(source);
            }
            let mut cx = DrawContext {
                gpu,
                pass,
                uniforms,
                view,
            };
            layers[*r].draw(&mut cx, draw);
        }
    }

    /// Render the layers of a stack (indices into `draws`) for drape tile `tile` into pool
    /// texture `slot`.
    #[allow(clippy::too_many_arguments)]
    fn render_drape(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        style: &Style,
        camera: &Camera,
        sources: &[SourceTiles<'_>],
        draws: &[(usize, Option<usize>, LayerDraw<'_>)],
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
        let mut pass = self
            .terrain
            .begin_drape(&self.gpu, encoder, size as u32, slot);
        let all: Vec<usize> = (0..local_draws.len()).collect();
        Self::draw_layers(
            &mut self.layers,
            &self.gpu,
            &mut self.uniforms,
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
    ) -> Vec<(usize, Option<usize>, LayerDraw<'a>)> {
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

impl sparkle_view::map::TileUploader for Renderer {
    type Tile = GpuTile;

    fn upload(&mut self, style: &Style, tile: ProcessedTile, states: &FeatureStates) -> GpuTile {
        Renderer::upload(self, style, tile, states)
    }

    fn tile_bytes(&self, tile: &GpuTile) -> usize {
        tile.bytes
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

/// The frame's draws as stacks of draped layers and the 3D layers between them.
fn segments(draws: &[(usize, Option<usize>, LayerDraw<'_>)]) -> Vec<Segment> {
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
#[allow(clippy::too_many_arguments)]
fn draw_masks(
    gpu: &Gpu,
    uniforms: &mut UniformArena,
    mask: &wgpu::RenderPipeline,
    terrain: Option<(&TerrainRenderer, Option<&TerrainTiles>)>,
    pass: &mut wgpu::RenderPass<'_>,
    view: &View,
    source: &SourceTiles<'_>,
    clear: bool,
) {
    pass.set_pipeline(mask);
    let mut u = view.uniforms();
    u.extent = 1.0;
    if clear {
        // The unit square to the whole of clip space, flat.
        u.matrix = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [-1.0, -1.0, 0.0, 1.0],
        ];
        u.globe = [0.0; 4];
        let (group, offset) = uniforms.push(gpu, &u);
        pass.set_bind_group(0, &group, &[offset]);
        pass.set_stencil_reference(0);
        pass.draw(0..4, 0..1);
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
            renderer.draw_mask(gpu, pass, uniforms, view, tiles, t.clip, k as u32 + 1);
            continue;
        }
        u.matrix = view.camera.tile_matrix(&view.view_projection, t.clip, 1);
        let (group, offset) = uniforms.push(gpu, &u);
        pass.set_bind_group(0, &group, &[offset]);
        pass.set_stencil_reference(k as u32 + 1);
        pass.draw(0..4, 0..1);
    }
}

async fn request_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            ..Default::default()
        })
        .await
        .context("no suitable GPU adapter")?;
    log::info!("using {:?}", adapter.get_info());
    Ok(adapter)
}

async fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue)> {
    Ok(adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("sparkle"),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            ..Default::default()
        })
        .await?)
}

fn create_msaa(gpu: &Gpu, [width, height]: [u32; 2]) -> Option<wgpu::TextureView> {
    (gpu.sample_count > 1).then(|| {
        gpu.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: gpu.sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: gpu.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    })
}

fn create_depth_stencil(gpu: &Gpu, [width, height]: [u32; 2]) -> wgpu::TextureView {
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth stencil"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: gpu.sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_STENCIL_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
