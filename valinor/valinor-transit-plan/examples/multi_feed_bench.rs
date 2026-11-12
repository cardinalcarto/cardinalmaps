//! Benchmarks the planner over a transit build: load, memory, and named trips across feeds.
//!
//! ```text
//! cargo run --release -p valinor-transit-plan --example multi_feed_bench -- \
//!     --build DIR --tiles PATH [--feeds a,b] [--date YYYYMMDD] [--rt DIR] [--repeat N]
//! ```
//!
//! `--rt DIR` ingests `tripupdates.pb`, `vehiclepositions.pb`, and `alerts.pb` for King County
//! Metro before planning, and plans at the capture's own time.
//!
//! After the named trips it prints where a full plan's time goes:
//! one table per trip and travel mode, of the median stage times across `--repeat` warm plans.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "a reporting tool"
)]

use core::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::Type as CostingType;
use valinor_gtfs::LatLon;
use valinor_gtfs_rt::FeedKind;
use valinor_router::tiles::TileSets;
use valinor_sif::AnyCosting;
use valinor_sif::costing_options::parse_costing;
use valinor_sif::json::JsonValue;
use valinor_transit::{ProfileOptions, RouteFilter, Router, StopId, StopTable, Time, TransitQuery};
use valinor_transit_plan::response::{JourneyLeg, StopRef, TransitLeg};
use valinor_transit_plan::{
    FeedUpdate, PlanResponse, PlanStage, StreetConfig, StreetPlanner, TransitBuild, TransitRequest,
    TravelMode,
};
use valinor_walk::{AccessSide, StopTables, TableRequest, TableTransfers};

const WASHINGTON: &str = "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09";
const STREETS: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const METRO: &str = "us-wa-Metro-Transit";
/// 2026-11-10 00:00 Pacific, a Tuesday.
const TUESDAY_MIDNIGHT: i64 = 1_794_297_600;

/// A trip tracked by name: its name, then where it starts and ends as `(lon, lat)`.
type NamedTrip = (&'static str, (f64, f64), (f64, f64));

/// `(name, from, to)` as `(lon, lat)`.
const NAMED: [NamedTrip; 2] = [
    (
        "Fremont to Central District",
        (-122.349_869, 47.649_735),
        (-122.302_605, 47.613_442),
    ),
    (
        "Ballard to SeaTac",
        (-122.385_345, 47.668_606),
        (-122.298_523, 47.441_296),
    ),
];

fn flag(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == format!("--{name}") {
            return args.next();
        }
    }
    None
}

/// One `/proc/self/status` field, in MB.
fn status_mb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status
                .lines()
                .find(|line| line.starts_with(key))?
                .to_owned();
            line.split_whitespace().nth(1)?.parse::<f64>().ok()
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Resident memory, with what is heap apart from file-backed mapped tiles and timetables.
fn rss() -> String {
    format!(
        "RSS {:.0} MB (anonymous {:.0} MB, file-backed {:.0} MB)",
        status_mb("VmRSS:"),
        status_mb("RssAnon:"),
        status_mb("RssFile:")
    )
}

