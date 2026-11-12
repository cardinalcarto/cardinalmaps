//! Planning and unpacking regions over small archives built here, and over the Seattle test
//! release when `CARDINAL_TEST_PMTILES` names it.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;

use offline_map::cover::{Polygon, cover};
use offline_map::mbtiles::Basemap;
use offline_map::plan::{GAP_BYTES, MAX_CHUNK_BYTES, MapPlanner, PlannedTile, chunks};
use offline_map::pmtiles::{Compression, FIRST_FETCH, Header, tile_id};
use sparkle::TileId;
use sparkle_view::mbtiles::{MbTiles, TileFormat};

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// `(tile ID, offset, length, run length)`.
type RawEntry = (u64, u64, u32, u32);

fn directory(entries: &[RawEntry]) -> Vec<u8> {
    let mut out = Vec::new();
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
    for e in entries {
        varint(&mut out, e.1 + 1);
    }
    gzip(&out)
}

/// A PMTiles archive of MLT-typed, gzipped tiles (the contents are just bytes), with leaf
/// directories of `leaf_size` entries if given. Repeated contents are stored once.
fn archive(tiles: &[(u8, u32, u32, Vec<u8>)], leaf_size: Option<usize>) -> Vec<u8> {
    let mut by_id: BTreeMap<u64, &Vec<u8>> = BTreeMap::new();
    for (z, x, y, data) in tiles {
        by_id.insert(tile_id(*z, *x, *y), data);
    }
    let mut data = Vec::new();
    let mut stored: BTreeMap<&Vec<u8>, (u64, u32)> = BTreeMap::new();
    let mut entries: Vec<RawEntry> = Vec::new();
    for (id, raw) in by_id {
        let (offset, length) = *stored.entry(raw).or_insert_with(|| {
            let bytes = gzip(raw);
            let at = data.len() as u64;
            data.extend_from_slice(&bytes);
            (at, bytes.len() as u32)
        });
        entries.push((id, offset, length, 1));
    }
    let (root, leaves) = match leaf_size {
        None => (directory(&entries), Vec::new()),
        Some(size) => {
            let mut leaves = Vec::new();
            let mut pointers = Vec::new();
            for part in entries.chunks(size) {
                let bytes = directory(part);
                pointers.push((part[0].0, leaves.len() as u64, bytes.len() as u32, 0));
                leaves.extend_from_slice(&bytes);
            }
            (directory(&pointers), leaves)
        }
    };
    let root_offset = 127u64;
    let leaf_offset = root_offset + root.len() as u64;
    let data_offset = leaf_offset + leaves.len() as u64;
    let mut out = b"PMTiles\x03".to_vec();
    for value in [
        root_offset,
        root.len() as u64,
        leaf_offset, // metadata: empty
        0,
        leaf_offset,
        leaves.len() as u64,
        data_offset,
        data.len() as u64,
        tiles.len() as u64,
        entries.len() as u64,
        stored.len() as u64,
    ] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[1, 2, 2, 6, 0, 14]); // clustered, gzip, gzip, MLT, zooms
    for degrees in [-123.0f64, 47.0, -122.0, 48.0] {
        out.extend_from_slice(&((degrees * 1e7) as i32).to_le_bytes());
    }
    out.push(10);
    for degrees in [-122.3f64, 47.6] {
        out.extend_from_slice(&((degrees * 1e7) as i32).to_le_bytes());
    }
    assert_eq!(out.len(), 127);
    out.extend_from_slice(&root);
    out.extend_from_slice(&leaves);
    out.extend_from_slice(&data);
    out
}

fn downtown() -> Polygon {
    Polygon::new(vec![
        (-122.36, 47.595),
        (-122.31, 47.595),
        (-122.30, 47.62),
        (-122.34, 47.63),
    ])
    .unwrap()
}

fn content(z: u8, x: u32, y: u32) -> Vec<u8> {
    format!(
        "tile {z}/{x}/{y} {}",
        "x".repeat((x as usize * 7 + y as usize) % 300)
    )
    .into_bytes()
}

/// Every tile over a box around downtown, and some ocean tiles that repeat one content.
fn seattle_tiles() -> Vec<(u8, u32, u32, Vec<u8>)> {
    let around = Polygon::new(vec![
        (-122.5, 47.5),
        (-122.2, 47.5),
        (-122.2, 47.7),
        (-122.5, 47.7),
    ])
    .unwrap();
    cover(&around, 0, 14)
        .into_iter()
        .map(|(z, x, y)| {
            let data = if z == 14 && x % 5 == 0 {
                b"ocean".to_vec()
            } else {
                content(z, x, y)
            };
            (z, x, y, data)
        })
        .collect()
}

