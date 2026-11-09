//! Loading a build directory: the manifest's checks, and a client's partial tile set.

use core::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use valinor_gtfs::{FeedId, FeedInput, InMemorySource, LatLon, compile};
use valinor_transit::StopId;
use valinor_walk::transfer_tile::{TileWalk, TransferMode, TransferTile, TransferTileRow};

use crate::build::{BuildLoadError, TransitBuild};
use crate::plan::TransitRequest;
use crate::street::{OpenPlannerError, StreetConfig, StreetPlanner};

const MANIFEST_ID: u64 = 0x1234_5678_9abc_def0;
const BOUND: u32 = 900;

fn feed_bytes(id: &str, stops: &[(&str, f64, f64)]) -> Vec<u8> {
    let mut csv = String::from("stop_id,stop_name,stop_lat,stop_lon\n");
    for (name, lat, lon) in stops {
        csv.push_str(name);
        writeln!(csv, ",{name},{lat},{lon}").expect("to a string");
    }
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_name,agency_timezone,agency_url\nAG,America/Los_Angeles,https://a.test\n",
        )
        .with("routes.txt", "route_id,route_short_name,route_type\nR1,1,3\n")
        .with("stops.txt", &csv)
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,1,1,20260601,20260630\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR1,WK,T1\n")
        .with(
            "stop_times.txt",
            &format!(
                "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
                 T1,08:00:00,08:00:00,{},1\nT1,08:10:00,08:10:00,{},2\n",
                stops[0].0, stops[1].0
            ),
        );
    compile(vec![FeedInput {
        id: FeedId::try_new(id.to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compiles")
    .bytes
}

fn hex_sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").expect("to a string");
            out
        })
}

fn write_tile(dir: &Path, mode: TransferMode, x: u32, y: u32, rows: &[(u32, u32, Vec<TileWalk>)]) {
    let rows: Vec<TransferTileRow<'_>> = rows
        .iter()
        .map(|(feed, stop, walks)| TransferTileRow {
            feed: *feed,
            stop: *stop,
            walks,
        })
        .collect();
    let tile = TransferTile::from_rows(mode, (x, y), MANIFEST_ID, BOUND, &rows).expect("valid");
    let path = dir
        .join("transfers")
        .join(mode.as_str())
        .join("12")
        .join(x.to_string());
    fs::create_dir_all(&path).expect("mkdir");
    let compressed = zstd::bulk::compress(tile.as_bytes(), 3).expect("compress");
    fs::write(path.join(format!("{y}.bin.zst")), compressed).expect("write");
}

/// A build of feeds `one` and `two`, and tiles at (10, 20) and (11, 20).
struct Fixture {
    dir: TempDir,
    manifest: serde_json::Value,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp");
        fs::create_dir_all(dir.path().join("timetables")).expect("mkdir");
        fs::create_dir_all(dir.path().join("transfers")).expect("mkdir");
        let one = feed_bytes("one", &[("A", 47.60, -122.33), ("B", 47.61, -122.33)]);
        let two = feed_bytes("two", &[("A", 47.70, -122.33), ("B", 47.71, -122.33)]);
        fs::write(dir.path().join("timetables/one.timetable"), &one).expect("write");
        fs::write(dir.path().join("timetables/two.timetable"), &two).expect("write");
        let tiles = |tiles: &[[u32; 2]]| json!({ "walking": tiles.to_vec(), "cycling": [] });
        let manifest = json!({
            "manifest_id": format!("{MANIFEST_ID:016x}"),
            "valhalla_build_id": 77,
            "tile_format_version": valinor_walk::transfer_tile::TILE_FORMAT_VERSION,
            "zoom": 12,
            "walking_bound_seconds": BOUND,
            "cycling_bound_seconds": BOUND,
            "feeds": [
                {
                    "id": "one",
                    "timetable": "timetables/one.timetable",
                    "timetable_sha256": hex_sha(&one),
                    "stop_count": 2,
                    "transfer_tiles": tiles(&[[10, 20]]),
                },
                {
                    "id": "failed",
                    "timetable": null,
                    "timetable_sha256": null,
                    "stop_count": 0,
                    "transfer_tiles": tiles(&[]),
                },
                {
                    "id": "two",
                    "timetable": "timetables/two.timetable",
                    "timetable_sha256": hex_sha(&two),
                    "stop_count": 2,
                    "transfer_tiles": tiles(&[[11, 20]]),
                },
            ],
        });
        let fixture = Self { dir, manifest };
        fixture.write_manifest();
        write_tile(
            fixture.path(),
            TransferMode::Walking,
            10,
            20,
            &[(
                0,
                1,
                vec![
                    TileWalk {
                        feed: 0,
                        stop: 0,
                        seconds: 30,
                    },
                    TileWalk {
                        feed: 2,
                        stop: 0,
                        seconds: 90,
                    },
                ],
            )],
        );
        write_tile(
            fixture.path(),
            TransferMode::Walking,
            11,
            20,
            &[(
                2,
                0,
                vec![TileWalk {
                    feed: 0,
                    stop: 1,
                    seconds: 95,
                }],
            )],
        );
        fixture
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write_manifest(&self) {
        fs::write(
            self.path().join("transfers/manifest.json"),
            serde_json::to_vec(&self.manifest).expect("json"),
        )
        .expect("write");
    }

