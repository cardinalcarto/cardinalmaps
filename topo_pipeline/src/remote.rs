//! Rasters read from zip archives in an S3-compatible bucket, for sources too large to keep
//! on disk.
//!
//! A raster is one deflated entry of a zip, so a range request gets it alone. GeoTIFF headers
//! can be at the end of a file, which a deflated stream can't skip to, so reading each of them
//! would take downloading the whole source. So georeferencing comes from file names, as in
//! `ALPSMLC30_N047W123_DSM.tif`, and one sample raster per latitude, since rasters at a
//! latitude have the same shape. Each raster's real header is checked against this when it's
//! decoded (see `dem::read_values`), as is its CRC, so a wrong guess stops the run.
//!
//! What's learned is kept in `<work>/remote-<source>.tsv`, so it's only done once per archive.
//! An archive is only used once its `.sha256` is in the bucket, which should be uploaded last.
//!
//! Credentials come from `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use flate2::Crc;
use flate2::read::DeflateDecoder;
use rayon::prelude::*;
use rusty_s3::actions::{ListObjectsV2, S3Action};
use rusty_s3::{Bucket, Credentials, UrlStyle};

use crate::config::{S3Config, SourceConfig};
use crate::dem::{Raster, parse_header};

const ATTEMPTS: u32 = 6;
const SIGNED_FOR: Duration = Duration::from_secs(900);
/// The end of a zip to read first, which holds the central directory of any small archive.
const TAIL: u64 = 256 * 1024;
const LOCAL_HEADER: usize = 30;

pub struct Store {
    bucket: Bucket,
    credentials: Credentials,
    agent: ureq::Agent,
    prefix: String,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Store({}/{})", self.bucket.name(), self.prefix)
    }
}

fn retrying<T>(what: &str, mut attempt: impl FnMut() -> Result<T>) -> Result<T> {
    let mut wait = Duration::from_secs(2);
    for _ in 1..ATTEMPTS {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(e) => {
                eprintln!("{what}: {e:#}; retrying in {}s", wait.as_secs());
                std::thread::sleep(wait);
                wait *= 2;
            }
        }
    }
    attempt().with_context(|| format!("{what}, after {ATTEMPTS} attempts"))
}

impl Store {
    pub fn new(config: &S3Config) -> Result<Store> {
        let region = match &config.region {
            Some(region) => region.clone(),
            // As in s3.us-west-000.backblazeb2.com or s3.us-east-1.amazonaws.com.
            None => config.endpoint.split('.').nth(1).context("no region in the S3 endpoint; set `region`")?.to_owned(),
        };
        let endpoint = format!("https://{}", config.endpoint).parse().context("parsing the S3 endpoint")?;
        let bucket = Bucket::new(endpoint, UrlStyle::Path, config.bucket.clone(), region)?;
        let credentials = Credentials::from_env().context("AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY must be set")?;
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .timeout_recv_body(Some(Duration::from_secs(600)))
            .build()
            .into();
        Ok(Store { bucket, credentials, agent, prefix: config.prefix.clone() })
    }

    /// Bytes `start..end` of an object, or to its end if that comes first.
    fn range(&self, key: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        retrying(&format!("reading {key}"), || {
            let url = self.bucket.get_object(Some(&self.credentials), key).sign(SIGNED_FOR);
            let mut response = self
                .agent
                .get(url.as_str())
                .header("Range", format!("bytes={start}-{}", end - 1))
                .call()?;
            let data = response.body_mut().with_config().limit(u64::MAX).read_to_vec()?;
            let wanted = end - start;
            if data.len() as u64 > wanted {
                bail!("got {} bytes of a range of {wanted}", data.len());
            }
            Ok(data)
        })
    }

    /// Every object under the prefix, with its size.
    fn list(&self) -> Result<BTreeMap<String, u64>> {
        let mut objects = BTreeMap::new();
        let mut token: Option<String> = None;
        loop {
            let page = retrying("listing the bucket", || {
                let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
                action.with_prefix(self.prefix.as_str());
                if let Some(token) = &token {
                    action.with_continuation_token(token.as_str());
                }
                let url = action.sign(SIGNED_FOR);
                let text = self.agent.get(url.as_str()).call()?.body_mut().with_config().limit(u64::MAX).read_to_string()?;
                Ok(ListObjectsV2::parse_response(&text)?)
            })?;
            for object in page.contents {
                objects.insert(object.key, object.size.unwrap_or(0));
            }
            match page.next_continuation_token {
                Some(next) => token = Some(next),
                None => return Ok(objects),
            }
        }
    }
}

/// Where in a bucket a raster is.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub store: Arc<Store>,
    pub key: String,
    pub local_offset: u64,
    pub compressed: u64,
    pub uncompressed: u64,
    pub crc: u32,
}

