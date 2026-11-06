//! Elevation rasters: finding them, and resampling them onto global lattices.
//!
//! Every value this module produces is a function of the global position alone, never of
//! the window it was asked for: filtering and feathering only look a fixed distance away,
//! and windows are read with that much margin. That is what makes contours match across
//! chunk boundaries bit for bit.
//!
//! A lattice with refinement `r` has its points at `(i + 0.5) / (3600 r)` degrees from
//! (-180, -90). For odd `r` the points of the 1 arc second lattice are also points of
//! the finer one.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

use crate::config::{BASE_PER_DEGREE, SourceConfig};
use crate::remote::{self, RemoteEntry};

/// A half-open rectangle of lattice points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: i64,
    pub y0: i64,
    pub x1: i64,
    pub y1: i64,
}

impl Rect {
    pub fn width(&self) -> usize {
        (self.x1 - self.x0) as usize
    }
    pub fn height(&self) -> usize {
        (self.y1 - self.y0) as usize
    }
    pub fn grow(&self, by: i64) -> Rect {
        Rect { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }
}

/// Values on a rectangle of a lattice; NaN is no data. Row 0 is the southernmost.
#[derive(Clone, Debug)]
pub struct Grid {
    pub rect: Rect,
    pub refine: i64,
    pub values: Vec<f32>,
}

impl Grid {
    pub fn new(rect: Rect, refine: i64) -> Grid {
        Grid { rect, refine, values: vec![f32::NAN; rect.width() * rect.height()] }
    }
    #[inline]
    pub fn at(&self, col: usize, row: usize) -> f32 {
        self.values[row * self.rect.width() + col]
    }
    /// Degrees east of -180 of a fractional lattice column index (absolute).
    #[inline]
    pub fn lon(x: f64, refine: i64) -> f64 {
        (x + 0.5) / (BASE_PER_DEGREE * refine) as f64 - 180.0
    }
    #[inline]
    pub fn lat(y: f64, refine: i64) -> f64 {
        (y + 0.5) / (BASE_PER_DEGREE * refine) as f64 - 90.0
    }
}

/// One raster file's georeferencing.
#[derive(Clone, Debug)]
pub struct Raster {
    pub path: PathBuf,
    pub width: usize,
    pub height: usize,
    /// The outer edges of the raster's first pixel.
    pub west: f64,
    pub north: f64,
    pub dx: f64,
    pub dy: f64,
    pub nodata: Option<f64>,
    pub bytes: u64,
    /// Where it is in a bucket, if `path` isn't on disk.
    pub remote: Option<RemoteEntry>,
}

impl Raster {
    pub fn east(&self) -> f64 {
        self.west + self.width as f64 * self.dx
    }
    pub fn south(&self) -> f64 {
        self.north - self.height as f64 * self.dy
    }
}

fn doubles(value: tiff::decoder::ifd::Value) -> Result<Vec<f64>> {
    Ok(value.into_f64_vec()?)
}

/// Reads a GeoTIFF's georeferencing, rejecting rasters that aren't in geographic coordinates.
pub fn read_header(path: &Path) -> Result<Raster> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let bytes = file.metadata()?.len();
    parse_header(path, bytes, BufReader::new(file))
}

