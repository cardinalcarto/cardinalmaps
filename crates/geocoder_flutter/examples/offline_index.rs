//! Builds and searches the device's offline index on the desktop, as the app does, for
//! end-to-end checks and benchmarks.
//!
//!   # Fetch a region's Who's On First places from a WOF service (with curl), in boxes:
//!   offline_index wof --url http://127.0.0.1:3000 --bbox w,s,e,n --out <dir> [--side 3]
//!
//!   # Build an index from an MBTiles file's z14 tiles (a release's, or a device basemap's
//!   # region with --region) and stored WOF responses:
//!   offline_index build --tiles <file.mbtiles> [--region <id>] --wof <dir> --out <index dir>
//!       [--heap-mb 64] [--threads 2] [--no-merge]
//!
//!   # Search it; one query per argument, or per line of --queries <file>:
//!   offline_index search --index <dir> [--tagger tagger.model] [--categories eng.amcats]
//!       [--focus lon,lat] [--lang en] [--limit 5] [--json] [--repeat n] <query>...
//!
//! `build` prints time, peak RSS, documents and the index's size; `search` prints each
//! query's time and results. The MBTiles file is opened read only.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use geocoder_flutter::build::{BuildOptions, IndexBuild, default_builders};
use geocoder_flutter::extract::TileKey;
use geocoder_flutter::search::{DeviceIndex, Query};
use geocoder_flutter::wof::{WofPlaces, boxes};
use offline_map::mbtiles::BasemapReader;

/// Counts the heap in use, to tell what the build holds from the index's mapped files and
/// what the allocator keeps.
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

// Not on Android, where the library's mimalloc is the allocator (and the heap isn't
// counted): run there through adb for the phone's numbers.
#[cfg_attr(not(target_os = "android"), global_allocator)]
#[cfg_attr(target_os = "android", allow(dead_code))]
static ALLOCATOR: Counting = Counting;

fn peak_heap_mb() -> usize {
    PEAK_HEAP.load(Ordering::Relaxed) / 1_000_000
}

