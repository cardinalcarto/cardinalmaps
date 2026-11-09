//! Benchmarks the planner over a transit build: load, memory, and named trips across feeds.
//!
//! ```text
//! cargo run --release -p valinor-transit-plan --example multi_feed_bench -- \
//!     --build DIR --tiles PATH [--feeds a,b] [--date YYYYMMDD] [--rt DIR] [--repeat N]
//! ```
//!
//! `--rt DIR` ingests `tripupdates.pb`, `vehiclepositions.pb`, and `alerts.pb` for King County
//! Metro before planning, and plans at the capture's own time.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    reason = "a reporting tool"
)]

use core::num::NonZeroUsize;
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
use valinor_transit_plan::response::{JourneyLeg, TransitLeg};
use valinor_transit_plan::{
    FeedUpdate, StreetConfig, StreetPlanner, TransitBuild, TransitRequest, TravelMode,
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
    let tile_sets = TileSets::open(&[tiles.as_str()], NonZeroUsize::new(1024).ok_or("nonzero")?)?;
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
    println!("at the end: {}", rss());
    Ok(())
}
