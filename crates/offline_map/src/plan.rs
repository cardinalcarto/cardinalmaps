//! Planning a region's map download: the archive's tiles that cover a polygon, and the
//! byte ranges to fetch them in.
//!
//! The app fetches the archive's first [`FIRST_FETCH`](crate::pmtiles::FIRST_FETCH) bytes
//! (header and root directory), makes a [`MapPlanner`], fetches the leaf directories it asks
//! for (more may be asked for once those are in), and then takes the tiles and [`chunks`].

use std::collections::HashMap;

use crate::Error;
use crate::cover::{Polygon, cover};
use crate::pmtiles::{Entry, Found, Header, find, parse_directory, tile_id};

/// The most bytes between two tiles that one range still spans, rather than starting another.
///
/// Fetching a gap costs its bytes; starting another range costs a request: on a phone each is
/// a background task, scheduled by the OS (tens to hundreds of milliseconds) plus a round trip.
/// At 20 Mbit/s, 256 KiB is about 100 ms, so a gap below it is cheaper to fetch than to skip.
pub const GAP_BYTES: u64 = 256 * 1024;

/// The most bytes in one range. A failed range is fetched again whole, and unpacking reads it
/// into memory, so this bounds both; at 20 Mbit/s it is about 7 seconds.
pub const MAX_CHUNK_BYTES: u64 = 16 * 1024 * 1024;

/// How deep leaf directories may nest (go-pmtiles writes one level).
const MAX_DEPTH: usize = 4;

/// A tile of the archive and where its bytes are, from the start of the archive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub offset: u64,
    pub length: u32,
}

/// A tile in a chunk, at an offset from the chunk's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub offset: u32,
    pub length: u32,
}

/// One byte range to download, and the tiles in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// From the start of the archive.
    pub offset: u64,
    pub length: u64,
    pub tiles: Vec<ChunkTile>,
}

enum Resolved {
    Tile(u64, u32),
    Absent,
    NeedLeaf(u64, u32),
}

/// Finds tiles in an archive (a polygon's, or a list's), asking for leaf directories as it
/// needs them.
pub struct MapPlanner {
    header: Header,
    root: Vec<Entry>,
    /// By offset from the start of the archive.
    leaves: HashMap<u64, Vec<Entry>>,
    wanted: Vec<(u8, u32, u32, u64)>,
}

impl MapPlanner {
    /// A planner for the tiles from `min_zoom` to `max_zoom` (clamped to the archive's) that
    /// intersect `polygon`, from the archive's first bytes.
    ///
    /// # Errors
    ///
    /// If the bytes aren't a PMTiles header followed by its root directory.
    pub fn new(first: &[u8], polygon: &Polygon, min_zoom: u8, max_zoom: u8) -> Result<Self, Error> {
        let header = Header::parse(first)?;
        let start = usize::try_from(header.root_offset).map_err(|_| Error::RootNotInFirstBytes)?;
        let end = usize::try_from(header.root_offset + header.root_length)
            .map_err(|_| Error::RootNotInFirstBytes)?;
        let root_bytes = first.get(start..end).ok_or(Error::RootNotInFirstBytes)?;
        let root = parse_directory(root_bytes, header.internal_compression)?;
        let (min_zoom, max_zoom) = (min_zoom.max(header.min_zoom), max_zoom.min(header.max_zoom));
        let wanted = if min_zoom > max_zoom {
            Vec::new()
        } else {
            cover(polygon, min_zoom, max_zoom)
                .into_iter()
                .map(|(z, x, y)| (z, x, y, tile_id(z, x, y)))
                .collect()
        };
        Ok(Self {
            header,
            root,
            leaves: HashMap::new(),
            wanted,
        })
    }

    /// A planner for the tiles `wanted` (as `(z, x, y)`), from the archive's first bytes.
    ///
    /// # Errors
    ///
    /// If the bytes aren't a PMTiles header followed by its root directory.
    pub fn for_tiles(first: &[u8], wanted: &[(u8, u32, u32)]) -> Result<Self, Error> {
        let header = Header::parse(first)?;
        let start = usize::try_from(header.root_offset).map_err(|_| Error::RootNotInFirstBytes)?;
        let end = usize::try_from(header.root_offset + header.root_length)
            .map_err(|_| Error::RootNotInFirstBytes)?;
        let root_bytes = first.get(start..end).ok_or(Error::RootNotInFirstBytes)?;
        let root = parse_directory(root_bytes, header.internal_compression)?;
        let mut planner = Self {
            header,
            root,
            leaves: HashMap::new(),
            wanted: Vec::new(),
        };
        planner.want(wanted);
        Ok(planner)
    }

