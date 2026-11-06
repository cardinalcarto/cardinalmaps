//! The pipeline's unit of work: a quarter degree square of contours, cached on disk.
//!
//! Chunks are computed in parallel and independently; [`crate::assemble`] then gives their
//! lines IDs and writes them out in order. A chunk's file name hashes everything its
//! contents depend on, so adding a higher resolution DEM recomputes only the chunks it
//! touches.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use rayon::prelude::*;

use crate::config::{BASE_PER_DEGREE, Config};
use crate::contour::{self, Keep, Line, Params, Unit};
use crate::dem::{Grid, RasterCache, Rect, Source};

/// Base lattice cells along a chunk's side: 900 arc seconds, a quarter degree.
pub const CHUNK_CELLS: i64 = 900;
/// The most nodes in a way, as in the OSM API.
pub const MAX_WAY_NODES: usize = 2000;
/// Bumped when the output of the same input changes.
const ALGORITHM: u64 = 1;

/// A chunk's position, in chunks east of -180 and north of -90.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkId {
    pub y: i32,
    pub x: i32,
}

impl ChunkId {
    pub fn containing(lon: f64, lat: f64) -> ChunkId {
        let per_degree = (BASE_PER_DEGREE / CHUNK_CELLS) as f64;
        ChunkId {
            y: ((lat + 90.0) * per_degree).floor() as i32,
            x: ((lon + 180.0) * per_degree).floor() as i32,
        }
    }

    /// (west, south, east, north), in degrees, of the rectangle its cells span.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let side = CHUNK_CELLS as f64 / BASE_PER_DEGREE as f64;
        let half = 0.5 / BASE_PER_DEGREE as f64;
        let (w, s) = (f64::from(self.x) * side - 180.0 + half, f64::from(self.y) * side - 90.0 + half);
        (w, s, w + side, s + side)
    }

    /// The range of base lattice points it spans, both ends included.
    fn base(&self) -> ((i64, i64), (i64, i64)) {
        let (x, y) = (i64::from(self.x) * CHUNK_CELLS, i64::from(self.y) * CHUNK_CELLS);
        ((x, x + CHUNK_CELLS), (y, y + CHUNK_CELLS))
    }

    fn name(&self) -> String {
        format!("{}_{}", self.y, self.x)
    }
}

/// The index on a lattice refined by `r` of base lattice point `base`.
#[inline]
fn refined(base: i64, r: i64) -> i64 {
    r * base + (r - 1) / 2
}

pub struct Mosaic {
    pub sources: Vec<Source>,
    pub cache: RasterCache,
    params: Params,
    config_hash: u64,
}

fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

impl Mosaic {
    pub fn open(config: &Config, work: &Path) -> Result<Mosaic> {
        let sources = config.sources.iter().map(|s| Source::open(s, work)).collect::<Result<Vec<_>>>()?;
        let c = &config.contours;
        let mut hash = 0xcbf29ce484222325;
        fnv(&mut hash, &ALGORITHM.to_le_bytes());
        fnv(
            &mut hash,
            format!(
                "{} {} {} {} {}",
                c.interval_m, c.interval_ft, c.simplify_m, c.round_corners, c.min_loop_m
            ).as_bytes(),
        );
        Ok(Mosaic {
            sources,
            cache: RasterCache::new((c.cache_gb * 1e9) as u64),
            params: Params {
                interval_m: c.interval_m,
                interval_ft: c.interval_ft,
                simplify_m: c.simplify_m,
                min_loop_m: c.min_loop_m,
                round_corners: c.round_corners,
            },
            config_hash: hash,
        })
    }

    /// Every chunk with data, within an optional (west, south, east, north), south to north.
    pub fn chunks(&self, bbox: Option<(f64, f64, f64, f64)>) -> Vec<ChunkId> {
        let mut chunks = Vec::new();
        for source in &self.sources {
            for raster in &source.rasters {
                let (mut w, mut s, mut e, mut n) =
                    (raster.west, raster.south(), raster.east(), raster.north);
                if let Some(b) = bbox {
                    (w, s, e, n) = (w.max(b.0), s.max(b.1), e.min(b.2), n.min(b.3));
                }
                if w >= e || s >= n {
                    continue;
                }
                let (a, b) = (ChunkId::containing(w, s), ChunkId::containing(e - 1e-9, n - 1e-9));
                for y in a.y..=b.y {
                    for x in a.x..=b.x {
                        chunks.push(ChunkId { y, x });
                    }
                }
            }
        }
        // By block, then within it south to north, since `ensure` works through them in order.
        chunks.sort_unstable_by_key(|id| (block_of(*id), *id));
        chunks.dedup();
        chunks
    }