fn costing(costing_type: CostingType) -> Result<AnyCosting, Box<dyn Error>> {
    let doc = JsonValue::parse(b"{}").map_err(|e| format!("{e:?}"))?;
    let mut proto = CostingProto::default();
    parse_costing(
        &doc,
        "/none",
        &mut proto,
        &mut Vec::new(),
        Some(costing_type),
    )
    .map_err(|e| format!("{e:?}"))?;
    Ok(AnyCosting::new(&proto)?)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn nearest(locations: &[(StopId, f64, f64)], point: (f64, f64)) -> Option<(StopId, f64)> {
    locations
        .iter()
        .map(|&(stop, lat, lon)| {
            let dx = (lon - point.0) * 47.6_f64.to_radians().cos() * 111_195.0;
            let dy = (lat - point.1) * 111_195.0;
            (stop, (dx * dx + dy * dy).sqrt())
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

fn describe(leg: &TransitLeg, origin: i64) -> String {
    let route = leg
        .route
        .as_ref()
        .and_then(|route| route.short_name.clone().or_else(|| route.long_name.clone()))
        .unwrap_or_default();
    let feed = leg.trip.feed.clone().unwrap_or_default();
    let board = leg
        .stops
        .first()
        .and_then(|s| s.stop.name.clone())
        .unwrap_or_default();
    let alight = leg
        .stops
        .last()
        .and_then(|s| s.stop.name.clone())
        .unwrap_or_default();
    format!(
        "{feed} {route} {board} -> {alight} (dep +{}m{}{})",
        (leg.departure - origin) / 60,
        leg.delay_seconds
            .map_or(String::new(), |d| format!(" delay {d}s")),
        if leg.realtime { " rt" } else { "" },
    )
}

fn ingest_rt(planner: &StreetPlanner, directory: &Path) -> Result<i64, Box<dyn Error>> {
    let state = planner.state();
    let name = directory
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("capture name")?;
    let parsed = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", name)?;
    let now = parsed
        .to_zoned(jiff::tz::TimeZone::UTC)?
        .timestamp()
        .as_second();
    for (file, kind) in [
        ("tripupdates.pb", FeedKind::TripUpdates),
        ("vehiclepositions.pb", FeedKind::VehiclePositions),
        ("alerts.pb", FeedKind::Alerts),
    ] {
        let bytes = std::fs::read(directory.join(file))?;
        let result = state.ingest_and_refresh(
            FeedUpdate {
                feed: METRO,
                kind,
                bytes: Some(&bytes),
                received_at: now,
                stale_after: Some(3600),
            },
            now,
        )?;
        println!("ingest {file}: {:?}", result.outcome);
    }
    Ok(now)
}

/// Where each stage of a plan's time went, attributing each interval to the stage it was in.
struct StageClock {
    at: Cell<Instant>,
    current: Cell<PlanStage>,
    /// Milliseconds and entries per stage.
    totals: RefCell<BTreeMap<PlanStage, (f64, u32)>>,
}

impl StageClock {
    fn new() -> Self {
        Self {
            at: Cell::new(Instant::now()),
            current: Cell::new(PlanStage::Setup),
            totals: RefCell::new(BTreeMap::new()),
        }
    }

    /// Closes the current interval.
    fn tick(&self) {
        let now = Instant::now();
        let ms = (now - self.at.replace(now)).as_secs_f64() * 1000.0;
        self.totals
            .borrow_mut()
            .entry(self.current.get())
            .or_insert((0.0, 0))
            .0 += ms;
    }

    fn enter(&self, stage: PlanStage) {
        self.tick();
        self.current.set(stage);
        self.totals.borrow_mut().entry(stage).or_insert((0.0, 0)).1 += 1;
    }
}

/// The stages, in the order a plan runs them, with the name used in the table.
const STAGES: [(PlanStage, &str); 16] = [
    (PlanStage::Setup, "pin, worker, options"),
    (PlanStage::Locate, "locate origin and destination"),
    (PlanStage::AccessTable, "access table"),
    (PlanStage::EgressTable, "egress table"),
    (PlanStage::DirectRoute, "direct street route"),
    (PlanStage::Search, "transit search (profile)"),
    (PlanStage::DrawAccessEgress, "draw access and egress"),
    (PlanStage::DrawTransfer, "draw transfers (search)"),
    (PlanStage::ThorTrip, "Thor trip (with polyline)"),
    (PlanStage::Narrate, "narration"),
    (PlanStage::ConvertWalkPath, "walk path conversion"),
    (PlanStage::TransitLeg, "ride legs"),
    (PlanStage::TransitShape, "ride shape encoding"),
    (PlanStage::Departures, "ride departure ranges"),
    (PlanStage::Assemble, "journey assembly"),
    (PlanStage::Release, "release worker"),
];

/// A leg's stop as `(feed, stop ID)`: two legs repeat when these are the same.
fn stop_key(stop: &StopRef) -> (Option<String>, String) {
    (stop.feed.clone(), stop.stop_id.clone())
}

/// How many legs a plan has, how many of them are distinct, and what its JSON weighs.
#[derive(Default)]
struct LegCounts {
    journeys: usize,
    access: usize,
    rides: usize,
    transfers: usize,
    egress: usize,
    distinct_access: usize,
    distinct_rides: usize,
    distinct_transfers: usize,
    distinct_egress: usize,
    departures: usize,
    json_bytes: usize,
}

fn leg_counts(response: &PlanResponse, json_bytes: usize) -> LegCounts {
    let mut counts = LegCounts {
        journeys: response.journeys.len(),
        json_bytes,
        ..LegCounts::default()
    };
    let (mut access, mut egress) = (HashSet::new(), HashSet::new());
    let (mut rides, mut transfers) = (HashSet::new(), HashSet::new());
    for leg in response.journeys.iter().flat_map(|journey| &journey.legs) {
        match leg {
            JourneyLeg::Access(leg) => {
                counts.access += 1;
                access.insert(stop_key(&leg.stop));
            }
            JourneyLeg::Egress(leg) => {
                counts.egress += 1;
                egress.insert(stop_key(&leg.stop));
            }
            JourneyLeg::Transfer(leg) => {
                counts.transfers += 1;
                transfers.insert((stop_key(&leg.from), stop_key(&leg.to)));
            }
            JourneyLeg::Transit(leg) => {
                counts.rides += 1;
                counts.departures += leg.departures.len();
                let ends = leg
                    .stops
                    .first()
                    .zip(leg.stops.last())
                    .map(|(first, last)| (stop_key(&first.stop), stop_key(&last.stop)));
                rides.insert((
                    leg.trip.feed.clone(),
                    leg.trip.trip_id.clone(),
                    leg.trip.start_date.clone(),
                    ends,
                ));
            }
        }
    }
    (counts.distinct_access, counts.distinct_egress) = (access.len(), egress.len());
    (counts.distinct_rides, counts.distinct_transfers) = (rides.len(), transfers.len());
    counts
}

/// One period's medians of a plan's split, in milliseconds, with its leg counts.
struct PeriodSplit {
    stages: BTreeMap<PlanStage, f64>,
    /// How many times each stage began, in the last plan.
    entries: BTreeMap<PlanStage, u32>,
    plan_ms: f64,
    json_ms: f64,
    counts: LegCounts,
}

/// Plans one request `repeat` times, keeping the median of each stage.
fn split_period(
    planner: &StreetPlanner,
    request: &TransitRequest,
    repeat: usize,
) -> Result<PeriodSplit, Box<dyn Error>> {
    let mut samples: BTreeMap<PlanStage, Vec<f64>> = BTreeMap::new();
    let (mut plan_ms, mut json_ms) = (Vec::new(), Vec::new());
    let mut entries = BTreeMap::new();
    let mut counts = LegCounts::default();
    for _ in 0..repeat {
        let clock = StageClock::new();
        let started = Instant::now();
        let response = planner.plan_probed(request, "en-US", &|| false, &|s| clock.enter(s))?;
        plan_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        clock.tick();
        let started = Instant::now();
        let json = serde_json::to_vec(&response)?;
        json_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        counts = leg_counts(&response, json.len());
        let totals = clock.totals.borrow();
        for (stage, _) in STAGES {
            let (ms, entered) = totals.get(&stage).copied().unwrap_or((0.0, 0));
            samples.entry(stage).or_default().push(ms);
            entries.insert(stage, entered);
        }
    }
    Ok(PeriodSplit {
        stages: samples.into_iter().map(|(s, v)| (s, median(v))).collect(),
        entries,
        plan_ms: median(plan_ms),
        json_ms: median(json_ms),
        counts,
    })
}

const PERIODS: [(&str, i64); 3] = [("morning", 8), ("midday", 12), ("evening", 17)];

/// Prints where a full plan's time goes for one trip and travel mode.
fn print_split(name: &str, mode_name: &str, splits: &[PeriodSplit], repeat: usize) {
    println!("== split: {name}, {mode_name} (median ms of {repeat} warm plans)");
    print!("  {:32}", "stage");
    for (period, _) in PERIODS {
        print!("{period:>12}");
    }
    println!();
    let row = |name: &str, value: &dyn Fn(&PeriodSplit) -> String| {
        print!("  {name:32}");
        for split in splits {
            print!("{:>12}", value(split));
        }
        println!();
    };
    for (stage, stage_name) in STAGES {
        row(stage_name, &|split| format!("{:.2}", split.stages[&stage]));
    }
    row("sum of stages", &|split| {
        format!("{:.1}", split.stages.values().sum::<f64>())
    });
    row("full plan", &|split| format!("{:.1}", split.plan_ms));
    row("JSON serialization", &|split| {
        format!("{:.2}", split.json_ms)
    });
    row("JSON KB", &|split| {
        format!("{:.0}", split.counts.json_bytes as f64 / 1024.0)
    });
    row("journeys", &|split| split.counts.journeys.to_string());
    row("access legs (distinct)", &|split| {
        format!("{} ({})", split.counts.access, split.counts.distinct_access)
    });
    row("rides (distinct)", &|split| {
        format!("{} ({})", split.counts.rides, split.counts.distinct_rides)
    });
    row("departures listed", &|split| {
        split.counts.departures.to_string()
    });
    row("transfers (distinct)", &|split| {
        format!(
            "{} ({})",
            split.counts.transfers, split.counts.distinct_transfers
        )
    });
    row("egress legs (distinct)", &|split| {
        format!("{} ({})", split.counts.egress, split.counts.distinct_egress)
    });
    row("paths drawn by Thor", &|split| {
        split.entries[&PlanStage::ThorTrip].to_string()
    });
}

/// Prints the split for each named trip and travel mode.
fn print_splits(
    planner: &StreetPlanner,
    rt_now: Option<i64>,
    midnight: i64,
    repeat: usize,
) -> Result<(), Box<dyn Error>> {
    for (name, from, to) in NAMED {
        for (mode_name, mode) in [
            ("walking", TravelMode::Walking),
            ("cycling", TravelMode::Cycling),
            ("wheelchair", TravelMode::Wheelchair),
        ] {
            let mut splits = Vec::new();
            for (_, hour) in PERIODS {
                let departure = rt_now.unwrap_or(midnight + hour * 3600);
                let mut request = TransitRequest::new(
                    LatLon {
                        lat: from.1,
                        lon: from.0,
                    },
                    LatLon {
                        lat: to.1,
                        lon: to.0,
                    },
                    departure,
                    departure - 60,
                );
                request.mode = mode;
                splits.push(split_period(planner, &request, repeat)?);
            }
            print_split(name, mode_name, &splits, repeat);
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let build_dir = PathBuf::from(flag("build").unwrap_or_else(|| WASHINGTON.to_owned()));
    let tiles = flag("tiles").unwrap_or_else(|| STREETS.to_owned());
    let feeds: Option<Vec<String>> =
        flag("feeds").map(|text| text.split(',').map(str::to_owned).collect());
    let repeat: usize = flag("repeat").and_then(|n| n.parse().ok()).unwrap_or(5);
    let date_offset: i64 = flag("day").and_then(|n| n.parse().ok()).unwrap_or(0);
    let midnight = TUESDAY_MIDNIGHT + date_offset * 86_400;

    println!("before loading: {}", rss());
    let started = Instant::now();
    let build = TransitBuild::load(&build_dir, feeds.as_deref())?;
    println!(
        "loaded {} files, {} stops: {:?} in {:.2?}; {}",
        build.set.part_count(),
        build.set.stop_count(),
        build.report,
        started.elapsed(),
        rss()
    );
    let started = Instant::now();
    let planner = StreetPlanner::from_build(build, &[tiles.as_str()], StreetConfig::default())?;
    println!(
        "opened planner (snaps every stop twice) in {:.2?}; {}",
        started.elapsed(),
        rss()
    );

    let rt_now = match flag("rt") {
        Some(directory) => Some(ingest_rt(&planner, Path::new(&directory))?),
        None => None,
    };

    let probe = rt_now.unwrap_or(midnight + 8 * 3600);
    let snapshot = planner
        .state()
        .pin_for_departure(probe, probe, probe + 3600)?;
    println!(
        "window: {} stops, {} routes, overlay arena {:?}; {}",
        snapshot.snapshot().stop_count(),
        snapshot.snapshot().route_count(),
        snapshot.snapshot().overlay_usage(),
        rss()
    );
    if rt_now.is_some() {
        println!("overlay stats: {:?}", snapshot.stats());
    }

    let locations: Vec<(StopId, f64, f64)> = snapshot
        .stop_locations()
        .map(|location| (location.stop, location.latitude, location.longitude))
        .collect();
    let tile_sets = TileSets::open(&[tiles.as_str()])?;
    let walking = costing(CostingType::Pedestrian)?;
    let mut tables = StopTables::new(&tile_sets, walking.clone());
    let snaps = snapshot.snaps().ok_or("no snaps")?;
    tables.bind(Arc::clone(snaps.for_mode(TravelMode::Walking)))?;
    let mut router: Router<TableTransfers> = Router::new(TableTransfers::empty());
    router
        .transfers_mut()
        .bind(Arc::clone(snaps.transfers_for_mode(TravelMode::Walking)));
    let options = ProfileOptions::default();

    for (name, from, to) in NAMED {
        let (from_stop, from_m) = nearest(&locations, from).ok_or("no stop")?;
        let (to_stop, to_m) = nearest(&locations, to).ok_or("no stop")?;
        println!(
            "== {name}: from stop {from_stop} ({from_m:.0} m), to stop {to_stop} ({to_m:.0} m)"
        );
        for (period, hour) in [("morning", 8), ("midday", 12), ("evening", 17)] {
            let departure = rt_now.unwrap_or(midnight + hour * 3600);
            let origin = snapshot.origin_unix();
            let from_seeds = tables.stop_seeds(from_stop)?;
            let to_seeds = tables.stop_seeds(to_stop)?;
            let (mut table_ms, mut now_ms, mut profile_ms, mut plan_ms) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for _ in 0..repeat {
                let t = Instant::now();
                let access: StopTable = tables
                    .compute(
                        snapshot.snapshot().stop_count(),
                        &TableRequest {
                            side: AccessSide::Access,
                            seeds: &from_seeds,
                            max_seconds: 1800,
                            interrupt: &|| false,
                        },
                    )?
                    .ok_or("interrupted")?;
                let egress: StopTable = tables
                    .compute(
                        snapshot.snapshot().stop_count(),
                        &TableRequest {
                            side: AccessSide::Egress,
                            seeds: &to_seeds,
                            max_seconds: 1800,
                            interrupt: &|| false,
                        },
                    )?
                    .ok_or("interrupted")?;
                table_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                let query = TransitQuery {
                    snapshot: Arc::clone(snapshot.snapshot()),
                    access: &access,
                    egress: &egress,
                    filter: RouteFilter::ALL,
                    max_trips: 8,
                };
                let at = i32::try_from(departure - origin)?;
                let t = Instant::now();
                let now = router
                    .leave_at(&query, Time::try_new(at)?, options, &|| false)
                    .map_err(|e| e.to_string())?;
                now_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                let t = Instant::now();
                let profile = router
                    .profile(
                        &query,
                        Time::try_new(at)?,
                        Time::try_new(at + 3600)?,
                        &|| false,
                    )
                    .map_err(|e| e.to_string())?;
                profile_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                let entries = profile.entries.len().max(now.entries.len());

                let t = Instant::now();
                let request = TransitRequest::new(
                    LatLon {
                        lat: from.1,
                        lon: from.0,
                    },
                    LatLon {
                        lat: to.1,
                        lon: to.0,
                    },
                    departure,
                    departure - 60,
                );
                let response = planner.plan(&request, "en-US", &|| false)?;
                plan_ms.push(t.elapsed().as_secs_f64() * 1000.0);
                if profile_ms.len() == repeat {
                    println!(
                        "  {period}: tables {:.1} ms, leave now {:.1} ms, 60-min profile {:.1} ms \
                         ({entries} entries), full plan with shapes {:.1} ms; {} journeys, level {:?}",
                        median(table_ms.clone()),
                        median(now_ms.clone()),
                        median(profile_ms.clone()),
                        median(plan_ms.clone()),
                        response.journeys.len(),
                        response.realtime.level,
                    );
                    for (i, journey) in response.journeys.iter().enumerate() {
                        let rides: Vec<String> = journey
                            .legs
                            .iter()
                            .filter_map(|leg| match leg {
                                JourneyLeg::Transit(leg) => Some(describe(leg, departure)),
                                _ => None,
                            })
                            .collect();
                        let walks: Vec<String> = journey
                            .legs
                            .iter()
                            .filter_map(|leg| match leg {
                                JourneyLeg::Transfer(t) => Some(format!(
                                    "{}s {}m -> {} ({} maneuvers, shape {} chars)",
                                    t.duration_seconds,
                                    t.walk.as_ref().map_or(0.0, |w| w.distance_meters),
                                    t.to.name.clone().unwrap_or_default(),
                                    t.walk.as_ref().map_or(0, |w| w.maneuvers.len()),
                                    t.walk.as_ref().map_or(0, |w| w.shape.len()),
                                )),
                                _ => None,
                            })
                            .collect();
                        println!(
                            "    [{i}] {}m, {} trips: {} | transfers: {}",
                            journey.duration_seconds / 60,
                            journey.trips,
                            rides.join(" + "),
                            walks.join("; "),
                        );
                    }
                }
            }
        }
    }
    print_splits(&planner, rt_now, midnight, repeat)?;
    println!("at the end: {}", rss());
    Ok(())
}
