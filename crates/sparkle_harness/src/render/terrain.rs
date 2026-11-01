//! Drawing the map in 3D: over terrain, on the globe, and under the style's sky.
//!
//! As MapLibre does with terrain, the flat layers (fills, lines, rasters and the like) are
//! not drawn onto the screen directly but rendered tile by tile into textures, which are
//! then draped over a mesh per tile ([`sparkle::terrain::TerrainMesh`]). The mesh's
//! vertex shader lifts it by the DEM and bends it onto the globe, so the flat layers'
//! own shaders never need to know about either: only vertex and fragment shaders and
//! render-to-texture passes, as Flutter's GPU API allows. Layers that are 3D themselves
//! (symbols, circles, heatmaps, extrusions) are drawn into the main pass and sample the
//! same DEM in their vertex shaders.
//!
//! Elevations reach the GPU through one texture array, the DEM atlas, bound with every
//! draw's uniforms: each DEM tile in use gets a layer.

use std::collections::{HashMap, HashSet};

use sparkle::TileId;
use sparkle::terrain::{ElevationSampler, MESH_SIZE, TerrainMesh, TerrainTiles, TerrainVertex};

use super::gpu::{DepthMode, Gpu, PipelineSpec, TileGeo, UniformArena, View};
use crate::camera::Placement;

/// Tile units across a drape tile: only precision depends on it.
pub const DRAPE_EXTENT: u32 = 8192;
/// How many tiles' textures are rendered before they're draped, per batch.
pub const DRAPE_POOL: usize = 16;
/// DEM tiles the atlas holds at once.
const ATLAS_LAYERS: u32 = 64;

struct DemAtlas {
    texture: wgpu::Texture,
    /// Pixels per side of a layer: the DEM tiles' size with their border.
    size: u32,
    /// By tile: the layer, the version uploaded, and the frame it was last needed.
    slots: HashMap<TileId, (u32, u64, u64)>,
}

/// The textures one batch of tiles is rendered into, and what rendering them takes.
struct DrapePool {
    /// Pixels per side.
    size: u32,
    textures: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
    /// Multisampled color, resolved into the pool's textures; `None` without MSAA.
    msaa: Option<wgpu::TextureView>,
    depth_stencil: wgpu::TextureView,
}

/// The terrain as drawn this frame, for passes of 3D layers that must be hidden behind it
/// (see [`Ground::draw_depth`]).
pub struct Ground<'a> {
    pub renderer: &'a TerrainRenderer,
    pub tiles: Option<&'a TerrainTiles>,
    pub placements: &'a [Placement],
}

impl Ground<'_> {
    /// Draw the terrain's depth (only) into `pass`, which has a color target of the
    /// renderer's format and a depth-stencil attachment, both multisampled as the map is.
    pub fn draw_depth(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        arena: &mut UniformArena,
        view: &View,
    ) {
        let r = self.renderer;
        for &tile in self.placements {
            let (geo, _) = r.geo(self.tiles, view.z_per_meter, tile, DRAPE_EXTENT);
            let matrix = view
                .camera
                .tile_matrix(&view.view_projection, tile, DRAPE_EXTENT);
            let u = view.tile_uniforms(matrix, &geo);
            let (group, offset) = arena.push(gpu, &u);
            pass.set_pipeline(&r.depth);
            pass.set_bind_group(0, &group, &[offset]);
            pass.set_vertex_buffer(0, r.vertices.slice(..));
            pass.set_index_buffer(r.indices.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..r.grid_index_count, 0, 0..1);
        }
    }
}

pub struct TerrainRenderer {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// All of the mesh's indices, and the grid's alone (without skirts).
    index_count: u32,
    grid_index_count: u32,
    drape: wgpu::RenderPipeline,
    /// The terrain's depth alone.
    depth: wgpu::RenderPipeline,
    mask: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    atlas: Option<DemAtlas>,
    pool: Option<DrapePool>,
    frame: u64,
    warned_dim: bool,
}

