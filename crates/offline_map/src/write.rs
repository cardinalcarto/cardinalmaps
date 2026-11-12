//! Writing a PMTiles v3 archive of tiles whose sizes are known before their bytes are read,
//! in one pass and without a temporary copy: the directories are built first, then the tile
//! data is streamed in tile ID order (so the archive is clustered).

use std::io::{self, Write};

use flate2::Compression as GzipLevel;
use flate2::write::GzEncoder;

use crate::Error;
use crate::pmtiles::{Compression, FIRST_FETCH, HEADER_LEN};

/// What an archive holds, for its header and metadata.
#[derive(Clone, Debug)]
pub struct ArchiveSpec {
    /// 0 unknown, 1 MVT, 2 PNG, 3 JPEG, 4 WebP, 5 AVIF, 6 MLT.
    pub tile_type: u8,
    pub tile_compression: Compression,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// `[west, south, east, north]`.
    pub bounds: [f64; 4],
    /// The metadata JSON.
    pub metadata: String,
}

/// One tile to write: its tile ID and size. Tiles are given in increasing tile ID order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileSize {
    pub tile_id: u64,
    pub length: u32,
}

/// What was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub bytes: u64,
    pub tiles: u64,
    pub leaf_directories: u64,
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// `(tile ID, offset, length, run length)`; run length 0 is a leaf pointer.
type Entry = (u64, u64, u32, u32);

fn gzip(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), GzipLevel::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

fn directory(entries: &[Entry]) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(entries.len() * 6);
    varint(&mut out, entries.len() as u64);
    let mut last = 0;
    for e in entries {
        varint(&mut out, e.0 - last);
        last = e.0;
    }
    for e in entries {
        varint(&mut out, u64::from(e.3));
    }
    for e in entries {
        varint(&mut out, u64::from(e.2));
    }
    for (i, e) in entries.iter().enumerate() {
        // 0 means "right after the previous entry", which tile data always is here.
        let contiguous = i > 0 && entries[i - 1].1 + u64::from(entries[i - 1].2) == e.1;
        varint(&mut out, if contiguous { 0 } else { e.1 + 1 });
    }
    gzip(&out)
}

/// The root directory and the leaf directories (concatenated), with the root small enough
/// to be in the archive's first [`FIRST_FETCH`] bytes.
fn directories(entries: &[Entry]) -> io::Result<(Vec<u8>, Vec<u8>, u64)> {
    let room = FIRST_FETCH as usize - HEADER_LEN;
    let root = directory(entries)?;
    if root.len() <= room {
        return Ok((root, Vec::new(), 0));
    }
    let mut leaf_size = 4096;
    loop {
        let mut leaves = Vec::new();
        let mut pointers = Vec::new();
        for part in entries.chunks(leaf_size) {
            let bytes = directory(part)?;
            pointers.push((
                part[0].0,
                leaves.len() as u64,
                u32::try_from(bytes.len()).map_err(io::Error::other)?,
                0,
            ));
            leaves.extend_from_slice(&bytes);
        }
        let root = directory(&pointers)?;
        if root.len() <= room {
            return Ok((root, leaves, pointers.len() as u64));
        }
        leaf_size *= 2;
    }
}

/// Counts what passes through, so each tile's writer can be held to its length.
struct Counting<'a, W: Write> {
    inner: &'a mut W,
    written: u64,
}

impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Writes an archive of `tiles` (in increasing tile ID order, each ID once) to `out`.
/// `fill(i, out)` writes tile `i`'s bytes, exactly `tiles[i].length` of them.
///
/// # Errors
///
/// If the tiles aren't in order, a tile's bytes aren't its length, or writing fails.
pub fn write_archive<W: Write>(
    out: &mut W,
    spec: &ArchiveSpec,
    tiles: &[TileSize],
    mut fill: impl FnMut(usize, &mut dyn Write) -> Result<(), Error>,
) -> Result<Written, Error> {
    if tiles.windows(2).any(|w| w[0].tile_id >= w[1].tile_id) {
        return Err(Error::Unordered);
    }
    let mut offset = 0u64;
    let entries: Vec<Entry> = tiles
        .iter()
        .map(|t| {
            let entry = (t.tile_id, offset, t.length, 1);
            offset += u64::from(t.length);
            entry
        })
        .collect();
    let data_length = offset;
    let (root, leaves, leaf_count) = directories(&entries)?;
    let metadata = gzip(spec.metadata.as_bytes())?;
    let root_offset = HEADER_LEN as u64;
    let metadata_offset = root_offset + root.len() as u64;
    let leaf_offset = metadata_offset + metadata.len() as u64;
    let data_offset = leaf_offset + leaves.len() as u64;

    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(b"PMTiles\x03");
    for value in [
        root_offset,
        root.len() as u64,
        metadata_offset,
        metadata.len() as u64,
        leaf_offset,
        leaves.len() as u64,
        data_offset,
        data_length,
        tiles.len() as u64,
        tiles.len() as u64,
        tiles.len() as u64,
    ] {
        header.extend_from_slice(&value.to_le_bytes());
    }
    let compression = |c: Compression| match c {
        Compression::Unknown => 0,
        Compression::None => 1,
        Compression::Gzip => 2,
        Compression::Brotli => 3,
        Compression::Zstd => 4,
    };
    header.extend_from_slice(&[
        1, // clustered
        compression(Compression::Gzip),
        compression(spec.tile_compression),
        spec.tile_type,
        spec.min_zoom,
        spec.max_zoom,
    ]);
    let e7 = |degrees: f64| ((degrees * 1e7).round() as i32).to_le_bytes();
    let [w, s, e, n] = spec.bounds;
    for degrees in [w, s, e, n] {
        header.extend_from_slice(&e7(degrees));
    }
    header.push(spec.min_zoom);
    header.extend_from_slice(&e7(f64::midpoint(w, e)));
    header.extend_from_slice(&e7(f64::midpoint(s, n)));
    debug_assert_eq!(header.len(), HEADER_LEN);

    out.write_all(&header)?;
    out.write_all(&root)?;
    out.write_all(&metadata)?;
    out.write_all(&leaves)?;
    for (i, tile) in tiles.iter().enumerate() {
        let mut counting = Counting {
            inner: out,
            written: 0,
        };
        fill(i, &mut counting)?;
        if counting.written != u64::from(tile.length) {
            return Err(Error::TileLength {
                tile_id: tile.tile_id,
                expected: tile.length,
                found: counting.written,
            });
        }
    }
    out.flush()?;
    Ok(Written {
        bytes: data_offset + data_length,
        tiles: tiles.len() as u64,
        leaf_directories: leaf_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::MapPlanner;
    use crate::pmtiles::{Header, tile_id};

    fn spec() -> ArchiveSpec {
        ArchiveSpec {
            tile_type: 0,
            tile_compression: Compression::None,
            min_zoom: 12,
            max_zoom: 12,
            bounds: [-180.0, -85.0, 180.0, 85.0],
            metadata: r#"{"name":"test"}"#.to_owned(),
        }
    }

    /// Every tile reads back through the planner, with and without leaf directories.
    #[test]
    fn archives_read_back() {
        for count in [10u32, 100_000] {
            // Scattered tiles of scattered sizes, as real ones are, so the directory doesn't
            // compress into the root.
            let mut keys: Vec<(u8, u32, u32)> = (0..count)
                .map(|i| (12, i % 4096, (i / 4096) * 97))
                .collect();
            keys.sort_by_key(|&(z, x, y)| tile_id(z, x, y));
            keys.dedup();
            let body = |i: usize| {
                // xorshift: lengths that don't repeat in a pattern gzip would find.
                let mut n = i as u64 + 0x9e37_79b9_7f4a_7c15;
                n ^= n << 13;
                n ^= n >> 7;
                n ^= n << 17;
                format!("tile {i} {}", "x".repeat((n % 997) as usize)).into_bytes()
            };
            let tiles: Vec<TileSize> = keys
                .iter()
                .enumerate()
                .map(|(i, &(z, x, y))| TileSize {
                    tile_id: tile_id(z, x, y),
                    length: body(i).len() as u32,
                })
                .collect();
            let mut archive = Vec::new();
            let written = write_archive(&mut archive, &spec(), &tiles, |i, out| {
                Ok(out.write_all(&body(i))?)
            })
            .unwrap();
            assert_eq!(written.bytes, archive.len() as u64);
            assert_eq!(written.leaf_directories > 0, count > 10);
            let header = Header::parse(&archive).unwrap();
            assert_eq!(header.tile_compression, Compression::None);
            assert_eq!((header.min_zoom, header.max_zoom), (12, 12));
            let first = &archive[..archive.len().min(FIRST_FETCH as usize)];
            let mut planner = MapPlanner::for_tiles(first, &keys).unwrap();
            for (offset, length) in planner.needed_leaves() {
                let at = offset as usize;
                planner
                    .add_leaf(offset, &archive[at..at + length as usize])
                    .unwrap();
            }
            assert!(planner.needed_leaves().is_empty());
            let found = planner.tiles().unwrap();
            assert_eq!(found.len(), keys.len());
            for t in found {
                let i = keys.iter().position(|&k| k == (t.z, t.x, t.y)).unwrap();
                let at = t.offset as usize;
                assert_eq!(&archive[at..at + t.length as usize], &body(i)[..]);
            }
        }
    }

    #[test]
    fn refuses_unordered_tiles_and_wrong_lengths() {
        let tiles = [
            TileSize {
                tile_id: 5,
                length: 1,
            },
            TileSize {
                tile_id: 3,
                length: 1,
            },
        ];
        assert!(matches!(
            write_archive(&mut Vec::new(), &spec(), &tiles, |_, _| Ok(())),
            Err(Error::Unordered)
        ));
        let tiles = [TileSize {
            tile_id: 3,
            length: 2,
        }];
        assert!(matches!(
            write_archive(&mut Vec::new(), &spec(), &tiles, |_, out| Ok(
                out.write_all(b"x")?
            )),
            Err(Error::TileLength { .. })
        ));
    }
}
