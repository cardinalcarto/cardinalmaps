//! End-to-end tests on real King County data, kept out of git and run explicitly.
//!
//! These compile the static feed and replay captured GTFS-RT from
//! `/mnt/maps/cardinal/transit/kcm`. Run with:
//!
//! ```text
//! cargo test -p valinor-transit-plan --release e2e -- --ignored --nocapture
//! ```
//!
//! Set `TRANSIT_E2E_JSON` to a path to write one real journey's response JSON.
//!
//! Each query's `now` is the capture's own header timestamp, so its real-time data is fresh.
//! Daytime queries use the schedule alone, because captures only exist overnight.

#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::unwrap_used,
    reason = "reporting tests"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use valinor_gtfs::{
    FeedId as CompileFeedId, FeedInput as CompileInput, LatLon, LoadedTimetable, ZipSource, compile,
};
use valinor_gtfs_rt::FeedKind;
use valinor_transit::Router;

use crate::plan::{TransitRequest, plan};
use crate::response::{JourneyLeg, PlanResponse, TransitLeg};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{StraightLineAccess, StraightLineTransfers, StraightWalk};

const ROOT: &str = "/mnt/maps/cardinal/transit/kcm";
const FEED: &str = "kcm";

fn compiled_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let zip = Path::new(ROOT).join("static/kcm-FAL26-161.2.zip");
        let started = Instant::now();
        let output = compile(vec![CompileInput {
            id: CompileFeedId::try_new(FEED.to_owned()).unwrap(),
            source: Box::new(ZipSource::from_path(&zip).unwrap()),
        }])
        .unwrap();
        println!("compile: {:?}", started.elapsed());
        output.bytes
    })
}

fn state() -> TransitState {
    let started = Instant::now();
    let loaded = LoadedTimetable::from_bytes(compiled_bytes().to_vec()).unwrap();
    let state = TransitState::new(loaded, StateConfig::default()).unwrap();
    println!("TransitState::new: {:?}", started.elapsed());
    state
}

/// The capture directories under `rt/`, oldest first.
fn captures() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = fs::read_dir(Path::new(ROOT).join("rt"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    directories
}

/// The first capture at or after `stamp`, a `YYYYMMDDTHHMMSSZ` directory name.
fn capture_at(stamp: &str) -> PathBuf {
    captures()
        .into_iter()
        .find(|path| path.file_name().unwrap().to_str().unwrap() >= stamp)
        .unwrap()
}

/// Ingests one capture's three feeds and returns the moment to query at.
fn ingest_capture(state: &TransitState, directory: &Path) -> i64 {
    let mut newest = 0;
    for (file, kind) in [
        ("tripupdates.pb", FeedKind::TripUpdates),
        ("vehiclepositions.pb", FeedKind::VehiclePositions),
        ("alerts.pb", FeedKind::Alerts),
    ] {
        let bytes = fs::read(directory.join(file)).unwrap();
        // The file names the feed's own header time; fall back to the directory if needed.
        let decoded = valinor_gtfs_rt::DecodedFeed::decode(&bytes).unwrap();
        let timestamp = decoded.timestamp().unwrap_or(0);
        newest = newest.max(timestamp);
        state
            .ingest(FeedUpdate {
                feed: FEED,
                kind,
                bytes: Some(&bytes),
                received_at: timestamp,
                stale_after: None,
            })
            .unwrap();
    }
    newest
}

fn transit_legs<W>(journey: &crate::response::Journey<W>) -> Vec<&TransitLeg> {
    journey
        .legs
        .iter()
        .filter_map(|leg| match leg {
            JourneyLeg::Transit(leg) => Some(leg.as_ref()),
            _ => None,
        })
        .collect()
}

fn query(
    state: &TransitState,
    name: &str,
    from: LatLon,
    to: LatLon,
    now: i64,
    departure: i64,
) -> PlanResponse<StraightWalk> {
    let snapshot = state.pin(now).unwrap();
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        800.0,
        1.33,
    ));
    let mut access = StraightLineAccess::default();
    let request = TransitRequest::new(from, to, departure, now);
    let started = Instant::now();
    let response = plan(state, &mut router, &mut access, &request, &|| false).unwrap();
    let elapsed = started.elapsed();
    println!(
        "{name}: {} journeys in {elapsed:?} ({:?})",
        response.journeys.len(),
        response.realtime.level,
    );
    for journey in &response.journeys {
        let legs = transit_legs(journey);
        let route_names: Vec<String> = legs
            .iter()
            .map(|leg| {
                leg.route
                    .as_ref()
                    .and_then(|r| r.short_name.clone())
                    .unwrap_or_default()
            })
            .collect();
        let realtime = legs.iter().filter(|leg| leg.realtime).count();
        let alerts = legs.iter().map(|leg| leg.alerts.len()).sum::<usize>();
        let vehicle = legs.iter().filter(|leg| leg.vehicle.is_some()).count();
        println!(
            "  dep +{:>4}s arr +{:>5}s {} trips [{}] walk {}s realtime-legs {realtime} vehicles {vehicle} alerts {alerts}",
            journey.departure - departure,
            journey.arrival - departure,
            journey.trips,
            route_names.join(", "),
            journey.walking.total_seconds,
        );
    }
    response
}