fn peak_rss_mb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0)
        / 1024
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| {
                    let meta = e.metadata().ok();
                    if meta.as_ref().is_some_and(std::fs::Metadata::is_dir) {
                        dir_bytes(&e.path())
                    } else {
                        meta.map_or(0, |m| m.len())
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

struct Args(Vec<String>);

impl Args {
    fn take(&mut self, flag: &str) -> Option<String> {
        let i = self.0.iter().position(|a| a == flag)?;
        self.0.remove(i);
        Some(self.0.remove(i))
    }

    fn flag(&mut self, flag: &str) -> bool {
        let found = self.0.iter().position(|a| a == flag);
        if let Some(i) = found {
            self.0.remove(i);
        }
        found.is_some()
    }

    fn numbers<const N: usize>(&mut self, flag: &str) -> Option<[f64; N]> {
        let text = self.take(flag)?;
        let values: Vec<f64> = text
            .split(',')
            .map(|v| v.trim().parse().expect("a number"))
            .collect();
        Some(values.try_into().expect("the right count of numbers"))
    }
}

fn main() {
    let mut args = Args(std::env::args().skip(1).collect());
    if args.0.is_empty() {
        eprintln!("usage: offline_index wof|build|search ... (see the source)");
        std::process::exit(2);
    }
    let command = args.0.remove(0);
    match command.as_str() {
        "wof" => wof(args),
        "build" => build(args),
        "search" => search(args),
        other => panic!("unknown command {other}"),
    }
}

fn wof(mut args: Args) {
    let url = args.take("--url").expect("--url");
    let bbox = args.numbers::<4>("--bbox").expect("--bbox w,s,e,n");
    let out = PathBuf::from(args.take("--out").expect("--out"));
    let side: f64 = args.take("--side").map_or(3.0, |s| s.parse().unwrap());
    std::fs::create_dir_all(&out).unwrap();
    let start = Instant::now();
    let mut total = 0;
    for (i, [w, s, e, n]) in boxes(bbox, side).into_iter().enumerate() {
        let file = out.join(format!("{i}.json"));
        let status = Command::new("curl")
            .args(["-sSf", "--compressed", "-o"])
            .arg(&file)
            .arg(format!("{url}/v1/places?bbox={w},{s},{e},{n}"))
            .status()
            .expect("running curl");
        assert!(status.success(), "fetching box {i}");
        let bytes = std::fs::metadata(&file).unwrap().len();
        total += bytes;
        println!(
            "box {i} [{w:.3},{s:.3},{e:.3},{n:.3}]: {:.1} MB",
            bytes as f64 / 1e6
        );
    }
    println!(
        "{:.1} MB in {:.1} s",
        total as f64 / 1e6,
        start.elapsed().as_secs_f64()
    );
}

fn build(mut args: Args) {
    let tiles = PathBuf::from(args.take("--tiles").expect("--tiles"));
    let region: Option<i64> = args.take("--region").map(|r| r.parse().unwrap());
    let wof_dir = PathBuf::from(args.take("--wof").expect("--wof"));
    let out = PathBuf::from(args.take("--out").expect("--out"));
    let heap_mb: usize = args.take("--heap-mb").map_or(64, |v| v.parse().unwrap());
    let threads: usize = args.take("--threads").map_or(2, |v| v.parse().unwrap());
    let builders: usize = args
        .take("--builders")
        .map_or_else(|| default_builders(threads), |v| v.parse().unwrap());
    let merge = !args.flag("--no-merge");
    assert!(!out.exists(), "{} exists", out.display());

    let start = Instant::now();
    let mut wof = WofPlaces::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&wof_dir)
        .unwrap()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    for file in &files {
        wof.add_response(&std::fs::read(file).unwrap()).unwrap();
    }
    let (known, indexed) = wof.len();
    println!(
        "wof: {} responses, {known} places, {indexed} indexed, {:.1} s",
        files.len(),
        start.elapsed().as_secs_f64()
    );

    let options = BuildOptions {
        heap_bytes: heap_mb * 1_000_000,
        threads,
        builders,
        merge,
    };
    let mut build = IndexBuild::new(&out, wof, options).unwrap();
    let reader = BasemapReader::open(&tiles).expect("opening the MBTiles");
    let mut after = None;
    loop {
        let page = match region {
            Some(region) => reader.region_tiles(region, 14, after, 256),
            None => reader.zoom_tiles(14, after, 256),
        }
        .expect("reading tiles");
        let Some(last) = page.last() else { break };
        after = Some((last.x, last.y));
        for t in page {
            let key = TileKey {
                z: t.z,
                x: t.x,
                y: t.y,
            };
            build.add_tile(key, &t.data).unwrap();
        }
    }
    let read = start.elapsed();
    let read_rss = peak_rss_mb();
    let read_heap = peak_heap_mb();
    let counts = build.finish().unwrap();
    let total = start.elapsed();
    println!(
        "tiles: {} ({:.1} MB gzipped, {} repeated); {:.1} s reading and indexing, {:.1} s in all \
         (joining, areas, postcodes, commit{}); peak RSS {read_rss} MB while reading, {} MB in all; \
         peak heap {read_heap} MB while reading, {} MB in all",
        counts.tiles,
        counts.bytes as f64 / 1e6,
        counts.repeated_tiles,
        read.as_secs_f64(),
        total.as_secs_f64(),
        if merge { ", merge" } else { "" },
        peak_rss_mb(),
        peak_heap_mb(),
    );
    let d = counts.documents;
    println!(
        "features: {} ({} not indexed, {} duplicate addresses dropped); documents: {} \
         (poi {}, address {}, admin {}, street {}, postcode {}; {} pointers, {} local IDs)",
        counts.features,
        counts.unindexed,
        counts.duplicate_addresses,
        d.iter().sum::<u64>(),
        d[0],
        d[1],
        d[2],
        d[3],
        d[4],
        counts.pointers,
        counts.local_ids,
    );
    println!(
        "index: {:.1} MB at {}",
        dir_bytes(&out) as f64 / 1e6,
        out.display()
    );
}

fn search(mut args: Args) {
    let dir = PathBuf::from(args.take("--index").expect("--index"));
    let tagger = args.take("--tagger").map(PathBuf::from);
    let categories = args.take("--categories").map(PathBuf::from);
    let focus = args.numbers::<2>("--focus");
    let lang = args.take("--lang");
    let limit: usize = args.take("--limit").map_or(5, |v| v.parse().unwrap());
    let repeat: usize = args.take("--repeat").map_or(1, |v| v.parse().unwrap());
    let json = args.flag("--json");
    let file = args.take("--queries");
    let mut queries = std::mem::take(&mut args.0);
    if let Some(file) = file {
        queries.extend(
            std::fs::read_to_string(file)
                .unwrap()
                .lines()
                .map(str::to_owned),
        );
    }

    let start = Instant::now();
    let index = DeviceIndex::open(&dir, tagger.as_deref(), categories.as_deref()).unwrap();
    println!(
        "opened {} documents in {:.0} ms (tagger {}, categories {:?}); peak RSS {} MB",
        index.documents().unwrap(),
        start.elapsed().as_secs_f64() * 1000.0,
        tagger.is_some(),
        index.category_language(),
        peak_rss_mb(),
    );
    for text in queries.iter().filter(|q| !q.trim().is_empty()) {
        let query = Query {
            text: text.clone(),
            lang: lang.clone(),
            prefix: false,
            typos: false,
            focus,
            viewport: None,
            limit,
        };
        let mut times = Vec::with_capacity(repeat);
        let mut results = Vec::new();
        for _ in 0..repeat {
            let t = Instant::now();
            results = index.search(&query).unwrap();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "\n{text:?}: {} results, {:.1} ms (median of {repeat}; first {:.1} ms)",
            results.len(),
            times[times.len() / 2],
            times.first().unwrap(),
        );
        for r in &results {
            if json {
                println!("  {r}");
                continue;
            }
            let address = &r["address"];
            let address = if address.is_null() {
                String::new()
            } else {
                format!(
                    " [{} {}{}]",
                    address["housenumber"].as_str().unwrap_or(""),
                    address["street"].as_str().unwrap_or(""),
                    address["postcode"]
                        .as_str()
                        .map(|p| format!(" {p}"))
                        .unwrap_or_default()
                )
            };
            let admins: Vec<&str> = r["admins"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|a| a["name"].as_str())
                        .take(2)
                        .collect()
                })
                .unwrap_or_default();
            println!(
                "  {:<8} {:<26} {}{} ({}){}",
                r["kind"].as_str().unwrap_or(""),
                r["id"].as_str().unwrap_or(""),
                r["name"].as_str().unwrap_or("-"),
                address,
                admins.join(", "),
                if r["pointer"].is_null() {
                    ""
                } else {
                    " *pointer"
                },
            );
        }
    }
    println!("\npeak RSS {} MB", peak_rss_mb());
}
