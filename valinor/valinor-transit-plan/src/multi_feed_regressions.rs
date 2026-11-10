//! Regressions from reviewing multi-feed loading: isolation, corruption, and bad manifests.

use core::fmt::Write as _;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use valinor_gtfs::{FeedId as CompileId, FeedInput, InMemorySource, LatLon, LoadedTimetable};
use valinor_gtfs::{TimetableSet, compile};
use valinor_gtfs_rt::{FeedKind, proto};
use valinor_transit::Router;
use valinor_walk::transfer_tile::{TILE_FORMAT_VERSION, TILE_ZOOM};

use crate::build::TransitBuild;
use crate::plan::{TransitRequest, plan};
use crate::state::{FeedUpdate, StateConfig, TransitState};
use crate::testing::{
    JUNE_15, StraightLineAccess, StraightLineTransfers, encoded_feed, entity, june_15,
};
use crate::window::dates_around;

fn source(tz: &str, range: (&str, &str), stops: [(&str, f64, f64); 2]) -> InMemorySource {
    let mut stops_csv = String::from("stop_id,stop_name,stop_lat,stop_lon\n");
    for (id, lat, lon) in stops {
        writeln!(stops_csv, "{id},{id} stop,{lat},{lon}").expect("to a string");
    }
    let mut trips = String::from("route_id,service_id,trip_id,direction_id\n");
    let mut times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for hour in 0..26 {
        let id = format!("T{hour:02}");
        writeln!(trips, "R1,ALL,{id},0").expect("to a string");
        write!(
            times,
            "{id},{hour:02}:30:00,{hour:02}:30:00,S1,1\n{id},{hour:02}:50:00,{hour:02}:50:00,S2,2\n"
        )
        .expect("to a string");
    }
    InMemorySource::new()
        .with(
            "agency.txt",
            &format!(
                "agency_id,agency_name,agency_timezone,agency_url\nAG,Agency,{tz},https://a.test\n"
            ),
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nR1,AG,1,3\n",
        )
        .with("stops.txt", &stops_csv)
        .with(
            "calendar.txt",
            &format!(
                "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,\
                 start_date,end_date\nALL,1,1,1,1,1,1,1,{},{}\n",
                range.0, range.1
            ),
        )
        .with("trips.txt", &trips)
        .with("stop_times.txt", &times)
}