    /// Contours for a chunk, from the DEMs.
    pub fn compute(&self, id: ChunkId) -> Result<Vec<Line>> {
        let (w, s, e, n) = id.bounds();
        let inside: Vec<&Source> =
            self.sources.iter().filter(|src| !src.overlapping(w, s, e, n).is_empty()).collect();
        let Some(r) = inside.iter().map(|src| src.refine).max() else {
            return Ok(Vec::new());
        };
        let ((bx0, bx1), (by0, by1)) = id.base();
        let (x0, x1, y0, y1) = (refined(bx0, r), refined(bx1, r), refined(by0, r), refined(by1, r));

        // Enough window that a loop small enough to drop fits entirely in it, whichever
        // chunk it touches. Longitude cells are the short ones, more so near the poles.
        let cell_m = 111_319.49 / (BASE_PER_DEGREE * r) as f64;
        let cos_lat = n.abs().max(s.abs()).to_radians().cos().max(0.05);
        let buffer = (self.params.min_loop_m / 2.0 / (cell_m * cos_lat)).ceil() as i64 + 2;

        let rect = Rect { x0: x0 - buffer, y0: y0 - buffer, x1: x1 + buffer + 1, y1: y1 + buffer + 1 };
        let mut grid = Grid::new(rect, r);
        for source in &self.sources {
            blend(&mut grid, source, &self.cache)?;
        }
        if grid.values.iter().all(|v| v.is_nan()) {
            return Ok(Vec::new());
        }
        let b = buffer as usize;
        let keep = Keep {
            cols: (b, b + (x1 - x0) as usize),
            rows: (b, b + (y1 - y0) as usize),
        };
        Ok(contour::contours(&grid, keep, &self.params))
    }

    fn path(&self, work: &Path, id: ChunkId) -> PathBuf {
        let (w, s, e, n) = id.bounds();
        let margin = 0.01; // Over any source margin worth caring about for invalidation.
        let mut hash = self.config_hash;
        for source in &self.sources {
            let config = &source.config;
            fnv(
                &mut hash,
                format!("{} {} {} {} {}", config.name, config.arcsec, config.median, config.sigma, config.feather)
                    .as_bytes(),
            );
            for raster in source.overlapping(w - margin, s - margin, e + margin, n + margin) {
                let name = raster.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                fnv(&mut hash, name.as_bytes());
                fnv(&mut hash, &raster.bytes.to_le_bytes());
            }
        }
        work.join("chunks").join(format!("{}-{hash:016x}.bin", id.name()))
    }

    /// Computes the chunks not on disk yet. Returns each chunk's file.
    pub fn ensure(&self, work: &Path, ids: &[ChunkId]) -> Result<Vec<(ChunkId, PathBuf)>> {
        fs::create_dir_all(work.join("chunks"))?;
        let done = AtomicU64::new(0);
        let total = ids.len() as u64;
        // A block at a time, keeping threads near each other, so that they share rasters in
        // the cache. Scattered across a planet, they'd have to keep rereading them, which for
        // a remote source means downloading them again.
        let mut files = Vec::with_capacity(ids.len());
        for block in ids.chunk_by(|a, b| block_of(*a) == block_of(*b)) {
            let finished: Vec<(ChunkId, PathBuf)> = block
                .par_iter()
                .map(|&id| {
                    let path = self.path(work, id);
                    if !path.exists() {
                        let lines = self.compute(id).with_context(|| format!("chunk {id:?}"))?;
                        let tmp = path.with_extension(format!("tmp{:?}", std::thread::current().id()));
                        fs::write(&tmp, encode(&lines)?)?;
                        fs::rename(&tmp, &path)?;
                    }
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if finished % 1000 == 0 || finished == total {
                        eprintln!("contours: {finished}/{total} chunks");
                    }
                    Ok((id, path))
                })
                .collect::<Result<_>>()?;
            files.extend(finished);
        }
        files.sort_unstable_by_key(|(id, _)| *id);
        Ok(files)
    }
}

/// The 5 degree square a chunk is in, as an archive of a remote source might be.
fn block_of(id: ChunkId) -> (i32, i32) {
    (id.y.div_euclid(20), id.x.div_euclid(20))
}

