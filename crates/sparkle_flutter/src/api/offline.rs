//! Offline map regions, for the app's download engine (`flutter/cardinal/lib/offline/`):
//! planning a region's byte ranges of the published `planet.pmtiles`, and unpacking
//! downloaded ranges into the device's `basemap.mbtiles` (see the `offline_map` crate).
//!
//! These live here, not in `routing_flutter`, because the map reads `basemap.mbtiles` through
//! this library's SQLite: two copies of SQLite in one process don't see each other's POSIX
//! locks, so a writer in another library could corrupt what the map reads.

use std::path::Path;

use flutter_rust_bridge::frb;
use offline_map::Error;
use offline_map::cover::Polygon;
use offline_map::mbtiles::{Basemap, BasemapReader};
use offline_map::plan::{
    GAP_BYTES, MAX_CHUNK_BYTES, decode_chunk_tiles, decode_tile_keys, encode_chunk_tiles,
    encode_tile_keys,
};
use offline_map::pmtiles::{FIRST_FETCH, Header};

use crate::api::map::LngLat;

/// A byte range of the archive.
pub struct ByteRange {
    /// From the start of the archive.
    pub offset: i64,
    pub length: i64,
}

/// One range to download, with its tiles (opaque bytes to keep until it is unpacked).
pub struct MapChunkPlan {
    /// From the start of the archive.
    pub offset: i64,
    pub length: i64,
    pub tile_count: u32,
    pub tiles: Vec<u8>,
}

/// What a region's map needs.
pub struct MapRegionPlan {
    /// Tiles that intersect the polygon, from zoom 0 to the archive's highest.
    pub covering_tiles: u32,
    /// Those the archive has.
    pub archive_tiles: u32,
    /// Those already in the basemap, which another region brought.
    pub present_tiles: u32,
    /// The bytes of the tiles to download, each stored tile once.
    pub tile_bytes: i64,
    /// The bytes of the ranges, gaps between tiles included.
    pub range_bytes: i64,
    pub chunks: Vec<MapChunkPlan>,
    /// Every one of the region's tiles the archive has (opaque bytes for
    /// [`basemap_claim`]).
    pub tile_keys: Vec<u8>,
}

/// What a downloaded range held.
pub struct UnpackedChunk {
    pub tiles: i64,
    pub bytes: i64,
}

/// What kind of problem it is.
pub enum MapErrorKind {
    /// The polygon has fewer than three points, or a point that isn't a coordinate.
    BadPolygon,
    /// The bytes aren't the archive's header and root directory, or a directory doesn't decode.
    BadArchive,
    /// [`MapPlanner::finish`] was called while leaf directories were still needed.
    LeavesMissing,
    /// A downloaded range isn't as long as asked for: download it again.
    ChunkLength,
    /// A tile in a downloaded range isn't whole: download it again.
    BadTile,
    /// The basemap file or a range's file can't be read or written.
    Storage,
}

/// Why a map operation failed.
pub struct MapError {
    pub kind: MapErrorKind,
    /// A description for logs.
    pub message: String,
}