fn loaded(id: &str, source: InMemorySource) -> LoadedTimetable {
    let compiled = compile(vec![FeedInput {
        id: CompileId::try_new(id.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("loads")
}

fn state_of(files: Vec<LoadedTimetable>) -> TransitState {
    TransitState::from_set(
        TimetableSet::new(files).expect("set"),
        StateConfig::default(),
    )
    .expect("state")
}

/// Stop IDs in the journey's legs, plus every time, as JSON; `None` if the state can't answer.
fn answer(state: &TransitState, from: LatLon, to: LatLon, departure: i64) -> Option<Value> {
    answer_at(state, from, to, departure, departure - 60)
}

fn answer_at(
    state: &TransitState,
    from: LatLon,
    to: LatLon,
    departure: i64,
    now: i64,
) -> Option<Value> {
    let snapshot = state
        .pin_for_departure(now, departure, departure + 3600)
        .ok()?;
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        200.0,
        1.33,
    ));
    let mut request = TransitRequest::new(from, to, departure, now);
    request.window_seconds = 3600;
    let response = plan(
        state,
        &mut router,
        &mut StraightLineAccess::default(),
        &request,
        &|| false,
    )
    .expect("plan succeeds");
    Some(serde_json::to_value(&response.journeys).expect("serializable"))
}

/// A feed's answers don't change when another feed in a far-off time zone loads beside it,
/// for queries across a DST weekend, and past the end of one feed's calendar.
#[test]
fn a_set_answers_like_each_file_alone_across_zones_dst_and_coverage() {
    let zones = [
        (
            "America/Los_Angeles",
            ("20261001", "20261130"),
            "Pacific/Auckland",
            ("20261001", "20261130"),
        ),
        (
            "America/New_York",
            ("20261001", "20261130"),
            "Asia/Tokyo",
            ("20261001", "20261130"),
        ),
        // Feed alpha's calendar ends on the DST Sunday.
        (
            "America/Los_Angeles",
            ("20261001", "20261101"),
            "Pacific/Honolulu",
            ("20261001", "20261130"),
        ),
        // Feed alpha's calendar starts on the DST Sunday.
        (
            "America/Los_Angeles",
            ("20261101", "20261130"),
            "Pacific/Apia",
            ("20261001", "20261130"),
        ),
    ];
    let a_stops = [("S1", 47.60, -122.33), ("S2", 47.62, -122.33)];
    let b_stops = [("S1", -36.85, 174.76), ("S2", -36.87, 174.76)];
    let (a_from, a_to) = (
        LatLon {
            lat: 47.5995,
            lon: -122.33,
        },
        LatLon {
            lat: 47.6205,
            lon: -122.33,
        },
    );
    let (b_from, b_to) = (
        LatLon {
            lat: -36.8495,
            lon: 174.76,
        },
        LatLon {
            lat: -36.8705,
            lon: 174.76,
        },
    );
    let mut failures = Vec::new();
    let mut nonempty = 0usize;
    for (a_tz, a_range, b_tz, b_range) in zones {
        let a = || loaded("alpha", source(a_tz, a_range, a_stops));
        let b = || loaded("bravo", source(b_tz, b_range, b_stops));
        let both = state_of(vec![a(), b()]);
        let reversed = state_of(vec![b(), a()]);
        let alone_a = state_of(vec![a()]);
        let alone_b = state_of(vec![b()]);
        // 2026-10-31 00:00Z through 2026-11-03 00:00Z.
        let mut t = 1_793_404_800_i64;
        while t < 1_793_664_000 {
            for (state_name, set) in [("both", &both), ("reversed", &reversed)] {
                for (name, alone, from, to) in [
                    ("alpha", &alone_a, a_from, a_to),
                    ("bravo", &alone_b, b_from, b_to),
                ] {
                    for now in [t - 129_600, t + 129_600] {
                        let single = answer_at(alone, from, to, t, now).unwrap_or(json!([]));
                        let combined = answer_at(set, from, to, t, now).unwrap_or(json!([]));
                        if single != combined {
                            failures.push(format!(
                                "on-demand {a_tz}/{b_tz} {state_name} {name} now={now} at {t}: alone {single} vs set {combined}"
                            ));
                        }
                    }
                    let single = answer(alone, from, to, t).unwrap_or(json!([]));
                    if single.as_array().is_some_and(|a| !a.is_empty()) {
                        nonempty += 1;
                    }
                    let combined = answer(set, from, to, t).unwrap_or(json!([]));
                    if single != combined {
                        failures.push(format!(
                            "{a_tz}/{b_tz} {state_name} {name} at {t}: alone {single} vs set {combined}"
                        ));
                    }
                }
            }
            t += 97 * 60;
        }
    }
    eprintln!("nonempty single answers: {nonempty}");
    let shown: Vec<&str> = failures.iter().take(5).map(String::as_str).collect();
    assert!(
        failures.is_empty(),
        "{} mismatches, first: {shown:#?}",
        failures.len()
    );
}

fn ingest(state: &TransitState, feed: &str, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(june_15(8, 10), entities);
    state
        .ingest(FeedUpdate {
            feed,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: june_15(8, 10),
            stale_after: None,
        })
        .expect("known feed");
}

fn seattle(id: &str) -> LoadedTimetable {
    loaded(
        id,
        source(
            "America/Los_Angeles",
            ("20260601", "20260630"),
            [("S1", 47.60, -122.33), ("S2", 47.62, -122.33)],
        ),
    )
}

fn new_stop(id: &str) -> proto::FeedEntity {
    let mut item = entity(&format!("stop-{id}"));
    item.stop = Some(proto::Stop {
        stop_id: Some(id.to_owned()),
        stop_lat: Some(47.61),
        stop_lon: Some(-122.33),
        ..proto::Stop::default()
    });
    item
}

/// Feed `alpha` sending more new stops than one build allows must not cost `bravo` its own.
#[test]
fn a_flood_of_new_stops_in_one_feed_does_not_starve_another() {
    let state = state_of(vec![seattle("alpha"), seattle("bravo")]);
    let flood = (0..10_500).map(|i| new_stop(&format!("N{i:05}"))).collect();
    ingest(&state, "alpha", flood);
    ingest(&state, "bravo", vec![new_stop("BRAVO-ONLY")]);
    let snapshot = state.pin(june_15(8, 10)).expect("pin");
    let bravo_new_stops = snapshot
        .new_stops()
        .iter()
        .filter(|stop| stop.id.feed.as_str() == "bravo")
        .count();
    assert_eq!(
        bravo_new_stops,
        1,
        "bravo's only new stop was dropped; invalid stops: {}, new stops: {}",
        snapshot.stats().invalid_stops,
        snapshot.new_stops().len()
    );
}

fn added_trip(id: &str, hour: i64) -> proto::FeedEntity {
    let event = |time: i64| proto::trip_update::StopTimeEvent {
        time: Some(time),
        ..proto::trip_update::StopTimeEvent::default()
    };
    let stop = |stop: &str, seq: u32, time: i64| proto::trip_update::StopTimeUpdate {
        stop_sequence: Some(seq),
        stop_id: Some(stop.to_owned()),
        arrival: Some(event(time)),
        departure: Some(event(time)),
        ..proto::trip_update::StopTimeUpdate::default()
    };
    let depart = june_15(hour, 15);
    let mut item = entity(&format!("added-{id}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some(id.to_owned()),
            route_id: Some("R1".to_owned()),
            start_date: Some(JUNE_15.to_owned()),
            schedule_relationship: Some(proto::trip_descriptor::ScheduleRelationship::New as i32),
            ..proto::TripDescriptor::default()
        },
        stop_time_update: vec![stop("S1", 1, depart), stop("S2", 2, depart + 600)],
        ..proto::TripUpdate::default()
    });
    item
}

/// Feed `alpha` using up every added-trip slot must not stop `bravo`'s single added trip.
#[test]
fn a_flood_of_added_trips_in_one_feed_does_not_starve_another() {
    let state = state_of(vec![seattle("alpha"), seattle("bravo")]);
    let flood = (0..4_200)
        .map(|i| added_trip(&format!("A{i:05}"), 10))
        .collect();
    ingest(&state, "alpha", flood);
    ingest(&state, "bravo", vec![added_trip("BRAVO-ADDED", 11)]);
    let snapshot = state.pin(june_15(8, 10)).expect("pin");
    let bravo_routable = snapshot
        .rider
        .trips()
        .iter()
        .any(|trip| trip.trip_id == "BRAVO-ADDED" && trip.state.is_some());
    assert!(
        bravo_routable,
        "bravo's added trip was dropped (added: {}, invalid: {})",
        snapshot.stats().trips_added,
        snapshot.stats().invalid_added_trips
    );
}

fn hex_sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").expect("to a string");
            out
        })
}

