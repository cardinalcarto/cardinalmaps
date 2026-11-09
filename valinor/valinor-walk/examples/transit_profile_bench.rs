//! End-to-end transit profile queries on real data: King County Metro over Washington tiles.
//!
//! Compiles a GTFS feed, materializes a three-day window around a weekday, snaps every stop,
//! and runs `Router::profile` for a 60-minute window between sampled pairs of stops
//! (stops drawn in proportion to the number of trips serving them, as in the paper),
//! spread across four distance bands and four departure periods.
//!
//! ```text
//! cargo run --release -p valinor-walk --example transit_profile_bench -- [flags]
//! ```
//!
//! Flags (all optional, defaults in parentheses):
//! `--feed PATH`, `--tiles PATH`, `--date YYYYMMDD` (20261110),
//! `--per-bin N` queries per period and distance band (4),
//! `--seed N` (1), `--max-trips N` (8), `--cold N` queries from a cold router (4),
//! `--warmup N` queries run and discarded before timing warm queries (24),
//! `--max-walk SECS` longest access or egress walk (86400),
//! `--csv PATH` one row per query, `--verbose` a line per query, `--trace` a line per street search,
//! `--walk-limit SECS` cap each transfer walk (changes results; to measure what a cap buys),
//! `--no-known-bounds`, `--no-own-seed-pruning`, `--flood-tables` turn a pruning off,
//! `--no-canary` skips the full-network table floods at the start and end
//! (they cost a second worker's memory, so use it when reading the memory lines),
//! `--compare` checks profiles with every pruning off and on, and
//! `--compare-each` does so for each pruning alone.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::struct_excessive_bools,
    reason = "a reporting tool with small numbers"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valinor_gtfs::{CompiledTimetable, FeedId, FeedInput, ZipSource, compile, parse_service_date};
use valinor_router::tiles::TileSets;
use valinor_sif::AnyCosting;
use valinor_transit::{
    Clock, DEFAULT_BOOTSTRAP_WALK_LIMIT, DaySlot, Direction, ProfileEntry, ProfileOptions,
    RouteFilter, RouteId, Router, ServiceDate, Snapshot, StopId, StopTable, Time, TransferArrival,
    TransferOutcome, TransferRequest, TransferSearch, TransferSeed, TransferSink, TransitQuery,
};
use valinor_walk::{
    AccessSide, ResolvedWalk, SnapConfig, SnapPoint, StopSnaps, StopTables, TableRequest,
    TableSeed, TableTransfers, TableWalk, TransferTable, WalkError, WalkHandle, WalkSearch,
    snap_stops,
};

/// Measures live and peak heap bytes.
struct CountingAllocator;

