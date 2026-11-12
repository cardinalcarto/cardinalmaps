//! Reading a PMTiles v3 archive's header and directories
//! (<https://github.com/protomaps/PMTiles/blob/main/spec/v3/spec.md>).
//!
//! Only what planning a download needs: the header, the directories, and finding a tile's
//! byte range. The bytes come from the caller (HTTP range requests made by the app), so
//! nothing here does I/O.

use std::io::Read;

use crate::Error;

/// The header's length; the root directory follows it, within the archive's first 16 KiB.
pub const HEADER_LEN: usize = 127;

/// The bytes a client fetches first: the header and the root directory are in them.
pub const FIRST_FETCH: u64 = 16_384;

/// A compression, as the header names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Unknown,
    None,
    Gzip,
    Brotli,
    Zstd,
}

impl Compression {
    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::None,
            2 => Self::Gzip,
            3 => Self::Brotli,
            4 => Self::Zstd,
            _ => Self::Unknown,
        }
    }
}

/// The archive's header.
#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub root_offset: u64,
    pub root_length: u64,
    pub metadata_offset: u64,
    pub metadata_length: u64,
    pub leaf_offset: u64,
    pub leaf_length: u64,
    pub data_offset: u64,
    pub data_length: u64,
    pub addressed_tiles: u64,
    pub clustered: bool,
    pub internal_compression: Compression,
    pub tile_compression: Compression,
    /// 1 MVT, 2 PNG, 3 JPEG, 4 WebP, 5 AVIF, 6 MLT.
    pub tile_type: u8,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// `[west, south, east, north]`.
    pub bounds: [f64; 4],
    /// `(longitude, latitude, zoom)`.
    pub center: (f64, f64, u8),
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

fn degrees_at(bytes: &[u8], at: usize) -> f64 {
    let mut raw = [0; 4];
    raw.copy_from_slice(&bytes[at..at + 4]);
    f64::from(i32::from_le_bytes(raw)) / 1e7
}

impl Header {
    /// Reads the header from the archive's first bytes.
    ///
    /// # Errors
    ///
    /// If they are too short, or aren't a PMTiles v3 header.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_LEN || &bytes[..7] != b"PMTiles" || bytes[7] != 3 {
            return Err(Error::NotPmtiles);
        }
        Ok(Self {
            root_offset: u64_at(bytes, 8),
            root_length: u64_at(bytes, 16),
            metadata_offset: u64_at(bytes, 24),
            metadata_length: u64_at(bytes, 32),
            leaf_offset: u64_at(bytes, 40),
            leaf_length: u64_at(bytes, 48),
            data_offset: u64_at(bytes, 56),
            data_length: u64_at(bytes, 64),
            addressed_tiles: u64_at(bytes, 72),
            clustered: bytes[96] == 1,
            internal_compression: Compression::from_byte(bytes[97]),
            tile_compression: Compression::from_byte(bytes[98]),
            tile_type: bytes[99],
            min_zoom: bytes[100],
            max_zoom: bytes[101],
            bounds: [
                degrees_at(bytes, 102),
                degrees_at(bytes, 106),
                degrees_at(bytes, 110),
                degrees_at(bytes, 114),
            ],
            center: (degrees_at(bytes, 119), degrees_at(bytes, 123), bytes[118]),
        })
    }
}

/// One directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub tile_id: u64,
    /// From the start of the tile data, or for a leaf, of the leaf directories.
    pub offset: u64,
    pub length: u32,
    /// How many consecutive tile IDs share these bytes; 0 for a leaf directory.
    pub run_length: u32,
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).ok_or(Error::BadDirectory)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::BadDirectory)
}