fn feed_bytes(id: &str) -> Vec<u8> {
    compile(vec![FeedInput {
        id: CompileId::try_new(id.to_owned()).expect("id"),
        source: Box::new(source(
            "America/Los_Angeles",
            ("20260601", "20260630"),
            [("S1", 47.60, -122.33), ("S2", 47.62, -122.33)],
        )),
    }])
    .expect("compiles")
    .bytes
}

/// Saved snaps of two stops for both modes, and their manifest entries.
fn snap_entries(dir: &Path) -> Value {
    let bytes = valinor_walk::encode_snap_file(
        valhalla_graphtile::graph_tile::BuildId::new(77),
        &[None, None],
    );
    fs::create_dir_all(dir.join("snaps")).expect("mkdir");
    fs::write(dir.join("snaps/s.bin"), &bytes).expect("write");
    let entry = json!({ "file": "snaps/s.bin", "sha256": hex_sha(&bytes) });
    json!({ "walking": entry, "cycling": entry })
}

/// A build directory of one feed named by `timetable`, whose hash `bytes` must match.
fn build_with(dir: &Path, timetable: &str, bytes: &[u8]) {
    fs::create_dir_all(dir.join("transfers")).expect("mkdir");
    let manifest = json!({
        "manifest_id": "00000000000000ff",
        "valhalla_build_id": 77,
        "tile_format_version": TILE_FORMAT_VERSION,
        "zoom": TILE_ZOOM,
        "walking_bound_seconds": 900,
        "cycling_bound_seconds": 900,
        "feeds": [{
            "id": "evil",
            "timetable": timetable,
            "timetable_sha256": hex_sha(bytes),
            "stop_count": 2,
            "transfer_tiles": {"walking": [], "cycling": []},
            "stop_snaps": snap_entries(dir),
        }],
    });
    fs::write(
        dir.join("transfers/manifest.json"),
        serde_json::to_vec(&manifest).expect("json"),
    )
    .expect("write");
}