static ALLOCATED: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards every call to the system allocator, only adding counters.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `alloc`'s contract.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let now =
                ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed) + layout.size() as u64;
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds `dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        ALLOCATED.fetch_sub(layout.size() as u64, Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

type Provider = &'static TileSets;
type Walk = WalkSearch<Provider, AnyCosting>;

/// Counts what a transfer search does, across a whole process.
#[derive(Clone, Copy, Debug, Default)]
struct Counters {
    runs: [u64; 2],
    searches: u64,
    street_nanos: u64,
    nodes_settled: u64,
    labels_settled: u64,
    arcs_relaxed: u64,
    seeds: u64,
}

impl Counters {
    fn since(self, earlier: Self) -> Self {
        Self {
            runs: [
                self.runs[0] - earlier.runs[0],
                self.runs[1] - earlier.runs[1],
            ],
            searches: self.searches - earlier.searches,
            street_nanos: self.street_nanos - earlier.street_nanos,
            nodes_settled: self.nodes_settled - earlier.nodes_settled,
            labels_settled: self.labels_settled - earlier.labels_settled,
            arcs_relaxed: self.arcs_relaxed - earlier.arcs_relaxed,
            seeds: self.seeds - earlier.seeds,
        }
    }
}

/// A [`WalkSearch`] with timing.
/// Walk radii a precomputed table might have, in seconds.
const HORIZON_LIMITS: [i32; 4] = [300, 600, 900, 1800];

/// How much of the street work a table of each radius could replace exactly.
#[derive(Clone, Copy, Debug, Default)]
struct HorizonStats {
    searches: u64,
    nanos: u64,
    seeds: u64,
    unbounded_nanos: u64,
    /// By radius: searches whose every seed could use only the table, and their time.
    whole_searches: [u64; 4],
    whole_nanos: [u64; 4],
    /// By radius: seeds with no useful walk longer than the radius.
    seeds_within: [u64; 4],
}

impl HorizonStats {
    fn add(&mut self, request: &TransferRequest<'_>, nanos: u64) {
        self.searches += 1;
        self.nanos += nanos;
        self.seeds += request.seeds.len() as u64;
        let bound = request.upper_bound.seconds();
        if bound >= (1 << 30) {
            self.unbounded_nanos += nanos;
            return;
        }
        for (index, limit) in HORIZON_LIMITS.iter().enumerate() {
            let within = request
                .seeds
                .iter()
                .filter(|seed| bound - seed.clock.seconds() <= *limit)
                .count();
            self.seeds_within[index] += within as u64;
            if within == request.seeds.len() {
                self.whole_searches[index] += 1;
                self.whole_nanos[index] += nanos;
            }
        }
    }

    fn print(&self, label: &str) {
        let percent = |part: u64, whole: u64| 100.0 * part as f64 / whole.max(1) as f64;
        println!(
            "{label}: {} searches, {:.0} ms street, {:.1}% of the time in searches with no bound",
            self.searches,
            self.nanos as f64 / 1e6,
            percent(self.unbounded_nanos, self.nanos),
        );
        println!("  radius  whole searches  their street time  seeds within radius");
        for (index, limit) in HORIZON_LIMITS.iter().enumerate() {
            println!(
                "  {:>3} min {:>13.1}% {:>17.1}% {:>19.1}%",
                limit / 60,
                percent(self.whole_searches[index], self.searches),
                percent(self.whole_nanos[index], self.nanos),
                percent(self.seeds_within[index], self.seeds),
            );
        }
    }
}

/// Discards what a shadow search reports; only its cost matters.
struct Discard;

impl TransferSink<WalkHandle> for Discard {
    fn reach(&mut self, arrival: TransferArrival<WalkHandle>) {
        core::hint::black_box(arrival.clock);
    }
}

/// Stop-to-stop walks within a radius: `(other stop, seconds)` by stop.
struct WalkTable {
    out: Vec<Vec<(u32, i32)>>,
    into: Vec<Vec<(u32, i32)>>,
}

/// What a precomputed table would cost, timed beside the real search so the results stay real.
struct Shadow {
    worker: Box<Worker>,
    table: Arc<WalkTable>,
    radius: i32,
    /// Every seed uses only the table, as if transfers were capped at the radius.
    table_only: bool,
    best: Vec<i32>,
    long: Vec<TransferSeed>,
    nanos: u64,
}

impl Shadow {
    fn run(&mut self, request: &TransferRequest<'_>) -> Result<(), WalkError> {
        let started = Instant::now();
        let table = match request.direction {
            Direction::Forward => &self.table.out,
            Direction::Backward => &self.table.into,
        };
        let bound = request.upper_bound.seconds();
        self.best.fill(i32::MAX);
        self.long.clear();
        for seed in request.seeds {
            if !self.table_only
                && i64::from(bound) - i64::from(seed.clock.seconds()) > i64::from(self.radius)
            {
                self.long.push(*seed);
                continue;
            }
            for &(stop, seconds) in &table[seed.stop.into_inner() as usize] {
                let clock = seed.clock.seconds().saturating_add(seconds);
                let slot = &mut self.best[stop as usize];
                if clock < bound && clock < *slot {
                    *slot = clock;
                }
            }
        }
        core::hint::black_box(&self.best);
        if !self.long.is_empty() {
            self.worker.router.transfers_mut().inner.search(
                &TransferRequest {
                    direction: request.direction,
                    seeds: &self.long,
                    upper_bound: request.upper_bound,
                    change_margin: request.change_margin,
                    interrupt: request.interrupt,
                },
                &mut Discard,
            )?;
        }
        self.nanos += started.elapsed().as_nanos() as u64;
        Ok(())
    }
}

/// Collects a one-stop search's arrivals for the walk table.
struct Collect(Vec<(u32, i32)>);

impl TransferSink<WalkHandle> for Collect {
    fn reach(&mut self, arrival: TransferArrival<WalkHandle>) {
        self.0
            .push((arrival.stop.into_inner(), arrival.clock.seconds()));
    }
}

/// Walks from every stop to the stops within `radius` seconds.
fn build_walk_table(
    search: &mut Walk,
    stops: usize,
    radius: i32,
) -> Result<WalkTable, Box<dyn Error>> {
    let mut out = Vec::with_capacity(stops);
    let mut into = vec![Vec::new(); stops];
    for stop in 0..stops as u32 {
        search.begin_run(Direction::Forward);
        let mut sink = Collect(Vec::new());
        let seeds = [TransferSeed {
            stop: StopId::new(stop),
            clock: Clock::new(0),
        }];
        search.search(
            &TransferRequest {
                direction: Direction::Forward,
                seeds: &seeds,
                upper_bound: Clock::new(radius),
                change_margin: TransferRequest::REPORT_ALL,
                interrupt: &|| false,
            },
            &mut sink,
        )?;
        let mut best: HashMap<u32, i32> = HashMap::new();
        for (other, seconds) in sink.0 {
            let slot = best.entry(other).or_insert(seconds);
            *slot = (*slot).min(seconds);
        }
        let mut walks: Vec<(u32, i32)> = best.into_iter().collect();
        walks.sort_unstable();
        for &(other, seconds) in &walks {
            into[other as usize].push((stop, seconds));
        }
        out.push(walks);
    }
    Ok(WalkTable { out, into })
}

struct Timed {
    inner: Walk,
    counters: Counters,
    horizons: HorizonStats,
    shadow: Option<Shadow>,
    /// A cap on every walk (`--walk-limit`), to measure what one buys.
    fixed_cap: Option<u32>,
    /// Print one line per street search, to see which rounds carry the work.
    trace: bool,
    direction: Direction,
    round: u32,
}

impl TransferSearch for Timed {
    type Parent = WalkHandle;
    type Walk = ResolvedWalk;
    type Error = WalkError;

    fn begin_run(&mut self, direction: Direction) {
        self.counters.runs[usize::from(direction == Direction::Backward)] += 1;
        self.direction = direction;
        self.round = 0;
        self.inner.begin_run(direction);
        if let Some(shadow) = &mut self.shadow {
            shadow
                .worker
                .router
                .transfers_mut()
                .inner
                .begin_run(direction);
        }
    }

    fn set_walk_limit(&mut self, limit: Option<u32>) {
        // A fixed cap outlives the bootstrap pass, which clears its own limit when it is done.
        self.inner.set_walk_limit(limit.or(self.fixed_cap));
        if let Some(shadow) = &mut self.shadow {
            let cap = limit.or(self.fixed_cap);
            shadow
                .worker
                .router
                .transfers_mut()
                .inner
                .set_walk_limit(cap);
        }
    }

    fn walk(&self, parent: WalkHandle) -> ResolvedWalk {
        self.inner.walk(parent)
    }

    fn search<K: TransferSink<WalkHandle>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, WalkError> {
        let started = Instant::now();
        let outcome = self.inner.search(request, sink);
        self.counters.street_nanos += started.elapsed().as_nanos() as u64;
        self.horizons
            .add(request, started.elapsed().as_nanos() as u64);
        if let Some(shadow) = &mut self.shadow {
            shadow.run(request)?;
        }
        self.counters.searches += 1;
        self.counters.seeds += request.seeds.len() as u64;
        let stats = self.inner.last_search_stats();
        self.counters.nodes_settled += stats.nodes_settled;
        self.counters.labels_settled += stats.labels_settled;
        self.counters.arcs_relaxed += stats.arcs_relaxed;
        self.round += 1;
        if self.trace {
            let first = request
                .seeds
                .iter()
                .map(|s| s.clock.seconds())
                .min()
                .unwrap_or(0);
            let last = request
                .seeds
                .iter()
                .map(|s| s.clock.seconds())
                .max()
                .unwrap_or(0);
            let bound = request.upper_bound.seconds();
            let horizon = if bound >= (1 << 30) {
                -1
            } else {
                bound - first
            };
            eprintln!(
                "trace {:?} walk {} seeds {} seed_span_s {} horizon_s {} nodes {} us {}",
                self.direction,
                self.round,
                request.seeds.len(),
                last - first,
                horizon,
                stats.nodes_settled,
                started.elapsed().as_micros(),
            );
        }
        outcome
    }
}

/// A small deterministic generator, so query sets are reproducible without a dependency.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Distance bands, in kilometers.
const BANDS: [(f64, f64, &str); 4] = [
    (0.0, 5.0, "0-5km"),
    (5.0, 15.0, "5-15km"),
    (15.0, 30.0, "15-30km"),
    (30.0, 400.0, "30+km"),
];

/// Query periods: `(name, start hour of a 60-minute departure window)`.
const PERIODS: [(&str, i32); 4] = [
    ("morning", 8),
    ("midday", 12),
    ("evening", 17),
    ("night", 23),
];

/// A trip tracked by name: its name, then where it starts and ends as `(lon, lat)`.
type NamedTrip = (&'static str, (f64, f64), (f64, f64));

/// Named trips, tracked by name across changes: `(name, from, to)`, as `(lon, lat)` near a stop.
const NAMED_TRIPS: [NamedTrip; 1] = [(
    "Fremont to Central District",
    (-122.349_869, 47.649_735),
    (-122.302_605, 47.613_442),
)];

/// How many times a named trip repeats; its timings are medians.
const NAMED_REPEATS: usize = 5;

/// One query: a pair of stops and a window.
#[derive(Clone, Copy, Debug)]
struct Sample {
    from: u32,
    to: u32,
    period: usize,
    band: usize,
    hour: i32,
}

/// One query's measurements, in milliseconds.
#[derive(Clone, Copy, Debug)]
struct Measure {
    sample: Sample,
    total_ms: f64,
    tables_ms: f64,
    street_ms: f64,
    scan_ms: f64,
    entries: usize,
    counters: Counters,
}

/// Parses `--key value` pairs.
struct Flags(HashMap<String, String>);

impl Flags {
    fn from_args() -> Self {
        let mut map = HashMap::new();
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            let Some(key) = arg.strip_prefix("--") else {
                continue;
            };
            let value = match args.peek() {
                Some(next) if !next.starts_with("--") => args.next().unwrap_or_default(),
                _ => "true".to_owned(),
            };
            map.insert(key.to_owned(), value);
        }
        Self(map)
    }

    fn string(&self, key: &str, default: &str) -> String {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    }

    fn number<T: core::str::FromStr>(&self, key: &str, default: T) -> T {
        self.0
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }

    fn present(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
}

fn costing_named(name: &str) -> Result<AnyCosting, Box<dyn Error>> {
    use valinor_sif::costing_options::{parse_costing, parse_costing_name};
    use valinor_sif::json::JsonValue;
    let doc = JsonValue::parse(b"{}").map_err(|e| format!("{e:?}"))?;
    let costing_type = parse_costing_name(name).ok_or("unknown costing")?;
    let mut proto = valhalla_proto::Costing::default();
    parse_costing(
        &doc,
        "/none",
        &mut proto,
        &mut Vec::new(),
        Some(costing_type),
    )
    .map_err(|e| format!("{e:?}"))?;
    AnyCosting::new(&proto).map_err(|e| format!("{e:?}").into())
}

fn haversine_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (lat1, lat2) = (a.1.to_radians(), b.1.to_radians());
    let dlat = lat2 - lat1;
    let dlon = (b.0 - a.0).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().min(1.0).asin()
}

fn opposing_edge<P: GraphTileProvider>(
    provider: &P,
    edge: GraphId,
) -> Result<Option<GraphId>, Box<dyn Error>> {
    let tile = provider.get_handle_for_tile_containing(edge)?;
    let directed = tile.get_directed_edge(edge).map_err(|e| format!("{e:?}"))?;
    let end = directed.end_node_id();
    let Ok(end_tile) = provider.get_handle_for_tile_containing(end) else {
        return Ok(None);
    };
    let node = end_tile.get_node(end).map_err(|e| format!("{e:?}"))?;
    let id = end
        .with_feature_index(
            u64::from(node.edge_index()) + u64::from(directed.opposing_edge_index()),
        )
        .map_err(|e| format!("{e:?}"))?;
    Ok(Some(id))
}

/// A point's seeds: its edge and the opposing edge, as a Loki correlation of a location has.
fn seeds_at(provider: Provider, point: SnapPoint) -> Result<Vec<TableSeed>, Box<dyn Error>> {
    let mut seeds = vec![TableSeed {
        edge: point.edge,
        percent_along: point.percent_along,
        initial: 0.0,
    }];
    if let Some(opposing) = opposing_edge(&provider, point.edge)? {
        seeds.push(TableSeed {
            edge: opposing,
            percent_along: 1.0 - point.percent_along,
            initial: 0.0,
        });
    }
    Ok(seeds)
}

/// Trips serving each stop on the window's middle day.
fn trip_weights(snapshot: &Snapshot) -> Vec<u64> {
    let mut weights = vec![0u64; snapshot.stop_count()];
    for route in 0..snapshot.route_count() {
        let route = RouteId::new(route as u32);
        let Some(stops) = snapshot.route_stops(route) else {
            continue;
        };
        let Some(lanes) = snapshot.block_lanes(route, DaySlot::new(1)) else {
            continue;
        };
        let trips: u64 = lanes
            .iter()
            .filter_map(|&lane| snapshot.lane_trip_count(lane))
            .map(u64::from)
            .sum();
        for stop in stops {
            weights[stop.into_inner() as usize] += trips;
        }
    }
    weights
}

/// Picks a stop in proportion to `cumulative`.
fn weighted_stop(cumulative: &[u64], rng: &mut SplitMix64) -> u32 {
    let total = *cumulative.last().unwrap_or(&1);
    let point = rng.next() % total;
    cumulative.partition_point(|&c| c <= point) as u32
}

/// Draws the queries: `per_bin` for each of the periods and distance bands.
fn sample_queries(
    per_bin: usize,
    seed: u64,
    weights: &[u64],
    points: &[Option<(f64, f64)>],
) -> Vec<Sample> {
    let mut cumulative = Vec::with_capacity(weights.len());
    let mut total = 0;
    for (weight, point) in weights.iter().zip(points) {
        if point.is_some() {
            total += *weight;
        }
        cumulative.push(total);
    }
    let mut rng = SplitMix64(seed);
    let mut out = Vec::new();
    for (period, &(_, hour)) in PERIODS.iter().enumerate() {
        let mut counts = [0usize; BANDS.len()];
        let mut attempts = 0u64;
        while counts.iter().any(|&c| c < per_bin) && attempts < 40_000_000 {
            attempts += 1;
            let from = weighted_stop(&cumulative, &mut rng);
            let to = weighted_stop(&cumulative, &mut rng);
            if from == to {
                continue;
            }
            let (Some(a), Some(b)) = (points[from as usize], points[to as usize]) else {
                continue;
            };
            let km = haversine_km(a, b);
            let Some(band) = BANDS.iter().position(|&(lo, hi, _)| km >= lo && km < hi) else {
                continue;
            };
            if counts[band] < per_bin {
                counts[band] += 1;
                out.push(Sample {
                    from,
                    to,
                    period,
                    band,
                    hour,
                });
            }
        }
    }
    out
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

fn sorted_of(measures: &[Measure], f: impl Fn(&Measure) -> f64) -> Vec<f64> {
    let mut values: Vec<f64> = measures.iter().map(f).collect();
    values.sort_by(f64::total_cmp);
    values
}

fn summarize(label: &str, measures: &[Measure]) {
    if measures.is_empty() {
        return;
    }
    let row = |name: &str, f: &dyn Fn(&Measure) -> f64| {
        let values = sorted_of(measures, |m| f(m));
        format!(
            "{name} {:>8.1} {:>8.1} {:>9.1}",
            percentile(&values, 0.5),
            percentile(&values, 0.95),
            values.last().copied().unwrap_or(f64::NAN)
        )
    };
    let n = measures.len() as f64;
    let mean = |f: &dyn Fn(&Measure) -> f64| measures.iter().map(f).sum::<f64>() / n;
    println!("{label} (n={}):", measures.len());
    println!("    metric            p50      p95       max");
    println!("  {}", row("total ms     ", &|m| m.total_ms));
    println!("  {}", row("tables ms    ", &|m| m.tables_ms));
    println!("  {}", row("street ms    ", &|m| m.street_ms));
    println!("  {}", row("scan ms      ", &|m| m.scan_ms));
    println!("  {}", row("entries      ", &|m| m.entries as f64));
    println!("  {}", row("fwd runs     ", &|m| m.counters.runs[0] as f64));
    println!("  {}", row("bwd runs     ", &|m| m.counters.runs[1] as f64));
    println!(
        "  {}",
        row("street count ", &|m| m.counters.searches as f64)
    );
    println!(
        "  {}",
        row("nodes/street ", &|m| {
            m.counters.nodes_settled as f64 / m.counters.searches.max(1) as f64
        })
    );
    println!(
        "  {}",
        row("seeds/street ", &|m| {
            m.counters.seeds as f64 / m.counters.searches.max(1) as f64
        })
    );
    println!(
        "  mean split: street {:.1}%, scan {:.1}%, tables {:.1}%",
        100.0 * mean(&|m| m.street_ms) / mean(&|m| m.total_ms),
        100.0 * mean(&|m| m.scan_ms) / mean(&|m| m.total_ms),
        100.0 * mean(&|m| m.tables_ms) / mean(&|m| m.total_ms),
    );
}

/// A line from `/proc/self/status`, in megabytes, if present.
fn proc_mb(key: &str) -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with(key))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// What one router needs to run queries.
struct Worker {
    tables: StopTables<Provider, AnyCosting>,
    router: Router<Timed>,
}

fn new_worker(
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    own_seed_pruning: bool,
    terminate_tables: bool,
) -> Result<(Worker, f64), Box<dyn Error>> {
    let started = Instant::now();
    let mut tables = StopTables::new(provider, costing.clone());
    tables.set_terminate_when_stops_settled(terminate_tables);
    tables.bind(snaps.clone())?;
    let mut search = WalkSearch::new(provider, costing.clone());
    search.set_own_seed_pruning(own_seed_pruning);
    let walk_limit: Option<u32> = std::env::args()
        .skip_while(|arg| arg != "--walk-limit")
        .nth(1)
        .and_then(|limit| limit.parse().ok());
    search.set_walk_limit(walk_limit);
    search.bind(snaps.clone())?;
    let bind_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok((
        Worker {
            tables,
            router: Router::new(Timed {
                inner: search,
                counters: Counters::default(),
                horizons: HorizonStats::default(),
                shadow: None,
                fixed_cap: walk_limit,
                trace: std::env::args().any(|arg| arg == "--trace"),
                direction: Direction::Forward,
                round: 0,
            }),
        },
        bind_ms,
    ))
}

struct Env {
    provider: Provider,
    snapshot: Arc<Snapshot>,
    day_start: i32,
    max_trips: u8,
    max_walk: u32,
    options: ProfileOptions,
}

fn run_query(worker: &mut Worker, env: &Env, sample: Sample) -> Result<Measure, Box<dyn Error>> {
    run_query_with(worker, env, sample, env.options).map(|(measure, _)| measure)
}

/// One profile's `(departure, arrival, trips)`, for comparing configurations.
type Triples = Vec<(i32, i32, u8)>;

fn run_query_with(
    worker: &mut Worker,
    env: &Env,
    sample: Sample,
    options: ProfileOptions,
) -> Result<(Measure, Triples), Box<dyn Error>> {
    let started = Instant::now();
    let before = worker.router.transfers().counters;
    let (access, egress) = walk_tables(worker, env, sample)?;
    let tables_ms = started.elapsed().as_secs_f64() * 1000.0;

    let earliest = Time::try_new(env.day_start + sample.hour * 3600)?;
    let latest = Time::try_new(env.day_start + sample.hour * 3600 + 3600)?;
    let query = TransitQuery {
        snapshot: env.snapshot.clone(),
        access: &access,
        egress: &egress,
        filter: RouteFilter::ALL,
        max_trips: env.max_trips,
    };
    let profile_started = Instant::now();
    let profile = worker
        .router
        .profile_with(&query, earliest, latest, options, &|| false)
        .map_err(|e| e.to_string())?;
    let profile_ms = profile_started.elapsed().as_secs_f64() * 1000.0;
    let counters = worker.router.transfers().counters.since(before);
    let street_ms = counters.street_nanos as f64 / 1e6;
    let triples = profile
        .entries
        .iter()
        .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
        .collect();
    let measure = Measure {
        sample,
        total_ms: tables_ms + profile_ms,
        tables_ms,
        street_ms,
        scan_ms: profile_ms - street_ms,
        entries: profile.entries.len(),
        counters,
    };
    Ok((measure, triples))
}

/// The access and egress tables of a query.
fn walk_tables(
    worker: &mut Worker,
    env: &Env,
    sample: Sample,
) -> Result<(StopTable, StopTable), Box<dyn Error>> {
    let origin = seeds_at(env.provider, worker_point(env, sample.from)?)?;
    let target = seeds_at(env.provider, worker_point(env, sample.to)?)?;
    let stop_count = env.snapshot.stop_count();
    let mut table = |seeds: &[TableSeed], side| -> Result<StopTable, Box<dyn Error>> {
        worker
            .tables
            .compute(
                stop_count,
                &TableRequest {
                    side,
                    seeds,
                    max_seconds: env.max_walk,
                    interrupt: &|| false,
                },
            )?
            .ok_or_else(|| "interrupted".into())
    };
    let access = table(&origin, AccessSide::Access)?;
    let egress = table(&target, AccessSide::Egress)?;
    Ok((access, egress))
}

/// A leave-now query, and the query for each way of missing one of its journeys.
#[derive(Clone, Debug)]
struct LeaveNow {
    tables_ms: f64,
    now_ms: f64,
    /// All the missed-it queries together; they reuse the tables.
    misses_ms: f64,
    worst_miss_ms: f64,
    options: Triples,
    /// `(departure missed, the journeys after missing it)`, latest departure first.
    next: Vec<(i32, Triples)>,
}

impl LeaveNow {
    fn first_response_ms(&self) -> f64 {
        self.tables_ms + self.now_ms
    }

    fn with_misses_ms(&self) -> f64 {
        self.first_response_ms() + self.misses_ms
    }
}

/// Times a leave-now query at the start of `sample`'s hour, then every missed-it query.
fn run_leave_now(
    worker: &mut Worker,
    env: &Env,
    sample: Sample,
) -> Result<LeaveNow, Box<dyn Error>> {
    let started = Instant::now();
    let (access, egress) = walk_tables(worker, env, sample)?;
    let tables_ms = started.elapsed().as_secs_f64() * 1000.0;
    let query = TransitQuery {
        snapshot: env.snapshot.clone(),
        access: &access,
        egress: &egress,
        filter: RouteFilter::ALL,
        max_trips: env.max_trips,
    };
    let triples_of = |entries: &[ProfileEntry<_>]| -> Triples {
        entries
            .iter()
            .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
            .collect()
    };

    let at = Time::try_new(env.day_start + sample.hour * 3600)?;
    let now_started = Instant::now();
    let now = worker
        .router
        .leave_at(&query, at, env.options, &|| false)
        .map_err(|e| e.to_string())?;
    let now_ms = now_started.elapsed().as_secs_f64() * 1000.0;
    let options = triples_of(&now.entries);

    let mut departures: Vec<i32> = options.iter().map(|&(d, _, _)| d).collect();
    departures.sort_unstable_by(|a, b| b.cmp(a));
    departures.dedup();
    let (mut misses_ms, mut worst_miss_ms) = (0.0_f64, 0.0_f64);
    let mut next = Vec::with_capacity(departures.len());
    for departure in departures {
        let miss_started = Instant::now();
        let after = worker
            .router
            .leave_at(&query, Time::try_new(departure + 1)?, env.options, &|| {
                false
            })
            .map_err(|e| e.to_string())?;
        let miss_ms = miss_started.elapsed().as_secs_f64() * 1000.0;
        misses_ms += miss_ms;
        worst_miss_ms = worst_miss_ms.max(miss_ms);
        next.push((departure, triples_of(&after.entries)));
    }
    Ok(LeaveNow {
        tables_ms,
        now_ms,
        misses_ms,
        worst_miss_ms,
        options,
        next,
    })
}

/// Prints leave-now timings over the sampled queries.
fn report_leave_now(
    worker: &mut Worker,
    env: &Env,
    samples: &[Sample],
) -> Result<(), Box<dyn Error>> {
    let mut runs = Vec::with_capacity(samples.len());
    for sample in samples {
        runs.push(run_leave_now(worker, env, *sample)?);
    }
    let without = runs.iter().filter(|run| run.options.is_empty()).count();
    let row = |name: &str, f: &dyn Fn(&LeaveNow) -> f64| {
        let mut values: Vec<f64> = runs.iter().map(f).collect();
        values.sort_by(f64::total_cmp);
        println!(
            "  {name:<26} {:>8.1} {:>8.1} {:>9.1}",
            percentile(&values, 0.5),
            percentile(&values, 0.95),
            percentile(&values, 1.0),
        );
    };
    println!(
        "leave now over {} sampled queries ({without} with no journey):",
        runs.len()
    );
    println!("  {:<26} {:>8} {:>8} {:>9}", "metric", "p50", "p95", "max");
    row("tables ms", &|run| run.tables_ms);
    row("leave now ms", &|run| run.now_ms);
    row("first response ms", &LeaveNow::first_response_ms);
    row("every missed-it query ms", &|run| run.misses_ms);
    row("slowest missed-it query ms", &|run| run.worst_miss_ms);
    row("response with misses ms", &LeaveNow::with_misses_ms);
    row("journeys", &|run| run.options.len() as f64);
    row("distinct departures", &|run| run.next.len() as f64);
    println!();
    Ok(())
}

/// A new worker with nothing memoized, and how long binding it took in milliseconds.
type FreshWorker<'a> = &'a dyn Fn() -> Result<(Worker, f64), Box<dyn Error>>;

/// Prints leave-now timings where every query gets a new worker.
fn report_cold(
    fresh: FreshWorker<'_>,
    env: &Env,
    samples: &[Sample],
    located: &[Option<(f64, f64)>],
) -> Result<(), Box<dyn Error>> {
    for (name, from, to) in NAMED_TRIPS {
        let (from, _) = nearest_stop(located, from).ok_or("no stop near the origin")?;
        let (to, _) = nearest_stop(located, to).ok_or("no stop near the destination")?;
        println!("cold, new worker per query: {name}, first run then median of {NAMED_REPEATS}");
        for (period, (period_name, hour)) in PERIODS.iter().enumerate() {
            let sample = Sample {
                from,
                to,
                period,
                band: 0,
                hour: *hour,
            };
            let (mut binds, mut runs, mut profiles) = (Vec::new(), Vec::new(), Vec::new());
            for _ in 0..NAMED_REPEATS {
                let (mut worker, bind_ms) = fresh()?;
                binds.push(bind_ms);
                runs.push(run_leave_now(&mut worker, env, sample)?);
                let (mut worker, profile_bind_ms) = fresh()?;
                let profile = run_query(&mut worker, env, sample)?;
                profiles.push(profile_bind_ms + profile.total_ms);
            }
            let first = runs.first().ok_or("no runs")?;
            println!(
                "  {period_name} {hour:02}:00: first response first run {:.1}, median {:.1} \
                 (bind {:.1}, tables {:.1}, leave now {:.1}); missed-it queries {:.1}; \
                 60-minute profile {:.1} ms",
                binds[0] + first.first_response_ms(),
                median(
                    runs.iter()
                        .zip(&binds)
                        .map(|(r, b)| b + r.first_response_ms())
                        .collect()
                ),
                median(binds.clone()),
                median(runs.iter().map(|r| r.tables_ms).collect()),
                median(runs.iter().map(|r| r.now_ms).collect()),
                median(runs.iter().map(|r| r.misses_ms).collect()),
                median(profiles),
            );
        }
        println!();
    }

    let mut runs = Vec::with_capacity(samples.len());
    for sample in samples {
        let (mut worker, bind_ms) = fresh()?;
        runs.push((bind_ms, run_leave_now(&mut worker, env, *sample)?));
    }
    let row = |name: &str, f: &dyn Fn(&(f64, LeaveNow)) -> f64| {
        let mut values: Vec<f64> = runs.iter().map(f).collect();
        values.sort_by(f64::total_cmp);
        println!(
            "  {name:<26} {:>8.1} {:>8.1} {:>9.1}",
            percentile(&values, 0.5),
            percentile(&values, 0.95),
            percentile(&values, 1.0),
        );
    };
    println!(
        "cold leave now, new worker per query, over {} sampled queries:",
        runs.len()
    );
    println!("  {:<26} {:>8} {:>8} {:>9}", "metric", "p50", "p95", "max");
    row("bind ms", &|(bind, _)| *bind);
    row("tables ms", &|(_, run)| run.tables_ms);
    row("leave now ms", &|(_, run)| run.now_ms);
    row("first response ms", &|(bind, run)| {
        bind + run.first_response_ms()
    });
    row("every missed-it query ms", &|(_, run)| run.misses_ms);
    println!();
    Ok(())
}

/// How to run the walk table experiment.
struct TableMode {
    radius: i32,
    table_only: bool,
    dump: Option<String>,
}

/// Times a table beside the real search: `(real ms, with the table ms)` per query.
fn report_table(
    fresh: FreshWorker<'_>,
    worker: &mut Worker,
    env: &Env,
    samples: &[Sample],
    mode: &TableMode,
) -> Result<(), Box<dyn Error>> {
    let stops = env.snapshot.stop_count();
    let started = Instant::now();
    let (mut builder, _) = fresh()?;
    let table = build_walk_table(
        &mut builder.router.transfers_mut().inner,
        stops,
        mode.radius,
    )?;
    let build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let pairs: usize = table.out.iter().map(Vec::len).sum();
    let mut counts: Vec<f64> = table.out.iter().map(|v| v.len() as f64).collect();
    counts.sort_by(f64::total_cmp);
    println!(
        "radius {} min: built in {build_ms:.0} ms; {pairs} pairs, per stop p50 {:.0} p95 {:.0} max {:.0}; \
         about {:.1} MB both directions at 8 bytes a walk",
        mode.radius / 60,
        percentile(&counts, 0.5),
        percentile(&counts, 0.95),
        percentile(&counts, 1.0),
        pairs as f64 * 16.0 / 1e6,
    );
    let (shadow_worker, _) = fresh()?;
    worker.router.transfers_mut().shadow = Some(Shadow {
        worker: Box::new(shadow_worker),
        table: Arc::new(table),
        radius: mode.radius,
        table_only: mode.table_only,
        best: vec![i32::MAX; stops],
        long: Vec::new(),
        nanos: 0,
    });
    for sample in samples.iter().take(16) {
        run_query(worker, env, *sample)?;
    }

    let mut dump = String::new();
    let shadow_ms = |worker: &Worker| {
        worker
            .router
            .transfers()
            .shadow
            .as_ref()
            .map_or(0.0, |s| s.nanos as f64 / 1e6)
    };
    let (mut now_real, mut now_table, mut profile_real, mut profile_table) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (index, sample) in samples.iter().enumerate() {
        let (access, egress) = walk_tables(worker, env, *sample)?;
        let query = TransitQuery {
            snapshot: env.snapshot.clone(),
            access: &access,
            egress: &egress,
            filter: RouteFilter::ALL,
            max_trips: env.max_trips,
        };
        let before_street = worker.router.transfers().counters.street_nanos;
        let before_shadow = shadow_ms(worker);
        let started = Instant::now();
        let at = Time::try_new(env.day_start + sample.hour * 3600)?;
        let now = worker
            .router
            .leave_at(&query, at, env.options, &|| false)
            .map_err(|e| e.to_string())?;
        // The shadow runs inside this span, so it comes out of the real time.
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        let street = (worker.router.transfers().counters.street_nanos - before_street) as f64 / 1e6;
        let shadow = shadow_ms(worker) - before_shadow;
        let scan = elapsed - street - shadow;
        now_real.push(scan + street);
        now_table.push(scan + shadow);
        write!(dump, "L {index}")?;
        for e in &now.entries {
            write!(
                dump,
                " {},{},{}",
                e.departure.into_inner(),
                e.arrival.into_inner(),
                e.trips
            )?;
        }
        dump.push('\n');

        let before_shadow = shadow_ms(worker);
        let (measure, triples) = run_query_with(worker, env, *sample, env.options)?;
        let shadow = shadow_ms(worker) - before_shadow;
        let scan = measure.total_ms - measure.tables_ms - measure.street_ms - shadow;
        profile_real.push(scan + measure.street_ms);
        profile_table.push(scan + shadow);
        write!(dump, "P {index}")?;
        for (d, a, k) in triples {
            write!(dump, " {d},{a},{k}")?;
        }
        dump.push('\n');
    }
    let line = |name: &str, values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        println!(
            "  {name:<30} p50 {:>8.1} p95 {:>8.1} max {:>9.1}",
            percentile(values, 0.5),
            percentile(values, 0.95),
            percentile(values, 1.0),
        );
    };
    println!("search time without the tables, ms:");
    line("leave now, real", &mut now_real);
    line("leave now, with table", &mut now_table);
    line("60-minute profile, real", &mut profile_real);
    line("60-minute profile, with table", &mut profile_table);
    println!(
        "retained by the access/egress tables: {:.1} MB; by the real transfer search: {:.1} MB labels, {:.1} MB graph memo",
        worker.tables.stats().bytes as f64 / 1e6,
        worker.router.transfers().inner.working_memory_bytes() as f64 / 1e6,
        worker.router.transfers().inner.graph_stats().bytes as f64 / 1e6,
    );
    if let Some(path) = &mode.dump {
        std::fs::write(path, dump)?;
    }
    Ok(())
}

/// Runs real table-backed transfers: Sif-costed walks from a precomputed [`TransferTable`].
fn report_sif_transfers(
    worker: &mut Worker,
    env: &Env,
    samples: &[Sample],
    located: &[Option<(f64, f64)>],
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    flags: &Flags,
) -> Result<(), Box<dyn Error>> {
    let radius: u32 = flags.number("sif-transfers", 900);
    let started = Instant::now();
    let table = TransferTable::build(
        &env.provider,
        costing,
        snaps,
        radius,
        NonZeroUsize::new(16).ok_or("threads")?,
        &|| false,
    )?
    .ok_or("interrupted")?;
    println!(
        "radius {} min: built in {:.2?}, {} walks, {:.1} MB",
        radius / 60,
        started.elapsed(),
        table.walk_count(),
        table.bytes() as f64 / 1e6
    );
    let mut router = Router::new(TableTransfers::new(Arc::new(table)));
    let mut run = |worker: &mut Worker, sample: Sample| -> Result<_, Box<dyn Error>> {
        let started = Instant::now();
        let (access, egress) = walk_tables(worker, env, sample)?;
        let tables_ms = started.elapsed().as_secs_f64() * 1000.0;
        let query = TransitQuery {
            snapshot: env.snapshot.clone(),
            access: &access,
            egress: &egress,
            filter: RouteFilter::ALL,
            max_trips: env.max_trips,
        };
        let at = Time::try_new(env.day_start + sample.hour * 3600)?;
        let started = Instant::now();
        let now = router
            .leave_at(&query, at, env.options, &|| false)
            .map_err(|e| e.to_string())?;
        let now_ms = started.elapsed().as_secs_f64() * 1000.0;
        let started = Instant::now();
        let mut departures: Vec<i32> = now
            .entries
            .iter()
            .map(|e| e.departure.into_inner())
            .collect();
        departures.sort_unstable();
        departures.dedup();
        for departure in departures {
            router
                .leave_at(&query, Time::try_new(departure + 1)?, env.options, &|| {
                    false
                })
                .map_err(|e| e.to_string())?;
        }
        let misses_ms = started.elapsed().as_secs_f64() * 1000.0;
        let latest = Time::try_new(at.into_inner() + 3600)?;
        let started = Instant::now();
        let profile = router
            .profile_with(&query, at, latest, env.options, &|| false)
            .map_err(|e| e.to_string())?;
        let profile_ms = started.elapsed().as_secs_f64() * 1000.0;
        let triples = |entries: &[ProfileEntry<TableWalk>]| -> Triples {
            entries
                .iter()
                .map(|e| (e.departure.into_inner(), e.arrival.into_inner(), e.trips))
                .collect()
        };
        Ok((
            [tables_ms, now_ms, misses_ms, profile_ms],
            triples(&now.entries),
            triples(&profile.entries),
        ))
    };
    for sample in samples.iter().take(8) {
        run(worker, *sample)?;
    }
    let mut dump = String::new();
    let mut times = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for (index, sample) in samples.iter().enumerate() {
        let (measured, now, profile) = run(worker, *sample)?;
        for (slot, value) in times.iter_mut().zip(measured) {
            slot.push(value);
        }
        for (tag, list) in [("L", now), ("P", profile)] {
            write!(dump, "{tag} {index}")?;
            for (d, a, k) in list {
                write!(dump, " {d},{a},{k}")?;
            }
            dump.push('\n');
        }
    }
    if let Some(path) = flags.0.get("dump") {
        std::fs::write(path, dump)?;
    }
    println!(
        "{:<30} {:>8} {:>8} {:>9}",
        "ms over 64 trips", "p50", "p95", "max"
    );
    let names = ["tables", "leave now", "all missed-it", "60-minute profile"];
    for (name, values) in names.iter().zip(times.iter_mut()) {
        values.sort_by(f64::total_cmp);
        println!(
            "{name:<30} {:>8.2} {:>8.2} {:>9.2}",
            percentile(values, 0.5),
            percentile(values, 0.95),
            percentile(values, 1.0)
        );
    }
    for (name, from, to) in NAMED_TRIPS {
        let (from, _) = nearest_stop(located, from).ok_or("no stop near the origin")?;
        let (to, _) = nearest_stop(located, to).ok_or("no stop near the destination")?;
        println!("{name}, median of {NAMED_REPEATS}:");
        for (period, (period_name, hour)) in PERIODS.iter().enumerate() {
            let sample = Sample {
                from,
                to,
                period,
                band: 0,
                hour: *hour,
            };
            let mut runs = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            let mut count = 0;
            for _ in 0..NAMED_REPEATS {
                let (measured, now, _) = run(worker, sample)?;
                count = now.len();
                for (slot, value) in runs.iter_mut().zip(measured) {
                    slot.push(value);
                }
            }
            let [tables, now, misses, profile] = runs.map(median);
            println!(
                "  {period_name} {hour:02}:00: tables {tables:.1}, leave now {now:.2} ({count} journeys), \
                 all missed-it {misses:.2}, 60-minute profile {profile:.2} ms"
            );
        }
    }
    Ok(())
}

/// A varint, least significant group first.
fn push_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A transfer table as a download would hold it: one direction, in three columns of varints.
fn encode_transfers(table: &TransferTable) -> Vec<u8> {
    let (mut counts, mut stops, mut seconds) = (Vec::new(), Vec::new(), Vec::new());
    for from in 0..table.stop_count() as u32 {
        let walks: Vec<_> = table.walks_from(StopId::new(from)).collect();
        push_varint(&mut counts, walks.len() as u32);
        let mut previous = 0;
        for (stop, cost) in walks {
            push_varint(&mut stops, stop.into_inner() - previous);
            previous = stop.into_inner();
            push_varint(&mut seconds, cost);
        }
    }
    let mut out = counts;
    out.extend(stops);
    out.extend(seconds);
    out
}

/// The stop nearest a point, and how far away it is in meters.
fn nearest_stop(located: &[Option<(f64, f64)>], point: (f64, f64)) -> Option<(u32, f64)> {
    located
        .iter()
        .enumerate()
        .filter_map(|(stop, at)| Some((stop as u32, haversine_km((*at)?, point) * 1000.0)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    percentile(&values, 0.5)
}

/// Prints the named trips: leave now, missing each journey, and the 60-minute profile.
fn report_named(
    worker: &mut Worker,
    env: &Env,
    located: &[Option<(f64, f64)>],
) -> Result<(), Box<dyn Error>> {
    let clock = |seconds: i32, hour: i32| {
        let offset = seconds - (env.day_start + hour * 3600);
        format!("{:+}m", offset.div_euclid(60))
    };
    for (name, from, to) in NAMED_TRIPS {
        let (from, from_m) = nearest_stop(located, from).ok_or("no stop near the origin")?;
        let (to, to_m) = nearest_stop(located, to).ok_or("no stop near the destination")?;
        println!(
            "{name}: stops {from} ({from_m:.0} m away) to {to} ({to_m:.0} m away), \
             median of {NAMED_REPEATS}"
        );
        for (period, (period_name, hour)) in PERIODS.iter().enumerate() {
            let sample = Sample {
                from,
                to,
                period,
                band: 0,
                hour: *hour,
            };
            let (mut runs, mut profiles) = (Vec::new(), Vec::new());
            for _ in 0..NAMED_REPEATS {
                runs.push(run_leave_now(worker, env, sample)?);
                profiles.push(run_query(worker, env, sample)?);
            }
            let last = runs.last().ok_or("no runs")?;
            println!(
                "  {period_name} {hour:02}:00: tables {:.1}, leave now {:.1}, first response {:.1}, \
                 all {} missed-it queries {:.1}, with misses {:.1}; 60-minute profile {:.1} ms \
                 ({} entries)",
                median(runs.iter().map(|r| r.tables_ms).collect()),
                median(runs.iter().map(|r| r.now_ms).collect()),
                median(runs.iter().map(LeaveNow::first_response_ms).collect()),
                last.next.len(),
                median(runs.iter().map(|r| r.misses_ms).collect()),
                median(runs.iter().map(LeaveNow::with_misses_ms).collect()),
                median(profiles.iter().map(|m| m.total_ms).collect()),
                profiles.last().map_or(0, |m| m.entries),
            );
            let show = |triples: &Triples| {
                triples
                    .iter()
                    .map(|&(d, a, k)| {
                        format!("leave {} arrive {} ({k})", clock(d, *hour), clock(a, *hour))
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            println!("    now: {}", show(&last.options));
            for (departure, after) in &last.next {
                println!("    missed {}: {}", clock(*departure, *hour), show(after));
            }
        }
        println!();
    }
    Ok(())
}

/// Times the two tables of a query with a full flood: a constant workload across code changes,
/// for spotting how fast the machine is running while measuring.
fn run_table_canary(worker: &mut Worker, env: &Env, sample: Sample) -> Result<f64, Box<dyn Error>> {
    let origin = seeds_at(env.provider, worker_point(env, sample.from)?)?;
    let target = seeds_at(env.provider, worker_point(env, sample.to)?)?;
    let stop_count = env.snapshot.stop_count();
    let started = Instant::now();
    for (seeds, side) in [(&origin, AccessSide::Access), (&target, AccessSide::Egress)] {
        worker.tables.compute(
            stop_count,
            &TableRequest {
                side,
                seeds,
                max_seconds: 86_400,
                interrupt: &|| false,
            },
        )?;
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn worker_point(env: &Env, stop: u32) -> Result<SnapPoint, Box<dyn Error>> {
    SNAP_POINTS
        .get()
        .and_then(|points| points.get(stop as usize).copied().flatten())
        .ok_or_else(|| format!("stop {stop} has no snap ({})", env.max_trips).into())
}

/// One way of turning prunings on and off.
#[derive(Clone, Copy, Debug)]
struct Config {
    name: &'static str,
    known_journey_bounds: bool,
    own_seed_pruning: bool,
    terminate_tables: bool,
    bootstrap: bool,
}

const ALL_OFF: Config = Config {
    name: "all off",
    known_journey_bounds: false,
    own_seed_pruning: false,
    terminate_tables: false,
    bootstrap: false,
};

const ALL_ON: Config = Config {
    name: "all on",
    known_journey_bounds: true,
    own_seed_pruning: true,
    terminate_tables: true,
    bootstrap: true,
};

/// Runs every query under `config` and `ALL_OFF`, and reports any profile that differs.
fn compare_with_reference(
    env: &Env,
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    samples: &[Sample],
    config: Config,
) -> Result<usize, Box<dyn Error>> {
    let worker_of = |c: Config| {
        new_worker(
            provider,
            costing,
            snaps,
            c.own_seed_pruning,
            c.terminate_tables,
        )
        .map(|(worker, _)| worker)
    };
    let options_of = |c: Config| ProfileOptions {
        known_journey_bounds: c.known_journey_bounds,
        bootstrap_walk_limit: c.bootstrap.then_some(DEFAULT_BOOTSTRAP_WALK_LIMIT),
    };
    let mut reference = worker_of(ALL_OFF)?;
    let mut candidate = worker_of(config)?;
    let mut differing = 0usize;
    let mut entries = 0usize;
    for sample in samples {
        let (_, expected) = run_query_with(&mut reference, env, *sample, options_of(ALL_OFF))?;
        let (_, got) = run_query_with(&mut candidate, env, *sample, options_of(config))?;
        entries += expected.len();
        if expected != got {
            differing += 1;
            println!("DIFFERENT {sample:?}\n  reference {expected:?}\n  candidate {got:?}");
        }
    }
    println!(
        "{}: compared {} queries, {entries} profile entries: {differing} differ",
        config.name,
        samples.len(),
    );
    Ok(differing)
}

/// Compares all prunings on against all off.
fn compare_configurations(
    env: &Env,
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    samples: &[Sample],
) -> Result<(), Box<dyn Error>> {
    let differing = compare_with_reference(env, provider, costing, snaps, samples, ALL_ON)?;
    if differing > 0 {
        return Err(format!("{differing} profiles differ").into());
    }
    Ok(())
}

/// Compares each pruning on its own against all off.
fn compare_each(
    env: &Env,
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    samples: &[Sample],
) -> Result<(), Box<dyn Error>> {
    let singles = [
        Config {
            name: "known journey bounds only",
            known_journey_bounds: true,
            ..ALL_OFF
        },
        Config {
            name: "own-seed pruning only",
            own_seed_pruning: true,
            ..ALL_OFF
        },
        Config {
            name: "table termination only",
            terminate_tables: true,
            ..ALL_OFF
        },
        Config {
            name: "known bounds with bootstrap",
            known_journey_bounds: true,
            bootstrap: true,
            ..ALL_OFF
        },
    ];
    let mut differing = 0;
    for config in singles {
        differing += compare_with_reference(env, provider, costing, snaps, samples, config)?;
    }
    if differing > 0 {
        return Err(format!("{differing} profiles differ").into());
    }
    Ok(())
}

/// Checks the skipped walk of a search's final round.
///
/// A walk after the last allowed round can't change anything, so a profile with `K` trips must
/// equal the entries with at most `K` trips of a profile allowing `K + 1`, whose round `K` walks.
fn compare_final_round(
    env: &Env,
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    samples: &[Sample],
) -> Result<(), Box<dyn Error>> {
    let (mut limited, _) = new_worker(provider, costing, snaps, true, true)?;
    let (mut larger, _) = new_worker(provider, costing, snaps, true, true)?;
    let bigger_env = Env {
        max_trips: env.max_trips + 1,
        provider: env.provider,
        snapshot: env.snapshot.clone(),
        day_start: env.day_start,
        max_walk: env.max_walk,
        options: env.options,
    };
    let (mut differing, mut entries) = (0usize, 0usize);
    for sample in samples {
        let (_, expected) = run_query_with(&mut limited, env, *sample, env.options)?;
        let (_, wider) = run_query_with(&mut larger, &bigger_env, *sample, env.options)?;
        let filtered: Triples = wider
            .into_iter()
            .filter(|&(_, _, trips)| trips <= env.max_trips)
            .collect();
        entries += expected.len();
        if expected != filtered {
            differing += 1;
            println!("DIFFERENT {sample:?}\n  limited {expected:?}\n  larger  {filtered:?}");
        }
    }
    println!(
        "final-round walk, max_trips {} vs {}: compared {} queries, {entries} entries: {differing} differ",
        env.max_trips,
        bigger_env.max_trips,
        samples.len(),
    );
    if differing > 0 {
        return Err(format!("{differing} profiles differ").into());
    }
    Ok(())
}

/// Times access and egress `StopTables` alone, per query, and what they retain, as the plan
/// layer holds them: one for access and one for egress.
#[allow(clippy::too_many_arguments, reason = "a reporting tool")]
fn report_stop_tables(
    provider: Provider,
    costing: &AnyCosting,
    snaps: &Arc<StopSnaps>,
    env: &Env,
    samples: &[Sample],
    cold: usize,
    warmup: usize,
    dump: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    type Tables = StopTables<Provider, AnyCosting>;
    let stop_count = env.snapshot.stop_count();
    let bound = || -> Result<(Tables, Tables, f64), Box<dyn Error>> {
        let (mut access, mut egress) = (
            StopTables::new(provider, costing.clone()),
            StopTables::new(provider, costing.clone()),
        );
        let started = Instant::now();
        access.bind(snaps.clone())?;
        egress.bind(snaps.clone())?;
        Ok((access, egress, started.elapsed().as_secs_f64() * 1000.0))
    };
    let compute = |access: &mut Tables,
                   egress: &mut Tables,
                   sample: Sample|
     -> Result<(StopTable, StopTable, f64), Box<dyn Error>> {
        let origin = seeds_at(env.provider, worker_point(env, sample.from)?)?;
        let target = seeds_at(env.provider, worker_point(env, sample.to)?)?;
        let request = |side, seeds| TableRequest {
            side,
            seeds,
            max_seconds: env.max_walk,
            interrupt: &|| false,
        };
        let started = Instant::now();
        let a = access
            .compute(stop_count, &request(AccessSide::Access, &origin))?
            .ok_or("interrupted")?;
        let e = egress
            .compute(stop_count, &request(AccessSide::Egress, &target))?
            .ok_or("interrupted")?;
        Ok((a, e, started.elapsed().as_secs_f64() * 1000.0))
    };
    let total =
        |tables: &Tables| -> usize { tables.memory_parts().iter().map(|(_, bytes)| bytes).sum() };

    let mut cold_ms = Vec::new();
    let mut bind_ms = Vec::new();
    for sample in samples.iter().take(cold) {
        let (mut access, mut egress, bind) = bound()?;
        bind_ms.push(bind);
        let (_, _, ms) = compute(&mut access, &mut egress, *sample)?;
        cold_ms.push(ms);
    }

    let (mut access, mut egress, _) = bound()?;
    for sample in samples.iter().take(warmup) {
        compute(&mut access, &mut egress, *sample)?;
    }
    let mut out = dump
        .map(|path| std::fs::File::create(path).map(std::io::BufWriter::new))
        .transpose()?;
    let mut warm_ms = Vec::new();
    let mut labels = 0u64;
    let mut peak = 0usize;
    for (query, sample) in samples.iter().enumerate() {
        let (a, e, ms) = compute(&mut access, &mut egress, *sample)?;
        warm_ms.push(ms);
        labels += access.stats().labels_settled + egress.stats().labels_settled;
        peak = peak.max(total(&access)).max(total(&egress));
        if let Some(out) = out.as_mut() {
            use std::hash::{Hash, Hasher};
            use std::io::Write as _;
            for (name, table, tables) in [("a", &a, &access), ("e", &e, &egress)] {
                for index in 0..stop_count {
                    let stop = StopId::new(index as u32);
                    let Some(cost) = table.cost(stop) else {
                        continue;
                    };
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    format!("{:?}", tables.leg(stop)).hash(&mut hasher);
                    writeln!(
                        out,
                        "{query} {name} {index} {} {:016x}",
                        cost.into_inner(),
                        hasher.finish()
                    )?;
                }
            }
        }
    }
    if let Some(mut out) = out {
        use std::io::Write as _;
        out.flush()?;
    }
    warm_ms.sort_by(f64::total_cmp);
    cold_ms.sort_by(f64::total_cmp);
    println!(
        "tables: warm n={} p50 {:.1} p95 {:.1} max {:.1} ms; cold n={} p50 {:.1} p95 {:.1} ms; \
         bind {:.1} ms; labels settled per query {:.0}",
        warm_ms.len(),
        percentile(&warm_ms, 0.5),
        percentile(&warm_ms, 0.95),
        warm_ms.last().copied().unwrap_or(f64::NAN),
        cold_ms.len(),
        percentile(&cold_ms, 0.5),
        percentile(&cold_ms, 0.95),
        bind_ms.iter().sum::<f64>() / bind_ms.len().max(1) as f64,
        labels as f64 / warm_ms.len() as f64,
    );
    for (label, tables) in [("access", &access), ("egress", &egress)] {
        println!("retained by {label}: {:.2} MB", total(tables) as f64 / 1e6);
        for (name, bytes) in tables.memory_parts() {
            println!("  {name:<16} {:>8.2} MB", bytes as f64 / 1e6);
        }
    }
    println!(
        "peak retained by one StopTables across queries: {:.2} MB",
        peak as f64 / 1e6
    );
    Ok(())
}

static SNAP_POINTS: std::sync::OnceLock<Vec<Option<SnapPoint>>> = std::sync::OnceLock::new();

fn main() -> Result<(), Box<dyn Error>> {
    let flags = Flags::from_args();
    let feed = flags.string(
        "feed",
        "/mnt/maps/cardinal/transit/kcm/static/kcm-FAL26-161.2.zip",
    );
    let tiles = flags.string(
        "tiles",
        "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar",
    );
    let date = parse_service_date(&flags.string("date", "20261110")).ok_or("bad date")?;
    let per_bin: usize = flags.number("per-bin", 4);
    let seed: u64 = flags.number("seed", 1);
    let cold: usize = flags.number("cold", 4);
    let warmup: usize = flags.number("warmup", 24);
    let verbose = flags.present("verbose");

    let started = Instant::now();
    let output = compile(vec![FeedInput {
        id: FeedId::try_new("kcm".to_owned())?,
        source: Box::new(ZipSource::from_path(Path::new(&feed))?),
    }])?;
    let bytes = output.bytes;
    let view = CompiledTimetable::parse(&bytes)?;
    let days = date.into_inner();
    let window = view.service_window(&[
        ServiceDate::new(days.saturating_sub(1)),
        date,
        ServiceDate::new(days + 1),
    ])?;
    let timetable = view.to_timetable()?;
    let base = timetable.materialize(&window.days)?;
    let snapshot = Snapshot::schedule(Arc::new(base));
    let day_start_unix = view
        .service_day_start_unix(date)
        .ok_or("date not covered")?;
    let day_start = (day_start_unix - window.origin_unix) as i32;
    eprintln!(
        "compiled {} stops, {} routes in {:.2?}",
        snapshot.stop_count(),
        snapshot.route_count(),
        started.elapsed()
    );

    let costing_name = flags.string("costing", "pedestrian");
    let costing = costing_named(&costing_name)?;
    let tile_sets = TileSets::open(&[tiles.as_str()], NonZeroUsize::new(1024).ok_or("nonzero")?)?;
    let provider: Provider = Box::leak(Box::new(tile_sets));

    let stops: Vec<(f64, f64)> = (0..snapshot.stop_count())
        .map(|i| {
            let info = view.stop(StopId::new(i as u32));
            info.and_then(|s| s.coordinate)
                .map_or((f64::NAN, f64::NAN), |c| (c.lon, c.lat))
        })
        .collect();
    let snap_started = Instant::now();
    let snaps = snap_stops(
        &provider,
        &costing,
        &SnapConfig::default(),
        provider.build_id(),
        &stops,
    )?;
    let points: Vec<Option<SnapPoint>> = (0..snapshot.stop_count())
        .map(|i| snaps.point(StopId::new(i as u32)))
        .collect();
    let located: Vec<Option<(f64, f64)>> = points.iter().map(|p| p.map(|p| p.point)).collect();
    SNAP_POINTS.set(points).map_err(|_| "set snap points")?;
    let snaps = Arc::new(snaps);
    eprintln!(
        "snapped {} stops in {:.2?}: {} problems",
        snaps.stop_count(),
        snap_started.elapsed(),
        snaps.reports().len()
    );

    let weights = trip_weights(&snapshot);
    let samples = sample_queries(per_bin, seed, &weights, &located);
    let env = Env {
        provider,
        snapshot: snapshot.clone(),
        day_start,
        max_trips: flags.number("max-trips", 8),
        max_walk: flags.number("max-walk", 86_400),
        options: ProfileOptions {
            known_journey_bounds: !flags.present("no-known-bounds"),
            bootstrap_walk_limit: (!flags.present("no-bootstrap"))
                .then(|| flags.number("bootstrap-limit", DEFAULT_BOOTSTRAP_WALK_LIMIT)),
        },
    };
    println!(
        "date {} max_trips {} max_walk {} queries {} (per bin {per_bin}), seed {seed}",
        flags.string("date", "20261110"),
        env.max_trips,
        env.max_walk,
        samples.len(),
    );
    println!(
        "load: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    let own_seed_pruning = !flags.present("no-own-seed-pruning");
    let terminate_tables = !flags.present("flood-tables");
    if flags.present("compare-final-round") {
        return compare_final_round(&env, provider, &costing, &snaps, &samples);
    }
    if flags.present("compare-each") {
        return compare_each(&env, provider, &costing, &snaps, &samples);
    }
    if flags.present("compare") {
        return compare_configurations(&env, provider, &costing, &snaps, &samples);
    }

    if flags.present("tables-report") {
        return report_stop_tables(
            provider,
            &costing,
            &snaps,
            &env,
            &samples,
            cold,
            warmup,
            flags.0.get("dump-tables").map(String::as_str),
        );
    }

    let canary_sample = samples[0];
    let canary = |label: &str| -> Result<(), Box<dyn Error>> {
        let (mut worker, _) = new_worker(provider, &costing, &snaps, true, false)?;
        worker.tables.set_terminate_when_stops_settled(false);
        let mut best = f64::MAX;
        for _ in 0..3 {
            best = best.min(run_table_canary(&mut worker, &env, canary_sample)?);
        }
        println!("canary ({label}): {best:.0} ms for a full-network table pair (best of 3)");
        Ok(())
    };
    let run_canary = !flags.present("no-canary");
    if run_canary {
        canary("before")?;
    }

    // Cold: a brand new worker per query.
    let mut cold_measures = Vec::new();
    let mut bind_ms = Vec::new();
    for sample in samples.iter().take(cold) {
        let (mut worker, bound) = new_worker(
            provider,
            &costing,
            &snaps,
            own_seed_pruning,
            terminate_tables,
        )?;
        bind_ms.push(bound);
        let measure = run_query(&mut worker, &env, *sample)?;
        cold_measures.push(measure);
    }

    // Warm: one worker.
    let (mut worker, _) = new_worker(
        provider,
        &costing,
        &snaps,
        own_seed_pruning,
        terminate_tables,
    )?;
    for (i, sample) in samples.iter().take(warmup).enumerate() {
        let measure = run_query(&mut worker, &env, *sample)?;
        if verbose {
            eprintln!("warmup {i}: {:.0} ms", measure.total_ms);
        }
    }
    let fresh = || {
        new_worker(
            provider,
            &costing,
            &snaps,
            own_seed_pruning,
            terminate_tables,
        )
    };
    if flags.present("build-transfers") {
        for mode in ["pedestrian", "bicycle"] {
            let mode_costing = costing_named(mode)?;
            let mode_snaps = Arc::new(snap_stops(
                &provider,
                &mode_costing,
                &SnapConfig::default(),
                provider.build_id(),
                &stops,
            )?);
            for (radius, threads) in [(600, 16), (900, 16), (1200, 16)] {
                let started = Instant::now();
                let table = TransferTable::build(
                    &provider,
                    &mode_costing,
                    &mode_snaps,
                    radius,
                    NonZeroUsize::new(threads).ok_or("threads")?,
                    &|| false,
                )?
                .ok_or("interrupted")?;
                println!(
                    "{mode} {} min, {threads} threads: built in {:.2?}; {} walks, {:.1} per stop; {:.1} MB",
                    radius / 60,
                    started.elapsed(),
                    table.walk_count(),
                    table.walk_count() as f64 / table.stop_count().max(1) as f64,
                    table.bytes() as f64 / 1e6,
                );
                if let Some(dir) = flags.0.get("transfer-files") {
                    std::fs::write(
                        format!("{dir}/{mode}-{}.bin", radius / 60),
                        encode_transfers(&table),
                    )?;
                }
                if mode == "pedestrian" && radius == 900 && threads == 16 {
                    let (mut builder, _) = fresh()?;
                    let node = build_walk_table(
                        &mut builder.router.transfers_mut().inner,
                        table.stop_count(),
                        radius as i32,
                    )?;
                    let mut deltas = Vec::new();
                    let mut only_node = 0_usize;
                    for (from, walks) in node.out.iter().enumerate() {
                        for &(to, seconds) in walks {
                            match table.seconds(StopId::new(from as u32), StopId::new(to)) {
                                Some(edge) => deltas.push(f64::from(edge as i32 - seconds)),
                                None => only_node += 1,
                            }
                        }
                    }
                    deltas.sort_by(f64::total_cmp);
                    println!(
                        "  against node-based walks: {} pairs in both, {only_node} only node-based; \
                         Sif minus node seconds p5 {:.0} p50 {:.0} p95 {:.0} max {:.0}",
                        deltas.len(),
                        percentile(&deltas, 0.05),
                        percentile(&deltas, 0.5),
                        percentile(&deltas, 0.95),
                        percentile(&deltas, 1.0),
                    );
                }
            }
        }
        return Ok(());
    }
    if flags.present("sif-transfers") {
        return report_sif_transfers(
            &mut worker,
            &env,
            &samples,
            &located,
            &costing,
            &snaps,
            &flags,
        );
    }
    if flags.present("table-radius") {
        return report_table(
            &fresh,
            &mut worker,
            &env,
            &samples,
            &TableMode {
                radius: flags.number("table-radius", 600),
                table_only: flags.present("table-only"),
                dump: flags.0.get("dump").cloned(),
            },
        );
    }
    if flags.present("horizons") {
        worker.router.transfers_mut().horizons = HorizonStats::default();
        for sample in &samples {
            run_leave_now(&mut worker, &env, *sample)?;
        }
        worker
            .router
            .transfers()
            .horizons
            .print("leave now and every missed-it query");
        worker.router.transfers_mut().horizons = HorizonStats::default();
        for sample in &samples {
            run_query(&mut worker, &env, *sample)?;
        }
        worker
            .router
            .transfers()
            .horizons
            .print("60-minute profiles");
        println!(
            "table stats: {} labels settled in the last query; {} tiles numbered by the table graph",
            worker.tables.stats().labels_settled,
            worker.tables.graph_tile_count(),
        );
        for (name, bytes) in worker.tables.memory_parts() {
            println!("  {name:<16} {:>8.2} MB", bytes as f64 / 1e6);
        }
        return Ok(());
    }
    if flags.present("named-only") {
        report_cold(&fresh, &env, &samples, &located)?;
        report_named(&mut worker, &env, &located)?;
        report_leave_now(&mut worker, &env, &samples)?;
        return Ok(());
    }
    let mut measures = Vec::with_capacity(samples.len());
    let run_started = Instant::now();
    for (i, sample) in samples.iter().enumerate() {
        let measure = run_query(&mut worker, &env, *sample)?;
        if matches!(i + 1, 1 | 4 | 16 | 64) {
            let search = &worker.router.transfers().inner;
            println!(
                "retained after {} warm queries: walk search {:.1} MB, graph memo {:.1} MB, tables {:.1} MB",
                i + 1,
                search.working_memory_bytes() as f64 / 1e6,
                search.graph_stats().bytes as f64 / 1e6,
                worker.tables.stats().bytes as f64 / 1e6,
            );
        }
        if verbose {
            eprintln!(
                "query {i}: {} {} -> {} {} {:.0} ms ({:.0} street, {:.0} tables, {} entries, \
                 {}f {}b, {} walks)",
                PERIODS[sample.period].0,
                sample.from,
                sample.to,
                BANDS[sample.band].2,
                measure.total_ms,
                measure.street_ms,
                measure.tables_ms,
                measure.entries,
                measure.counters.runs[0],
                measure.counters.runs[1],
                measure.counters.searches,
            );
        }
        measures.push(measure);
    }
    println!(
        "warm run: {} queries in {:.1?}; load: {}",
        measures.len(),
        run_started.elapsed(),
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    if let Some(path) = flags.0.get("csv") {
        let mut csv = String::from(
            "from,to,period,band,total_ms,tables_ms,street_ms,scan_ms,entries,fwd,bwd,walks,\
             nodes,labels,arcs,seeds\n",
        );
        for m in &measures {
            let c = m.counters;
            writeln!(
                csv,
                "{},{},{},{},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{},{},{},{}",
                m.sample.from,
                m.sample.to,
                PERIODS[m.sample.period].0,
                BANDS[m.sample.band].2,
                m.total_ms,
                m.tables_ms,
                m.street_ms,
                m.scan_ms,
                m.entries,
                c.runs[0],
                c.runs[1],
                c.searches,
                c.nodes_settled,
                c.labels_settled,
                c.arcs_relaxed,
                c.seeds,
            )?;
        }
        std::fs::write(path, csv)?;
    }

    if run_canary {
        canary("after")?;
    }

    println!();
    report_cold(&fresh, &env, &samples, &located)?;
    report_named(&mut worker, &env, &located)?;
    report_leave_now(&mut worker, &env, &samples)?;
    summarize("warm, all queries", &measures);
    for (index, (name, _)) in PERIODS.iter().enumerate() {
        let period: Vec<Measure> = measures
            .iter()
            .copied()
            .filter(|m| m.sample.period == index)
            .collect();
        summarize(&format!("warm, {name}"), &period);
    }
    for (index, (_, _, name)) in BANDS.iter().enumerate() {
        let band: Vec<Measure> = measures
            .iter()
            .copied()
            .filter(|m| m.sample.band == index)
            .collect();
        summarize(&format!("warm, {name}"), &band);
    }
    summarize("cold (new router per query)", &cold_measures);
    if !bind_ms.is_empty() {
        println!(
            "bind per worker (tables + search): {:.1} ms mean",
            bind_ms.iter().sum::<f64>() / bind_ms.len() as f64
        );
    }

    let search = &worker.router.transfers().inner;
    let graph = search.graph_stats();
    println!(
        "memory: walk search {:.1} MB, graph memo {:.1} MB ({} arcs, {} tiles), tables {:.1} MB",
        search.working_memory_bytes() as f64 / 1e6,
        graph.bytes as f64 / 1e6,
        graph.arcs_stored,
        graph.tile_slots,
        worker.tables.stats().bytes as f64 / 1e6,
    );
    println!(
        "memory: heap peak {:.0} MB (live now {:.0} MB); VmHWM {:.0} MB, VmRSS {:.0} MB",
        PEAK.load(Ordering::Relaxed) as f64 / 1e6,
        ALLOCATED.load(Ordering::Relaxed) as f64 / 1e6,
        proc_mb("VmHWM:").unwrap_or(f64::NAN),
        proc_mb("VmRSS:").unwrap_or(f64::NAN),
    );
    Ok(())
}