/// Overnight service, replayed at its own capture time, with real-time data applied.
#[test]
#[ignore = "needs the real King County data under /mnt/maps/cardinal/transit"]
fn e2e_overnight_replay() {
    let state = state();
    let directory = capture_at("20261109T073000Z");
    println!("capture: {}", directory.display());
    let started = Instant::now();
    let now = ingest_capture(&state, &directory);
    println!("now = {now}, ingest {:?}", started.elapsed());
    let started = Instant::now();
    let snapshot = state.refresh(now).unwrap();
    println!("overlay {:?}: {:?}", started.elapsed(), snapshot.stats());
    assert!(snapshot.stats().trip_updates_matched > 100);

    let cases = [
        (
            "3rd/Pine to U District (49)",
            point(47.6105, -122.3376),
            point(47.6585, -122.3130),
        ),
        (
            "3rd/Pine to Aurora 130th (E)",
            point(47.6105, -122.3376),
            point(47.7248, -122.3448),
        ),
        (
            "Tukwila to Federal Way (A)",
            point(47.4640, -122.2880),
            point(47.3180, -122.3095),
        ),
        (
            "Rainier Beach to Downtown (7)",
            point(47.5223, -122.2660),
            point(47.6020, -122.3300),
        ),
        (
            "Ballard to U District (44)",
            point(47.6685, -122.3840),
            point(47.6597, -122.3170),
        ),
    ];
    let mut json_written = false;
    for (name, from, to) in cases {
        let response = query(&state, name, from, to, now, now);
        assert!(!response.journeys.is_empty(), "{name} has no journey");
        assert_ne!(
            response.realtime.level,
            crate::response::RealtimeLevel::Schedule
        );
        assert!(
            response
                .journeys
                .iter()
                .flat_map(transit_legs)
                .any(|leg| leg.realtime),
            "{name} has no real-time leg"
        );
        if !json_written
            && let Ok(path) = std::env::var("TRANSIT_E2E_JSON")
            && let Some(journey) = response
                .journeys
                .iter()
                .find(|j| transit_legs(j).iter().any(|leg| leg.vehicle.is_some()))
        {
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "generated_at": response.generated_at,
                "window": response.window,
                "realtime": response.realtime,
                "journey": journey,
            }))
            .unwrap();
            fs::write(path, body).unwrap();
            json_written = true;
        }
    }

    // Replaying a later capture updates the snapshot in place.
    let later = capture_at("20261109T083000Z");
    let later_now = ingest_capture(&state, &later);
    let started = Instant::now();
    let snapshot = state.refresh(later_now).unwrap();
    println!(
        "later capture {}: overlay {:?}, {} matched updates, {} vehicles",
        later.display(),
        started.elapsed(),
        snapshot.stats().trip_updates_matched,
        snapshot.stats().vehicles_matched,
    );
    let response = query(
        &state,
        "later: 3rd/Pine to U District",
        point(47.6105, -122.3376),
        point(47.6585, -122.3130),
        later_now,
        later_now,
    );
    assert!(!response.journeys.is_empty());
    println!(
        "feed ages at query: {:?}",
        response
            .realtime
            .feeds
            .iter()
            .map(|f| (f.kind, f.age_seconds))
            .collect::<Vec<_>>()
    );
}

/// A stale capture degrades every feed but alerts to the schedule.
#[test]
#[ignore = "needs the real King County data under /mnt/maps/cardinal/transit"]
fn e2e_stale_capture_falls_back_to_schedule() {
    let state = state();
    let now = ingest_capture(&state, &capture_at("20261109T073000Z"));
    let response = query(
        &state,
        "20 minutes after the capture",
        point(47.6105, -122.3376),
        point(47.6585, -122.3130),
        now + 1200,
        now + 1200,
    );
    assert_eq!(
        response.realtime.level,
        crate::response::RealtimeLevel::Degraded
    );
    assert!(
        response
            .journeys
            .iter()
            .flat_map(transit_legs)
            .all(|leg| leg.vehicle.is_none())
    );
}

