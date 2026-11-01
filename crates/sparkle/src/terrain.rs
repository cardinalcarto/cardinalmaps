//! 3D terrain from the style's `terrain` source: the loaded DEM tiles, elevations looked
//! up anywhere on the map, and the mesh tiles are draped over.
//!
//! As in MapLibre, one generic mesh serves every tile: a grid over the tile with a skirt
//! hanging from its edges, which hides the cracks where neighbouring tiles' elevations
//! don't quite meet (tiles of different zoom levels, or edges not backfilled yet). A
//! renderer lifts each vertex by the elevation it samples from the tile's DEM, the same
//! bilinear sampling as [`DemData::elevation_at`], which [`TerrainTiles`] uses on the CPU.

use std::collections::HashMap;
use std::sync::Arc;

use crate::TileId;
use crate::dem::DemData;
use crate::projection::EARTH_RADIUS;

/// Grid cells per side of [`TerrainMesh::grid`] that renderers use.
pub const MESH_SIZE: u32 = 64;

/// A vertex of [`TerrainMesh`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainVertex {
    /// `(0, 0)` is the tile's top left corner, `(1, 1)` its bottom right.
    pub position: [f32; 2],
    /// 1 for the bottom of the skirt, which hangs below the edge vertex at `position`.
    pub skirt: f32,
}

/// The mesh every terrain tile is drawn with, as an indexed triangle list.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainMesh {
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u16>,
}

impl TerrainMesh {
    /// `cells`² grid cells over the tile, two triangles each, and a skirt of one quad per
    /// edge cell. Triangles wind counter-clockwise seen from above (`y` down).
    ///
    /// # Panics
    ///
    /// If the mesh needs more vertices than 16-bit indices reach.
    pub fn grid(cells: u32) -> Self {
        let n = cells + 1;
        assert!(n * n + 4 * n <= 1 << 16, "terrain mesh too fine");
        let mut vertices = Vec::with_capacity((n * n + 4 * n) as usize);
        let at = |i: u32| i as f32 / cells as f32;
        for y in 0..n {
            for x in 0..n {
                vertices.push(TerrainVertex {
                    position: [at(x), at(y)],
                    skirt: 0.0,
                });
            }
        }
        let mut indices = Vec::new();
        let index = |x: u32, y: u32| (y * n + x) as u16;
        for y in 0..cells {
            for x in 0..cells {
                let (a, b, c, d) = (
                    index(x, y),
                    index(x + 1, y),
                    index(x, y + 1),
                    index(x + 1, y + 1),
                );
                indices.extend([a, c, b, b, c, d]);
            }
        }
        // Each edge as a run of grid vertices, walked so the tile's inside is on the left.
        let edges: [Vec<(u32, u32)>; 4] = [
            (0..n).map(|i| (i, 0)).collect(),
            (0..n).map(|i| (cells, i)).collect(),
            (0..n).rev().map(|i| (i, cells)).collect(),
            (0..n).rev().map(|i| (0, i)).collect(),
        ];
        for edge in edges {
            let start = vertices.len() as u16;
            for &(x, y) in &edge {
                vertices.push(TerrainVertex {
                    position: [at(x), at(y)],
                    skirt: 1.0,
                });
            }
            for (i, pair) in edge.windows(2).enumerate() {
                let (top_a, top_b) = (index(pair[0].0, pair[0].1), index(pair[1].0, pair[1].1));
                let (low_a, low_b) = (start + i as u16, start + i as u16 + 1);
                indices.extend([top_a, top_b, low_a, top_b, low_b, low_a]);
            }
        }
        Self { vertices, indices }
    }
}

/// Logical pixels per meter at a world `world_size` logical pixels wide, at the latitude
/// of world `y`: what elevations are scaled by to stand on a Mercator map.
pub fn pixels_per_meter(world_size: f64, y: f64) -> f64 {
    let lat = crate::projection::world_to_lng_lat([0.0, y])[1];
    world_size / (2.0 * std::f64::consts::PI * EARTH_RADIUS * lat.cos())
}

/// Elevations under one tile, from a DEM tile at the same place or an ancestor of it.
#[derive(Debug, Clone)]
pub struct ElevationSampler {
    pub dem: Arc<DemData>,
    /// Where the tile's top left corner is in the DEM tile, and the fraction of the DEM
    /// tile it spans.
    pub offset: [f64; 2],
    pub scale: f64,
    /// Tile units across the tile.
    pub extent: f32,
    /// What a meter of elevation becomes, terrain exaggeration included.
    pub z_per_meter: f32,
}

impl PartialEq for ElevationSampler {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.dem, &other.dem)
            && self.offset == other.offset
            && self.scale == other.scale
            && self.extent == other.extent
            && self.z_per_meter == other.z_per_meter
    }
}