    fn feeds(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }
}

#[test]
fn loads_every_compiled_feed_across_tiles_and_feeds() {
    let fixture = Fixture::new();
    let build = TransitBuild::load(fixture.path(), None).expect("loads");
    assert_eq!(build.set.part_count(), 2);
    assert_eq!(build.report.walks[0], 3, "walking");
    assert_eq!(build.report.tiles_read[0], 2);
    // `one`'s stops are local 0 and 1, `two`'s are 2 and 3; feed indices follow the manifest.
    assert_eq!(
        build.walking.seconds(StopId::new(1), StopId::new(2)),
        Some(90)
    );
    assert_eq!(
        build.walking.seconds(StopId::new(1), StopId::new(0)),
        Some(30)
    );
    assert_eq!(
        build.walking.seconds(StopId::new(2), StopId::new(1)),
        Some(95)
    );
    assert_eq!(build.walking.stop_count(), 4);
}

#[test]
fn a_subset_of_feeds_leaves_out_walks_to_the_others() {
    let fixture = Fixture::new();
    let build = TransitBuild::load(fixture.path(), Some(&Fixture::feeds(&["one"]))).expect("loads");
    assert_eq!(build.set.part_count(), 1);
    assert_eq!(build.walking.stop_count(), 2);
    assert_eq!(
        build.walking.seconds(StopId::new(1), StopId::new(0)),
        Some(30)
    );
    assert_eq!(build.report.walks[0], 1, "only the walk within feed one");
}

#[test]
fn a_tile_missing_from_disk_simply_has_no_walks() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.path().join("transfers/walking/12/11/20.bin.zst")).expect("remove");
    let build = TransitBuild::load(fixture.path(), None).expect("a partial download loads");
    assert_eq!(build.report.tiles_read[0], 1);
    assert_eq!(build.report.tiles_absent[0], 1);
    // `one`'s stops still have the walks of their own tile, to `two` included.
    assert_eq!(
        build.walking.seconds(StopId::new(1), StopId::new(2)),
        Some(90)
    );
    // `two`'s stops, with no tile here, have none.
    assert_eq!(build.walking.seconds(StopId::new(2), StopId::new(1)), None);
}

#[test]
fn a_named_feed_must_exist_and_have_a_timetable() {
    let fixture = Fixture::new();
    assert!(matches!(
        TransitBuild::load(fixture.path(), Some(&Fixture::feeds(&["nope"]))),
        Err(BuildLoadError::UnknownFeed(_))
    ));
    assert!(matches!(
        TransitBuild::load(fixture.path(), Some(&Fixture::feeds(&["failed"]))),
        Err(BuildLoadError::NotCompiled(_))
    ));
    // Asking for a feed by name is fine, however it is listed.
    assert!(
        TransitBuild::load(
            fixture.path(),
            Some(&Fixture::feeds(&["two", "one", "one"]))
        )
        .is_ok()
    );
}

#[test]
fn a_timetable_not_matching_the_manifest_is_refused() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path().join("timetables/one.timetable"),
        feed_bytes("one", &[("A", 47.60, -122.33), ("B", 47.62, -122.33)]),
    )
    .expect("write");
    assert!(matches!(
        TransitBuild::load(fixture.path(), None),
        Err(BuildLoadError::TimetableMismatch { .. })
    ));
}

