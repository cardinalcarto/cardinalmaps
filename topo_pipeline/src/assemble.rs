//! Turns chunk files into a `.osm.pbf`, and splices that into a planet file.
//!
//! Planetiler reads nodes, then ways, then relations, each ascending by ID, so "appending"
//! means splicing: the planet's nodes, contour nodes, the planet's ways, contour ways, the
//! planet's relations. Contour IDs start above OSM's, and `append` checks that they do.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use prost::Message;
use rayon::prelude::*;

use crate::chunks::{self, ChunkId, MAX_WAY_NODES};
use crate::contour::{Line, Unit};
use crate::pbf::{self, BlockKind, DenseNodes, HeaderBlock, PrimitiveBlock, PrimitiveGroup, StringTable, Way};

const NODES_PER_BLOCK: usize = 8000;
const WAYS_PER_BLOCK: usize = 8000;
const REFS_PER_BLOCK: usize = 400_000;
const SORTED: &str = "Sort.Type_then_ID";

#[derive(Default, Clone, Copy, Debug)]
pub struct Totals {
    pub lines: u64,
    pub nodes: u64,
    pub ways: u64,
}

struct NodeBlocks {
    id: Vec<i64>,
    lat: Vec<i64>,
    lon: Vec<i64>,
    last: (i64, i64, i64),
    out: Vec<u8>,
}

impl NodeBlocks {
    fn push(&mut self, id: i64, lon: i64, lat: i64) -> Result<()> {
        self.id.push(id - self.last.0);
        self.lat.push(lat - self.last.1);
        self.lon.push(lon - self.last.2);
        self.last = (id, lat, lon);
        if self.id.len() >= NODES_PER_BLOCK {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.id.is_empty() {
            return Ok(());
        }
        let block = PrimitiveBlock {
            stringtable: StringTable { s: vec![Vec::new()] },
            primitivegroup: vec![PrimitiveGroup {
                dense: Some(DenseNodes {
                    id: std::mem::take(&mut self.id),
                    lat: std::mem::take(&mut self.lat),
                    lon: std::mem::take(&mut self.lon),
                }),
                ..PrimitiveGroup::default()
            }],
        };
        self.out.extend(pbf::encode_blob("OSMData", &block.encode_to_vec())?);
        self.last = (0, 0, 0);
        Ok(())
    }
}

#[derive(Default)]
struct WayBlocks {
    ways: Vec<Way>,
    refs: usize,
    table: Vec<Vec<u8>>,
    out: Vec<u8>,
}

impl WayBlocks {
    fn string(&mut self, s: &str) -> u32 {
        if self.table.is_empty() {
            self.table.push(Vec::new());
        }
        if let Some(i) = self.table.iter().rposition(|t| t == s.as_bytes()) {
            return i as u32;
        }
        self.table.push(s.as_bytes().to_vec());
        (self.table.len() - 1) as u32
    }