/// A manifest may only name files under its build.
#[test]
fn a_manifest_cannot_name_a_timetable_outside_its_build() {
    let elsewhere = TempDir::new().expect("temp");
    let outside = elsewhere.path().join("outside.timetable");
    let bytes = feed_bytes("evil");
    fs::write(&outside, &bytes).expect("write");
    let build = TempDir::new().expect("temp");
    build_with(build.path(), outside.to_str().expect("utf-8"), &bytes);
    let absolute = TransitBuild::load(build.path(), None);
    assert!(
        absolute.is_err(),
        "an absolute path outside the build was loaded: {absolute:?}"
    );
}

#[test]
fn a_manifest_cannot_traverse_out_of_its_build() {
    let root = TempDir::new().expect("temp");
    let bytes = feed_bytes("evil");
    fs::write(root.path().join("outside.timetable"), &bytes).expect("write");
    let build = root.path().join("build");
    build_with(&build, "../outside.timetable", &bytes);
    let relative = TransitBuild::load(&build, None);
    assert!(
        relative.is_err(),
        "a ../ path outside the build was loaded: {relative:?}"
    );
}

/// `/dev/zero` never ends, so reading it would never return; a manifest can't name it.
#[test]
fn a_manifest_naming_dev_zero_fails_cleanly() {
    let build = TempDir::new().expect("temp");
    build_with(build.path(), "/dev/zero", b"not relevant");
    let result = TransitBuild::load(build.path(), None);
    assert!(result.is_err());
}

/// A relative name can still be a link to a device; what it leads to is what's checked.
#[test]
fn a_manifest_cannot_name_a_link_to_a_device() {
    let build = TempDir::new().expect("temp");
    std::os::unix::fs::symlink("/dev/zero", build.path().join("timetable.bin")).expect("link");
    build_with(build.path(), "timetable.bin", b"not relevant");
    let result = TransitBuild::load(build.path(), None);
    assert!(
        matches!(
            result,
            Err(crate::build::BuildLoadError::UnsuitableFile { .. })
        ),
        "{result:?}"
    );
}

/// A file whose SHA-256 matches the manifest but whose bytes are hostile must be an error.
#[test]
fn random_corruption_of_a_timetable_never_panics() {
    let original = feed_bytes("evil");
    let dir = TempDir::new().expect("temp");
    let timetable_rel = "timetable.bin";
    let path: PathBuf = dir.path().join(timetable_rel);
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut panics = Vec::new();
    let (mut ok, mut refused) = (0, 0);
    for case in 0..3_000 {
        let mut bytes = original.clone();
        for _ in 0..=(next() % 6) {
            let at = usize::try_from(next() % bytes.len() as u64).expect("fits");
            bytes[at] = u8::try_from(next() & 0xff).expect("fits");
        }
        if case % 3 == 0 {
            let keep = usize::try_from(next() % bytes.len() as u64).expect("fits");
            bytes.truncate(keep);
        }
        fs::write(&path, &bytes).expect("write");
        build_with(dir.path(), timetable_rel, &bytes);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let Ok(build) = TransitBuild::load(dir.path(), None) else {
                return false;
            };
            let Ok(state) = TransitState::from_set(build.set, StateConfig::default()) else {
                return false;
            };
            for hour in [0, 8, 26] {
                let at = june_15(hour, 0);
                if let Ok(snapshot) = state.pin(at) {
                    let _ = snapshot.stop_locations().count();
                }
            }
            true
        }));
        match result {
            Ok(true) => ok += 1,
            Ok(false) => refused += 1,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
                    .unwrap_or_else(|| "non-string panic".to_owned());
                panics.push((case, message));
            }
        }
    }
    eprintln!(
        "corruption: {ok} survived, {refused} refused, {} panicked",
        panics.len()
    );
    assert!(
        panics.is_empty(),
        "first panics: {:#?}",
        &panics[..panics.len().min(5)]
    );
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn try_plan(state: &TransitState, departure: i64) {
    let now = departure - 60;
    let Ok(snapshot) = state.pin_for_departure(now, departure, departure + 3600) else {
        return;
    };
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        200.0,
        1.33,
    ));
    let mut request = TransitRequest::new(
        LatLon {
            lat: 47.5995,
            lon: -122.33,
        },
        LatLon {
            lat: 47.6205,
            lon: -122.33,
        },
        departure,
        now,
    );
    request.window_seconds = 7200;
    let _ = plan(
        state,
        &mut router,
        &mut StraightLineAccess::default(),
        &request,
        &|| false,
    );
}