#[test]
fn a_tile_of_another_build_is_refused() {
    let mut fixture = Fixture::new();
    fixture.manifest["manifest_id"] = json!("00000000000000ff");
    fixture.write_manifest();
    assert!(matches!(
        TransitBuild::load(fixture.path(), None),
        Err(BuildLoadError::Assemble { .. })
    ));
}

#[test]
fn a_mismatched_street_build_is_refused() {
    let fixture = Fixture::new();
    let build = TransitBuild::load(fixture.path(), None).expect("loads");
    let tiles = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles.tar");
    assert!(matches!(
        StreetPlanner::from_build(build, &[tiles], StreetConfig::default()),
        Err(OpenPlannerError::Build(
            BuildLoadError::StreetBuildMismatch { manifest: 77, .. }
        ))
    ));
}

#[test]
fn a_timetable_with_the_wrong_contents_is_refused() {
    let mut fixture = Fixture::new();
    fixture.manifest["feeds"][0]["stop_count"] = json!(5);
    fixture.write_manifest();
    assert!(matches!(
        TransitBuild::load(fixture.path(), Some(&Fixture::feeds(&["one"]))),
        Err(BuildLoadError::TimetableContents { .. })
    ));
}

#[test]
fn corrupt_or_truncated_timetables_are_errors_not_panics() {
    let fixture = Fixture::new();
    let path = fixture.path().join("timetables/one.timetable");
    let good = fs::read(&path).expect("read");
    let mut variants: Vec<Vec<u8>> = (0..good.len())
        .step_by(good.len().div_ceil(32).max(1))
        .map(|length| good[..length].to_vec())
        .collect();
    variants.push(Vec::new());
    for position in (0..good.len()).step_by(good.len().div_ceil(64).max(1)) {
        let mut flipped = good.clone();
        flipped[position] ^= 0x40;
        variants.push(flipped);
    }
    for variant in variants {
        // Written to pass the manifest's hash, so that loading must judge the bytes themselves.
        fs::write(&path, &variant).expect("write");
        let mut manifest = fixture.manifest.clone();
        manifest["feeds"][0]["timetable_sha256"] = json!(hex_sha(&variant));
        fs::write(
            fixture.path().join("transfers/manifest.json"),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");
        assert!(TransitBuild::load(fixture.path(), Some(&Fixture::feeds(&["one"]))).is_err());
    }
}

#[test]
fn a_corrupt_tile_is_an_error_not_a_panic() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path().join("transfers/walking/12/10/20.bin.zst"),
        [1, 2, 3],
    )
    .expect("write");
    assert!(matches!(
        TransitBuild::load(fixture.path(), None),
        Err(BuildLoadError::Tile { .. })
    ));
}

#[test]
fn a_prepared_table_is_reused_until_the_tiles_change() {
    let fixture = Fixture::new();
    let tile = fixture.path().join("transfers/walking/12/10/20.bin.zst");
    let first = TransitBuild::load(fixture.path(), None).expect("loads");
    assert_eq!(first.report.walks[0], 3);
    drop(first);
    let original = fs::read(&tile).expect("read");

    // The prepared table doesn't read the tile again: garbage in it goes unnoticed.
    fs::write(&tile, [1, 2, 3]).expect("write");
    let again = TransitBuild::load(fixture.path(), None).expect("reuses");
    assert_eq!(
        again.walking.seconds(StopId::new(1), StopId::new(2)),
        Some(90)
    );
    drop(again);

    // A tile disappearing changes what is prepared.
    fs::write(&tile, &original).expect("write");
    fs::remove_file(fixture.path().join("transfers/walking/12/11/20.bin.zst")).expect("remove");
    let fewer = TransitBuild::load(fixture.path(), None).expect("rebuilds");
    assert_eq!(fewer.report.walks[0], 2);
    assert_eq!(fewer.walking.seconds(StopId::new(2), StopId::new(1)), None);
    drop(fewer);
    let names: Vec<String> = fs::read_dir(fixture.path().join("prepared"))
        .expect("prepared")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("walking-"))
        .collect();
    assert_eq!(
        names.len(),
        1,
        "older tables of this mode are removed: {names:?}"
    );
}

