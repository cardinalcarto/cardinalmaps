//! Replays what the app does with an offline area, on the desktop, and reports memory after
//! each step: street routes through [`OfflineRouter`], then transit plans in each mode through
//! [`OfflineTransit`], with real-time messages as the app hands them in.
//!
//! ```text
//! cargo run --release --example device_memory -- \
//!     --area DIR --rt DIR[,DIR...] [--tiles PATH] [--out DIR] [--threads N] [--rounds N] \
//!     [--stop-after STEP]
//! ```
//!
//! - `--area` is an offline area as the app reads it (`tiles.tar`, `timetables/`, `transfers/`);
//!   its first transit open writes `prepared/`, so point it at a copy.
//! - `--tiles` opens other tiles than the area's `tiles.tar`: a tarball or a tile directory
//!   (as the app keeps its tiles loose); put it under `--area` so its pages are reported.
//! - `--rt` names real-time captures, each a directory of `<index>-<feed>.pb` messages and an
//!   `index.tsv` of `index, feed, url` (as the area's `realtime.json` lists them).
//!   The first plan gets every message of the first capture, as the app's first plan waits
//!   for its first fetch; each later capture goes to one later plan, as the app's fetches do.
//!   Plans run at the capture's time, so their answers are the same on every run.
//! - `--out` writes every request's answer there, as `<step>.json`, to compare runs.
//! - Calls run one at a time on a pool of `--threads` threads (default 8),
//!   as flutter_rust_bridge's pool runs the app's awaited calls.
//! - `--rounds` repeats the street routes and plans (default 2), to see where memory levels off.
//! - `--stop-after` exits right after the named step, without dropping anything,
//!   so a heap profiler's at-exit view shows what that step left alive.
//!
//! Memory is reported after each step as:
//! live heap (this binary's allocator counts every byte asked for and not yet freed);
//! glibc's mapped heap (`mallinfo2`: what the allocator holds from the system, freed or not);
//! the process's resident anonymous memory, PSS, and file-backed PSS (`/proc/self/smaps_rollup`);
//! and resident pages of the area's files (`/proc/self/smaps`).
//! So it runs on Linux with glibc, which has `mallinfo2`; under another allocator
//! (`LD_PRELOAD` of scudo, as Android uses) the mapped column is that allocator's `mallinfo2`.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::too_many_lines,
    reason = "a reporting tool"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use routing_flutter::api::directions::{DirectionsAdapter, DirectionsPoint, DistanceUnits};
use routing_flutter::api::router::OfflineRouter;
use routing_flutter::api::transit::{OfflineTransit, TransitFeed, TransitFeedKind};

/// The system allocator, counting the bytes live and the most ever live.
struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

impl Counting {
    fn add(size: usize) {
        let now = LIVE.fetch_add(size as isize, Ordering::Relaxed) + size as isize;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }
    fn sub(size: usize) {
        LIVE.fetch_sub(size as isize, Ordering::Relaxed);
    }
}

// SAFETY: every call goes straight to `System`, with the same arguments.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is `System`'s.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            Self::add(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is `System`'s.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            Self::add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.dealloc(ptr, layout) };
        Self::sub(layout.size());
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller's contract is `System`'s.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            Self::sub(layout.size());
            Self::add(new_size);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// glibc's `struct mallinfo2`.
#[repr(C)]
#[derive(Default)]
struct MallInfo2 {
    arena: usize,
    ordblks: usize,
    smblks: usize,
    hblks: usize,
    hblkhd: usize,
    usmblks: usize,
    fsmblks: usize,
    uordblks: usize,
    fordblks: usize,
    keepcost: usize,
}

unsafe extern "C" {
    fn mallinfo2() -> MallInfo2;
}

const MIB: f64 = 1024.0 * 1024.0;

fn mib(bytes: isize) -> f64 {
    bytes as f64 / MIB
}

/// Fields of `/proc/self/smaps_rollup`, in bytes.
fn rollup() -> BTreeMap<String, isize> {
    let text = std::fs::read_to_string("/proc/self/smaps_rollup").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            let kb: isize = rest.trim().strip_suffix(" kB")?.trim().parse().ok()?;
            Some((key.to_owned(), kb * 1024))
        })
        .collect()
}

