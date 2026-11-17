//! Extracts the offline index's features from an MBTiles file's z14 tiles, on the desktop, and
//! prints what it found.
//!
//!   cargo run --release -p geocoder_flutter --example extract_stats -- <file.mbtiles>
//!       [--region <id>] [--tiles <n>] [--show <n>]
//!
//! With `--region`, only that region's tiles (a device basemap's `tile_regions`); otherwise
//! every z14 tile, so a release's MBTiles works too. `--tiles` stops after that many tiles, and
//! `--show` prints that many features of each layer. The file is opened read only.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use geocoder_flutter::extract::{ExtractedFeature, SourceLayer, TileKey};
use geocoder_flutter::region::{CountingSink, FeatureSink, RegionExtractor};
use offline_map::mbtiles::BasemapReader;

/// Counts, and keeps a few features of each layer to show.
struct Sink {
    counts: CountingSink,
    show: usize,
    shown: BTreeMap<SourceLayer, Vec<ExtractedFeature>>,
    per_layer: BTreeMap<SourceLayer, u64>,
    names: u64,
}

impl FeatureSink for Sink {
    fn feature(&mut self, feature: ExtractedFeature) {
        *self.per_layer.entry(feature.layer).or_default() += 1;
        self.names += feature.names.len() as u64;
        let shown = self.shown.entry(feature.layer).or_default();
        if shown.len() < self.show && !feature.names.is_empty() {
            shown.push(feature.clone());
        }
        self.counts.feature(feature);
    }
}

/// Counts the heap in use, to tell what the extraction holds from what the allocator keeps.
struct Counting;

static HEAP: AtomicUsize = AtomicUsize::new(0);
static PEAK_HEAP: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let now = HEAP.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK_HEAP.fetch_max(now, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn heap_mb() -> (usize, usize) {
    (
        HEAP.load(Ordering::Relaxed) >> 20,
        PEAK_HEAP.load(Ordering::Relaxed) >> 20,
    )
}

fn peak_rss_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let (mut region, mut max_tiles, mut show) = (None, u64::MAX, 0);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .expect("a value")
                .parse::<u64>()
                .expect("a number")
        };
        match arg.as_str() {
            "--region" => region = Some(value() as i64),
            "--tiles" => max_tiles = value(),
            "--show" => show = value() as usize,
            _ => path = Some(PathBuf::from(arg)),
        }
    }
    let path = path.expect("usage: extract_stats <file.mbtiles> [--region id] [--tiles n]");
    let reader = BasemapReader::open(&path).expect("opening the MBTiles");
    let mut extractor = RegionExtractor::new();
    let mut sink = Sink {
        counts: CountingSink::default(),
        show,
        shown: BTreeMap::new(),
        per_layer: BTreeMap::new(),
        names: 0,
    };
    let start = Instant::now();
    let mut after = None;
    let mut read = 0u64;
    'pages: loop {
        let page = match region {
            Some(region) => reader.region_tiles(region, 14, after, 256),
            None => reader.zoom_tiles(14, after, 256),
        }
        .expect("reading tiles");
        let Some(last) = page.last() else { break };
        after = Some((last.x, last.y));
        for t in page {
            if read == max_tiles {
                break 'pages;
            }
            read += 1;
            let key = TileKey {
                z: t.z,
                x: t.x,
                y: t.y,
            };
            if let Err(e) = extractor.add_tile(key, &t.data, &mut sink) {
                eprintln!("{e}");
            }
        }
    }
    let read_time = start.elapsed();
    let read_rss = peak_rss_mb().unwrap_or(0);
    let read_heap = heap_mb();
    let counts = extractor.finish(&mut sink);
    let total = start.elapsed();

    println!("file: {}", path.display());
    println!(
        "tiles: {} ({:.1} MB gzipped), {:.1} s reading and extracting, {:.1} s with joining; \
         peak RSS {} MB after reading, {} MB at the end; heap {} MB after reading (peak {} MB), \
         peak {} MB",
        counts.tiles,
        counts.bytes as f64 / 1e6,
        read_time.as_secs_f64(),
        total.as_secs_f64(),
        read_rss,
        peak_rss_mb().unwrap_or(0),
        read_heap.0,
        read_heap.1,
        heap_mb().1,
    );
    println!(
        "{:<20} {:>10} {:>10} {:>10} {:>10} {:>10}",
        "layer", "features", "points", "buffer", "pieces", "made"
    );
    for (i, layer) in SourceLayer::ALL.iter().enumerate() {
        let l = &counts.layers;
        println!(
            "{:<20} {:>10} {:>10} {:>10} {:>10} {:>10}",
            layer.name(),
            l.features[i],
            l.points[i],
            l.buffer_points[i],
            l.pieces[i],
            sink.per_layer.get(layer).copied().unwrap_or(0),
        );
    }
    let m = &counts.merge;
    println!(
        "pieces joined: {} into {} features ({} unnamed dropped)",
        m.pieces, m.features, m.unnamed
    );
    let c = &sink.counts;
    println!(
        "features made: {} (with ID {}, names {}, address {}, admins {}, importance {}); {} names",
        c.features,
        c.with_id,
        c.with_names,
        c.with_address,
        c.with_admins,
        c.with_importance,
        sink.names
    );
    for (layer, features) in &sink.shown {
        for f in features {
            let names: Vec<_> = f.names.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let tags: Vec<_> = f
                .tags
                .iter()
                .take(6)
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!(
                "  {} id={:?} tiles={} names=[{}] addr={:?} admins={:?} importance={:?} tags=[{}] geometry={}",
                layer.name(),
                f.id,
                f.tiles.len(),
                names.join(", "),
                f.address,
                f.admins,
                f.importance,
                tags.join(", "),
                geometry_kind(&f.geometry),
            );
        }
    }
}

fn geometry_kind(g: &geo::Geometry<f64>) -> String {
    use geo::CoordsIter;
    let kind = match g {
        geo::Geometry::Point(_) => "point",
        geo::Geometry::MultiPoint(_) => "multipoint",
        geo::Geometry::LineString(_) => "line",
        geo::Geometry::MultiLineString(_) => "multiline",
        geo::Geometry::Polygon(_) => "polygon",
        geo::Geometry::MultiPolygon(_) => "multipolygon",
        _ => "other",
    };
    format!("{kind}({} points)", g.coords_count())
}
