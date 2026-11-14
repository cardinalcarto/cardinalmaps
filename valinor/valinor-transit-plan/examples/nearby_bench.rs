//! Measures nearby departures over a transit build: latency by stage, from named places.
//!
//! ```text
//! cargo run --release -p valinor-transit-plan --example nearby_bench -- \
//!     --build DIR --tiles PATH [--feeds a,b] [--walk SECONDS] [--rt DIR] [--repeat N]
//! ```
//!
//! For each place, period, and travel mode, it prints the median and slowest of `--repeat` warm
//! queries after one cold one, how many routes and pages were listed, and the response's
//! size, then the median time of each stage. `--rt DIR` ingests a King County Metro capture
//! (`tripupdates.pb`, `vehiclepositions.pb`, and `alerts.pb`) and queries at its time.

#![allow(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "a reporting tool"
)]

use core::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use valinor_gtfs::LatLon;
use valinor_gtfs_rt::FeedKind;
use valinor_transit_plan::{
    FeedUpdate, NearbyRequest, PlanStage, StreetConfig, StreetPlanner, TransitBuild, TravelMode,
};

const WASHINGTON: &str = "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09";
const STREETS: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const METRO: &str = "us-wa-Metro-Transit";
/// 2026-11-10 00:00 Pacific, a Tuesday.
const TUESDAY_MIDNIGHT: i64 = 1_794_297_600;

/// `(name, (lon, lat))`.
const PLACES: [(&str, (f64, f64)); 5] = [
    ("Fremont", (-122.349_869, 47.649_735)),
    ("Downtown (Westlake)", (-122.336_9, 47.611_5)),
    ("Capitol Hill", (-122.320_3, 47.619_2)),
    ("Ballard", (-122.385_345, 47.668_606)),
    ("Kent East Hill", (-122.186_5, 47.380_5)),
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

fn ingest_rt(planner: &StreetPlanner, directory: &Path) -> Result<i64, Box<dyn Error>> {
    let name = directory
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("capture name")?;
    let now = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", name)?
        .to_zoned(jiff::tz::TimeZone::UTC)?
        .timestamp()
        .as_second();
    for (file, kind) in [
        ("tripupdates.pb", FeedKind::TripUpdates),
        ("vehiclepositions.pb", FeedKind::VehiclePositions),
        ("alerts.pb", FeedKind::Alerts),
    ] {
        let bytes = std::fs::read(directory.join(file))?;
        let result = planner.state().ingest_and_refresh(
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

/// Where a query's time went, attributing each interval to the stage it was in.
struct StageClock {
    at: Cell<Instant>,
    current: Cell<PlanStage>,
    totals: RefCell<BTreeMap<PlanStage, f64>>,
}

impl StageClock {
    fn new() -> Self {
        Self {
            at: Cell::new(Instant::now()),
            current: Cell::new(PlanStage::Setup),
            totals: RefCell::new(BTreeMap::new()),
        }
    }

    fn enter(&self, stage: PlanStage) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.at.get()).as_secs_f64() * 1000.0;
        *self
            .totals
            .borrow_mut()
            .entry(self.current.get())
            .or_insert(0.0) += elapsed;
        self.at.set(now);
        self.current.set(stage);
    }

    fn finish(self) -> BTreeMap<PlanStage, f64> {
        self.enter(PlanStage::Release);
        self.totals.into_inner()
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn main() -> Result<(), Box<dyn Error>> {
    let build_dir = PathBuf::from(flag("build").unwrap_or_else(|| WASHINGTON.to_owned()));
    let tiles = flag("tiles").unwrap_or_else(|| STREETS.to_owned());
    let feeds: Option<Vec<String>> =
        flag("feeds").map(|text| text.split(',').map(str::to_owned).collect());
    let repeat: usize = flag("repeat").and_then(|n| n.parse().ok()).unwrap_or(5);
    let walk: u32 = flag("walk").and_then(|n| n.parse().ok()).unwrap_or(1800);

    let started = Instant::now();
    let build = TransitBuild::load(&build_dir, feeds.as_deref())?;
    println!(
        "loaded {} files, {} stops in {:.2?}",
        build.set.part_count(),
        build.set.stop_count(),
        started.elapsed()
    );
    let planner = StreetPlanner::from_build(build, &[tiles.as_str()], StreetConfig::default())?;
    let rt_now = match flag("rt") {
        Some(directory) => Some(ingest_rt(&planner, Path::new(&directory))?),
        None => None,
    };

    let periods: Vec<(&str, i64)> = match rt_now {
        Some(now) => vec![("capture", now)],
        None => vec![
            ("morning", TUESDAY_MIDNIGHT + 8 * 3600),
            ("midday", TUESDAY_MIDNIGHT + 12 * 3600),
            ("evening", TUESDAY_MIDNIGHT + 17 * 3600),
        ],
    };
    for (name, (lon, lat)) in PLACES {
        println!("== {name}, a walk of up to {walk} s");
        for &(period, time) in &periods {
            for mode in [TravelMode::Walking, TravelMode::Wheelchair] {
                let request = NearbyRequest {
                    mode,
                    max_walk_seconds: walk,
                    ..NearbyRequest::new(LatLon { lat, lon }, time, time)
                };
                let t = Instant::now();
                let cold = planner.nearby(&request, &|| false)?;
                let cold_ms = t.elapsed().as_secs_f64() * 1000.0;
                let mut totals = Vec::new();
                let mut stages: BTreeMap<PlanStage, Vec<f64>> = BTreeMap::new();
                for _ in 0..repeat {
                    let clock = StageClock::new();
                    let t = Instant::now();
                    planner.nearby_probed(&request, &|| false, &|stage| clock.enter(stage))?;
                    totals.push(t.elapsed().as_secs_f64() * 1000.0);
                    for (stage, ms) in clock.finish() {
                        stages.entry(stage).or_default().push(ms);
                    }
                }
                let slowest = totals.iter().copied().fold(0.0, f64::max);
                let pages: usize = cold.routes.iter().map(|r| r.pages.len()).sum();
                let stops: std::collections::BTreeSet<&str> = cold
                    .routes
                    .iter()
                    .flat_map(|r| &r.pages)
                    .map(|p| p.stop.stop_id.as_str())
                    .collect();
                let bytes = serde_json::to_vec(&cold)?.len();
                println!(
                    "{period:>8} {mode:>10?}: cold {cold_ms:7.1} ms, warm median {:7.1} ms, \
                     max {slowest:7.1} ms; {} routes, {pages} pages from {} stops, \
                     {bytes} bytes",
                    median(totals),
                    cold.routes.len(),
                    stops.len(),
                );
                let line: Vec<String> = stages
                    .into_iter()
                    .filter(|(stage, _)| *stage != PlanStage::Release)
                    .map(|(stage, ms)| format!("{stage:?} {:.1}", median(ms)))
                    .collect();
                println!("{:>21}  {}", "", line.join(", "));
            }
        }
    }
    Ok(())
}