/// Reads a raster's georeferencing from its file, of `bytes` bytes, named `path`.
pub fn parse_header<R: Read + Seek>(path: &Path, bytes: u64, reader: R) -> Result<Raster> {
    let mut decoder = Decoder::new(reader).with_context(|| format!("reading {}", path.display()))?;
    let (width, height) = decoder.dimensions()?;
    let scale = decoder
        .find_tag(Tag::ModelPixelScaleTag)?
        .map(doubles)
        .transpose()?
        .with_context(|| format!("{}: no ModelPixelScale", path.display()))?;
    let tie = decoder
        .find_tag(Tag::ModelTiepointTag)?
        .map(doubles)
        .transpose()?
        .with_context(|| format!("{}: no ModelTiepoint", path.display()))?;
    if scale.len() < 2 || tie.len() < 6 {
        bail!("{}: malformed georeferencing", path.display());
    }

    // GeoKeyDirectory: a header of 4 shorts, then (key, location, count, value) entries.
    let keys: Vec<u64> = decoder
        .find_tag(Tag::GeoKeyDirectoryTag)?
        .map(|v| v.into_u64_vec())
        .transpose()?
        .unwrap_or_default();
    let key = |id: u64| {
        keys.chunks_exact(4).skip(1).find(|entry| entry[0] == id && entry[1] == 0).map(|entry| entry[3])
    };
    // GTModelTypeGeoKey: 2 is geographic.
    if key(1024) != Some(2) {
        bail!(
            "{}: not in geographic coordinates; reproject first, e.g. \
             gdalwarp -t_srs EPSG:4326 -tr <deg> <deg> -tap -r bilinear -of COG in.tif out.tif",
            path.display()
        );
    }
    // GTRasterTypeGeoKey: 2 is PixelIsPoint, where the tiepoint is the pixel's center.
    let is_point = key(1025) == Some(2);
    let (dx, dy) = (scale[0], scale[1]);
    let shift = if is_point { 0.5 } else { 0.0 };
    let west = tie[3] - (tie[0] + shift) * dx;
    let north = tie[4] + (tie[1] + shift) * dy;

    let nodata = match decoder.find_tag(Tag::GdalNodata)? {
        Some(value) => value.into_string().ok().and_then(|s| s.trim_matches('\0').trim().parse().ok()),
        None => None,
    };
    Ok(Raster {
        path: path.to_owned(),
        width: width as usize,
        height: height as usize,
        west,
        north,
        dx,
        dy,
        nodata,
        bytes,
        remote: None,
    })
}

/// Reads a raster's elevations as f32 with NaN for no data, north to south.
pub fn read_values(raster: &Raster) -> Result<Vec<f32>> {
    let Some(remote) = &raster.remote else {
        return decode_values(raster, BufReader::new(File::open(&raster.path)?));
    };
    let file = remote.fetch()?;
    // Georeferencing of a remote raster is inferred, so this is where it's checked.
    let real = parse_header(&raster.path, file.len() as u64, Cursor::new(&file))?;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    if real.width != raster.width
        || real.height != raster.height
        || !close(real.west, raster.west)
        || !close(real.north, raster.north)
        || !close(real.dx, raster.dx)
        || !close(real.dy, raster.dy)
        || real.nodata != raster.nodata
    {
        bail!("{}: its header doesn't match what was inferred: {real:?} vs {raster:?}", raster.path.display());
    }
    decode_values(raster, Cursor::new(&file))
}

fn decode_values<R: Read + Seek>(raster: &Raster, reader: R) -> Result<Vec<f32>> {
    let mut decoder = Decoder::new(reader)?.with_limits(Limits::unlimited());
    let image = decoder
        .read_image()
        .with_context(|| format!("decoding {}", raster.path.display()))?;
    let values: Vec<f32> = match image {
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
        _ => bail!("{}: unsupported sample type", raster.path.display()),
    };
    if values.len() != raster.width * raster.height {
        bail!("{}: expected a single band", raster.path.display());
    }
    let nodata = raster.nodata;
    Ok(values
        .into_iter()
        .map(|v| {
            let missing = !v.is_finite()
                || nodata.is_some_and(|n| (f64::from(v) - n).abs() < 1e-3)
                // Sentinels of files without the nodata tag.
                || v <= -9000.0
                || v >= 10000.0;
            if missing { f32::NAN } else { v }
        })
        .collect())
}

/// Decoded rasters, most recently used kept, up to a byte limit.
pub struct RasterCache {
    limit: u64,
    inner: Mutex<CacheInner>,
    /// A lock for each raster being read, so that it's only read once.
    loading: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

#[derive(Default)]
struct CacheInner {
    entries: HashMap<PathBuf, (Arc<Vec<f32>>, u64)>,
    bytes: u64,
    clock: u64,
}

impl RasterCache {
    pub fn new(limit: u64) -> RasterCache {
        RasterCache { limit, inner: Mutex::default(), loading: Mutex::default() }
    }

    fn cached(&self, raster: &Raster) -> Option<Arc<Vec<f32>>> {
        let mut inner = self.inner.lock().unwrap();
        inner.clock += 1;
        let clock = inner.clock;
        let (values, used) = inner.entries.get_mut(&raster.path)?;
        *used = clock;
        Some(values.clone())
    }