/// Daytime routing on the schedule alone; there are no daytime captures to replay.
#[test]
#[ignore = "needs the real King County data under /mnt/maps/cardinal/transit"]
fn e2e_daytime_schedule() {
    let state = state();
    // Monday 2026-11-09 08:30 PST.
    // 2026-11-09T00:00:00-08:00 is 1794211200.
    let now = 1_794_211_200 + 8 * 3600 + 30 * 60;
    let cases = [
        (
            "Capitol Hill to Bellevue",
            point(47.6210, -122.3210),
            point(47.6190, -122.1990),
        ),
        (
            "Northgate to Downtown",
            point(47.7063, -122.3280),
            point(47.6050, -122.3325),
        ),
        (
            "White Center to UW",
            point(47.5150, -122.3550),
            point(47.6553, -122.3100),
        ),
    ];
    for (name, from, to) in cases {
        let response = query(&state, name, from, to, now, now);
        assert!(!response.journeys.is_empty(), "{name} has no journey");
        assert_eq!(
            response.realtime.level,
            crate::response::RealtimeLevel::Schedule
        );
    }
}

/// Every `STOP_MOVED` alert that names a stop: try to board there and find the alert marked.
#[test]
#[ignore = "needs the real King County data under /mnt/maps/cardinal/transit"]
fn e2e_stop_moved_marks_the_boarding_leg() {
    let state = state();
    let now = ingest_capture(&state, &capture_at("20261109T073000Z"));
    let snapshot = state.refresh(now).unwrap();
    let compiled = *snapshot.data.loaded.timetable();

    let mut moved = Vec::new();
    for alert in snapshot.rider().alerts() {
        if alert.effect != Some(valinor_gtfs_rt::proto::alert::Effect::StopMoved) {
            continue;
        }
        for selector in &alert.informed {
            if let (Some(route), Some(stop)) =
                (selector.route_id.as_deref(), selector.stop_id.as_deref())
            {
                moved.push((route.to_owned(), stop.to_owned()));
            }
        }
    }
    println!(
        "{} route-and-stop selectors on STOP_MOVED alerts",
        moved.len()
    );
    assert!(!moved.is_empty());

    let mut asked = 0;
    let mut marked = Vec::new();
    for (route_id, stop_id) in &moved {
        let Some(route) = compiled.route_by_gtfs_id(FEED, route_id) else {
            continue;
        };
        let Some(patterns) = compiled.route_patterns(route) else {
            continue;
        };
        let Some(stop) = compiled.stop_by_gtfs_id(FEED, stop_id) else {
            continue;
        };
        // Ride several stops along a pattern that serves it, so walking isn't quicker.
        let Some(target) = patterns.iter().find_map(|&pattern| {
            let stops: Vec<_> = compiled.pattern(pattern)?.stops().collect();
            let position = stops.iter().position(|&s| s == stop)?;
            stops.get(position + 8).copied()
        }) else {
            continue;
        };
        let (Some(origin), Some(destination)) = (
            compiled.stop(stop).and_then(|info| info.coordinate),
            compiled.stop(target).and_then(|info| info.coordinate),
        ) else {
            continue;
        };
        asked += 1;
        // Overnight service is thin; ask at midday, on the schedule's times, with the alert.
        let departure = 1_794_211_200 + 12 * 3600;
        let response = query(
            &state,
            &format!("board at moved stop {stop_id} on {route_id}"),
            origin,
            destination,
            now,
            departure,
        );
        for journey in &response.journeys {
            for leg in transit_legs(journey) {
                for alert in &leg.alerts {
                    if let Some(moved_stops) = &alert.moved_stops {
                        marked.push((stop_id.clone(), moved_stops.clone()));
                    }
                }
            }
        }
    }
    println!("asked {asked}; marked legs: {}", marked.len());
    if let Some((stop, moved_stops)) = marked.first() {
        println!("first marked: stop {stop}, {moved_stops:?}");
    }
    assert!(!marked.is_empty(), "no leg was marked");
}

fn point(lat: f64, lon: f64) -> LatLon {
    LatLon { lat, lon }
}