/// Resident bytes of mappings whose path contains `root`, grouped by what they are.
fn file_rss(root: &Path) -> BTreeMap<&'static str, isize> {
    let text = std::fs::read_to_string("/proc/self/smaps").unwrap_or_default();
    let root = root.to_string_lossy();
    let mut out = BTreeMap::new();
    let mut current: Option<&'static str> = None;
    for line in text.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.contains('-') && !first.ends_with(':') {
            // A mapping's header: `start-end perms offset dev inode path`.
            let path = line.split_whitespace().nth(5).unwrap_or("");
            current = path.contains(&*root).then(|| {
                if path.ends_with("tiles.tar") {
                    "tiles.tar"
                } else if path.ends_with(".gph") {
                    "tile files"
                } else if path.contains("/timetables/") {
                    "timetables"
                } else if path.contains("/prepared/") {
                    "prepared"
                } else if path.contains("/transfers/") {
                    "transfers"
                } else {
                    "other"
                }
            });
        } else if let Some(group) = current
            && let Some(rest) = line.strip_prefix("Rss:")
            && let Some(kb) = rest.trim().strip_suffix(" kB")
            && let Ok(kb) = kb.trim().parse::<isize>()
        {
            *out.entry(group).or_insert(0) += kb * 1024;
        }
    }
    out
}

/// One line of the report: memory now, after `step`.
fn report(step: &str, area: &Path, millis: f64) {
    let live = LIVE.load(Ordering::Relaxed);
    let peak = PEAK.load(Ordering::Relaxed);
    // SAFETY: `mallinfo2` takes nothing and only reads the allocator's statistics.
    let info = unsafe { mallinfo2() };
    let rollup = rollup();
    let get = |key: &str| rollup.get(key).copied().unwrap_or(0);
    let files = file_rss(area);
    let files: Vec<String> = files
        .iter()
        .map(|(k, v)| format!("{k}={:.1}", mib(*v)))
        .collect();
    println!(
        "{step:<44} {millis:>8.0} ms  live {:>7.1}  peak {:>7.1}  mapped {:>7.1}  \
         anon {:>7.1}  rss {:>7.1}  pss {:>7.1}  pss_file {:>7.1}  [{}]",
        mib(live),
        mib(peak),
        mib((info.arena + info.hblkhd) as isize),
        mib(get("Anonymous")),
        mib(get("Rss")),
        mib(get("Pss")),
        mib(get("Pss_File")),
        files.join(" "),
    );
}

/// Calls run one at a time, each on whichever pool thread takes it,
/// as flutter_rust_bridge's `threadpool` runs the app's awaited calls.
type Job = Box<dyn FnOnce() + Send>;