impl RemoteEntry {
    /// The raster's file.
    pub fn fetch(&self) -> Result<Vec<u8>> {
        // Enough for its local header in one request, nearly always.
        let guess = self.local_offset + LOCAL_HEADER as u64 + 512 + self.compressed;
        let data = self.store.range(&self.key, self.local_offset, guess)?;
        if data.len() < LOCAL_HEADER || le32(&data, 0) != 0x0403_4b50 {
            bail!("{}: no local file header at {}", self.key, self.local_offset);
        }
        let start = LOCAL_HEADER + usize::from(le16(&data, 26)) + usize::from(le16(&data, 28));
        let end = start + self.compressed as usize;
        let more;
        let deflated = if data.len() >= end {
            &data[start..end]
        } else {
            more = self.store.range(
                &self.key,
                self.local_offset + start as u64,
                self.local_offset + end as u64,
            )?;
            &more[..]
        };
        let mut out = Vec::with_capacity(self.uncompressed as usize);
        DeflateDecoder::new(deflated).read_to_end(&mut out).with_context(|| format!("inflating from {}", self.key))?;
        let mut crc = Crc::new();
        crc.update(&out);
        if out.len() as u64 != self.uncompressed || crc.sum() != self.crc {
            bail!("{}: entry at {} fails its size or CRC", self.key, self.local_offset);
        }
        Ok(out)
    }
}

fn le16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn le32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    name: String,
    compressed: u64,
    uncompressed: u64,
    local_offset: u64,
    crc: u32,
}

/// The entries in a zip's central directory that end in `suffix`.
fn central_directory(directory: &[u8], suffix: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut at = 0;
    while at + 46 <= directory.len() && le32(directory, at) == 0x0201_4b50 {
        let (name_len, extra, comment) = (
            usize::from(le16(directory, at + 28)),
            usize::from(le16(directory, at + 30)),
            usize::from(le16(directory, at + 32)),
        );
        let name = String::from_utf8_lossy(&directory[at + 46..at + 46 + name_len]).into_owned();
        if name.ends_with(suffix) {
            if le16(directory, at + 10) != 8 {
                bail!("{name}: not deflated");
            }
            let (compressed, uncompressed, local_offset) =
                (le32(directory, at + 20), le32(directory, at + 24), le32(directory, at + 42));
            if compressed == u32::MAX || uncompressed == u32::MAX || local_offset == u32::MAX {
                bail!("{name}: zip64 entries aren't supported");
            }
            entries.push(Entry {
                name,
                compressed: compressed.into(),
                uncompressed: uncompressed.into(),
                local_offset: local_offset.into(),
                crc: le32(directory, at + 16),
            });
        }
        at += 46 + name_len + extra + comment;
    }
    Ok(entries)
}

fn read_directory(store: &Store, key: &str, size: u64, suffix: &str) -> Result<Vec<Entry>> {
    let start = size.saturating_sub(TAIL);
    let tail = store.range(key, start, size)?;
    let Some(end) = (0..tail.len().saturating_sub(21)).rev().find(|&i| le32(&tail, i) == 0x0605_4b50) else {
        bail!("{key}: no end of central directory");
    };
    let (length, offset) = (u64::from(le32(&tail, end + 12)), u64::from(le32(&tail, end + 16)));
    if offset == u64::from(u32::MAX) {
        bail!("{key}: zip64 archives aren't supported");
    }
    if offset >= start {
        let from = (offset - start) as usize;
        central_directory(&tail[from..from + length as usize], suffix)
    } else {
        central_directory(&store.range(key, offset, offset + length)?, suffix)
    }
}

/// The (longitude, latitude) of a raster's southwest corner, from a file name like
/// `ALPSMLC30_N047W123_DSM.tif`.
fn corner(name: &str) -> Option<(i32, i32)> {
    let file = name.rsplit('/').next()?;
    file.split('_').find_map(|part| {
        let bytes = part.as_bytes();
        if bytes.len() != 8 || !part.is_ascii() {
            return None;
        }
        let lat: i32 = part[1..4].parse().ok()?;
        let lon: i32 = part[5..8].parse().ok()?;
        let lat = match bytes[0] {
            b'N' => lat,
            b'S' => -lat,
            _ => return None,
        };
        let lon = match bytes[4] {
            b'E' => lon,
            b'W' => -lon,
            _ => return None,
        };
        Some((lon, lat))
    })
}

/// What rasters at one latitude share.
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    west_offset: f64,
    north_offset: f64,
    dx: f64,
    dy: f64,
    width: usize,
    height: usize,
    nodata: Option<f64>,
}