#[test]
fn tiles_can_be_discarded_after_the_table_is_prepared() {
    let fixture = Fixture::new();
    let prepared = TransitBuild::load(fixture.path(), None).expect("loads");
    assert_eq!(prepared.report.walks[0], 3);
    drop(prepared);
    fs::remove_dir_all(fixture.path().join("transfers/walking")).expect("remove");
    let build = TransitBuild::load(fixture.path(), None).expect("loads with no tiles");
    assert_eq!(
        build.report.walks[0], 3,
        "the walks live on in the prepared table"
    );
    assert_eq!(
        build.walking.seconds(StopId::new(1), StopId::new(2)),
        Some(90)
    );
}

#[test]
fn a_damaged_prepared_table_is_replaced() {
    let fixture = Fixture::new();
    TransitBuild::prepare(fixture.path(), None).expect("prepares");
    let prepared = fixture.path().join("prepared");
    let name = fs::read_dir(&prepared)
        .expect("prepared")
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().starts_with("walking-"))
        .expect("a walking table")
        .path();
    fs::write(&name, b"not a table").expect("write");
    let build = TransitBuild::load(fixture.path(), None).expect("rebuilds");
    assert_eq!(build.report.walks[0], 3);
}

// Real data: the tiles of Seattle only, with King County Metro and Sound Transit loaded.
const WASHINGTON: &str = "/mnt/maps/cardinal/transit/global/us-wa-2026-11-09";
const STREETS: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";
const METRO: &str = "us-wa-Metro-Transit";
const SOUND_TRANSIT: &str = "us-wa-Sound-Transit";

fn tile_xy(lat: f64, lon: f64) -> (u32, u32) {
    let n = 2_f64.powi(12);
    let x = ((lon + 180.0) / 360.0 * n).floor();
    let lat = lat.to_radians();
    let y = ((1.0 - lat.tan().asinh() / core::f64::consts::PI) / 2.0 * n).floor();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "tile indices"
    )]
    (x as u32, y as u32)
}

/// A copy of the Washington build with only the tiles of a bounding box, by symbolic links.
fn seattle_only(area: (f64, f64, f64, f64)) -> TempDir {
    let (south, west, north, east) = area;
    let (west_x, north_y) = tile_xy(north, west);
    let (east_x, south_y) = tile_xy(south, east);
    let dir = TempDir::new().expect("temp");
    let root = Path::new(WASHINGTON);
    std::os::unix::fs::symlink(root.join("timetables"), dir.path().join("timetables"))
        .expect("link");
    fs::create_dir_all(dir.path().join("transfers")).expect("mkdir");
    fs::copy(
        root.join("transfers/manifest.json"),
        dir.path().join("transfers/manifest.json"),
    )
    .expect("copy manifest");
    for mode in ["walking", "cycling"] {
        for entry in fs::read_dir(root.join("transfers").join(mode).join("12")).expect("modes") {
            let entry = entry.expect("entry");
            let x: u32 = entry.file_name().to_string_lossy().parse().expect("x");
            if !(west_x..=east_x).contains(&x) {
                continue;
            }
            let target = dir
                .path()
                .join("transfers")
                .join(mode)
                .join("12")
                .join(x.to_string());
            for file in fs::read_dir(entry.path()).expect("x dir") {
                let file = file.expect("file");
                let name = file.file_name().to_string_lossy().into_owned();
                let y: u32 = name.trim_end_matches(".bin.zst").parse().expect("y");
                if (north_y..=south_y).contains(&y) {
                    fs::create_dir_all(&target).expect("mkdir");
                    std::os::unix::fs::symlink(file.path(), target.join(name)).expect("link");
                }
            }
        }
    }
    dir
}

