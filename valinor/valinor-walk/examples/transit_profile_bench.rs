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
    DaySlot, Direction, ProfileOptions, RouteFilter, RouteId, Router, ServiceDate, Snapshot,
    StopId, Time, TransferOutcome, TransferRequest, TransferSearch, TransferSink, TransitQuery,
};
use valinor_walk::{
    AccessSide, ResolvedWalk, SnapConfig, SnapPoint, StopSnaps, StopTables, TableRequest,
    TableSeed, WalkError, WalkHandle, WalkSearch, snap_stops,
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
struct Timed {
    inner: Walk,
    counters: Counters,
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
    Ok(AnyCosting::new(&proto).map_err(|e| format!("{e:?}"))?)
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
fn seeds_at(provider: &Provider, point: SnapPoint) -> Result<Vec<TableSeed>, Box<dyn Error>> {
    let mut seeds = vec![TableSeed {
        edge: point.edge,
        percent_along: point.percent_along,
        initial: 0.0,
    }];
    if let Some(opposing) = opposing_edge(provider, point.edge)? {
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
    if let Some(limit) = std::env::args()
        .skip_while(|arg| arg != "--walk-limit")
        .nth(1)
        .and_then(|limit| limit.parse().ok())
    {
        search.set_walk_limit(Some(limit));
    }
    search.bind(snaps.clone())?;
    let bind_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok((
        Worker {
            tables,
            router: Router::new(Timed {
                inner: search,
                counters: Counters::default(),
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
    let snaps_point =
        |stop: u32| -> Result<SnapPoint, Box<dyn Error>> { Ok(worker_point(env, stop)?) };
    let origin = seeds_at(&env.provider, snaps_point(sample.from)?)?;
    let target = seeds_at(&env.provider, snaps_point(sample.to)?)?;

    let started = Instant::now();
    let before = worker.router.transfers().counters;
    let stop_count = env.snapshot.stop_count();
    let table = |worker: &mut Worker, seeds: &[TableSeed], side| -> Result<_, Box<dyn Error>> {
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
    let access = table(worker, &origin, AccessSide::Access)?;
    let egress = table(worker, &target, AccessSide::Egress)?;
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

/// Times the two tables of a query with a full flood: a constant workload across code changes,
/// for spotting how fast the machine is running while measuring.
fn run_table_canary(worker: &mut Worker, env: &Env, sample: Sample) -> Result<f64, Box<dyn Error>> {
    let origin = seeds_at(&env.provider, worker_point(env, sample.from)?)?;
    let target = seeds_at(&env.provider, worker_point(env, sample.to)?)?;
    let stop_count = env.snapshot.stop_count();
    let started = Instant::now();
    for (seeds, side) in [(&origin, AccessSide::Access), (&target, AccessSide::Egress)] {
        worker.tables.compute(
            stop_count,
            &TableRequest {
                side,
                seeds,
                max_seconds: env.max_walk,
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
}

const ALL_OFF: Config = Config {
    name: "all off",
    known_journey_bounds: false,
    own_seed_pruning: false,
    terminate_tables: false,
};

const ALL_ON: Config = Config {
    name: "all on",
    known_journey_bounds: true,
    own_seed_pruning: true,
    terminate_tables: true,
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

    let costing = costing_named("pedestrian")?;
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
    if flags.present("compare-each") {
        return compare_each(&env, provider, &costing, &snaps, &samples);
    }
    if flags.present("compare") {
        return compare_configurations(&env, provider, &costing, &snaps, &samples);
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
    let mut measures = Vec::with_capacity(samples.len());
    let run_started = Instant::now();
    for (i, sample) in samples.iter().enumerate() {
        let measure = run_query(&mut worker, &env, *sample)?;
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