/// As above, but with a checksum recomputed, as a hostile build would have one.
#[test]
fn random_corruption_with_a_valid_checksum_never_panics() {
    let original = feed_bytes("evil");
    let dir = TempDir::new().expect("temp");
    let path: PathBuf = dir.path().join("timetable.bin");
    let mut seed = 0x1234_5678_9ABC_DEF1_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut panics = Vec::new();
    let (mut ok, mut refused) = (0, 0);
    for case in 0..20_000 {
        let mut bytes = original.clone();
        let flips = 1 + next() % 8;
        for _ in 0..flips {
            let at = 16 + usize::try_from(next() % (bytes.len() as u64 - 16)).expect("fits");
            bytes[at] = u8::try_from(next() & 0xff).expect("fits");
        }
        let checksum = crc32(&bytes[16..]);
        bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
        fs::write(&path, &bytes).expect("write");
        build_with(dir.path(), "timetable.bin", &bytes);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let Ok(build) = TransitBuild::load(dir.path(), None) else {
                return false;
            };
            let Ok(state) = TransitState::from_set(build.set, StateConfig::default()) else {
                return false;
            };
            try_plan(&state, june_15(8, 0));
            try_plan(&state, june_15(30, 0));
            true
        }));
        match result {
            Ok(true) => ok += 1,
            Ok(false) => refused += 1,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(ToString::to_string))
                    .unwrap_or_else(|| "non-string panic".to_owned());
                panics.push((case, message));
            }
        }
    }
    eprintln!(
        "valid-checksum corruption: {ok} survived, {refused} refused, {} panicked",
        panics.len()
    );
    assert!(
        panics.is_empty(),
        "first panics: {:#?}",
        &panics[..panics.len().min(8)]
    );
}

/// How long the per-query date check takes over the real 26-file build.
#[test]
#[ignore = "needs the Washington build under /mnt/maps/cardinal/transit"]
fn pin_overhead_on_washington() {
    let dir = "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09";
    let start = std::time::Instant::now();
    let build = TransitBuild::load(Path::new(dir), None).expect("loads");
    eprintln!("load: {:?}, report {:?}", start.elapsed(), build.report);
    eprintln!("files: {}", build.set.part_count());
    let at = 1_794_238_000;
    let start = std::time::Instant::now();
    let mut days = 0;
    for offset in 0..100_000 {
        days += dates_around(&build.set, at + offset).len();
    }
    let elapsed = start.elapsed();
    eprintln!(
        "dates_around: {:?}/call over 26 files (sum of days {days})",
        elapsed / 100_000
    );
}

fn ingest_at(state: &TransitState, feed: &str, now: i64, entities: Vec<proto::FeedEntity>) {
    let bytes = encoded_feed(now, entities);
    state
        .ingest(FeedUpdate {
            feed,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: now,
            stale_after: Some(100_000),
        })
        .expect("known feed");
}