    pub fn get(&self, raster: &Raster) -> Result<Arc<Vec<f32>>> {
        if let Some(values) = self.cached(raster) {
            return Ok(values);
        }
        let lock = self.loading.lock().unwrap().entry(raster.path.clone()).or_default().clone();
        let _reading = lock.lock().unwrap();
        // Another thread may have just finished.
        if let Some(values) = self.cached(raster) {
            return Ok(values);
        }
        let values = Arc::new(read_values(raster)?);
        let size = (values.len() * 4) as u64;
        let mut inner = self.inner.lock().unwrap();
        inner.clock += 1;
        let clock = inner.clock;
        if inner.entries.insert(raster.path.clone(), (values.clone(), clock)).is_none() {
            inner.bytes += size;
        }
        while inner.bytes > self.limit && inner.entries.len() > 1 {
            let oldest = inner.entries.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone());
            if let Some(path) = oldest {
                if let Some((old, _)) = inner.entries.remove(&path) {
                    inner.bytes -= (old.len() * 4) as u64;
                }
            }
        }
        drop(inner);
        self.loading.lock().unwrap().remove(&raster.path);
        Ok(values)
    }
}

/// One configured source and its files.
pub struct Source {
    pub config: SourceConfig,
    pub refine: i64,
    pub rasters: Vec<Raster>,
    /// Indices into `rasters`, by the whole degree cell (floor lon, floor lat).
    index: HashMap<(i32, i32), Vec<u32>>,
}

fn find_files(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        // E.g. a volume's lost+found: not ours to read, and not where DEMs are.
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {}: {e}", dir.display());
            return Ok(());
        }
        entries => entries.with_context(|| format!("reading {}", dir.display()))?,
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            find_files(&path, suffix, out)?;
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(suffix)) {
            out.push(path);
        }
    }
    Ok(())
}

impl Source {
    pub fn open(config: &SourceConfig, work: &Path) -> Result<Source> {
        if let Some(s3) = &config.s3 {
            return Source::new(config.clone(), remote::open_source(config, s3, work)?);
        }
        let mut paths = Vec::new();
        find_files(&config.dir, &config.suffix, &mut paths)?;
        paths.sort();
        if paths.is_empty() {
            bail!("source {}: no *{} under {}", config.name, config.suffix, config.dir.display());
        }
        use rayon::prelude::*;
        let rasters = paths.par_iter().map(|p| read_header(p)).collect::<Result<Vec<_>>>()?;
        Source::new(config.clone(), rasters)
    }

    pub fn new(config: SourceConfig, rasters: Vec<Raster>) -> Result<Source> {
        let mut index: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for (i, raster) in rasters.iter().enumerate() {
            for lat in raster.south().floor() as i32..raster.north.ceil() as i32 {
                for lon in raster.west.floor() as i32..raster.east().ceil() as i32 {
                    index.entry((lon, lat)).or_default().push(i as u32);
                }
            }
        }
        Ok(Source { refine: config.refine()?, config, rasters, index })
    }

