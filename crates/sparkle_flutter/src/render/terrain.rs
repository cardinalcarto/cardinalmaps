//! Drawing the map in 3D: over terrain, on the globe, and under the style's sky.
//!
//! As MapLibre does with terrain, the flat layers (fills, lines, rasters and the like) are
//! not drawn onto the screen directly but rendered tile by tile into textures, which are
//! then draped over a mesh per tile ([`sparkle::terrain::TerrainMesh`]). The mesh's
//! vertex shader lifts it by the DEM and bends it onto the globe, so the flat layers'
//! own shaders never need to know about either. Layers that are 3D themselves (symbols,
//! circles, heatmaps, extrusions) are drawn into the main pass and sample the same DEM in
//! their vertex shaders.
//!
//! Where the harness keeps DEM tiles in one texture array, Flutter GPU has none: each DEM
//! tile in use gets a texture of its own, bound as `dem` with the draws over it.

use std::collections::{HashMap, HashSet};

use sparkle::TileId;
use sparkle::terrain::{ElevationSampler, MESH_SIZE, TerrainMesh, TerrainTiles, TerrainVertex};
use sparkle_view::camera::Placement;

use super::Gpu;
use super::layers::bind_uniforms;
use super::view::{TileGeo, View};
use crate::gpu::{
    Blend, Buffer, Compare, DepthMode, DepthStencilDesc, Load, Pass, PassDesc, Pipeline,
    PipelineSpec, StencilOp, StencilState, Store, Texture, TextureFormat, Topology,
    VertexBufferLayout, VertexFormat,
};
use crate::render::LINEAR;

/// Tile units across a drape tile: only precision depends on it.
pub const DRAPE_EXTENT: u32 = 8192;
/// How many tiles' textures are rendered before they're draped, per batch.
pub const DRAPE_POOL: usize = 16;
/// DEM tiles kept on the GPU at once.
const MAX_DEMS: usize = 64;

/// A DEM tile's texture: the tile, the version uploaded, and the frame it was last needed.
struct Dem {
    texture: Texture,
    version: u64,
    last_used: u64,
}

/// The textures one batch of tiles is rendered into, and what rendering them takes.
struct DrapePool {
    /// Pixels per side.
    size: u32,
    textures: Vec<Texture>,
    /// Multisampled color, resolved into the pool's textures; `None` without MSAA.
    msaa: Option<Texture>,
    depth_stencil: Texture,
}

/// The terrain as drawn this frame, for passes of 3D layers that must be hidden behind it
/// (see [`Ground::draw_depth`]).
pub struct Ground<'a> {
    pub renderer: &'a TerrainRenderer,
    pub tiles: Option<&'a TerrainTiles>,
    pub placements: &'a [Placement],
}

impl Ground<'_> {
    /// Draw the terrain's depth (only) into `pass`, which has a depth-stencil attachment.
    pub fn draw_depth(&self, gpu: &Gpu, pass: &mut Pass<'_>, view: &View) {
        let r = self.renderer;
        for &tile in self.placements {
            let (geo, _) = r.geo(self.tiles, view.z_per_meter, tile, DRAPE_EXTENT);
            let matrix = view
                .camera
                .tile_matrix(&view.view_projection, tile, DRAPE_EXTENT);
            let u = view.tile_uniforms(matrix, &geo);
            pass.set_pipeline(&r.depth);
            bind_uniforms(gpu, pass, &u, geo.dem_texture.as_ref());
            pass.set_vertex_buffer(0, r.vertices.slice());
            pass.set_index_buffer(r.indices.slice(), true);
            pass.draw_indexed(r.grid_index_count, 1);
        }
    }
}

pub struct TerrainRenderer {
    vertices: Buffer,
    indices: Buffer,
    /// All of the mesh's indices, and the grid's alone (without skirts).
    index_count: u32,
    grid_index_count: u32,
    drape: Pipeline,
    /// The terrain's depth alone.
    depth: Pipeline,
    mask: Pipeline,
    sky: Pipeline,
    dems: HashMap<TileId, Dem>,
    /// Pixels per side of the DEM tiles in use, without their border.
    dem_dim: Option<u32>,
    pool: Option<DrapePool>,
    frame: u64,
    warned_dim: bool,
}