/// Lays a source over the grid, where it has data.
fn blend(grid: &mut Grid, source: &Source, cache: &RasterCache) -> Result<()> {
    let r = grid.refine;
    let rs = source.refine;
    let rect = grid.rect;
    let to_source = |i: i64| (i as f64 + 0.5) * rs as f64 / r as f64 - 0.5;
    let area = Rect {
        x0: to_source(rect.x0).floor() as i64 - 1,
        y0: to_source(rect.y0).floor() as i64 - 1,
        x1: to_source(rect.x1 - 1).ceil() as i64 + 2,
        y1: to_source(rect.y1 - 1).ceil() as i64 + 2,
    };
    let west = Grid::lon(area.x0 as f64, rs);
    let south = Grid::lat(area.y0 as f64, rs);
    let east = Grid::lon(area.x1 as f64, rs);
    let north = Grid::lat(area.y1 as f64, rs);
    let margin = source.config.margin() as f64 / (BASE_PER_DEGREE * rs) as f64;
    if source.overlapping(west - margin, south - margin, east + margin, north + margin).is_empty() {
        return Ok(());
    }
    let (values, weights) = source.read_lattice(area, cache)?;
    let (sw, width) = (area.width(), rect.width());
    let same = rs == r;
    // Fractions of the source lattice for each column; exact on nested points.
    let locate = |i: i64, origin: i64| {
        let p = to_source(i) - origin as f64;
        let nearest = p.round();
        if same || (p - nearest).abs() < 1e-6 {
            (nearest as usize, 0.0f32)
        } else {
            (p.floor() as usize, (p - p.floor()) as f32)
        }
    };
    let columns: Vec<(usize, f32)> = (0..width).map(|c| locate(rect.x0 + c as i64, area.x0)).collect();
    for row in 0..rect.height() {
        let (sy, fy) = locate(rect.y0 + row as i64, area.y0);
        for (col, &(sx, fx)) in columns.iter().enumerate() {
            let at = |x: usize, y: usize| (values.values[y * sw + x], weights[y * sw + x]);
            let (v, w) = interpolate(fx, fy, |dx, dy| at(sx + dx, sy + dy));
            if v.is_nan() {
                continue;
            }
            let out = &mut grid.values[row * width + col];
            *out = if out.is_nan() || w >= 1.0 { v } else { *out + (v - *out) * w };
        }
    }
    Ok(())
}

/// Bilinear in x and y, without touching neighbors at fraction zero, so that those points
/// stay valid next to missing data. Missing if any neighbor it needs is.
#[inline]
fn interpolate(fx: f32, fy: f32, at: impl Fn(usize, usize) -> (f32, f32)) -> (f32, f32) {
    let row = |dy: usize| {
        let a = at(0, dy);
        if fx == 0.0 {
            return a;
        }
        let b = at(1, dy);
        (a.0 + (b.0 - a.0) * fx, a.1 + (b.1 - a.1) * fx)
    };
    let a = row(0);
    if fy == 0.0 {
        return a;
    }
    let b = row(1);
    (a.0 + (b.0 - a.0) * fy, a.1 + (b.1 - a.1) * fy)
}

/// The count of (nodes, ways) a line becomes.
pub fn counts(line: &Line) -> (u64, u64) {
    let n = line.points.len();
    let nodes = if line.closed { n - 1 } else { n };
    let ways = (n - 1).div_ceil(MAX_WAY_NODES - 1);
    (nodes as u64, ways as u64)
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}

/// A chunk file: counts of nodes and ways, then the lines, deflated.
pub fn encode(lines: &[Line]) -> Result<Vec<u8>> {
    let (mut nodes, mut ways) = (0, 0);
    let mut body = Vec::new();
    put_varint(&mut body, lines.len() as u64);
    for line in lines {
        let (n, w) = counts(line);
        nodes += n;
        ways += w;
        body.push(line.unit as u8 | u8::from(line.closed) << 1);
        put_varint(&mut body, zigzag(i64::from(line.elevation)));
        put_varint(&mut body, line.points.len() as u64);
        let (mut px, mut py) = (0i64, 0i64);
        for &(x, y) in &line.points {
            put_varint(&mut body, zigzag(i64::from(x) - px));
            put_varint(&mut body, zigzag(i64::from(y) - py));
            (px, py) = (x.into(), y.into());
        }
    }
    let mut out = Vec::with_capacity(body.len() / 2 + 16);
    out.extend_from_slice(&u64::to_le_bytes(nodes));
    out.extend_from_slice(&u64::to_le_bytes(ways));
    let mut encoder = ZlibEncoder::new(out, Compression::fast());
    encoder.write_all(&body)?;
    Ok(encoder.finish()?)
}

/// The counts of (nodes, ways) in a chunk file, without decoding the rest.
pub fn read_counts(path: &Path) -> Result<(u64, u64)> {
    let mut buf = [0; 16];
    fs::File::open(path)?.read_exact(&mut buf)?;
    Ok((
        u64::from_le_bytes(buf[..8].try_into().unwrap()),
        u64::from_le_bytes(buf[8..].try_into().unwrap()),
    ))
}

pub fn read_lines(path: &Path) -> Result<Vec<Line>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut body = Vec::new();
    ZlibDecoder::new(&bytes[16..]).read_to_end(&mut body)?;
    let mut at = 0;
    let varint = |at: &mut usize| {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let b = body[*at];
            *at += 1;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return v;
            }
            shift += 7;
        }
    };
    let count = varint(&mut at);
    let mut lines = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let flags = body[at];
        at += 1;
        let elevation = unzigzag(varint(&mut at)) as i32;
        let n = varint(&mut at) as usize;
        let mut points = Vec::with_capacity(n);
        let (mut x, mut y) = (0i64, 0i64);
        for _ in 0..n {
            x += unzigzag(varint(&mut at));
            y += unzigzag(varint(&mut at));
            points.push((x as i32, y as i32));
        }
        lines.push(Line {
            unit: if flags & 1 == 0 { Unit::Meters } else { Unit::Feet },
            elevation,
            closed: flags & 2 != 0,
            points,
        });
    }
    Ok(lines)
}