    /// The source's rasters that overlap a lon/lat box, in a stable order.
    pub fn overlapping(&self, west: f64, south: f64, east: f64, north: f64) -> Vec<&Raster> {
        let mut found: Vec<u32> = Vec::new();
        for lat in south.floor() as i32..=north.floor() as i32 {
            for lon in west.floor() as i32..=east.floor() as i32 {
                if let Some(ids) = self.index.get(&(lon, lat)) {
                    found.extend_from_slice(ids);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        found
            .into_iter()
            .map(|i| &self.rasters[i as usize])
            .filter(|r| r.west < east && r.east() > west && r.south() < north && r.north > south)
            .collect()
    }

    /// The source's own filtered lattice over `rect` (in this source's lattice), with each
    /// point's blending weight (0 to 1) from the feather.
    pub fn read_lattice(&self, rect: Rect, cache: &RasterCache) -> Result<(Grid, Vec<f32>)> {
        let margin = self.config.margin();
        let read = rect.grow(margin);
        let mut grid = Grid::new(read, self.refine);
        let (west, south) = (
            Grid::lon(read.x0 as f64 - 0.5, self.refine),
            Grid::lat(read.y0 as f64 - 0.5, self.refine),
        );
        let (east, north) = (
            Grid::lon(read.x1 as f64 - 0.5, self.refine),
            Grid::lat(read.y1 as f64 - 0.5, self.refine),
        );
        for raster in self.overlapping(west, south, east, north) {
            let values = cache.get(raster)?;
            fill_from_raster(&mut grid, raster, &values);
        }
        if self.config.median {
            median3(&mut grid);
        }
        let weights = feather_weights(&grid, self.config.feather);
        if self.config.sigma > 0.0 {
            gaussian(&mut grid, self.config.sigma);
        }
        let (width, m) = (read.width(), margin as usize);
        let mut out = Grid::new(rect, self.refine);
        let mut out_weights = vec![0.0; rect.width() * rect.height()];
        for row in 0..rect.height() {
            let from = (row + m) * width + m;
            let to = row * rect.width();
            out.values[to..to + rect.width()].copy_from_slice(&grid.values[from..from + rect.width()]);
            out_weights[to..to + rect.width()].copy_from_slice(&weights[from..from + rect.width()]);
        }
        Ok((out, out_weights))
    }
}

/// Samples a raster at the grid's lattice points inside it, bilinearly (which is an exact
/// copy where the raster's pixel centers are the lattice points), clamping at the raster's
/// edges. Later rasters overwrite earlier ones.
fn fill_from_raster(grid: &mut Grid, raster: &Raster, values: &[f32]) {
    let rect = grid.rect;
    let width = rect.width();
    let refine = grid.refine;
    let lattice_to_px = |lon: f64| (lon - raster.west) / raster.dx - 0.5;
    let lattice_to_py = |lat: f64| (raster.north - lat) / raster.dy - 0.5;

    // Precompute columns inside the raster.
    let columns: Vec<(usize, usize, usize, f32)> = (0..width)
        .filter_map(|col| {
            let lon = Grid::lon((rect.x0 + col as i64) as f64, refine);
            if lon < raster.west || lon >= raster.east() {
                return None;
            }
            let px = lattice_to_px(lon).clamp(0.0, (raster.width - 1) as f64);
            let (px0, f) = snap(px);
            let px1 = (px0 + 1).min(raster.width - 1);
            Some((col, px0, px1, f))
        })
        .collect();

    for row in 0..rect.height() {
        let lat = Grid::lat((rect.y0 + row as i64) as f64, refine);
        if lat >= raster.north || lat < raster.south() {
            continue;
        }
        let py = lattice_to_py(lat).clamp(0.0, (raster.height - 1) as f64);
        let (py0, fy) = snap(py);
        let py1 = (py0 + 1).min(raster.height - 1);
        let (north_row, south_row) = (py0 * raster.width, py1 * raster.width);
        let out = &mut grid.values[row * width..(row + 1) * width];
        for &(col, px0, px1, fx) in &columns {
            let value = if fx == 0.0 && fy == 0.0 {
                values[north_row + px0]
            } else {
                let n = lerp(values[north_row + px0], values[north_row + px1], fx);
                let s = lerp(values[south_row + px0], values[south_row + px1], fx);
                lerp(n, s, fy)
            };
            if value.is_finite() {
                out[col] = value;
            }
        }
    }
}

/// Splits a raster position into pixel and fraction, treating values within rounding of
/// a pixel center as exactly on it, so matching grids copy instead of interpolating.
fn snap(p: f64) -> (usize, f32) {
    let nearest = p.round();
    if (p - nearest).abs() < 1e-6 {
        return (nearest as usize, 0.0);
    }
    (p.floor() as usize, (p - p.floor()) as f32)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    if t == 0.0 {
        a
    } else if t == 1.0 {
        b
    } else {
        a + (b - a) * t
    }
}

/// A 3x3 median ignoring no-data neighbors. Points without data stay without.
pub fn median3(grid: &mut Grid) {
    let (w, h) = (grid.rect.width(), grid.rect.height());
    let source = grid.values.clone();
    let mut around = [0f32; 9];
    for row in 1..h.saturating_sub(1) {
        for col in 1..w.saturating_sub(1) {
            let i = row * w + col;
            if source[i].is_nan() {
                continue;
            }
            let mut n = 0;
            for r in row - 1..=row + 1 {
                for c in col - 1..=col + 1 {
                    let v = source[r * w + c];
                    if !v.is_nan() {
                        around[n] = v;
                        n += 1;
                    }
                }
            }
            let slice = &mut around[..n];
            slice.sort_unstable_by(f32::total_cmp);
            grid.values[i] = slice[n / 2];
        }
    }
}

/// A separable Gaussian that renormalizes around no-data points, truncated at 3 sigma.
pub fn gaussian(grid: &mut Grid, sigma: f64) {
    let radius = (sigma * 3.0).ceil() as usize;
    let kernel: Vec<f32> = (0..=radius)
        .map(|d| (-((d * d) as f64) / (2.0 * sigma * sigma)).exp() as f32)
        .collect();
    let (w, h) = (grid.rect.width(), grid.rect.height());
    let mut tmp = grid.values.clone();
    use rayon::prelude::*;
    let source = &grid.values;
    tmp.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        let line = &source[row * w..(row + 1) * w];
        for col in 0..w {
            if line[col].is_nan() {
                continue;
            }
            let (mut sum, mut total) = (line[col] * kernel[0], kernel[0]);
            for d in 1..=radius {
                if col >= d && !line[col - d].is_nan() {
                    sum += line[col - d] * kernel[d];
                    total += kernel[d];
                }
                if col + d < w && !line[col + d].is_nan() {
                    sum += line[col + d] * kernel[d];
                    total += kernel[d];
                }
            }
            out[col] = sum / total;
        }
    });
    let tmp_ref = &tmp;
    grid.values.par_chunks_mut(w).enumerate().for_each(|(row, out)| {
        for col in 0..w {
            let center = tmp_ref[row * w + col];
            if center.is_nan() {
                continue;
            }
            let (mut sum, mut total) = (center * kernel[0], kernel[0]);
            for d in 1..=radius {
                if row >= d {
                    let v = tmp_ref[(row - d) * w + col];
                    if !v.is_nan() {
                        sum += v * kernel[d];
                        total += kernel[d];
                    }
                }
                if row + d < h {
                    let v = tmp_ref[(row + d) * w + col];
                    if !v.is_nan() {
                        sum += v * kernel[d];
                        total += kernel[d];
                    }
                }
            }
            out[col] = sum / total;
        }
    });
}

/// For each point, how far it is inside the data (0 at the edge, 1 at `feather` cells or
/// more), smoothly. Uses an integer 3-4 chamfer distance capped at the feather, so a value
/// depends only on points within `feather` cells.
pub fn feather_weights(grid: &Grid, feather: u32) -> Vec<f32> {
    let (w, h) = (grid.rect.width(), grid.rect.height());
    if feather == 0 {
        return grid.values.iter().map(|v| if v.is_nan() { 0.0 } else { 1.0 }).collect();
    }
    let cap = feather * 3;
    let mut dist: Vec<u32> =
        grid.values.iter().map(|v| if v.is_nan() { 0 } else { cap }).collect();
    let relax = |dist: &mut [u32], i: usize, j: usize, cost: u32| {
        let through = dist[j].saturating_add(cost);
        if through < dist[i] {
            dist[i] = through;
        }
    };
    for row in 0..h {
        for col in 0..w {
            let i = row * w + col;
            if col > 0 {
                relax(&mut dist, i, i - 1, 3);
            }
            if row > 0 {
                relax(&mut dist, i, i - w, 3);
                if col > 0 {
                    relax(&mut dist, i, i - w - 1, 4);
                }
                if col + 1 < w {
                    relax(&mut dist, i, i - w + 1, 4);
                }
            }
        }
    }
    for row in (0..h).rev() {
        for col in (0..w).rev() {
            let i = row * w + col;
            if col + 1 < w {
                relax(&mut dist, i, i + 1, 3);
            }
            if row + 1 < h {
                relax(&mut dist, i, i + w, 3);
                if col + 1 < w {
                    relax(&mut dist, i, i + w + 1, 4);
                }
                if col > 0 {
                    relax(&mut dist, i, i + w - 1, 4);
                }
            }
        }
    }
    dist.into_iter()
        .map(|d| {
            let t = d as f32 / cap as f32;
            t * t * (3.0 - 2.0 * t)
        })
        .collect()
}