#[derive(Default)]
struct Index {
    /// (key, size) of each archive read, with its rasters.
    archives: BTreeMap<(String, u64), Vec<Entry>>,
    shapes: BTreeMap<i32, Shape>,
}

impl Index {
    fn load(path: &Path) -> Result<Index> {
        let mut index = Index::default();
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(index),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut load = |line: &str| -> Option<()> {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields[..] {
                ["archive", key, size] => {
                    index.archives.entry((key.to_owned(), size.parse().ok()?)).or_default();
                }
                ["entry", key, size, name, compressed, uncompressed, local_offset, crc] => {
                    index.archives.entry((key.to_owned(), size.parse().ok()?)).or_default().push(Entry {
                        name: name.to_owned(),
                        compressed: compressed.parse().ok()?,
                        uncompressed: uncompressed.parse().ok()?,
                        local_offset: local_offset.parse().ok()?,
                        crc: crc.parse().ok()?,
                    });
                }
                ["shape", lat, west, north, dx, dy, width, height, nodata] => {
                    index.shapes.insert(lat.parse().ok()?, Shape {
                        west_offset: west.parse().ok()?,
                        north_offset: north.parse().ok()?,
                        dx: dx.parse().ok()?,
                        dy: dy.parse().ok()?,
                        width: width.parse().ok()?,
                        height: height.parse().ok()?,
                        nodata: if nodata.is_empty() { None } else { Some(nodata.parse().ok()?) },
                    });
                }
                _ => return None,
            }
            Some(())
        };
        for line in text.lines() {
            if load(line).is_none() {
                bail!("{}: can't read the line {line:?}; delete the file to rebuild it", path.display());
            }
        }
        Ok(index)
    }

