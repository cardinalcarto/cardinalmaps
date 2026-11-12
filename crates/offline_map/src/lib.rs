//! Offline map regions for the Cardinal app.
//!
//! A region's map comes from the published `planet.pmtiles` by HTTP range requests, made by
//! the app: [`plan`] reads the archive's directories and says which byte ranges hold the
//! tiles covering the region's polygon, and [`mbtiles`] checks each downloaded range's tiles
//! and writes them into the device's one MBTiles file, which sparkle reads.
//!
//! Nothing here does network I/O.

pub mod cover;
pub mod mbtiles;
pub mod plan;
pub mod pmtiles;

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
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