    /// Looks for `wanted` instead, keeping the leaf directories already fetched: for sizing
    /// another set of tiles of the same archive cheaply.
    pub fn want(&mut self, wanted: &[(u8, u32, u32)]) {
        self.wanted = wanted
            .iter()
            .map(|&(z, x, y)| (z, x, y, tile_id(z, x, y)))
            .collect();
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// How many tiles intersect the polygon, whether or not the archive has them.
    pub fn covering_tiles(&self) -> usize {
        self.wanted.len()
    }

    fn resolve(&self, id: u64) -> Resolved {
        let mut directory = &self.root;
        for _ in 0..MAX_DEPTH {
            match find(directory, id) {
                Found::Tile { offset, length } => {
                    return Resolved::Tile(self.header.data_offset + offset, length);
                }
                Found::Absent => return Resolved::Absent,
                Found::Leaf { offset, length } => {
                    let at = self.header.leaf_offset + offset;
                    match self.leaves.get(&at) {
                        Some(leaf) => directory = leaf,
                        None => return Resolved::NeedLeaf(at, length),
                    }
                }
            }
        }
        Resolved::Absent
    }

    /// The leaf directories still needed, as `(offset from the archive's start, length)`.
    pub fn needed_leaves(&self) -> Vec<(u64, u32)> {
        let mut needed: Vec<(u64, u32)> = self
            .wanted
            .iter()
            .filter_map(|&(.., id)| match self.resolve(id) {
                Resolved::NeedLeaf(offset, length) => Some((offset, length)),
                _ => None,
            })
            .collect();
        needed.sort_unstable();
        needed.dedup();
        needed
    }

    /// Adds a leaf directory's bytes, fetched from `offset` (from the archive's start).
    ///
    /// # Errors
    ///
    /// If they don't decode.
    pub fn add_leaf(&mut self, offset: u64, bytes: &[u8]) -> Result<(), Error> {
        let entries = parse_directory(bytes, self.header.internal_compression)?;
        self.leaves.insert(offset, entries);
        Ok(())
    }

    /// The archive's tiles that intersect the polygon.
    ///
    /// # Errors
    ///
    /// [`Error::LeavesMissing`] while [`Self::needed_leaves`] isn't empty.
    pub fn tiles(&self) -> Result<Vec<PlannedTile>, Error> {
        let mut tiles = Vec::with_capacity(self.wanted.len());
        for &(z, x, y, id) in &self.wanted {
            match self.resolve(id) {
                Resolved::Tile(offset, length) => tiles.push(PlannedTile {
                    z,
                    x,
                    y,
                    offset,
                    length,
                }),
                Resolved::Absent => {}
                Resolved::NeedLeaf(..) => return Err(Error::LeavesMissing),
            }
        }
        Ok(tiles)
    }
}

/// Groups tiles into byte ranges: tiles at most `gap` bytes apart share a range, and no range
/// is longer than `max` (unless one tile is). Tiles sharing bytes (an archive stores repeated
/// tiles, such as open ocean, once) share them in the range too.
pub fn chunks(tiles: &[PlannedTile], gap: u64, max: u64) -> Vec<Chunk> {
    let mut sorted = tiles.to_vec();
    sorted.sort_unstable_by_key(|t| (t.offset, t.z, t.x, t.y));
    let mut chunks: Vec<Chunk> = Vec::new();
    for tile in sorted {
        let end = tile.offset + u64::from(tile.length);
        let joins = chunks.last().is_some_and(|chunk| {
            let chunk_end = chunk.offset + chunk.length;
            tile.offset <= chunk_end + gap && end.max(chunk_end) - chunk.offset <= max
        });
        if !joins {
            chunks.push(Chunk {
                offset: tile.offset,
                length: 0,
                tiles: Vec::new(),
            });
        }
        let Some(chunk) = chunks.last_mut() else {
            continue;
        };
        chunk.length = chunk.length.max(end - chunk.offset);
        chunk.tiles.push(ChunkTile {
            z: tile.z,
            x: tile.x,
            y: tile.y,
            // A chunk is at most `max` long, or one tile, whose length is 32 bits.
            offset: u32::try_from(tile.offset - chunk.offset).unwrap_or(u32::MAX),
            length: tile.length,
        });
    }
    chunks
}

const CHUNK_TILE_BYTES: usize = 17;

/// A chunk's tiles as bytes, for the app to keep with the chunk until it is unpacked:
/// each `z` (1 byte), `x`, `y`, offset, and length (4 bytes each, little-endian).
pub fn encode_chunk_tiles(tiles: &[ChunkTile]) -> Vec<u8> {
    let mut out = Vec::with_capacity(tiles.len() * CHUNK_TILE_BYTES);
    for t in tiles {
        out.push(t.z);
        for value in [t.x, t.y, t.offset, t.length] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut raw = [0; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    u32::from_le_bytes(raw)
}

/// The tiles [`encode_chunk_tiles`] encoded.
///
/// # Errors
///
/// If the bytes aren't a whole number of tiles.
pub fn decode_chunk_tiles(bytes: &[u8]) -> Result<Vec<ChunkTile>, Error> {
    if !bytes.len().is_multiple_of(CHUNK_TILE_BYTES) {
        return Err(Error::BadTileList);
    }
    Ok(bytes
        .as_chunks::<CHUNK_TILE_BYTES>()
        .0
        .iter()
        .map(|b| ChunkTile {
            z: b[0],
            x: u32_at(b, 1),
            y: u32_at(b, 5),
            offset: u32_at(b, 9),
            length: u32_at(b, 13),
        })
        .collect())
}

const TILE_KEY_BYTES: usize = 9;

/// Tile coordinates as bytes: each `z` (1 byte), `x` and `y` (4 bytes each, little-endian).
pub fn encode_tile_keys(tiles: impl IntoIterator<Item = (u8, u32, u32)>) -> Vec<u8> {
    let mut out = Vec::new();
    for (z, x, y) in tiles {
        out.push(z);
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
    }
    out
}

/// The coordinates [`encode_tile_keys`] encoded.
///
/// # Errors
///
/// If the bytes aren't a whole number of tiles.
pub fn decode_tile_keys(bytes: &[u8]) -> Result<Vec<(u8, u32, u32)>, Error> {
    if !bytes.len().is_multiple_of(TILE_KEY_BYTES) {
        return Err(Error::BadTileList);
    }
    Ok(bytes
        .as_chunks::<TILE_KEY_BYTES>()
        .0
        .iter()
        .map(|b| (b[0], u32_at(b, 1), u32_at(b, 5)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: u32, offset: u64, length: u32) -> PlannedTile {
        PlannedTile {
            z: 14,
            x,
            y: 0,
            offset,
            length,
        }
    }

    #[test]
    fn merges_ranges_across_small_gaps() {
        let tiles = [
            tile(0, 1000, 100),
            tile(1, 1100, 100),
            // A repeated tile: the same bytes.
            tile(2, 1000, 100),
            // 50 bytes on: joins.
            tile(3, 1250, 10),
            // Far: a new range.
            tile(4, 100_000, 10),
        ];
        let found = chunks(&tiles, 100, 1 << 20);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].offset, found[0].length), (1000, 260));
        assert_eq!(found[0].tiles.len(), 4);
        let repeated: Vec<_> = found[0].tiles.iter().filter(|t| t.offset == 0).collect();
        assert_eq!(repeated.len(), 2);
        assert_eq!((found[1].offset, found[1].length), (100_000, 10));
    }

    #[test]
    fn caps_a_ranges_length() {
        let tiles: Vec<_> = (0..10).map(|i| tile(i, u64::from(i) * 100, 100)).collect();
        let found = chunks(&tiles, 1000, 350);
        assert_eq!(
            found.iter().map(|c| c.length).collect::<Vec<_>>(),
            [300, 300, 300, 100]
        );
    }

    #[test]
    fn tile_lists_round_trip() {
        let tiles = chunks(&[tile(7, 10, 3), tile(9, 13, 5)], 0, 100)[0]
            .tiles
            .clone();
        assert_eq!(
            decode_chunk_tiles(&encode_chunk_tiles(&tiles)).unwrap(),
            tiles
        );
        let keys = vec![(14, 2624, 5721), (0, 0, 0)];
        assert_eq!(
            decode_tile_keys(&encode_tile_keys(keys.clone())).unwrap(),
            keys
        );
        assert!(decode_tile_keys(&[1, 2]).is_err());
    }
}