    fn save(&self, path: &Path) -> Result<()> {
        use std::fmt::Write;
        let mut text = String::new();
        for ((key, size), entries) in &self.archives {
            writeln!(text, "archive\t{key}\t{size}")?;
            for e in entries {
                writeln!(
                    text,
                    "entry\t{key}\t{size}\t{}\t{}\t{}\t{}\t{}",
                    e.name, e.compressed, e.uncompressed, e.local_offset, e.crc
                )?;
            }
        }
        for (lat, s) in &self.shapes {
            let nodata = s.nodata.map(|n| n.to_string()).unwrap_or_default();
            writeln!(
                text,
                "shape\t{lat}\t{}\t{}\t{}\t{}\t{}\t{}\t{nodata}",
                s.west_offset, s.north_offset, s.dx, s.dy, s.width, s.height
            )?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn raster(store: &Arc<Store>, key: &str, entry: &Entry, corner: (i32, i32), shape: &Shape) -> Raster {
    Raster {
        path: PathBuf::from(&entry.name),
        width: shape.width,
        height: shape.height,
        west: f64::from(corner.0) + shape.west_offset,
        north: f64::from(corner.1) + shape.north_offset,
        dx: shape.dx,
        dy: shape.dy,
        nodata: shape.nodata,
        // A local copy of the file has the same size and name, so finished chunks stay valid
        // whichever way the source is read.
        bytes: entry.uncompressed,
        remote: Some(RemoteEntry {
            store: store.clone(),
            key: key.to_owned(),
            local_offset: entry.local_offset,
            compressed: entry.compressed,
            uncompressed: entry.uncompressed,
            crc: entry.crc,
        }),
    }
}

/// A source's rasters, from the archives finished in its bucket.
pub fn open_source(config: &SourceConfig, s3: &S3Config, work: &Path) -> Result<Vec<Raster>> {
    let store = Arc::new(Store::new(s3)?);
    std::fs::create_dir_all(work)?;
    let path = work.join(format!("remote-{}.tsv", config.name));
    let mut index = Index::load(&path)?;

    let objects = store.list()?;
    let archives: Vec<(String, u64)> = objects
        .iter()
        .filter(|(key, _)| key.ends_with(".zip") && objects.contains_key(&format!("{key}.sha256")))
        .map(|(key, size)| (key.clone(), *size))
        .collect();
    if archives.is_empty() {
        bail!("source {}: no finished archives under {}", config.name, s3.prefix);
    }
    let wanted: HashSet<&(String, u64)> = archives.iter().collect();
    index.archives.retain(|archive, _| wanted.contains(archive));

    let new: Vec<&(String, u64)> = archives.iter().filter(|a| !index.archives.contains_key(*a)).collect();
    if !new.is_empty() {
        eprintln!("source {}: reading the directories of {} archives", config.name, new.len());
        let found: Vec<_> = new
            .par_iter()
            .map(|&archive| {
                let entries = read_directory(&store, &archive.0, archive.1, &config.suffix)?;
                Ok((archive.clone(), entries))
            })
            .collect::<Result<_>>()?;
        index.archives.extend(found);
        index.save(&path)?;
    }

    // One sample at each latitude that isn't known yet.
    let mut samples: HashMap<i32, (String, Entry, (i32, i32))> = HashMap::new();
    for ((key, _), entries) in &index.archives {
        for entry in entries {
            let at = corner(&entry.name).with_context(|| format!("no corner in the name {}", entry.name))?;
            if !index.shapes.contains_key(&at.1) {
                samples.entry(at.1).or_insert_with(|| (key.clone(), entry.clone(), at));
            }
        }
    }
    if !samples.is_empty() {
        eprintln!("source {}: reading a sample at {} latitudes", config.name, samples.len());
        let shapes: Vec<(i32, Shape)> = samples
            .par_iter()
            .map(|(&lat, (key, entry, at))| {
                let sample = raster(&store, key, entry, *at, &Shape {
                    west_offset: 0.0,
                    north_offset: 0.0,
                    dx: 0.0,
                    dy: 0.0,
                    width: 0,
                    height: 0,
                    nodata: None,
                });
                let file = sample.remote.as_ref().unwrap().fetch()?;
                let header = parse_header(Path::new(&entry.name), file.len() as u64, Cursor::new(file))?;
                Ok((lat, Shape {
                    west_offset: header.west - f64::from(at.0),
                    north_offset: header.north - f64::from(at.1),
                    dx: header.dx,
                    dy: header.dy,
                    width: header.width,
                    height: header.height,
                    nodata: header.nodata,
                }))
            })
            .collect::<Result<_>>()?;
        index.shapes.extend(shapes);
        index.save(&path)?;
    }

    let mut rasters = Vec::new();
    for ((key, _), entries) in &index.archives {
        for entry in entries {
            let at = corner(&entry.name).with_context(|| format!("no corner in the name {}", entry.name))?;
            rasters.push(raster(&store, key, entry, at, &index.shapes[&at.1]));
        }
    }
    rasters.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(rasters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_come_from_file_names() {
        assert_eq!(corner("N045W125_N050W120/ALPSMLC30_N047W123_DSM.tif"), Some((-123, 47)));
        assert_eq!(corner("S005E010_N000E015/ALPSMLC30_S001E014_DSM.tif"), Some((14, -1)));
        assert_eq!(corner("a/ALPSMLC30_DSM.tif"), None);
    }

    #[test]
    fn central_directory_entries_are_filtered_and_read() {
        let mut directory = Vec::new();
        let mut add = |name: &str, method: u16, crc: u32, compressed: u32, uncompressed: u32, offset: u32| {
            directory.extend(0x0201_4b50u32.to_le_bytes());
            directory.extend([0; 6]);
            directory.extend(method.to_le_bytes());
            directory.extend([0; 4]);
            directory.extend(crc.to_le_bytes());
            directory.extend(compressed.to_le_bytes());
            directory.extend(uncompressed.to_le_bytes());
            directory.extend((name.len() as u16).to_le_bytes());
            directory.extend([0; 12]);
            directory.extend(offset.to_le_bytes());
            directory.extend(name.as_bytes());
        };
        add("x/a_MSK.tif", 8, 1, 2, 3, 4);
        add("x/a_DSM.tif", 8, 5, 6, 7, 8);
        let entries = central_directory(&directory, "_DSM.tif").unwrap();
        assert_eq!(entries, vec![Entry {
            name: "x/a_DSM.tif".to_owned(),
            compressed: 6,
            uncompressed: 7,
            local_offset: 8,
            crc: 5,
        }]);
    }

    #[test]
    fn the_index_survives_a_round_trip() {
        let dir = std::env::temp_dir().join(format!("topo-index-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("index.tsv");
        let mut index = Index::default();
        index.archives.insert(("a.zip".to_owned(), 9), vec![Entry {
            name: "x/ALPSMLC30_N047W123_DSM.tif".to_owned(),
            compressed: 1,
            uncompressed: 2,
            local_offset: 3,
            crc: 4,
        }]);
        index.archives.insert(("empty.zip".to_owned(), 1), Vec::new());
        let shape = Shape {
            west_offset: 0.0,
            north_offset: 1.0,
            dx: 1.0 / 3600.0,
            dy: 1.0 / 3600.0,
            width: 3600,
            height: 3600,
            nodata: Some(-9999.0),
        };
        index.shapes.insert(47, shape.clone());
        index.save(&path).unwrap();
        let loaded = Index::load(&path).unwrap();
        assert_eq!(loaded.archives, index.archives);
        assert_eq!(loaded.shapes[&47], shape);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