#[test]
#[ignore = "needs the Washington build under /mnt/maps/cardinal/transit"]
fn a_downloaded_area_loads_and_plans_with_partial_tiles() {
    let area = (47.48, -122.46, 47.76, -122.22);
    let dir = seattle_only(area);
    let feeds = [METRO.to_owned(), SOUND_TRANSIT.to_owned()];
    let build = TransitBuild::load(dir.path(), Some(&feeds)).expect("loads");
    println!("seattle subset: {:?}", build.report);
    assert!(build.report.tiles_read[0] > 0);
    assert!(
        build.report.tiles_absent[0] > 0,
        "Metro reaches beyond these tiles"
    );
    assert!(build.report.walks[0] > 0);

    // A Metro stop far outside the area has no transfers; a Seattle stop does.
    let set = &build.set;
    let (mut outside_empty, mut outside_total, mut inside_with) = (0, 0, 0);
    for part in set.parts() {
        for raw in 0..u32::try_from(part.compiled().stop_count()).unwrap() {
            let local = StopId::new(raw);
            let Some(coordinate) = part.compiled().stop(local).and_then(|i| i.coordinate) else {
                continue;
            };
            let (south, west, north, east) = area;
            let global = part.global_stop(local);
            let inside = coordinate.lat > south + 0.02
                && coordinate.lat < north - 0.02
                && coordinate.lon > west + 0.02
                && coordinate.lon < east - 0.02;
            let walks = build.walking.walks_from(global).count();
            if coordinate.lon > -121.9 {
                outside_total += 1;
                if walks == 0 {
                    outside_empty += 1;
                }
            }
            if inside && walks > 0 {
                inside_with += 1;
            }
        }
    }
    println!(
        "outside stops without walks: {outside_empty} of {outside_total}; inside with walks: {inside_with}"
    );
    assert!(outside_total > 0);
    assert_eq!(outside_empty, outside_total);
    assert!(inside_with > 100);

    let planner =
        StreetPlanner::from_build(build, &[STREETS], StreetConfig::default()).expect("opens");
    // Fremont to the Central District, 2026-11-10 09:00 Pacific.
    let departure = 1_794_330_000;
    let request = TransitRequest::new(
        LatLon {
            lat: 47.649_735,
            lon: -122.349_869,
        },
        LatLon {
            lat: 47.613_442,
            lon: -122.302_605,
        },
        departure,
        departure - 60,
    );
    let response = planner.plan(&request, "en-US", &|| false).expect("plans");
    assert!(
        !response.journeys.is_empty(),
        "journeys exist within the area"
    );
}

fn named_requests() -> Vec<TransitRequest> {
    let tuesday_midnight = 1_794_297_600;
    let trips = [
        ((47.649_735, -122.349_869), (47.613_442, -122.302_605)),
        ((47.668_606, -122.385_345), (47.441_296, -122.298_523)),
    ];
    let mut requests = Vec::new();
    for (from, to) in trips {
        for hour in [8, 12, 17] {
            let departure = tuesday_midnight + hour * 3600;
            requests.push(TransitRequest::new(
                LatLon {
                    lat: from.0,
                    lon: from.1,
                },
                LatLon {
                    lat: to.0,
                    lon: to.1,
                },
                departure,
                departure - 60,
            ));
        }
    }
    requests
}

/// Loading Metro from the build matches its single file with walks built in memory.
#[test]
#[ignore = "needs the Washington build under /mnt/maps/cardinal/transit"]
fn metro_from_tiles_matches_metro_alone_with_built_walks() {
    let feeds = [METRO.to_owned()];
    let build = TransitBuild::load(Path::new(WASHINGTON), Some(&feeds)).expect("loads");
    let from_tiles =
        StreetPlanner::from_build(build, &[STREETS], StreetConfig::default()).expect("opens");
    let path = Path::new(WASHINGTON).join(format!("timetables/{METRO}.timetable"));
    // SAFETY: the Washington build's timetables are not modified while this test runs.
    let single = unsafe { valinor_gtfs::LoadedTimetable::map(&path) }.expect("maps");
    let in_memory = StreetPlanner::open_with_built_transfers(
        valinor_gtfs::TimetableSet::single(single).expect("set"),
        &[STREETS],
        StreetConfig::default(),
    )
    .expect("builds");
    let mut compared = 0;
    for request in named_requests() {
        let tiles = from_tiles
            .plan(&request, "en-US", &|| false)
            .expect("plans");
        let local = in_memory.plan(&request, "en-US", &|| false).expect("plans");
        assert!(!tiles.journeys.is_empty());
        assert_eq!(
            serde_json::to_value(&tiles).expect("json"),
            serde_json::to_value(&local).expect("json"),
            "a request answered differently from tiles"
        );
        compared += tiles.journeys.len();
    }
    println!("compared {compared} journeys");
}