impl From<Error> for MapError {
    fn from(error: Error) -> Self {
        let kind = match &error {
            Error::NotPmtiles
            | Error::RootNotInFirstBytes
            | Error::BadDirectory
            | Error::UnsupportedCompression(_)
            | Error::UnsupportedTileType(_) => MapErrorKind::BadArchive,
            Error::LeavesMissing => MapErrorKind::LeavesMissing,
            Error::BadTileList
            | Error::Io(_)
            | Error::Sqlite(_)
            | Error::Unordered
            | Error::TileLength { .. } => MapErrorKind::Storage,
            Error::ChunkLength { .. } => MapErrorKind::ChunkLength,
            Error::BadTile { .. } => MapErrorKind::BadTile,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Byte counts and offsets cross to Dart as `int` (64-bit signed): archives are far smaller
/// than 2^63 bytes.
fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn unsigned(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// How many bytes to fetch first: the archive's header and root directory are in them.
#[frb(sync)]
pub fn map_first_fetch() -> i64 {
    signed(FIRST_FETCH)
}

/// Finds a polygon's tiles in the archive. Opaque to Dart.
#[frb(opaque)]
pub struct MapPlanner {
    inner: offline_map::plan::MapPlanner,
}

impl MapPlanner {
    /// A planner for the tiles from `min_zoom` to `max_zoom` (clamped to the archive's) that
    /// intersect `polygon`, from the archive's first [`map_first_fetch`] bytes.
    pub fn new(
        first_bytes: Vec<u8>,
        polygon: Vec<LngLat>,
        min_zoom: u8,
        max_zoom: u8,
    ) -> Result<MapPlanner, MapError> {
        let polygon =
            Polygon::new(polygon.iter().map(|p| (p.lng, p.lat)).collect()).ok_or(MapError {
                kind: MapErrorKind::BadPolygon,
                message: "a region needs at least three points, each a longitude and latitude"
                    .to_owned(),
            })?;
        Ok(Self {
            inner: offline_map::plan::MapPlanner::new(&first_bytes, &polygon, min_zoom, max_zoom)?,
        })
    }

    /// The leaf directories to fetch next; empty once every tile is found. Fetching some
    /// may ask for more (directories can nest).
    #[frb(sync)]
    pub fn needed_leaves(&self) -> Vec<ByteRange> {
        self.inner
            .needed_leaves()
            .into_iter()
            .map(|(offset, length)| ByteRange {
                offset: signed(offset),
                length: i64::from(length),
            })
            .collect()
    }

    /// Adds a leaf directory fetched from `offset`.
    pub fn add_leaf(&mut self, offset: i64, bytes: Vec<u8>) -> Result<(), MapError> {
        Ok(self.inner.add_leaf(unsigned(offset), &bytes)?)
    }

    /// The region's plan: its tiles, less those `basemap` (if it exists) has, in ranges.
    pub fn finish(&self, basemap: Option<String>) -> Result<MapRegionPlan, MapError> {
        let tiles = self.inner.tiles()?;
        let present = match basemap.as_deref().map(Path::new) {
            Some(path) if path.exists() => Basemap::open(path)?.present(&tiles)?,
            _ => Default::default(),
        };
        let fetch: Vec<_> = tiles
            .iter()
            .filter(|t| !present.contains(&(t.z, t.x, t.y)))
            .copied()
            .collect();
        let mut stored: Vec<(u64, u32)> = fetch.iter().map(|t| (t.offset, t.length)).collect();
        stored.sort_unstable();
        stored.dedup();
        let chunks = offline_map::plan::chunks(&fetch, GAP_BYTES, MAX_CHUNK_BYTES);
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        Ok(MapRegionPlan {
            covering_tiles: count(self.inner.covering_tiles()),
            archive_tiles: count(tiles.len()),
            present_tiles: count(present.len()),
            tile_bytes: signed(stored.iter().map(|&(_, length)| u64::from(length)).sum()),
            range_bytes: signed(chunks.iter().map(|c| c.length).sum()),
            chunks: chunks
                .into_iter()
                .map(|c| MapChunkPlan {
                    offset: signed(c.offset),
                    length: signed(c.length),
                    tile_count: count(c.tiles.len()),
                    tiles: encode_chunk_tiles(&c.tiles),
                })
                .collect(),
            tile_keys: encode_tile_keys(tiles.iter().map(|t| (t.z, t.x, t.y))),
        })
    }
}

/// Creates the basemap at `path` if needed, with metadata from the archive's header
/// (the first bytes the planner was given).
pub fn basemap_prepare(path: String, header_bytes: Vec<u8>, name: String) -> Result<(), MapError> {
    let header = Header::parse(&header_bytes)?;
    Basemap::open(Path::new(&path))?.set_metadata(&header, &name)?;
    Ok(())
}

/// Checks a downloaded range (the file at `chunk_file`, which must be `expected_length`
/// bytes) and writes its tiles into the basemap as in `region`.
/// A range that fails a check writes nothing.
pub fn basemap_unpack(
    path: String,
    chunk_file: String,
    expected_length: i64,
    tiles: Vec<u8>,
    region: i64,
) -> Result<UnpackedChunk, MapError> {
    let tiles = decode_chunk_tiles(&tiles)?;
    let bytes = std::fs::read(&chunk_file).map_err(|e| MapError {
        kind: MapErrorKind::Storage,
        message: format!("{chunk_file}: {e}"),
    })?;
    let mut basemap = Basemap::open(Path::new(&path))?;
    let compression = basemap.compression()?;
    let unpacked = basemap.unpack(
        &bytes,
        unsigned(expected_length),
        &tiles,
        region,
        compression,
    )?;
    Ok(UnpackedChunk {
        tiles: signed(unpacked.tiles),
        bytes: signed(unpacked.bytes),
    })
}

/// Adds `region` to the tiles of `tile_keys` that the basemap has; returns how many it lacks.
pub fn basemap_claim(path: String, region: i64, tile_keys: Vec<u8>) -> Result<u32, MapError> {
    let keys = decode_tile_keys(&tile_keys)?;
    let missing = Basemap::open(Path::new(&path))?.claim(region, &keys)?;
    Ok(u32::try_from(missing.len()).unwrap_or(u32::MAX))
}

/// Takes `region` off its tiles and deletes those no region holds; returns how many.
pub fn basemap_release(path: String, region: i64) -> Result<i64, MapError> {
    if !Path::new(&path).exists() {
        return Ok(0);
    }
    Ok(signed(Basemap::open(Path::new(&path))?.release(region)?))
}

/// How many tiles `region` holds in the basemap.
pub fn basemap_region_tiles(path: String, region: i64) -> Result<i64, MapError> {
    if !Path::new(&path).exists() {
        return Ok(0);
    }
    Ok(signed(
        Basemap::open(Path::new(&path))?.region_tiles(region)?,
    ))
}

/// One of the basemap's tiles: its bytes as stored (gzipped), and `y` from the top (XYZ).
pub struct BasemapTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub data: Vec<u8>,
}

/// Where a page of a region's tiles starts: after the tile at `x`/`y` (the last page's last).
pub struct BasemapTileAfter {
    pub x: u32,
    pub y: u32,
}

/// A page of `region`'s tiles at `zoom`, at most `limit`, in order of `x` then `y`, after
/// `after`; an empty page means there are no more.
///
/// For the offline geocoder (the `geocoder` plugin), which indexes a region's tiles: it reads
/// them through this library's SQLite, which writes the basemap, since a second copy of
/// SQLite in the process would break the writer's locks (see `offline_map::mbtiles`). The
/// pages go to the geocoder's library from Dart.
pub fn basemap_read_region_tiles(
    path: String,
    region: i64,
    zoom: u8,
    after: Option<BasemapTileAfter>,
    limit: u32,
) -> Result<Vec<BasemapTile>, MapError> {
    if !Path::new(&path).exists() {
        return Ok(Vec::new());
    }
    let tiles = BasemapReader::open(Path::new(&path))?.region_tiles(
        region,
        zoom,
        after.map(|a| (a.x, a.y)),
        limit,
    )?;
    Ok(tiles
        .into_iter()
        .map(|t| BasemapTile {
            z: t.z,
            x: t.x,
            y: t.y,
            data: t.data,
        })
        .collect())
}

/// The tiles `z`/`x`/`y` (from the top) of `keys` that the basemap has, as stored (gzipped),
/// for reading a few features (the offline geocoder's results). Read only.
pub fn basemap_read_tiles(
    path: String,
    keys: Vec<BasemapTileKey>,
) -> Result<Vec<BasemapTile>, MapError> {
    if !Path::new(&path).exists() {
        return Ok(Vec::new());
    }
    let reader = BasemapReader::open(Path::new(&path))?;
    let mut tiles = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(data) = reader.tile(key.z, key.x, key.y)? {
            tiles.push(BasemapTile {
                z: key.z,
                x: key.x,
                y: key.y,
                data,
            });
        }
    }
    Ok(tiles)
}

/// A tile, `y` from the top (XYZ).
pub struct BasemapTileKey {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// How many tiles `region` holds in the basemap at `zoom`.
pub fn basemap_region_zoom_tiles(path: String, region: i64, zoom: u8) -> Result<i64, MapError> {
    if !Path::new(&path).exists() {
        return Ok(0);
    }
    Ok(signed(
        BasemapReader::open(Path::new(&path))?.region_tile_count(region, zoom)?,
    ))
}