fn entity_delay(trip: &str, date: Option<&str>, delay: i32) -> proto::FeedEntity {
    use proto::trip_update::{StopTimeEvent, StopTimeUpdate};
    let mut item = entity(&format!("d-{trip}"));
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some(trip.to_owned()),
            start_date: date.map(str::to_owned),
            ..proto::TripDescriptor::default()
        },
        stop_time_update: vec![StopTimeUpdate {
            stop_sequence: Some(1),
            departure: Some(StopTimeEvent {
                delay: Some(delay),
                ..StopTimeEvent::default()
            }),
            arrival: Some(StopTimeEvent {
                delay: Some(delay),
                ..StopTimeEvent::default()
            }),
            ..StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    item
}

fn entity_time(trip: &str, date: &str, time: i64) -> proto::FeedEntity {
    use proto::trip_update::{StopTimeEvent, StopTimeUpdate};
    let mut item = entity(&format!("t-{trip}"));
    let event = StopTimeEvent {
        time: Some(time),
        ..StopTimeEvent::default()
    };
    item.trip_update = Some(proto::TripUpdate {
        trip: proto::TripDescriptor {
            trip_id: Some(trip.to_owned()),
            start_date: Some(date.to_owned()),
            ..proto::TripDescriptor::default()
        },
        stop_time_update: vec![StopTimeUpdate {
            stop_sequence: Some(1),
            departure: Some(event),
            arrival: Some(event),
            ..StopTimeUpdate::default()
        }],
        ..proto::TripUpdate::default()
    });
    item
}

fn cancel_run(route: &str, date: &str, start: &str) -> proto::FeedEntity {
    let mut item = crate::testing::alert_entity(
        "no-service",
        proto::alert::Effect::NoService,
        vec![proto::EntitySelector {
            trip: Some(proto::TripDescriptor {
                route_id: Some(route.to_owned()),
                direction_id: Some(0),
                start_date: Some(date.to_owned()),
                start_time: Some(start.to_owned()),
                ..proto::TripDescriptor::default()
            }),
            ..proto::EntitySelector::default()
        }],
    );
    item.id = format!("alert-{start}");
    item
}

/// Real-time data on a feed far from the origin's zone applies as it does alone, and a
/// `NO_SERVICE` alert on the same-named route in another feed never reaches it.
#[test]
fn realtime_on_a_far_zone_feed_matches_it_alone() {
    let date = "20261102";
    let a = || {
        loaded(
            "alpha",
            source(
                "America/Los_Angeles",
                ("20261001", "20261130"),
                [("S1", 47.60, -122.33), ("S2", 47.62, -122.33)],
            ),
        )
    };
    let b = || {
        loaded(
            "bravo",
            source(
                "Pacific/Auckland",
                ("20261001", "20261130"),
                [("S1", -36.85, 174.76), ("S2", -36.87, 174.76)],
            ),
        )
    };
    // 2026-11-02 03:00 NZDT is 2026-11-01 14:00Z.
    let depart = 1_793_628_000_i64;
    let t03_start = depart + 1800;
    let now = depart - 300;
    let entities = || {
        vec![
            entity_time("T03", date, t03_start + 180),
            entity_delay("T05", None, 60),
            cancel_run("R1", date, "04:30:00"),
        ]
    };
    let set = state_of(vec![a(), b()]);
    let alone = state_of(vec![b()]);
    ingest_at(&set, "bravo", now, entities());
    ingest_at(&alone, "bravo", now, entities());
    // The same route name in alpha is removed entirely.
    let wipe = crate::testing::alert_entity(
        "wipe",
        proto::alert::Effect::NoService,
        vec![crate::testing::route_selector("R1")],
    );
    ingest_at(&set, "alpha", now, vec![wipe]);

    let from = LatLon {
        lat: -36.8495,
        lon: 174.76,
    };
    let to = LatLon {
        lat: -36.8705,
        lon: 174.76,
    };
    let got = answer_at(&set, from, to, depart, now).expect("answers");
    let want = answer_at(&alone, from, to, depart, now).expect("answers");
    let snapshot = set.pin(now).expect("pin");
    eprintln!(
        "stats: matched {} outside {} unmatched {}",
        snapshot.stats().trip_updates_matched,
        snapshot.stats().trip_updates_outside_window,
        snapshot.stats().trip_updates_unmatched
    );
    assert!(
        got.as_array().is_some_and(|j| !j.is_empty()),
        "no journeys: {got}"
    );
    assert_eq!(got, want);
    // The trips ridden; a leg's `departures` also name the runs around it, which may be others.
    let ridden: Vec<&str> = got
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|journey| journey["legs"].as_array().into_iter().flatten())
        .filter(|leg| leg["type"] == "transit")
        .filter_map(|leg| leg["trip"]["trip_id"].as_str())
        .collect();
    assert!(ridden.contains(&"T03"), "{got}");
    assert!(
        !ridden.contains(&"T04"),
        "the no-service run was still offered: {got}"
    );
}