fn plan(archive: &[u8], polygon: &Polygon) -> (Vec<PlannedTile>, usize) {
    let first = &archive[..(FIRST_FETCH as usize).min(archive.len())];
    let mut planner = MapPlanner::new(first, polygon, 0, 14).unwrap();
    let mut rounds = 0;
    loop {
        let needed = planner.needed_leaves();
        if needed.is_empty() {
            break;
        }
        rounds += 1;
        for (offset, length) in needed {
            let at = offset as usize;
            planner
                .add_leaf(offset, &archive[at..at + length as usize])
                .unwrap();
        }
    }
    (planner.tiles().unwrap(), rounds)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("offline-map-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Downloads (slices) and unpacks every chunk of a plan into `basemap` for `region`.
fn install(basemap: &mut Basemap, archive: &[u8], tiles: &[PlannedTile], region: i64) {
    for chunk in chunks(tiles, GAP_BYTES, MAX_CHUNK_BYTES) {
        let at = chunk.offset as usize;
        let bytes = &archive[at..at + chunk.length as usize];
        basemap
            .unpack(bytes, chunk.length, &chunk.tiles, region, Compression::Gzip)
            .unwrap();
    }
}

#[test]
fn plans_through_leaf_directories() {
    let tiles = seattle_tiles();
    let polygon = downtown();
    let expected: HashSet<(u8, u32, u32)> = cover(&polygon, 0, 14).into_iter().collect();
    let flat = archive(&tiles, None);
    let leafy = archive(&tiles, Some(16));
    let (from_flat, rounds) = plan(&flat, &polygon);
    assert_eq!(rounds, 0);
    let (from_leaves, rounds) = plan(&leafy, &polygon);
    assert_eq!(rounds, 1);
    let keys = |tiles: &[PlannedTile]| -> HashSet<(u8, u32, u32)> {
        tiles.iter().map(|t| (t.z, t.x, t.y)).collect()
    };
    assert_eq!(keys(&from_flat), expected);
    assert_eq!(keys(&from_leaves), expected);
    // Each planned range holds that tile's gzipped bytes.
    for t in &from_leaves {
        let at = t.offset as usize;
        let mut raw = Vec::new();
        std::io::Read::read_to_end(
            &mut flate2::read::GzDecoder::new(&leafy[at..at + t.length as usize]),
            &mut raw,
        )
        .unwrap();
        let wanted = tiles
            .iter()
            .find(|(z, x, y, _)| (*z, *x, *y) == (t.z, t.x, t.y))
            .unwrap();
        assert_eq!(raw, wanted.3);
    }
}

#[test]
fn unpacked_tiles_read_back_through_sparkles_reader() {
    let tiles = seattle_tiles();
    let bytes = archive(&tiles, Some(64));
    let (planned, _) = plan(&bytes, &downtown());
    let dir = scratch("read");
    let path = dir.join("basemap.mbtiles");
    let mut basemap = Basemap::open(&path).unwrap();
    basemap
        .set_metadata(&Header::parse(&bytes).unwrap(), "test")
        .unwrap();
    install(&mut basemap, &bytes, &planned, 1);
    // Read while the writer is still open, as the map does during a download.
    let reader = MbTiles::open(&path).unwrap();
    let metadata = reader.metadata().unwrap();
    assert_eq!(metadata.format, TileFormat::Mlt);
    assert_eq!((metadata.min_zoom, metadata.max_zoom), (0, 14));
    for t in &planned {
        let got = reader.tile(TileId::new(t.z, t.x, t.y)).unwrap().unwrap();
        let want = tiles
            .iter()
            .find(|(z, x, y, _)| (*z, *x, *y) == (t.z, t.x, t.y))
            .unwrap();
        assert_eq!(got, want.3, "{}/{}/{}", t.z, t.x, t.y);
    }
    assert!(reader.tile(TileId::new(14, 0, 0)).unwrap().is_none());
    assert_eq!(basemap.region_tiles(1).unwrap(), planned.len() as u64);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_tile_or_a_short_range_fails_the_chunk() {
    let tiles = seattle_tiles();
    let bytes = archive(&tiles, None);
    let (planned, _) = plan(&bytes, &downtown());
    let chunk = chunks(&planned, GAP_BYTES, MAX_CHUNK_BYTES).remove(0);
    let at = chunk.offset as usize;
    let good = bytes[at..at + chunk.length as usize].to_vec();
    let dir = scratch("bad");
    let mut basemap = Basemap::open(&dir.join("basemap.mbtiles")).unwrap();

    let short = &good[..good.len() - 1];
    let error = basemap
        .unpack(short, chunk.length, &chunk.tiles, 1, Compression::Gzip)
        .unwrap_err();
    assert!(
        matches!(error, offline_map::Error::ChunkLength { .. }),
        "{error}"
    );

    // Flip a byte of the last tile's compressed data: its CRC-32 no longer matches.
    let last = chunk.tiles.iter().max_by_key(|t| t.offset).unwrap();
    let mut corrupt = good.clone();
    corrupt[(last.offset + last.length / 2) as usize] ^= 0x55;
    let error = basemap
        .unpack(&corrupt, chunk.length, &chunk.tiles, 1, Compression::Gzip)
        .unwrap_err();
    assert!(
        matches!(error, offline_map::Error::BadTile { .. }),
        "{error}"
    );
    // A truncated trailer (the ISIZE) fails too.
    let mut truncated = good.clone();
    truncated[(last.offset + last.length - 1) as usize] ^= 0x01;
    assert!(
        basemap
            .unpack(&truncated, chunk.length, &chunk.tiles, 1, Compression::Gzip)
            .is_err()
    );
    // Nothing was written.
    assert_eq!(basemap.tile_count().unwrap(), 0);
    basemap
        .unpack(&good, chunk.length, &chunk.tiles, 1, Compression::Gzip)
        .unwrap();
    assert_eq!(basemap.tile_count().unwrap(), chunk.tiles.len() as u64);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn overlapping_regions_share_tiles_until_both_are_deleted() {
    let tiles = seattle_tiles();
    let bytes = archive(&tiles, None);
    let west = Polygon::new(vec![
        (-122.40, 47.60),
        (-122.33, 47.60),
        (-122.33, 47.62),
        (-122.40, 47.62),
    ])
    .unwrap();
    let east = Polygon::new(vec![
        (-122.34, 47.60),
        (-122.28, 47.60),
        (-122.28, 47.62),
        (-122.34, 47.62),
    ])
    .unwrap();
    let dir = scratch("overlap");
    let path = dir.join("basemap.mbtiles");
    let mut basemap = Basemap::open(&path).unwrap();
    let (west_tiles, _) = plan(&bytes, &west);
    install(&mut basemap, &bytes, &west_tiles, 1);

    // The second region downloads only what isn't there, and claims the rest.
    let (east_tiles, _) = plan(&bytes, &east);
    let present = basemap.present(&east_tiles).unwrap();
    assert!(!present.is_empty() && present.len() < east_tiles.len());
    let keys: Vec<_> = east_tiles.iter().map(|t| (t.z, t.x, t.y)).collect();
    let missing = basemap.claim(2, &keys).unwrap();
    assert_eq!(missing.len(), east_tiles.len() - present.len());
    let to_fetch: Vec<_> = east_tiles
        .iter()
        .filter(|t| !present.contains(&(t.z, t.x, t.y)))
        .copied()
        .collect();
    install(&mut basemap, &bytes, &to_fetch, 2);
    assert_eq!(basemap.region_tiles(2).unwrap(), east_tiles.len() as u64);

    let union: HashSet<_> = west_tiles
        .iter()
        .chain(&east_tiles)
        .map(|t| (t.z, t.x, t.y))
        .collect();
    assert_eq!(basemap.tile_count().unwrap(), union.len() as u64);
    // Deleting the west region keeps the shared tiles.
    let removed = basemap.release(1).unwrap();
    assert_eq!(removed, (union.len() - east_tiles.len()) as u64);
    let reader = MbTiles::open(&path).unwrap();
    for t in &east_tiles {
        assert!(reader.tile(TileId::new(t.z, t.x, t.y)).unwrap().is_some());
    }
    assert_eq!(basemap.release(2).unwrap(), east_tiles.len() as u64);
    assert_eq!(basemap.tile_count().unwrap(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

/// The Seattle test release's archive (`map-convert --mlt-header` of `seattle.mbtiles`), and
/// that MBTiles: every tile is where the planner says, and a city polygon's ranges.
#[test]
#[ignore = "needs CARDINAL_TEST_PMTILES and CARDINAL_TEST_MBTILES"]
fn plans_the_seattle_archive() {
    let archive = std::fs::read(std::env::var("CARDINAL_TEST_PMTILES").unwrap()).unwrap();
    let source = PathBuf::from(std::env::var("CARDINAL_TEST_MBTILES").unwrap());
    let header = Header::parse(&archive).unwrap();
    println!("{header:?}");
    let source_reader = MbTiles::open(&source).unwrap();

    for (name, polygon) in [
        ("downtown", downtown()),
        (
            "city",
            Polygon::new(vec![
                (-122.44, 47.50),
                (-122.24, 47.50),
                (-122.24, 47.74),
                (-122.44, 47.74),
            ])
            .unwrap(),
        ),
    ] {
        let (tiles, rounds) = plan(&archive, &polygon);
        let exact: u64 = {
            let unique: HashSet<_> = tiles.iter().map(|t| (t.offset, t.length)).collect();
            unique.iter().map(|&(_, l)| u64::from(l)).sum()
        };
        for gap in [0, 64 * 1024, GAP_BYTES, 1024 * 1024] {
            let found = chunks(&tiles, gap, MAX_CHUNK_BYTES);
            let bytes: u64 = found.iter().map(|c| c.length).sum();
            println!(
                "{name}: {} tiles ({} covering), {rounds} leaf rounds, gap {gap}: {} ranges, \
                 {bytes} bytes, exact {exact} ({:.1}% over)",
                tiles.len(),
                cover(&polygon, 0, 14).len(),
                found.len(),
                (bytes as f64 / exact as f64 - 1.0) * 100.0
            );
        }
        let dir = scratch(&format!("seattle-{name}"));
        let path = dir.join("basemap.mbtiles");
        let mut basemap = Basemap::open(&path).unwrap();
        basemap.set_metadata(&header, "seattle").unwrap();
        install(&mut basemap, &archive, &tiles, 1);
        let reader = MbTiles::open(&path).unwrap();
        for t in &tiles {
            let id = TileId::new(t.z, t.x, t.y);
            assert_eq!(
                reader.tile(id).unwrap(),
                source_reader.tile(id).unwrap(),
                "{id:?}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// A basemap the app downloaded (the desktop end-to-end test leaves one): every tile
/// sparkle's reader finds in it is the source tileset's.
#[test]
#[ignore = "needs CARDINAL_TEST_BASEMAP and CARDINAL_TEST_MBTILES"]
fn reads_a_downloaded_basemap() {
    let path = PathBuf::from(std::env::var("CARDINAL_TEST_BASEMAP").unwrap());
    let source = MbTiles::open(&PathBuf::from(
        std::env::var("CARDINAL_TEST_MBTILES").unwrap(),
    ))
    .unwrap();
    let reader = MbTiles::open(&path).unwrap();
    let metadata = reader.metadata().unwrap();
    assert_eq!(metadata.format, TileFormat::Mlt);
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let ids: Vec<(u8, u32, u32)> = conn
        .prepare("SELECT zoom_level, tile_column, tile_row FROM tiles")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!ids.is_empty());
    for (z, x, row) in &ids {
        let id = TileId::new(*z, *x, (1u32 << z) - 1 - row);
        let tile = reader.tile(id).unwrap().unwrap();
        assert_eq!(Some(tile), source.tile(id).unwrap(), "{id:?}");
    }
    let regions: i64 = conn
        .query_row("SELECT count(DISTINCT region) FROM tile_regions", [], |r| {
            r.get(0)
        })
        .unwrap();
    println!("{} tiles read, of {regions} regions", ids.len());
}

/// The planet's transfer archives (`cardinal-pack transfers` output), from
/// `CARDINAL_TEST_TRANSFER_ARCHIVES`: the ranges a city's transfer tiles take.
#[test]
#[ignore = "needs CARDINAL_TEST_TRANSFER_ARCHIVES"]
fn plans_the_planet_transfer_archives() {
    let dir = PathBuf::from(std::env::var("CARDINAL_TEST_TRANSFER_ARCHIVES").unwrap());
    for (name, polygon) in [
        ("downtown", downtown()),
        (
            "city",
            Polygon::new(vec![
                (-122.44, 47.50),
                (-122.24, 47.50),
                (-122.24, 47.74),
                (-122.44, 47.74),
            ])
            .unwrap(),
        ),
    ] {
        let wanted = cover(&polygon, 12, 12);
        for mode in ["walking", "cycling", "wheelchair"] {
            let archive = std::fs::read(dir.join(format!("{mode}.pmtiles"))).unwrap();
            let first = &archive[..FIRST_FETCH as usize];
            let mut planner = MapPlanner::for_tiles(first, &wanted).unwrap();
            let mut leaves = 0;
            loop {
                let needed = planner.needed_leaves();
                if needed.is_empty() {
                    break;
                }
                for (offset, length) in needed {
                    leaves += 1;
                    let at = offset as usize;
                    planner
                        .add_leaf(offset, &archive[at..at + length as usize])
                        .unwrap();
                }
            }
            let tiles = planner.tiles().unwrap();
            let exact: u64 = tiles.iter().map(|t| u64::from(t.length)).sum();
            let largest = tiles.iter().map(|t| t.length).max().unwrap_or(0);
            for gap in [0, 64 * 1024, GAP_BYTES, 1024 * 1024] {
                let found = chunks(&tiles, gap, MAX_CHUNK_BYTES);
                let bytes: u64 = found.iter().map(|c| c.length).sum();
                println!(
                    "{name} {mode}: {} tiles (largest {largest} B), {leaves} leaves, gap {gap}: \
                     {} ranges, {bytes} B, exact {exact} ({:.1}% over)",
                    tiles.len(),
                    found.len(),
                    (bytes as f64 / exact.max(1) as f64 - 1.0) * 100.0
                );
            }
        }
    }
}
