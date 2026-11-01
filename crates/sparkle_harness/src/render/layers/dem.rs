//! What `hillshade` and `color-relief` share: DEM tiles on the GPU whose borders are
//! backfilled from their neighbours as those arrive, so shading and sampling are seamless
//! across tiles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use sparkle::TileId;
use sparkle::dem::DemData;

use crate::render::gpu::Gpu;

/// What a DEM tile's texture holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemTexture {
    /// [`DemData::hillshade_slopes`], `dim`² pixels.
    Slopes,
    /// [`DemData::to_rgba`], `(dim + 2)²` pixels including the border.
    Elevations,
}

/// One layer's DEM tile on the GPU.
pub struct DemTile {
    pub tile: TileId,
    dem: RefCell<DemData>,
    kind: DemTexture,
    pub texture: wgpu::Texture,
}

impl DemTile {
    /// Upload `dem`, using `contents` as the texture if it's already computed.
    pub fn new(
        gpu: &Gpu,
        tile: TileId,
        dem: DemData,
        kind: DemTexture,
        contents: Option<&[u8]>,
    ) -> Self {
        let size = Self::size(&dem, kind);
        let computed;
        let contents = match contents {
            Some(contents) => contents,
            None => {
                computed = Self::contents(&dem, kind, tile);
                &computed
            }
        };
        let texture = gpu.rgba_texture("dem", size, size, contents);
        Self {
            tile,
            dem: RefCell::new(dem),
            kind,
            texture,
        }
    }

    pub fn dim(&self) -> u32 {
        self.dem.borrow().dim
    }

    fn size(dem: &DemData, kind: DemTexture) -> u32 {
        match kind {
            DemTexture::Slopes => dem.dim,
            DemTexture::Elevations => dem.dim + 2,
        }
    }

    fn contents(dem: &DemData, kind: DemTexture, tile: TileId) -> Vec<u8> {
        match kind {
            DemTexture::Slopes => dem.hillshade_slopes(tile.z).pixels,
            DemTexture::Elevations => dem.to_rgba(),
        }
    }

    /// Recompute and upload the texture after the border changed.
    fn refresh(&self, gpu: &Gpu) {
        let dem = self.dem.borrow();
        let size = Self::size(&dem, self.kind);
        gpu.queue.write_texture(
            self.texture.as_image_copy(),
            &Self::contents(&dem, self.kind, self.tile),
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
    }
}

/// The DEM tiles uploaded for each layer, by `(style layer, tile)`, to find neighbours by.
/// Tiles stay alive only as long as the map caches them.
#[derive(Default)]
pub struct DemTiles {
    tiles: HashMap<(usize, TileId), Weak<DemTile>>,
}

impl DemTiles {
    /// Add a newly uploaded tile, exchanging border pixels with the neighbours of the same
    /// layer that are already loaded.
    pub fn insert(&mut self, gpu: &Gpu, layer: usize, tile: &Rc<DemTile>) {
        self.tiles.retain(|_, t| t.strong_count() > 0);
        let id = tile.tile;
        let n = 1i64 << id.z;
        let mut changed = false;
        for dy in -1..=1i32 {
            for dx in -1..=1i32 {
                let y = i64::from(id.y) + i64::from(dy);
                if (dx, dy) == (0, 0) || !(0..n).contains(&y) {
                    continue;
                }
                // The world wraps around horizontally.
                let x = (i64::from(id.x) + i64::from(dx)).rem_euclid(n);
                let neighbor_id = TileId::new(id.z, x as u32, y as u32);
                let Some(neighbor) = self
                    .tiles
                    .get(&(layer, neighbor_id))
                    .and_then(Weak::upgrade)
                else {
                    continue;
                };
                if neighbor.dim() != tile.dim() || Rc::ptr_eq(&neighbor, tile) {
                    continue;
                }
                tile.dem
                    .borrow_mut()
                    .backfill_border(&neighbor.dem.borrow(), dx, dy);
                neighbor
                    .dem
                    .borrow_mut()
                    .backfill_border(&tile.dem.borrow(), -dx, -dy);
                neighbor.refresh(gpu);
                changed = true;
            }
        }
        if changed {
            tile.refresh(gpu);
        }
        self.tiles.insert((layer, id), Rc::downgrade(tile));
    }
}