fn mesh_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32];
    wgpu::VertexBufferLayout {
        array_stride: size_of::<TerrainVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

impl TerrainRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let mesh = TerrainMesh::grid(MESH_SIZE);
        let texture_layout = gpu.texture_layout("drape", 1);
        let drape = gpu.create_pipeline(&PipelineSpec {
            label: "terrain",
            shader: include_str!("../shaders/terrain.wgsl"),
            vertex_buffers: &[mesh_layout()],
            clip: false,
            depth: DepthMode::TestWrite,
            bind_group_layouts: &[&texture_layout],
            ..Default::default()
        });
        let depth = gpu.create_pipeline(&PipelineSpec {
            label: "terrain depth",
            // The mask's shader, which writes nothing but its position.
            shader: include_str!("../shaders/clip_mesh.wgsl"),
            vertex_buffers: &[mesh_layout()],
            clip: false,
            depth: DepthMode::TestWrite,
            write_mask: wgpu::ColorWrites::empty(),
            blend: None,
            ..Default::default()
        });
        let replace = wgpu::StencilFaceState {
            compare: wgpu::CompareFunction::Always,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op: wgpu::StencilOperation::Replace,
        };
        let mask = gpu.create_pipeline(&PipelineSpec {
            label: "terrain tile mask",
            shader: include_str!("../shaders/clip_mesh.wgsl"),
            vertex_buffers: &[mesh_layout()],
            write_mask: wgpu::ColorWrites::empty(),
            blend: None,
            depth: DepthMode::Test,
            stencil: Some(wgpu::StencilState {
                front: replace,
                back: replace,
                read_mask: 0xff,
                write_mask: 0xff,
            }),
            ..Default::default()
        });
        let sky = gpu.create_pipeline(&PipelineSpec {
            label: "sky",
            shader: include_str!("../shaders/sky.wgsl"),
            clip: false,
            blend: None,
            ..Default::default()
        });
        Self {
            vertices: gpu.buffer(
                "terrain mesh",
                bytemuck::cast_slice(&mesh.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: gpu.buffer(
                "terrain mesh",
                bytemuck::cast_slice(&mesh.indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: mesh.indices.len() as u32,
            grid_index_count: MESH_SIZE * MESH_SIZE * 6,
            drape,
            depth,
            mask,
            sky,
            texture_layout,
            atlas: None,
            pool: None,
            frame: 0,
            warned_dim: false,
        }
    }

    /// Put the DEM tiles `wanted` into the atlas (uploading those that changed), making
    /// room by dropping the ones needed longest ago.
    pub fn update_atlas(
        &mut self,
        gpu: &Gpu,
        arena: &mut UniformArena,
        terrain: &TerrainTiles,
        wanted: &HashSet<TileId>,
    ) {
        self.frame += 1;
        let Some(dim) = wanted
            .iter()
            .find_map(|&id| terrain.get(id))
            .map(|(dem, _)| dem.dim)
        else {
            return;
        };
        let size = dim + 2;
        if self.atlas.as_ref().is_none_or(|a| a.size != size) {
            let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("DEM atlas"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: ATLAS_LAYERS,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            arena.set_dem_atlas(
                gpu,
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                }),
            );
            self.atlas = Some(DemAtlas {
                texture,
                size,
                slots: HashMap::new(),
            });
        }
        let atlas = self.atlas.as_mut().expect("made above");
        let frame = self.frame;
        atlas.slots.retain(|id, _| terrain.get(*id).is_some());
        let mut wanted: Vec<TileId> = wanted.iter().copied().collect();
        // Deepest first, so a full atlas keeps the detail near the camera.
        wanted.sort_by_key(|id| std::cmp::Reverse(id.z));
        for id in wanted {
            let Some((dem, version)) = terrain.get(id) else {
                continue;
            };
            if dem.dim != dim {
                if !self.warned_dim {
                    log::warn!("DEM tiles of different sizes; only {dim} pixel tiles are used");
                    self.warned_dim = true;
                }
                continue;
            }
            let layer = match atlas.slots.get_mut(&id) {
                Some(slot) if slot.1 == version => {
                    slot.2 = frame;
                    continue;
                }
                Some(slot) => slot.0,
                None => {
                    let used: HashSet<u32> = atlas.slots.values().map(|s| s.0).collect();
                    match (0..ATLAS_LAYERS).find(|l| !used.contains(l)) {
                        Some(layer) => layer,
                        None => {
                            // Reuse the layer of the tile needed longest ago, if not now.
                            let Some((&old, &(layer, _, last))) =
                                atlas.slots.iter().min_by_key(|(_, s)| s.2)
                            else {
                                continue;
                            };
                            if last == frame {
                                log::warn!("more than {ATLAS_LAYERS} DEM tiles in view");
                                break;
                            }
                            atlas.slots.remove(&old);
                            layer
                        }
                    }
                }
            };
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &dem.to_rgba(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
            atlas.slots.insert(id, (layer, version, frame));
        }
    }

    /// Where `tile` is and which elevations it stands on, for its draws' uniforms, and the
    /// same elevations for the CPU.
    pub fn geo(
        &self,
        terrain: Option<&TerrainTiles>,
        z_per_meter: f32,
        tile: Placement,
        extent: u32,
    ) -> (TileGeo, Option<ElevationSampler>) {
        let mut geo = TileGeo {
            tile: [
                tile.x as f32,
                tile.y as f32,
                f32::from(tile.z).exp2(),
                extent as f32,
            ],
            ..Default::default()
        };
        let (Some(terrain), Some(atlas)) = (terrain, &self.atlas) else {
            return (geo, None);
        };
        if z_per_meter <= 0.0 {
            return (geo, None);
        }
        let Some(dem_tile) = terrain.find(tile.id()) else {
            return (geo, None);
        };
        let Some(&(layer, _, _)) = atlas.slots.get(&dem_tile) else {
            return (geo, None);
        };
        let sampler = terrain.sampler(tile.id(), extent as f32, z_per_meter);
        let (offset, scale) = TerrainTiles::sub_rect(tile.id(), dem_tile);
        geo.terrain = [z_per_meter, layer as f32, (atlas.size - 2) as f32, 0.0];
        geo.dem = [offset[0] as f32, offset[1] as f32, scale as f32, 0.0];
        (geo, sampler)
    }

    /// The pool of drape textures, `size` pixels per side.
    fn pool(&mut self, gpu: &Gpu, size: u32) -> &DrapePool {
        if self.pool.as_ref().is_none_or(|p| p.size != size) {
            let texture = |label, format, samples, binding: bool| {
                let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
                if binding {
                    usage |= wgpu::TextureUsages::TEXTURE_BINDING;
                }
                gpu.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: size,
                            height: size,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: samples,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let textures = (0..DRAPE_POOL)
                .map(|_| {
                    let view = texture("drape", gpu.format, 1, true);
                    let group =
                        gpu.bind_textures(&self.texture_layout, &[&view], &gpu.linear_sampler);
                    (view, group)
                })
                .collect();
            self.pool = Some(DrapePool {
                size,
                textures,
                msaa: (gpu.sample_count > 1)
                    .then(|| texture("drape msaa", gpu.format, gpu.sample_count, false)),
                depth_stencil: texture(
                    "drape depth stencil",
                    super::gpu::DEPTH_STENCIL_FORMAT,
                    gpu.sample_count,
                    false,
                ),
            });
        }
        self.pool.as_ref().expect("made above")
    }

    /// Begin the pass rendering a tile's layers into pool texture `slot`.
    pub fn begin_drape<'e>(
        &mut self,
        gpu: &Gpu,
        encoder: &'e mut wgpu::CommandEncoder,
        size: u32,
        slot: usize,
    ) -> wgpu::RenderPass<'e> {
        let pool = self.pool(gpu, size);
        let target = &pool.textures[slot].0;
        let (view, resolve) = match &pool.msaa {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("drape"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
                view: &pool.depth_stencil,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0),
                    store: wgpu::StoreOp::Discard,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }

    /// Drape pool texture `slot` over `tile`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_tile(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        arena: &mut UniformArena,
        view: &View,
        terrain: Option<&TerrainTiles>,
        tile: Placement,
        slot: usize,
    ) {
        let Some(pool) = &self.pool else { return };
        let camera = &view.camera;
        let (geo, _) = self.geo(terrain, view.z_per_meter, tile, DRAPE_EXTENT);
        let matrix = camera.tile_matrix(&view.view_projection, tile, DRAPE_EXTENT);
        let mut u = view.tile_uniforms(matrix, &geo);
        // Deep enough to cover where neighbouring tiles' edges differ.
        u.extra[0][0] = (camera.tile_world_size(tile.z) / 16.0) as f32;
        let (group, offset) = arena.push(gpu, &u);
        pass.set_pipeline(&self.drape);
        pass.set_bind_group(0, &group, &[offset]);
        pass.set_bind_group(1, &pool.textures[slot].1, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }

    /// Write stencil value `reference` over `tile` as the terrain shows it.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_mask(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        arena: &mut UniformArena,
        view: &View,
        terrain: Option<&TerrainTiles>,
        tile: Placement,
        reference: u32,
    ) {
        let (geo, _) = self.geo(terrain, view.z_per_meter, tile, DRAPE_EXTENT);
        let matrix = view
            .camera
            .tile_matrix(&view.view_projection, tile, DRAPE_EXTENT);
        let mut u = view.tile_uniforms(matrix, &geo);
        u.extra[0][0] = 1e-6;
        let (group, offset) = arena.push(gpu, &u);
        pass.set_pipeline(&self.mask);
        pass.set_bind_group(0, &group, &[offset]);
        pass.set_stencil_reference(reference);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.grid_index_count, 0, 0..1);
    }

    /// Draw the style's sky behind everything, if it has one.
    pub fn draw_sky(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        arena: &mut UniformArena,
        view: &View,
    ) {
        let Some(sky) = &view.sky else { return };
        let camera = &view.camera;
        let Some(inverse) = crate::camera::invert(&camera.view_matrix()) else {
            return;
        };
        let mut u = view.uniforms();
        u.matrix = inverse.map(|c| c.map(|v| v as f32));
        u.extra[0] = sky.sky.0;
        u.extra[1] = sky.horizon.0;
        u.extra[2] = sky.haze().0;
        // The horizon of the plane is where view rays turn level: 90° - pitch above the
        // view's center.
        let pitch = camera.pitch.to_radians();
        let horizon = if pitch <= 1e-6 {
            1e6
        } else {
            (1.0 / pitch.tan() / (crate::camera::FOV / 2.0).tan()).min(1e6)
        };
        u.extra[3] = [
            horizon as f32,
            sky.sky_horizon_blend,
            sky.atmosphere_blend,
            camera.globe as f32,
        ];
        // The globe's radius, which `uniforms` only sets on the globe.
        u.globe[1] = camera.globe_frame().radius as f32;
        let (group, offset) = arena.push(gpu, &u);
        pass.set_pipeline(&self.sky);
        pass.set_bind_group(0, &group, &[offset]);
        pass.draw(0..3, 0..1);
    }
}