struct Pool {
    jobs: Option<Sender<Job>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Pool {
    fn new(threads: usize) -> Self {
        let (jobs, receiver) = channel::<Job>();
        let receiver: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(receiver));
        let threads = (0..threads)
            .map(|_| {
                let receiver = Arc::clone(&receiver);
                std::thread::spawn(move || {
                    loop {
                        let job = match receiver.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
            })
            .collect();
        Self {
            jobs: Some(jobs),
            threads,
        }
    }

    /// Runs `job` on a pool thread and waits for its result.
    fn run<T: Send + 'static>(&self, job: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, result) = channel();
        let sent = self.jobs.as_ref().map(|jobs| {
            jobs.send(Box::new(move || {
                let _ = done.send(job());
            }))
        });
        assert!(matches!(sent, Some(Ok(()))), "the pool is running");
        result.recv().expect("the job ran")
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.jobs = None;
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// A trip the app was tried on, as `(name, from, to)` with points as `(lat, lng)`.
type Trip = (&'static str, (f64, f64), (f64, f64));

const TRIPS: [Trip; 3] = [
    (
        "fremont-central-district",
        (47.649_735, -122.349_869),
        (47.613_442, -122.302_605),
    ),
    (
        "ballard-seatac",
        (47.668_606, -122.385_345),
        (47.441_296, -122.298_523),
    ),
    (
        "eastlake-boren-virginia",
        (47.640_300, -122.325_700),
        (47.616_900, -122.334_900),
    ),
];

/// The street modes, as the app's chips name their costings.
const COSTINGS: [&str; 3] = ["auto", "pedestrian", "bicycle"];

/// The transit modes, as the app's access chips send them.
const ACCESS: [&str; 3] = ["walking", "cycling", "wheelchair"];

/// What a real-time message carries, read from its URL as the app reads it
/// (`RealtimeSource.kind`): the file name or `feed_type`, never the rest of the URL.
fn kind_of(url: &str) -> TransitFeedKind {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let name = path.rsplit('/').next().unwrap_or("");
    let feed_type = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("feed_type="))
        .unwrap_or("");
    let text = format!("{name} {feed_type}").to_lowercase();
    let kinds = [
        text.contains("alert"),
        text.contains("vehicle"),
        text.contains("trip"),
    ];
    match kinds {
        [true, false, false] => TransitFeedKind::Alerts,
        [false, true, false] => TransitFeedKind::VehiclePositions,
        [false, false, true] => TransitFeedKind::TripUpdates,
        _ => TransitFeedKind::Combined,
    }
}

/// One capture's messages, and when it was made (POSIX seconds, from its directory name).
fn capture(dir: &Path) -> Result<(i64, Vec<TransitFeed>), Box<dyn Error>> {
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("a capture directory is named for its time")?;
    let at = utc_seconds(name).ok_or("a capture directory is named YYYYMMDDTHHMMSSZ")?;
    let index = std::fs::read_to_string(dir.join("index.tsv"))?;
    let mut feeds = Vec::new();
    for line in index.lines() {
        let mut fields = line.split('\t');
        let (Some(i), Some(feed), Some(url)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let path = dir.join(format!("{i}-{feed}.pb"));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        feeds.push(TransitFeed {
            feed: feed.to_owned(),
            kind: kind_of(url),
            bytes: Some(bytes),
        });
    }
    Ok((at, feeds))
}

/// POSIX seconds of a `YYYYMMDDTHHMMSSZ` time.
fn utc_seconds(name: &str) -> Option<i64> {
    let digits = |range: core::ops::Range<usize>| name.get(range)?.parse::<i64>().ok();
    if name.len() != 16 || name.as_bytes()[8] != b'T' || !name.ends_with('Z') {
        return None;
    }
    let (y, m, d) = (digits(0..4)?, digits(4..6)?, digits(6..8)?);
    let (h, min, s) = (digits(9..11)?, digits(11..13)?, digits(13..15)?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + min * 60 + s)
}

fn flag(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == format!("--{name}") {
            return args.next();
        }
    }
    None
}

static STEPS: AtomicUsize = AtomicUsize::new(0);

fn main() -> Result<(), Box<dyn Error>> {
    let area = PathBuf::from(flag("area").ok_or("--area DIR")?);
    let captures: Vec<PathBuf> = flag("rt")
        .ok_or("--rt DIR[,DIR...]")?
        .split(',')
        .map(PathBuf::from)
        .collect();
    let out = flag("out").map(PathBuf::from);
    if let Some(out) = &out {
        std::fs::create_dir_all(out)?;
    }
    let threads: usize = flag("threads").map_or(Ok(8), |t| t.parse())?;
    let rounds: usize = flag("rounds").map_or(Ok(2), |t| t.parse())?;
    let stop_after = flag("stop-after");
    let pool = Pool::new(threads);

    let finish = |step: &str, started: Instant, body: Option<&[u8]>| {
        let millis = started.elapsed().as_secs_f64() * 1000.0;
        let n = STEPS.fetch_add(1, Ordering::Relaxed);
        let step = format!("{n:02}-{step}");
        if let (Some(out), Some(body)) = (&out, body) {
            let _ = std::fs::write(out.join(format!("{step}.json")), body);
        }
        report(&step, &area, millis);
        if stop_after.as_deref() == Some(step.as_str()) {
            // Leave everything alive for the profiler's at-exit view.
            std::process::exit(0);
        }
    };

    finish("launch", Instant::now(), None);

    let tiles =
        flag("tiles").unwrap_or_else(|| area.join("tiles.tar").to_string_lossy().into_owned());
    let started = Instant::now();
    let router = {
        let tiles = tiles.clone();
        pool.run(move || OfflineRouter::open(vec![tiles], None))
            .map_err(|e| e.message)?
    };
    let router = Arc::new(router);
    finish("router-open", started, None);

    // Ferrostar's adapters, one per mode, kept as the app keeps them.
    let adapters: Vec<Arc<DirectionsAdapter>> = COSTINGS
        .iter()
        .map(|costing| {
            // Valhalla's defaults, so the answers are as before.
            DirectionsAdapter::valhalla(
                "offline:route".into(),
                (*costing).into(),
                3,
                "en-US".into(),
                DistanceUnits::Kilometers,
                false,
            )
            .map(Arc::new)
            .map_err(|e| e.message)
        })
        .collect::<Result<_, _>>()?;

    let street_round = |round: usize| {
        for (name, from, to) in TRIPS {
            for (costing, adapter) in COSTINGS.iter().zip(&adapters) {
                let request = adapter
                    .request(
                        DirectionsPoint {
                            lat: from.0,
                            lng: from.1,
                        },
                        DirectionsPoint {
                            lat: to.0,
                            lng: to.1,
                        },
                    )
                    .expect("Ferrostar makes a request");
                let started = Instant::now();
                let router = Arc::clone(&router);
                let response = pool.run(move || router.route(request.body));
                // The app hands the answer back to Rust for Ferrostar to read, off its UI thread.
                let adapter = Arc::clone(adapter);
                let body = response.body.clone();
                let routes = pool.run(move || adapter.parse(body).map(|r| r.len()));
                eprintln!(
                    "  street {name} {costing}: status {} routes {:?}",
                    response.status,
                    routes.map_err(|e| e.message)
                );
                finish(
                    &format!("r{round}-street-{name}-{costing}"),
                    started,
                    Some(&response.body),
                );
            }
        }
    };

    street_round(0);

    // The app's offline area loads the feeds whose timetables it holds, sorted by name.
    let mut feeds: Vec<String> = std::fs::read_dir(area.join("timetables"))?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            Some(name.strip_suffix(".timetable")?.to_owned())
        })
        .collect();
    feeds.sort();
    let started = Instant::now();
    let transit = {
        let area = area.to_string_lossy().into_owned();
        pool.run(move || OfflineTransit::open(area, Some(feeds), vec![tiles], None))
            .map_err(|e| e.message)?
    };
    let transit = Arc::new(transit);
    finish("transit-open", started, None);

    let mut captures = captures
        .iter()
        .map(|dir| capture(dir))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let (mut now, first) = captures.next().ok_or("at least one capture")?;
    let mut pending = Some(first);

    for round in 0..rounds {
        if round > 0 {
            street_round(round);
        }
        for (name, from, to) in TRIPS {
            for mode in ACCESS {
                // Each plan gets what was fetched since the last one.
                let feeds = match pending.take() {
                    Some(feeds) => feeds,
                    None => match captures.next() {
                        Some((at, feeds)) => {
                            now = at;
                            feeds
                        }
                        None => Vec::new(),
                    },
                };
                let sent = feeds.len();
                let request = serde_json::to_vec(&serde_json::json!({
                    "origin": {"lat": from.0, "lon": from.1},
                    "destination": {"lat": to.0, "lon": to.1},
                    "mode": mode,
                }))?;
                let started = Instant::now();
                let transit = Arc::clone(&transit);
                let response = pool.run(move || transit.plan_at(&request, feeds, now, &|| false));
                eprintln!(
                    "  transit {name} {mode}: status {} ({} bytes, {sent} messages)",
                    response.status,
                    response.body.len()
                );
                finish(
                    &format!("r{round}-transit-{name}-{mode}"),
                    started,
                    Some(&response.body),
                );
            }
        }
    }
    drop(pool);
    Ok(())
}