/// Decodes a directory's bytes, as stored (compressed with the header's internal compression).
///
/// # Errors
///
/// If it can't be decompressed or decoded.
pub fn parse_directory(stored: &[u8], compression: Compression) -> Result<Vec<Entry>, Error> {
    let raw;
    let bytes = match compression {
        Compression::None => stored,
        Compression::Gzip => {
            let mut out = Vec::with_capacity(stored.len() * 3);
            flate2::read::GzDecoder::new(stored)
                .read_to_end(&mut out)
                .map_err(|_| Error::BadDirectory)?;
            raw = out;
            &raw
        }
        other => return Err(Error::UnsupportedCompression(other)),
    };
    let mut at = 0;
    let count = usize::try_from(varint(bytes, &mut at)?).map_err(|_| Error::BadDirectory)?;
    // Each entry takes at least four bytes.
    if count > bytes.len() {
        return Err(Error::BadDirectory);
    }
    let mut entries = vec![
        Entry {
            tile_id: 0,
            offset: 0,
            length: 0,
            run_length: 0,
        };
        count
    ];
    let mut last = 0u64;
    for entry in &mut entries {
        last = last
            .checked_add(varint(bytes, &mut at)?)
            .ok_or(Error::BadDirectory)?;
        entry.tile_id = last;
    }
    for entry in &mut entries {
        entry.run_length =
            u32::try_from(varint(bytes, &mut at)?).map_err(|_| Error::BadDirectory)?;
    }
    for entry in &mut entries {
        entry.length = u32::try_from(varint(bytes, &mut at)?).map_err(|_| Error::BadDirectory)?;
    }
    for i in 0..count {
        let value = varint(bytes, &mut at)?;
        entries[i].offset = if value == 0 && i > 0 {
            entries[i - 1].offset + u64::from(entries[i - 1].length)
        } else {
            value.checked_sub(1).ok_or(Error::BadDirectory)?
        };
    }
    Ok(entries)
}

/// What a directory says about a tile ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The tile's bytes, from the start of the tile data.
    Tile { offset: u64, length: u32 },
    /// A leaf directory may have it, at this range from the start of the leaf directories.
    Leaf { offset: u64, length: u32 },
    /// The archive doesn't have it.
    Absent,
}

/// Looks a tile ID up in one directory.
pub fn find(entries: &[Entry], tile_id: u64) -> Found {
    let index = entries.partition_point(|entry| entry.tile_id <= tile_id);
    let Some(entry) = index.checked_sub(1).map(|i| entries[i]) else {
        return Found::Absent;
    };
    if entry.run_length == 0 {
        return Found::Leaf {
            offset: entry.offset,
            length: entry.length,
        };
    }
    if tile_id - entry.tile_id < u64::from(entry.run_length) {
        Found::Tile {
            offset: entry.offset,
            length: entry.length,
        }
    } else {
        Found::Absent
    }
}

/// The PMTiles tile ID of `z/x/y`: tiles of lower zooms first, then along a Hilbert curve.
pub fn tile_id(z: u8, x: u32, y: u32) -> u64 {
    let base = ((1u64 << (2 * u32::from(z))) - 1) / 3;
    let (mut x, mut y) = (u64::from(x), u64::from(y));
    let mut d = 0u64;
    let mut s = (1u64 << z) >> 1;
    while s > 0 {
        let rx = u64::from(x & s > 0);
        let ry = u64::from(y & s > 0);
        d += s * s * ((3 * rx) ^ ry);
        if ry == 0 {
            if rx == 1 {
                x = s - 1 - (x & (s - 1));
                y = s - 1 - (y & (s - 1));
            }
            std::mem::swap(&mut x, &mut y);
        }
        s >>= 1;
    }
    base + d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_ids_follow_the_spec() {
        // From the spec's examples and go-pmtiles' tests.
        assert_eq!(tile_id(0, 0, 0), 0);
        assert_eq!(tile_id(1, 0, 0), 1);
        assert_eq!(tile_id(1, 0, 1), 2);
        assert_eq!(tile_id(1, 1, 1), 3);
        assert_eq!(tile_id(1, 1, 0), 4);
        assert_eq!(tile_id(2, 0, 0), 5);
    }

    #[test]
    fn decodes_a_directory() {
        // Three entries: tile 0 (run 1), tiles 1-2 sharing bytes, and a leaf at tile 5.
        let bytes = [3, 0, 1, 4, 1, 2, 0, 10, 20, 30, 1, 0, 5];
        let entries = parse_directory(&bytes, Compression::None).unwrap();
        assert_eq!(
            entries,
            vec![
                Entry {
                    tile_id: 0,
                    offset: 0,
                    length: 10,
                    run_length: 1
                },
                Entry {
                    tile_id: 1,
                    offset: 10,
                    length: 20,
                    run_length: 2
                },
                Entry {
                    tile_id: 5,
                    offset: 4,
                    length: 30,
                    run_length: 0
                },
            ]
        );
        assert_eq!(
            find(&entries, 2),
            Found::Tile {
                offset: 10,
                length: 20
            }
        );
        assert_eq!(find(&entries, 3), Found::Absent);
        assert_eq!(
            find(&entries, 9),
            Found::Leaf {
                offset: 4,
                length: 30
            }
        );
    }
}