fn many_trips_source() -> InMemorySource {
    let mut trips = String::from("route_id,service_id,trip_id,direction_id\n");
    let mut times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
    for i in 0..2000_u32 {
        writeln!(trips, "R1,ALL,M{i},0").expect("to a string");
        let start = 5 * 3600 + i * 30;
        let (h, m, s) = (start / 3600, start % 3600 / 60, start % 60);
        let later = start + 300;
        let (h2, m2, s2) = (later / 3600, later % 3600 / 60, later % 60);
        write!(
            times,
            "M{i},{h:02}:{m:02}:{s:02},{h:02}:{m:02}:{s:02},S1,1\n\
             M{i},{h2:02}:{m2:02}:{s2:02},{h2:02}:{m2:02}:{s2:02},S2,2\n"
        )
        .expect("to a string");
    }
    InMemorySource::new()
        .with(
            "agency.txt",
            "agency_id,agency_name,agency_timezone,agency_url\nAG,Agency,America/Los_Angeles,https://a.test\n",
        )
        .with(
            "routes.txt",
            "route_id,agency_id,route_short_name,route_type\nR1,AG,1,3\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\nS1,S1,47.60,-122.33\nS2,S2,47.62,-122.33\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nALL,1,1,1,1,1,1,1,20260601,20260630\n",
        )
        .with("trips.txt", &trips)
        .with("stop_times.txt", &times)
}

/// Feed `alpha`'s alerts use up the alert budget, and `bravo`'s `NO_SERVICE` is silently dropped.
#[test]
fn one_feeds_alerts_cannot_exhaust_another_feeds_no_service_budget() {
    let alpha = loaded("alpha", many_trips_source());
    let bravo = loaded(
        "bravo",
        source(
            "America/Los_Angeles",
            ("20260601", "20260630"),
            [("S1", 47.40, -122.33), ("S2", 47.42, -122.33)],
        ),
    );
    let state = state_of(vec![alpha, bravo]);
    let selector = |minute: u32| proto::EntitySelector {
        trip: Some(proto::TripDescriptor {
            route_id: Some("R1".to_owned()),
            direction_id: Some(0),
            start_date: Some(JUNE_15.to_owned()),
            start_time: Some(format!("06:{minute:02}:00")),
            ..proto::TripDescriptor::default()
        }),
        ..proto::EntitySelector::default()
    };
    // Each run lookup charges the block's 2000 trips: 26,000 selectors spend 52M of 50M.
    let flood: Vec<proto::FeedEntity> = (0..26_000_u32)
        .collect::<Vec<_>>()
        .chunks(200)
        .enumerate()
        .map(|(n, chunk)| {
            let selectors = chunk.iter().map(|&i| selector(i % 60)).collect();
            let mut item = crate::testing::alert_entity(
                &format!("flood-{n}"),
                proto::alert::Effect::NoService,
                selectors,
            );
            item.alert.as_mut().expect("an alert").effect =
                Some(proto::alert::Effect::NoService as i32);
            item
        })
        .collect();
    let now = june_15(8, 0);
    let bravo_alert = crate::testing::alert_entity(
        "bravo-cut",
        proto::alert::Effect::NoService,
        vec![crate::testing::route_selector("R1")],
    );
    ingest_at(&state, "alpha", now, flood);
    ingest_at(&state, "bravo", now, vec![bravo_alert]);
    let from = LatLon {
        lat: 47.3995,
        lon: -122.33,
    };
    let to = LatLon {
        lat: 47.4205,
        lon: -122.33,
    };
    let snapshot = state.pin(now).expect("pin");
    let journeys = answer_at(&state, from, to, now, now).expect("answers");
    let exhausted = snapshot.stats().alert_work_exhausted;
    eprintln!(
        "alerts {} nosvc {} invalid sel {} removed {} dates {:?}",
        snapshot.stats().alerts,
        snapshot.stats().no_service_alerts,
        snapshot.stats().invalid_selectors,
        snapshot.stats().trips_removed,
        snapshot.window.dates
    );
    eprintln!("feed states {:?}", snapshot.feed_status_at(now));
    eprintln!(
        "alert work exhausted: {exhausted}; bravo journeys: {}",
        journeys.as_array().map_or(0, Vec::len)
    );
    assert!(
        journeys.as_array().is_some_and(Vec::is_empty),
        "bravo's NO_SERVICE alert was dropped by alpha's work (exhausted: {exhausted}): {journeys}"
    );
}