impl ElevationSampler {
    /// The elevation at `p` (tile units), scaled by [`Self::z_per_meter`].
    pub fn at(&self, p: [f32; 2]) -> f32 {
        let [u, v] = [0, 1].map(|i| self.offset[i] + f64::from(p[i] / self.extent) * self.scale);
        self.dem.elevation_at(u, v) * self.z_per_meter
    }
}

struct TerrainTile {
    dem: Arc<DemData>,
    /// Bumped whenever `dem`'s border changes, so renderers know to upload it again.
    version: u64,
    min_max: (f32, f32),
}

/// The loaded tiles of a terrain source, with their borders backfilled from each other so
/// elevations are seamless across tiles. Elevations are in meters, unexaggerated.
#[derive(Default)]
pub struct TerrainTiles {
    tiles: HashMap<TileId, TerrainTile>,
    /// How many tiles each zoom level has, to search only levels that have some.
    per_zoom: [usize; 32],
    next_version: u64,
}

impl TerrainTiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Add a tile, exchanging border pixels with its loaded neighbours of the same zoom.
    pub fn insert(&mut self, id: TileId, mut dem: Arc<DemData>) {
        if self.tiles.contains_key(&id) {
            self.remove(id);
        }
        let n = 1i64 << id.z;
        for dy in -1..=1i32 {
            for dx in -1..=1i32 {
                let y = i64::from(id.y) + i64::from(dy);
                if (dx, dy) == (0, 0) || !(0..n).contains(&y) {
                    continue;
                }
                // The world wraps around horizontally.
                let x = (i64::from(id.x) + i64::from(dx)).rem_euclid(n);
                let neighbor_id = TileId::new(id.z, x as u32, y as u32);
                self.next_version += 1;
                let version = self.next_version;
                let Some(neighbor) = self.tiles.get_mut(&neighbor_id) else {
                    continue;
                };
                if neighbor.dem.dim != dem.dim || neighbor_id == id {
                    continue;
                }
                Arc::make_mut(&mut dem).backfill_border(&neighbor.dem, dx, dy);
                Arc::make_mut(&mut neighbor.dem).backfill_border(&dem, -dx, -dy);
                neighbor.version = version;
            }
        }
        self.next_version += 1;
        let min_max = dem.min_max();
        self.tiles.insert(
            id,
            TerrainTile {
                dem,
                version: self.next_version,
                min_max,
            },
        );
        self.per_zoom[usize::from(id.z)] += 1;
    }

    pub fn remove(&mut self, id: TileId) {
        if self.tiles.remove(&id).is_some() {
            self.per_zoom[usize::from(id.z)] -= 1;
        }
    }

    /// A tile's elevations and their version (see [`Self::insert`]).
    pub fn get(&self, id: TileId) -> Option<(&Arc<DemData>, u64)> {
        self.tiles.get(&id).map(|t| (&t.dem, t.version))
    }

    /// The loaded tile nearest up the tree from `id` (itself included).
    pub fn find(&self, id: TileId) -> Option<TileId> {
        (0..=id.z)
            .map(|k| TileId::new(id.z - k, id.x >> k, id.y >> k))
            .find(|a| self.per_zoom[usize::from(a.z)] > 0 && self.tiles.contains_key(a))
    }

    /// Where tile `id` is inside `dem_tile`, one of its ancestors (or itself): the offset of
    /// its top left corner and the fraction it spans, as an [`ElevationSampler`] has them.
    pub fn sub_rect(id: TileId, dem_tile: TileId) -> ([f64; 2], f64) {
        let levels = id.z - dem_tile.z;
        let scale = 0.5f64.powi(i32::from(levels));
        let mask = (1u32 << levels) - 1;
        let offset = [id.x & mask, id.y & mask].map(|v| f64::from(v) * scale);
        (offset, scale)
    }

    /// Elevations for tile `id`, from it or its nearest loaded ancestor.
    pub fn sampler(&self, id: TileId, extent: f32, z_per_meter: f32) -> Option<ElevationSampler> {
        let dem_tile = self.find(id)?;
        let (offset, scale) = Self::sub_rect(id, dem_tile);
        Some(ElevationSampler {
            dem: self.tiles[&dem_tile].dem.clone(),
            offset,
            scale,
            extent,
            z_per_meter,
        })
    }

    /// The elevation at world coordinates `world` (`0..1`, `x` wrapping), from the most
    /// detailed tile loaded there.
    pub fn elevation_at(&self, world: [f64; 2]) -> Option<f32> {
        let x = world[0].rem_euclid(1.0);
        let y = world[1].clamp(0.0, 1.0 - 1e-12);
        (0..32u8).rev().find_map(|z| {
            if self.per_zoom[usize::from(z)] == 0 {
                return None;
            }
            let n = f64::from(z).exp2();
            let (tx, ty) = ((x * n).floor(), (y * n).floor());
            let tile = self.tiles.get(&TileId::new(z, tx as u32, ty as u32))?;
            Some(tile.dem.elevation_at(x * n - tx, y * n - ty))
        })
    }

    /// The lowest and highest elevation in tile `id`, as far as the loaded tiles tell: of
    /// the nearest loaded ancestor, which may span more than the tile.
    pub fn min_max(&self, id: TileId) -> Option<(f32, f32)> {
        self.find(id).map(|a| self.tiles[&a].min_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(dim: u32, offset: f32) -> Arc<DemData> {
        Arc::new(DemData::from_fn(dim, |x, y| {
            offset + x as f32 * 10.0 + y as f32
        }))
    }

    #[test]
    fn mesh_covers_the_tile_with_a_skirt() {
        let mesh = TerrainMesh::grid(4);
        assert_eq!(mesh.vertices.len(), 25 + 20);
        // 32 grid triangles and two per skirt quad, 4 quads per edge.
        assert_eq!(mesh.indices.len(), (32 + 4 * 4 * 2) * 3);
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
        let skirt = mesh.vertices.iter().filter(|v| v.skirt > 0.0);
        assert!(skirt.clone().count() == 20);
        assert!(
            skirt
                .into_iter()
                .all(|v| v.position.iter().any(|&c| c == 0.0 || c == 1.0))
        );
        // Every grid triangle winds the same way (counter-clockwise with y down means a
        // negative cross product).
        for t in mesh.indices[..96].chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize].position);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross < 0.0, "{t:?}");
        }
        assert_eq!(
            TerrainMesh::grid(MESH_SIZE).vertices.len(),
            65 * 65 + 4 * 65
        );
    }

    #[test]
    fn finds_the_deepest_tile_up_the_tree() {
        let mut tiles = TerrainTiles::new();
        tiles.insert(TileId::new(2, 1, 1), ramp(4, 0.0));
        assert_eq!(tiles.find(TileId::new(4, 5, 6)), Some(TileId::new(2, 1, 1)));
        assert_eq!(tiles.find(TileId::new(4, 0, 0)), None);
        assert_eq!(tiles.find(TileId::new(1, 0, 0)), None);
        let (offset, scale) = TerrainTiles::sub_rect(TileId::new(4, 5, 6), TileId::new(2, 1, 1));
        assert_eq!((offset, scale), ([0.25, 0.5], 0.25));
    }

    #[test]
    fn samples_elevations_like_the_dem() {
        let mut tiles = TerrainTiles::new();
        let dem = ramp(4, 100.0);
        tiles.insert(TileId::new(1, 1, 0), dem.clone());
        // The center of pixel (1, 2) of the tile spanning x 0.5..1, y 0..0.5.
        let world = [0.5 + 1.5 / 8.0, 2.5 / 8.0];
        assert_eq!(tiles.elevation_at(world), Some(112.0));
        assert_eq!(tiles.elevation_at([0.2, 0.2]), None);
        assert_eq!(tiles.min_max(TileId::new(3, 6, 1)), Some((100.0, 133.0)));

        // A sampler for a child tile reads the same point at its own tile units.
        let sampler = tiles.sampler(TileId::new(2, 2, 0), 512.0, 2.0).unwrap();
        let p = [
            (1.5 / 4.0 * 2.0 * 512.0) as f32,
            (2.5 / 4.0 * 2.0 * 512.0) as f32,
        ];
        assert_eq!(sampler.at(p), 224.0);
    }

    #[test]
    fn neighbours_share_borders() {
        let mut tiles = TerrainTiles::new();
        tiles.insert(TileId::new(3, 2, 2), ramp(4, 0.0));
        let (_, before) = tiles.get(TileId::new(3, 2, 2)).unwrap();
        tiles.insert(TileId::new(3, 3, 2), ramp(4, 1000.0));
        let (left, after) = tiles.get(TileId::new(3, 2, 2)).unwrap();
        assert!(after > before, "the left tile changed");
        assert_eq!(left.get(4, 1), 1001.0);
        let (right, _) = tiles.get(TileId::new(3, 3, 2)).unwrap();
        assert_eq!(right.get(-1, 1), 31.0);
        // Halfway between the two tiles' edge pixels, either tile gives the same height.
        let a = tiles.elevation_at([3.0 / 8.0 - 1e-9, 2.5 / 8.0]).unwrap();
        let b = tiles.elevation_at([3.0 / 8.0, 2.5 / 8.0]).unwrap();
        assert!((a - b).abs() < 1e-2, "{a} {b}");
    }

    #[test]
    fn pixels_per_meter_grow_toward_the_poles() {
        let equator = pixels_per_meter(512.0, 0.5);
        assert!((equator - 512.0 / 40_075_016.686).abs() < 1e-12);
        assert!(pixels_per_meter(512.0, 0.3) > equator);
    }
}