/// Whichever Metro capture is newest, ingested for the time it was taken.
fn newest_capture() -> (PathBuf, i64) {
    let mut names: Vec<_> = fs::read_dir("/mnt/maps/cardinal/transit/kcm/rt")
        .expect("captures")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    let name = names.pop().expect("a capture");
    let parsed = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", &name).expect("named by time");
    let now = parsed
        .to_zoned(jiff::tz::TimeZone::UTC)
        .expect("zoned")
        .timestamp()
        .as_second();
    (
        PathBuf::from("/mnt/maps/cardinal/transit/kcm/rt").join(name),
        now,
    )
}

#[test]
#[ignore = "needs the Washington build under /mnt/maps/cardinal/transit"]
fn washington_combines_metro_and_sound_transit_with_real_walks() {
    use crate::response::JourneyLeg;
    use crate::state::FeedUpdate;
    use valinor_gtfs_rt::FeedKind;

    let build = TransitBuild::load(Path::new(WASHINGTON), None).expect("loads");
    let planner =
        StreetPlanner::from_build(build, &[STREETS], StreetConfig::default()).expect("opens");
    let request = &named_requests()[3];
    let response = planner.plan(request, "en-US", &|| false).expect("plans");
    let mut both = 0;
    let mut drawn = 0;
    for journey in &response.journeys {
        let feeds: Vec<&str> = journey
            .legs
            .iter()
            .filter_map(|leg| match leg {
                JourneyLeg::Transit(leg) => leg.trip.feed.as_deref(),
                _ => None,
            })
            .collect();
        if feeds.contains(&METRO) && feeds.contains(&SOUND_TRANSIT) {
            both += 1;
        }
        for pair in journey.legs.windows(3) {
            let [
                JourneyLeg::Transit(before),
                JourneyLeg::Transfer(walk),
                JourneyLeg::Transit(after),
            ] = pair
            else {
                continue;
            };
            if before.trip.feed != after.trip.feed {
                assert!(
                    walk.walk
                        .as_ref()
                        .is_some_and(|w| !w.shape.is_empty() && w.distance_meters > 0.0)
                );
                drawn += 1;
            }
        }
    }
    println!("Ballard to SeaTac: {both} journeys used both agencies, {drawn} walks between them");
    assert!(both > 0, "Link should appear in these answers");
    assert!(drawn > 0);

    let (capture, now) = newest_capture();
    let mut accepted = 0;
    for (file, kind) in [
        ("tripupdates.pb", FeedKind::TripUpdates),
        ("alerts.pb", FeedKind::Alerts),
        ("vehiclepositions.pb", FeedKind::VehiclePositions),
    ] {
        let bytes = fs::read(capture.join(file)).expect("capture");
        let result = planner
            .state()
            .ingest_and_refresh(
                FeedUpdate {
                    feed: METRO,
                    kind,
                    bytes: Some(&bytes),
                    received_at: now,
                    stale_after: Some(3600),
                },
                now,
            )
            .expect("known feed");
        assert!(result.snapshot.is_ok());
        accepted += 1;
    }
    assert_eq!(accepted, 3);
    let snapshot = planner.state().refresh(now).expect("rebuilds");
    println!(
        "overlay: {} trips updated, {} vehicles",
        snapshot.stats().trips_updated,
        snapshot.stats().vehicles
    );
    assert!(snapshot.stats().trip_updates_matched > 0);
    assert!(snapshot.stats().vehicles_matched > 0);
    let request = TransitRequest::new(
        LatLon {
            lat: 47.668_606,
            lon: -122.385_345,
        },
        LatLon {
            lat: 47.441_296,
            lon: -122.298_523,
        },
        now,
        now,
    );
    let response = planner.plan(&request, "en-US", &|| false).expect("plans");
    assert!(
        response
            .journeys
            .iter()
            .any(|journey| journey.legs.iter().any(|leg| matches!(
                leg,
                JourneyLeg::Transit(leg) if leg.trip.feed.as_deref() == Some(METRO) && leg.realtime
            ))),
        "real-time Metro legs appear in a Washington-wide answer"
    );
}
