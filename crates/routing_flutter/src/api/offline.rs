//! Checking downloaded files (see `flutter/cardinal/lib/offline/`).

use std::fs::File;
use std::io::Read;

use flutter_rust_bridge::frb;
use sha2::{Digest, Sha256};

/// Why a file couldn't be hashed.
pub struct HashError {
    /// A description for logs, naming the file.
    pub message: String,
}

/// How much of a file is read at once.
const BLOCK: usize = 1 << 20;

/// The SHA-256 of a file, in lowercase hexadecimal.
///
/// Uses the CPU's SHA instructions where it has them (ARMv8's on phones, SHA-NI on x86),
/// found at run time. Not `#[frb(sync)]`: it runs on flutter_rust_bridge's worker pool,
/// reading a megabyte at a time, so a large timetable doesn't block the UI.
pub fn sha256_file(path: String) -> Result<String, HashError> {
    let error = |e: std::io::Error| HashError {
        message: format!("{path}: {e}"),
    };
    let mut file = File::open(&path).map_err(error)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BLOCK];
    loop {
        let read = file.read(&mut buffer).map_err(error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Why a downloaded transfer tile isn't the one asked for.
pub struct TileCheckError {
    /// A description for logs, naming the file.
    pub message: String,
}

/// The most a transfer tile may decompress to on a phone; real tiles are a few megabytes.
const MAX_TILE_BYTES: usize = 64 * 1024 * 1024;

/// Checks a downloaded transfer tile without a published hash: its zstd frame must decode
/// whole with nothing after it, the CRC-32 in its header must match the whole decompressed
/// tile (`valinor-walk`'s `transfer_tile` format: it covers every byte but itself), and the
/// header must name `mode`, tile `x`, `y` at zoom 12, and the build `manifest_id` (16
/// hexadecimal digits, as the manifest gives it).
pub fn check_transfer_tile(
    path: String,
    mode: String,
    x: u32,
    y: u32,
    manifest_id: String,
) -> Result<(), TileCheckError> {
    let error = |message: String| TileCheckError {
        message: format!("{path}: {message}"),
    };
    let bytes = std::fs::read(&path).map_err(|e| error(e.to_string()))?;
    check_tile_bytes(&bytes, &mode, x, y, &manifest_id).map_err(error)
}

fn check_tile_bytes(
    bytes: &[u8],
    mode: &str,
    x: u32,
    y: u32,
    manifest_id: &str,
) -> Result<(), String> {
    use valinor_walk::transfer_tile::{DecodeLimits, TransferMode, decode_tile};
    let tile = decode_tile(
        bytes,
        DecodeLimits {
            max_decompressed_bytes: MAX_TILE_BYTES,
        },
    )
    .map_err(|e| e.to_string())?;
    let raw = tile.as_bytes();
    // The header's mode byte is the mode's position in `TransferMode::ALL`.
    let mode_byte = TransferMode::ALL
        .iter()
        .position(|m| m.as_str() == mode)
        .ok_or_else(|| format!("{mode} isn't a transfer mode"))?;
    let u32_at = |at: usize| u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
    let mut id = [0u8; 8];
    id.copy_from_slice(&raw[16..24]);
    let expected_id = u64::from_str_radix(manifest_id, 16)
        .map_err(|_| format!("{manifest_id} isn't a manifest ID"))?;
    let found = (
        usize::from(raw[6]),
        u32_at(8),
        u32_at(12),
        u64::from_le_bytes(id),
    );
    if found != (mode_byte, x, y, expected_id) {
        return Err(format!(
            "it is mode {} tile {}/{} of build {:016x}, not {mode} {x}/{y} of {manifest_id}",
            found.0, found.1, found.2, found.3
        ));
    }
    Ok(())
}

/// A tile of an archive, as `(z, x, y)`.
pub struct ArchiveTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

/// A byte range of an archive.
pub struct ArchiveRange {
    /// From the start of the archive.
    pub offset: i64,
    pub length: i64,
}

/// A tile the archive has, and where.
pub struct FoundTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    /// From the start of the archive.
    pub offset: i64,
    pub length: i64,
}

/// A tile in a range to download, at an offset from the range's start.
pub struct RangeTile {
    pub z: u8,
    pub x: u32,
    pub y: u32,
    pub offset: i64,
    pub length: i64,
}

/// One range to download and the tiles in it.
pub struct ArchiveChunk {
    pub offset: i64,
    pub length: i64,
    pub tiles: Vec<RangeTile>,
}

/// Why an archive couldn't be read.
pub struct ArchiveError {
    pub message: String,
}

fn archive_error(error: offline_map::Error) -> ArchiveError {
    ArchiveError {
        message: error.to_string(),
    }
}

fn signed(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The most bytes between two tiles that one range still spans
/// (`offline_map::plan::GAP_BYTES`).
#[frb(sync)]
pub fn archive_gap_bytes() -> i64 {
    signed(offline_map::plan::GAP_BYTES)
}

/// The most bytes in a range of several tiles (`offline_map::plan::MAX_CHUNK_BYTES`); a
/// tile larger than this is a range of its own.
#[frb(sync)]
pub fn archive_max_range_bytes() -> i64 {
    signed(offline_map::plan::MAX_CHUNK_BYTES)
}

/// How many bytes of an archive to fetch first: its header and root directory.
#[frb(sync)]
pub fn archive_first_fetch() -> i64 {
    signed(offline_map::pmtiles::FIRST_FETCH)
}

/// Finds tiles in a PMTiles archive (transfer tiles' `transfers/MODE.pmtiles`), asking for
/// leaf directories as it needs them. Opaque to Dart.
#[frb(opaque)]
pub struct ArchivePlanner {
    inner: offline_map::plan::MapPlanner,
}

fn keys(tiles: &[ArchiveTile]) -> Vec<(u8, u32, u32)> {
    tiles.iter().map(|t| (t.z, t.x, t.y)).collect()
}

impl ArchivePlanner {
    /// A planner for `tiles`, from the archive's first [`archive_first_fetch`] bytes.
    pub fn new(
        first_bytes: Vec<u8>,
        tiles: Vec<ArchiveTile>,
    ) -> Result<ArchivePlanner, ArchiveError> {
        Ok(Self {
            inner: offline_map::plan::MapPlanner::for_tiles(&first_bytes, &keys(&tiles))
                .map_err(archive_error)?,
        })
    }

    /// Looks for `tiles` instead, keeping the leaf directories fetched so far: sizing another
    /// set of tiles costs only the leaves it needs that aren't fetched yet.
    #[frb(sync)]
    pub fn want(&mut self, tiles: Vec<ArchiveTile>) {
        self.inner.want(&keys(&tiles));
    }

    /// The leaf directories to fetch next; empty once every wanted tile is found.
    #[frb(sync)]
    pub fn needed_leaves(&self) -> Vec<ArchiveRange> {
        self.inner
            .needed_leaves()
            .into_iter()
            .map(|(offset, length)| ArchiveRange {
                offset: signed(offset),
                length: i64::from(length),
            })
            .collect()
    }

    /// Adds a leaf directory fetched from `offset`.
    pub fn add_leaf(&mut self, offset: i64, bytes: Vec<u8>) -> Result<(), ArchiveError> {
        self.inner
            .add_leaf(u64::try_from(offset).unwrap_or(0), &bytes)
            .map_err(archive_error)
    }

    /// The wanted tiles the archive has, with their sizes: the count and bytes of a set of
    /// tiles, without downloading them.
    #[frb(sync)]
    pub fn found(&self) -> Result<Vec<FoundTile>, ArchiveError> {
        Ok(self
            .inner
            .tiles()
            .map_err(archive_error)?
            .into_iter()
            .map(|t| FoundTile {
                z: t.z,
                x: t.x,
                y: t.y,
                offset: signed(t.offset),
                length: i64::from(t.length),
            })
            .collect())
    }

    /// The wanted tiles the archive has, in ranges at most `gap` bytes apart and at most
    /// `max` long (a tile longer than `max` is a range of its own).
    #[frb(sync)]
    pub fn ranges(&self, gap: i64, max: i64) -> Result<Vec<ArchiveChunk>, ArchiveError> {
        let tiles = self.inner.tiles().map_err(archive_error)?;
        let unsigned = |v: i64| u64::try_from(v).unwrap_or(0);
        Ok(
            offline_map::plan::chunks(&tiles, unsigned(gap), unsigned(max))
                .into_iter()
                .map(|c| ArchiveChunk {
                    offset: signed(c.offset),
                    length: signed(c.length),
                    tiles: c
                        .tiles
                        .into_iter()
                        .map(|t| RangeTile {
                            z: t.z,
                            x: t.x,
                            y: t.y,
                            offset: i64::from(t.offset),
                            length: i64::from(t.length),
                        })
                        .collect(),
                })
                .collect(),
        )
    }
}

/// A transfer tile in a downloaded range, and where its file goes.
pub struct RangeTransferTile {
    /// From the range's start.
    pub offset: i64,
    pub length: i64,
    /// Written to `<dest>.part`, then renamed to `dest`.
    pub dest: String,
    pub mode: String,
    pub x: u32,
    pub y: u32,
}

/// Checks the transfer tiles in a downloaded range of a transfer archive (the file at
/// `range_file`, which must be `expected_length` bytes) as [`check_transfer_tile`] does,
/// and writes each to its `dest`, through a temporary file and a rename. If any tile fails,
/// nothing is written, and the error names it.
pub fn unpack_transfer_range(
    range_file: String,
    expected_length: i64,
    tiles: Vec<RangeTransferTile>,
    manifest_id: String,
) -> Result<(), TileCheckError> {
    let error = |message: String| TileCheckError {
        message: format!("{range_file}: {message}"),
    };
    let bytes = std::fs::read(&range_file).map_err(|e| error(e.to_string()))?;
    if i64::try_from(bytes.len()).ok() != Some(expected_length) {
        return Err(error(format!(
            "{} bytes, not {expected_length}",
            bytes.len()
        )));
    }
    let mut slices = Vec::with_capacity(tiles.len());
    for t in &tiles {
        let start = usize::try_from(t.offset).map_err(|e| error(e.to_string()))?;
        let end = start + usize::try_from(t.length).map_err(|e| error(e.to_string()))?;
        let slice = bytes.get(start..end).ok_or_else(|| {
            error(format!(
                "{} {}/{} is past the range's end",
                t.mode, t.x, t.y
            ))
        })?;
        check_tile_bytes(slice, &t.mode, t.x, t.y, &manifest_id)
            .map_err(|e| error(format!("{} {}/{}: {e}", t.mode, t.x, t.y)))?;
        slices.push(slice);
    }
    for (t, slice) in tiles.iter().zip(slices) {
        let part = format!("{}.part", t.dest);
        if let Some(parent) = std::path::Path::new(&t.dest).parent() {
            std::fs::create_dir_all(parent).map_err(|e| error(e.to_string()))?;
        }
        std::fs::write(&part, slice).map_err(|e| error(format!("{part}: {e}")))?;
        std::fs::rename(&part, &t.dest).map_err(|e| error(format!("{}: {e}", t.dest)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_files() {
        let dir = std::env::temp_dir().join(format!("routing-flutter-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, b"").unwrap();
        let big = dir.join("big");
        // More than one block, so blocks are joined.
        std::fs::write(&big, vec![b'a'; BLOCK + 3]).unwrap();
        assert_eq!(
            sha256_file(empty.to_string_lossy().into_owned())
                .ok()
                .unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let expected: String = Sha256::digest(vec![b'a'; BLOCK + 3])
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            sha256_file(big.to_string_lossy().into_owned())
                .ok()
                .unwrap(),
            expected
        );
        let missing = sha256_file(dir.join("missing").to_string_lossy().into_owned());
        assert!(missing.err().unwrap().message.contains("missing"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A transfer tile, compressed as published.
    #[test]
    fn checks_transfer_tiles() {
        use valinor_walk::transfer_tile::{TransferMode, TransferTile, TransferTileRow};
        let walks = [valinor_walk::transfer_tile::TileWalk {
            feed: 0,
            stop: 2,
            seconds: 60,
        }];
        let tile = TransferTile::from_rows(
            TransferMode::Cycling,
            (2066, 1496),
            0x00ff_1234_5678_9abc,
            900,
            &[TransferTileRow {
                feed: 0,
                stop: 1,
                walks: &walks,
            }],
        )
        .ok()
        .unwrap();
        let dir = std::env::temp_dir().join(format!("routing-flutter-tile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("1496.bin.zst");
        let frame = zstd::bulk::compress(tile.as_bytes(), 3).unwrap();
        std::fs::write(&path, &frame).unwrap();
        let p = || path.to_string_lossy().into_owned();
        let id = "00ff123456789abc".to_owned();
        assert!(check_transfer_tile(p(), "cycling".into(), 2066, 1496, id.clone()).is_ok());
        for (mode, x, y, id) in [
            ("walking", 2066, 1496, id.clone()),
            ("cycling", 2067, 1496, id.clone()),
            ("cycling", 2066, 1496, "00ff123456789abd".to_owned()),
        ] {
            assert!(check_transfer_tile(p(), mode.into(), x, y, id).is_err());
        }
        // Any changed byte, or a short file, fails.
        for at in [0, frame.len() / 2, frame.len() - 1] {
            let mut bad = frame.clone();
            bad[at] ^= 0x10;
            std::fs::write(&path, &bad).unwrap();
            assert!(
                check_transfer_tile(p(), "cycling".into(), 2066, 1496, id.clone()).is_err(),
                "{at}"
            );
        }
        std::fs::write(&path, &frame[..frame.len() - 3]).unwrap();
        assert!(check_transfer_tile(p(), "cycling".into(), 2066, 1496, id).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn frame(x: u32, y: u32) -> Vec<u8> {
        use valinor_walk::transfer_tile::{TileWalk, TransferMode, TransferTile, TransferTileRow};
        let walks = [TileWalk {
            feed: 0,
            stop: 2,
            seconds: 60,
        }];
        let tile = TransferTile::from_rows(
            TransferMode::Walking,
            (x, y),
            0x00ff_1234_5678_9abc,
            900,
            &[TransferTileRow {
                feed: 0,
                stop: 1,
                walks: &walks,
            }],
        )
        .ok()
        .unwrap();
        zstd::bulk::compress(tile.as_bytes(), 3).unwrap()
    }

    /// Two tiles packed as `cardinal-pack` packs them, planned, and unpacked from one range.
    #[test]
    fn plans_and_unpacks_a_range_of_transfer_tiles() {
        use offline_map::pmtiles::{Compression, tile_id};
        use offline_map::write::{ArchiveSpec, TileSize, write_archive};
        let dir =
            std::env::temp_dir().join(format!("routing-flutter-range-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut tiles = vec![(656u32, 1430u32), (657, 1430)];
        tiles.sort_by_key(|&(x, y)| tile_id(12, x, y));
        let frames: Vec<Vec<u8>> = tiles.iter().map(|&(x, y)| frame(x, y)).collect();
        let sizes: Vec<TileSize> = tiles
            .iter()
            .zip(&frames)
            .map(|(&(x, y), f)| TileSize {
                tile_id: tile_id(12, x, y),
                length: f.len() as u32,
            })
            .collect();
        let spec = ArchiveSpec {
            tile_type: 0,
            tile_compression: Compression::None,
            min_zoom: 12,
            max_zoom: 12,
            bounds: [-180.0, -85.0, 180.0, 85.0],
            metadata: "{}".to_owned(),
        };
        let mut archive = Vec::new();
        write_archive(&mut archive, &spec, &sizes, |i, out| {
            Ok(out.write_all(&frames[i])?)
        })
        .ok()
        .unwrap();
        let want = |tiles: &[(u32, u32)]| {
            tiles
                .iter()
                .map(|&(x, y)| ArchiveTile { z: 12, x, y })
                .collect::<Vec<_>>()
        };
        let mut planner = ArchivePlanner::new(archive.clone(), want(&tiles))
            .ok()
            .unwrap();
        assert!(planner.needed_leaves().is_empty());
        let found = planner.found().ok().unwrap();
        assert_eq!(found.len(), 2);
        let ranges = planner
            .ranges(archive_gap_bytes(), archive_max_range_bytes())
            .ok()
            .unwrap();
        assert_eq!(ranges.len(), 1);
        // A tile the archive lacks is just not found.
        planner.want(want(&[(1, 1), (656, 1430)]));
        assert_eq!(planner.found().ok().unwrap().len(), 1);

        let range = &ranges[0];
        let start = range.offset as usize;
        let bytes = archive[start..start + range.length as usize].to_vec();
        let range_file = dir.join("range");
        let items = |bytes: &[u8]| {
            std::fs::write(&range_file, bytes).unwrap();
            range
                .tiles
                .iter()
                .map(|t| RangeTransferTile {
                    offset: t.offset,
                    length: t.length,
                    dest: dir
                        .join(format!("{}.bin.zst", t.x))
                        .to_string_lossy()
                        .into_owned(),
                    mode: "walking".to_owned(),
                    x: t.x,
                    y: t.y,
                })
                .collect::<Vec<_>>()
        };
        let path = range_file.to_string_lossy().into_owned();
        let id = "00ff123456789abc".to_owned();
        // A changed byte in the second tile: nothing is written.
        let mut bad = bytes.clone();
        let last = &range.tiles[1];
        bad[(last.offset + last.length / 2) as usize] ^= 0x40;
        let error = unpack_transfer_range(path.clone(), range.length, items(&bad), id.clone())
            .err()
            .unwrap();
        assert!(
            error.message.contains(&format!("{}/", last.x)),
            "{}",
            error.message
        );
        assert!(!dir.join("656.bin.zst").exists() && !dir.join("657.bin.zst").exists());
        assert!(
            unpack_transfer_range(path.clone(), range.length + 1, items(&bytes), id.clone())
                .is_err()
        );
        unpack_transfer_range(path, range.length, items(&bytes), id)
            .ok()
            .unwrap();
        for (i, &(x, _)) in tiles.iter().enumerate() {
            assert_eq!(
                std::fs::read(dir.join(format!("{x}.bin.zst"))).unwrap(),
                frames[i]
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