fn mesh_layout() -> VertexBufferLayout {
    let layout = VertexBufferLayout::packed(&[
        ("uv", VertexFormat::Float32x2),
        ("skirt", VertexFormat::Float32),
    ]);
    debug_assert_eq!(layout.stride as usize, size_of::<TerrainVertex>());
    layout
}

/// The mesh as `clip_mesh.vert` reads it: without `skirt`, which it doesn't use, so the
/// shader compiler drops that input and a layout naming it would be rejected.
fn mask_layout() -> VertexBufferLayout {
    let mut layout = mesh_layout();
    layout.attributes.retain(|(name, _, _)| name != "skirt");
    layout
}

impl TerrainRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        let mesh = TerrainMesh::grid(MESH_SIZE);
        let drape = gpu.pipeline(&PipelineSpec {
            program: "Terrain",
            vertex_buffers: vec![mesh_layout()],
            clip: false,
            depth: DepthMode::TestWrite,
            ..Default::default()
        });
        let depth = gpu.pipeline(&PipelineSpec {
            // The mask's shader, which writes nothing but its position.
            program: "ClipMesh",
            vertex_buffers: vec![mask_layout()],
            clip: false,
            depth: DepthMode::TestWrite,
            blend: Blend::NoColor,
            ..Default::default()
        });
        let mask = gpu.pipeline(&PipelineSpec {
            program: "ClipMesh",
            vertex_buffers: vec![mask_layout()],
            blend: Blend::NoColor,
            depth: DepthMode::Test,
            stencil: Some(StencilState {
                compare: Compare::Always,
                pass_op: StencilOp::Replace,
                read_mask: 0xff,
                write_mask: 0xff,
            }),
            ..Default::default()
        });
        let sky = gpu.pipeline(&PipelineSpec {
            program: "Sky",
            vertex_buffers: vec![Gpu::quad_layout()],
            topology: Topology::TriangleStrip,
            clip: false,
            blend: Blend::Replace,
            ..Default::default()
        });
        Self {
            vertices: gpu.buffer(bytemuck::cast_slice(&mesh.vertices)),
            indices: gpu.buffer(bytemuck::cast_slice(&mesh.indices)),
            index_count: mesh.indices.len() as u32,
            grid_index_count: MESH_SIZE * MESH_SIZE * 6,
            drape,
            depth,
            mask,
            sky,
            dems: HashMap::new(),
            dem_dim: None,
            pool: None,
            frame: 0,
            warned_dim: false,
        }
    }

    /// Upload the DEM tiles `wanted` (again, if they changed), dropping the ones needed
    /// longest ago beyond [`MAX_DEMS`].
    pub fn update_dems(&mut self, gpu: &Gpu, terrain: &TerrainTiles, wanted: &HashSet<TileId>) {
        self.frame += 1;
        let Some(dim) = wanted
            .iter()
            .find_map(|&id| terrain.get(id))
            .map(|(dem, _)| dem.dim)
        else {
            return;
        };
        if self.dem_dim != Some(dim) {
            self.dems.clear();
            self.dem_dim = Some(dim);
        }
        let frame = self.frame;
        self.dems.retain(|id, _| terrain.get(*id).is_some());
        for &id in wanted {
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
            match self.dems.get_mut(&id) {
                Some(d) if d.version == version => d.last_used = frame,
                Some(d) => {
                    gpu.write_texture(&d.texture, &dem.to_rgba());
                    d.version = version;
                    d.last_used = frame;
                }
                None => {
                    let size = dim + 2;
                    let texture = gpu.rgba_texture(size, size, &dem.to_rgba());
                    self.dems.insert(
                        id,
                        Dem {
                            texture,
                            version,
                            last_used: frame,
                        },
                    );
                }
            }
        }
        if self.dems.len() > MAX_DEMS {
            let mut by_age: Vec<(u64, TileId)> =
                self.dems.iter().map(|(id, d)| (d.last_used, *id)).collect();
            by_age.sort_unstable();
            for (last_used, id) in by_age.into_iter().take(self.dems.len() - MAX_DEMS) {
                if last_used == frame {
                    break;
                }
                self.dems.remove(&id);
            }
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
        let (Some(terrain), Some(dim)) = (terrain, self.dem_dim) else {
            return (geo, None);
        };
        if z_per_meter <= 0.0 {
            return (geo, None);
        }
        let Some(dem_tile) = terrain.find(tile.id()) else {
            return (geo, None);
        };
        let Some(dem) = self.dems.get(&dem_tile) else {
            return (geo, None);
        };
        let sampler = terrain.sampler(tile.id(), extent as f32, z_per_meter);
        let (offset, scale) = TerrainTiles::sub_rect(tile.id(), dem_tile);
        geo.terrain = [z_per_meter, 0.0, dim as f32, 0.0];
        geo.dem = [offset[0] as f32, offset[1] as f32, scale as f32, 0.0];
        geo.dem_texture = Some(dem.texture.clone());
        (geo, sampler)
    }

    /// The pool of drape textures, `size` pixels per side.
    fn pool(&mut self, gpu: &Gpu, size: u32) -> &DrapePool {
        if self.pool.as_ref().is_none_or(|p| p.size != size) {
            let samples = gpu.info.sample_count;
            self.pool = Some(DrapePool {
                size,
                textures: (0..DRAPE_POOL)
                    .map(|_| gpu.target([size; 2], TextureFormat::Screen, 1))
                    .collect(),
                msaa: (samples > 1).then(|| gpu.target([size; 2], TextureFormat::Screen, samples)),
                depth_stencil: gpu.target([size; 2], TextureFormat::DepthStencil, samples),
            });
        }
        self.pool.as_ref().expect("made above")
    }

    /// Begin the pass rendering a tile's layers into pool texture `slot`.
    pub fn begin_drape<'g>(&mut self, gpu: &'g Gpu, size: u32, slot: usize) -> Pass<'g> {
        let pool = self.pool(gpu, size);
        let target = &pool.textures[slot];
        let (color, resolve) = match &pool.msaa {
            Some(msaa) => (msaa, Some(target)),
            None => (target, None),
        };
        gpu.begin_pass(&PassDesc {
            color,
            resolve,
            color_load: Load::Clear([0.0; 4]),
            color_store: if resolve.is_some() {
                Store::Resolve
            } else {
                Store::Store
            },
            depth_stencil: Some(DepthStencilDesc {
                texture: &pool.depth_stencil,
                depth_load: Load::Clear(1.0),
                depth_store: Store::DontCare,
                stencil_load: Load::Clear(0),
                stencil_store: Store::DontCare,
            }),
        })
    }

    /// Drape pool texture `slot` over `tile`.
    pub fn draw_tile(
        &self,
        gpu: &Gpu,
        pass: &mut Pass<'_>,
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
        pass.set_pipeline(&self.drape);
        bind_uniforms(gpu, pass, &u, geo.dem_texture.as_ref());
        pass.set_texture("drape", &pool.textures[slot], LINEAR);
        pass.set_vertex_buffer(0, self.vertices.slice());
        pass.set_index_buffer(self.indices.slice(), true);
        pass.draw_indexed(self.index_count, 1);
    }

    /// Write stencil value `reference` over `tile` as the terrain shows it.
    pub fn draw_mask(
        &self,
        gpu: &Gpu,
        pass: &mut Pass<'_>,
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
        pass.set_pipeline(&self.mask);
        bind_uniforms(gpu, pass, &u, geo.dem_texture.as_ref());
        pass.set_stencil_reference(reference);
        pass.set_vertex_buffer(0, self.vertices.slice());
        pass.set_index_buffer(self.indices.slice(), true);
        pass.draw_indexed(self.grid_index_count, 1);
    }

    /// Draw the style's sky behind everything, if it has one.
    pub fn draw_sky(&self, gpu: &Gpu, pass: &mut Pass<'_>, view: &View) {
        let Some(sky) = &view.sky else { return };
        let camera = &view.camera;
        let Some(inverse) = sparkle_view::camera::invert(&camera.view_matrix()) else {
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
            (1.0 / pitch.tan() / (sparkle_view::camera::FOV / 2.0).tan()).min(1e6)
        };
        u.extra[3] = [
            horizon as f32,
            sky.sky_horizon_blend,
            sky.atmosphere_blend,
            camera.globe as f32,
        ];
        // The globe's radius, which `uniforms` only sets on the globe.
        u.globe[1] = camera.globe_frame().radius as f32;
        pass.set_pipeline(&self.sky);
        bind_uniforms(gpu, pass, &u, None);
        pass.set_vertex_buffer(0, gpu.quad.slice());
        pass.draw(4, 1);
    }
}
