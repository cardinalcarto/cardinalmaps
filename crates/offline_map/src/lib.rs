//! Offline map regions for the Cardinal app.
//!
//! A region's map comes from the published `planet.pmtiles` by HTTP range requests, made by
//! the app: [`plan`] reads the archive's directories and says which byte ranges hold the
//! tiles covering the region's polygon, and [`mbtiles`] checks each downloaded range's tiles
//! and writes them into the device's one MBTiles file, which sparkle reads.
//!
//! [`write`] writes archives of tiles whose sizes are known first, which `cardinal-pack`
//! uses to pack a transit build's transfer tiles, one archive per mode.
//!
//! Nothing here does network I/O.

pub mod cover;
#[cfg(feature = "mbtiles")]
pub mod mbtiles;
pub mod plan;
pub mod pmtiles;
pub mod write;

use pmtiles::Compression;

/// What went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a PMTiles v3 archive")]
    NotPmtiles,
    #[error("the root directory isn't in the bytes given")]
    RootNotInFirstBytes,
    #[error("a directory doesn't decode")]
    BadDirectory,
    #[error("directories compressed with {0:?} aren't supported")]
    UnsupportedCompression(Compression),
    #[error("tile type {0} isn't supported")]
    UnsupportedTileType(u8),
    #[error("leaf directories are still needed")]
    LeavesMissing,
    #[error("a tile list doesn't decode")]
    BadTileList,
    #[error("the downloaded range is {found} bytes, not {expected}")]
    ChunkLength { expected: u64, found: u64 },
    #[error("tile {z}/{x}/{y} isn't a whole gzip stream")]
    BadTile { z: u8, x: u32, y: u32 },
    #[error("tiles to write aren't in increasing tile ID order")]
    Unordered,
    #[error("tile {tile_id} is {found} bytes, not {expected}")]
    TileLength {
        tile_id: u64,
        expected: u32,
        found: u64,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(feature = "mbtiles")]
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