    fn push(&mut self, id: i64, line: &Line, refs: &[i64]) -> Result<()> {
        let mut tags = vec![("contour", "elevation".to_owned())];
        match line.unit {
            Unit::Meters => {
                tags.push(("contour:unit", "m".to_owned()));
                tags.push(("ele", line.elevation.to_string()));
            }
            Unit::Feet => {
                let meters = f64::from(line.elevation) * Unit::Feet.meters();
                tags.push(("contour:unit", "ft".to_owned()));
                tags.push(("ele", format!("{meters:.2}")));
                tags.push(("ele:ft", line.elevation.to_string()));
            }
        }
        let (mut keys, mut vals) = (Vec::new(), Vec::new());
        for (k, v) in &tags {
            keys.push(self.string(k));
            vals.push(self.string(v));
        }
        let mut last = 0;
        let refs_delta = refs
            .iter()
            .map(|&r| {
                let d = r - last;
                last = r;
                d
            })
            .collect();
        self.ways.push(Way { id, keys, vals, refs: refs_delta });
        self.refs += refs.len();
        if self.ways.len() >= WAYS_PER_BLOCK || self.refs >= REFS_PER_BLOCK {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.ways.is_empty() {
            return Ok(());
        }
        let block = PrimitiveBlock {
            stringtable: StringTable { s: std::mem::take(&mut self.table) },
            primitivegroup: vec![PrimitiveGroup {
                ways: std::mem::take(&mut self.ways),
                ..PrimitiveGroup::default()
            }],
        };
        self.refs = 0;
        self.out.extend(pbf::encode_blob("OSMData", &block.encode_to_vec())?);
        Ok(())
    }
}

/// Encodes a chunk into blobs of nodes and blobs of ways.
fn encode_chunk(path: &Path, mut node_id: i64, mut way_id: i64) -> Result<(Vec<u8>, Vec<u8>, u64)> {
    let lines = chunks::read_lines(path)?;
    let mut nodes = NodeBlocks {
        id: Vec::new(),
        lat: Vec::new(),
        lon: Vec::new(),
        last: (0, 0, 0),
        out: Vec::new(),
    };
    let mut ways = WayBlocks::default();
    let mut refs = Vec::new();
    for line in &lines {
        refs.clear();
        let distinct = if line.closed { line.points.len() - 1 } else { line.points.len() };
        for &(lon, lat) in &line.points[..distinct] {
            nodes.push(node_id, lon.into(), lat.into())?;
            refs.push(node_id);
            node_id += 1;
        }
        if line.closed {
            refs.push(refs[0]);
        }
        // Long lines become several ways sharing their end nodes.
        let mut start = 0;
        while start + 1 < refs.len() {
            let end = (start + MAX_WAY_NODES).min(refs.len());
            ways.push(way_id, line, &refs[start..end])?;
            way_id += 1;
            start = end - 1;
        }
    }
    nodes.flush()?;
    ways.flush()?;
    Ok((nodes.out, ways.out, lines.len() as u64))
}

/// Writes chunks, in order, as one sorted `.osm.pbf`.
pub fn write_contours(
    files: &[(ChunkId, PathBuf)],
    node_start: i64,
    way_start: i64,
    out: &Path,
) -> Result<Totals> {
    let counts = files
        .par_iter()
        .map(|(_, path)| chunks::read_counts(path))
        .collect::<Result<Vec<_>>>()?;
    let mut starts = Vec::with_capacity(files.len());
    let (mut node_id, mut way_id) = (node_start, way_start);
    for &(n, w) in &counts {
        starts.push((node_id, way_id));
        node_id += n as i64;
        way_id += w as i64;
    }

    let ways_path = out.with_extension("ways.tmp");
    let mut output = BufWriter::new(File::create(out).with_context(|| format!("creating {}", out.display()))?);
    let mut ways_out = BufWriter::new(File::create(&ways_path)?);
    let header = HeaderBlock {
        bbox: None,
        required_features: vec!["OsmSchema-V0.6".into(), "DenseNodes".into()],
        optional_features: vec![SORTED.into()],
        writingprogram: Some("cardinal topo_pipeline".into()),
    };
    pbf::write_blob(&mut output, "OSMHeader", &header.encode_to_vec())?;

    let mut lines = 0;
    let work: Vec<_> = files.iter().zip(&starts).collect();
    for batch in work.chunks(rayon::current_num_threads() * 8) {
        let encoded = batch
            .par_iter()
            .map(|&(file, &(node, way))| encode_chunk(&file.1, node, way))
            .collect::<Result<Vec<_>>>()?;
        for (nodes, ways, count) in encoded {
            output.write_all(&nodes)?;
            ways_out.write_all(&ways)?;
            lines += count;
        }
    }
    ways_out.flush()?;
    drop(ways_out);
    std::io::copy(&mut File::open(&ways_path)?, &mut output)?;
    output.flush()?;
    fs::remove_file(&ways_path)?;
    Ok(Totals {
        lines,
        nodes: (node_id - node_start) as u64,
        ways: (way_id - way_start) as u64,
    })
}

/// The first blob of a sorted file whose kind satisfies `past`, by bisection. Blocks can't
/// mix kinds in a way that matters here, as a sorted file has them grouped.
fn bisect(
    file: &mut File,
    blobs: &[pbf::BlobRef],
    past: impl Fn(BlockKind) -> bool,
) -> Result<usize> {
    let (mut lo, mut hi) = (1, blobs.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let kind = pbf::block_kind(&pbf::read_blob(file, &blobs[mid])?)?;
        if past(kind) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(lo)
}

fn copy_range(file: &mut File, blobs: &[pbf::BlobRef], out: &mut impl Write) -> Result<()> {
    let (Some(first), Some(last)) = (blobs.first(), blobs.last()) else {
        return Ok(());
    };
    file.seek(SeekFrom::Start(first.offset))?;
    let len = last.offset + last.len - first.offset;
    let copied = std::io::copy(&mut (&mut *file).take(len), out)?;
    if copied != len {
        bail!("file shrank while copying");
    }
    Ok(())
}

fn max_id(file: &mut File, blobs: &[pbf::BlobRef], range: std::ops::Range<usize>) -> Result<Option<i64>> {
    if range.is_empty() {
        return Ok(None);
    }
    Ok(match pbf::block_kind(&pbf::read_blob(file, &blobs[range.end - 1])?)? {
        BlockKind::Nodes(id) | BlockKind::Ways(id) | BlockKind::Relations(id) => Some(id),
        BlockKind::Empty => None,
    })
}

/// (first way blob, first relation blob) of a sorted file, after checking its header.
fn sections(file: &mut File, blobs: &[pbf::BlobRef], name: &Path) -> Result<(usize, usize)> {
    let Some(first) = blobs.first().filter(|b| b.is_header) else {
        bail!("{}: doesn't start with a header", name.display());
    };
    let header = HeaderBlock::decode(&pbf::read_blob(file, first)?[..])?;
    if !header.optional_features.iter().any(|f| f == SORTED)
        && !header.required_features.iter().any(|f| f == SORTED)
    {
        bail!("{}: not marked {SORTED}; sort it first, e.g. osmium sort", name.display());
    }
    let ways = bisect(file, blobs, |k| matches!(k, BlockKind::Ways(_) | BlockKind::Relations(_)))?;
    let relations = bisect(file, blobs, |k| matches!(k, BlockKind::Relations(_)))?;
    Ok((ways, relations))
}

/// Writes `planet` with `contours` spliced in, nothing of it decoded or rewritten.
pub fn append(planet: &Path, contours: &Path, out: &Path) -> Result<()> {
    let mut planet_file = File::open(planet).with_context(|| format!("opening {}", planet.display()))?;
    let mut contour_file = File::open(contours).with_context(|| format!("opening {}", contours.display()))?;
    let planet_blobs = pbf::scan_blobs(&mut planet_file)?;
    let contour_blobs = pbf::scan_blobs(&mut contour_file)?;
    let (p_ways, p_rels) = sections(&mut planet_file, &planet_blobs, planet)?;
    let (c_ways, c_rels) = sections(&mut contour_file, &contour_blobs, contours)?;
    if c_rels != contour_blobs.len() {
        bail!("{}: has relations; expected nodes and ways only", contours.display());
    }

    let first_id = |file: &mut File, blob: Option<&pbf::BlobRef>| -> Result<Option<i64>> {
        let Some(blob) = blob else { return Ok(None) };
        let contents = pbf::read_blob(file, blob)?;
        let block = PrimitiveBlock::decode(&contents[..])?;
        Ok(block.primitivegroup.iter().find_map(|g| {
            g.dense.as_ref().and_then(|d| d.id.first().copied()).or(g.ways.first().map(|w| w.id))
        }))
    };
    let planet_max_node = max_id(&mut planet_file, &planet_blobs, 1..p_ways)?;
    let planet_max_way = max_id(&mut planet_file, &planet_blobs, p_ways..p_rels)?;
    let contour_first_node = first_id(&mut contour_file, contour_blobs.get(1).filter(|_| c_ways > 1))?;
    let contour_first_way = first_id(&mut contour_file, contour_blobs.get(c_ways))?;
    if let (Some(a), Some(b)) = (planet_max_node, contour_first_node) {
        if a >= b {
            bail!("the planet's node IDs reach {a}, past the contours' first {b}; raise --node-id-start");
        }
    }
    if let (Some(a), Some(b)) = (planet_max_way, contour_first_way) {
        if a >= b {
            bail!("the planet's way IDs reach {a}, past the contours' first {b}; raise --way-id-start");
        }
    }
    eprintln!(
        "append: planet {} blobs (max node {planet_max_node:?}, max way {planet_max_way:?}), contours {} blobs",
        planet_blobs.len(),
        contour_blobs.len()
    );

    let tmp = out.with_extension("tmp");
    let mut output = BufWriter::with_capacity(1 << 24, File::create(&tmp)?);
    copy_range(&mut planet_file, &planet_blobs[..p_ways], &mut output)?;
    copy_range(&mut contour_file, &contour_blobs[1..c_ways], &mut output)?;
    copy_range(&mut planet_file, &planet_blobs[p_ways..p_rels], &mut output)?;
    copy_range(&mut contour_file, &contour_blobs[c_ways..], &mut output)?;
    copy_range(&mut planet_file, &planet_blobs[p_rels..], &mut output)?;
    output.flush()?;
    drop(output);
    fs::rename(&tmp, out)?;
    Ok(())
}
