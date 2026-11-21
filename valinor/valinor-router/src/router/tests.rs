use super::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::BuildId;
use valhalla_graphtile::tile_provider::DirectoryGraphTileProvider;
use valhalla_proto::prost::Message;
use valinor_tyr::tyr::{JSON_CONTENT_TYPE, PBF_CONTENT_TYPE};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures")
}

fn tiles() -> PathBuf {
    fixtures().join("andorra-tiles")
}

fn router() -> Router {
    Router::open(RouterConfig::default(), &[tiles()]).unwrap()
}

fn json(response: &HttpResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

fn post<'a>(target: &'a str, body: &'a str) -> HttpRequest<'a> {
    HttpRequest {
        method: &Method::POST,
        target: target.as_bytes(),
        content_type: None,
        body: body.as_bytes(),
    }
}

/// Ferrostar's `ValhallaHttpRequestGenerator` body, as in the `ferrostar_auto` capture.
const FERROSTAR: &str = r#"{"format": "osrm", "filters": {"action": "include", "attributes": ["shape_attributes.speed", "shape_attributes.speed_limit", "shape_attributes.time", "shape_attributes.length"]}, "banner_instructions": true, "voice_instructions": true, "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211, "street_side_tolerance": 5, "heading": 90}, {"lat": 42.5355, "lon": 1.583, "type": "break"}]}"#;

/// The fixture's build ID, as mainline 3.9.1 stamped it.
const FIXTURE_BUILD_ID: BuildId = BuildId::new(53174);

#[test]
fn is_send_and_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<Router>();
}

#[test]
fn routes_ferrostar_requests_as_mainline_does() {
    let router = router();
    let response = router.route(FERROSTAR.as_bytes());
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, JSON_CONTENT_TYPE);
    assert_eq!(response.content_disposition, None);
    let captured: Value = serde_json::from_str(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../valinor-tyr/fixtures/captures/andorra/ferrostar_auto/osrm/response.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(json(&response), captured["body"]);
}

/// Ferrostar's body with `valinor_leg_details` and an elevation interval, on the fixture's
/// tiles (which mainline built with elevation), by car, by bicycle, and on foot:
/// each leg gains `valinor_details` and the response is otherwise the one without the two
/// options. The runs add up to the leg, and the elevation is sampled every 30 m along it.
#[test]
fn legs_carry_details_when_asked_and_nothing_else_changes() {
    let router = router();
    for costing in ["auto", "bicycle", "pedestrian"] {
        let mut body: Value = serde_json::from_str(FERROSTAR).unwrap();
        body["costing"] = json!(costing);
        let plain = router.route(body.to_string().as_bytes());
        assert_eq!(plain.status, StatusCode::OK, "{costing}");
        let plain = json(&plain);

        body["valinor_leg_details"] = json!(true);
        body["elevation_interval"] = json!(30);
        let detailed = router.route(body.to_string().as_bytes());
        assert_eq!(detailed.status, StatusCode::OK, "{costing}");
        let mut detailed = json(&detailed);

        let leg = &mut detailed["routes"][0]["legs"][0];
        let details = leg.as_object_mut().unwrap().remove("valinor_details");
        let details = details.unwrap_or_else(|| panic!("{costing}: no details"));
        let (duration, distance) = (
            leg["duration"].as_f64().unwrap(),
            leg["distance"].as_f64().unwrap(),
        );
        assert_eq!(detailed, plain, "{costing}");
        let leg = &plain["routes"][0]["legs"][0];

        let runs = details["runs"].as_array().unwrap();
        let sum = |key: &str| -> f64 { runs.iter().map(|run| run[key].as_f64().unwrap()).sum() };
        assert!((sum("duration") - duration).abs() < 0.0015, "{costing}");
        assert!((sum("distance") - distance).abs() < 1.0, "{costing}");
        let mode = match costing {
            "auto" => "drive",
            other => other,
        };
        for run in runs {
            assert_eq!(run["travel_mode"], mode, "{costing}");
            assert_eq!(run.get("cycle_lane").is_some(), costing == "bicycle");
            assert_eq!(run.get("bicycle_network").is_some(), costing == "bicycle");
        }
        // Fewer runs than intersections: edges of one kind are joined.
        let intersections: usize = leg["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| step["intersections"].as_array().unwrap().len())
            .sum();
        assert!(!runs.is_empty() && runs.len() < intersections, "{costing}");

        assert_eq!(details["elevation_interval"], json!(30.0), "{costing}");
        let heights = details["elevation"].as_array().unwrap();
        // A sample every 30 m from the start, and the leg's end.
        // Edges' sampling intervals are rounded, so the count can be one off.
        let expected = (distance / 30.0).floor() + 2.0;
        #[expect(clippy::cast_precision_loss, reason = "a small count")]
        let count = heights.len() as f64;
        assert!(
            (count - expected).abs() <= 1.0,
            "{costing}: {count} of {expected}"
        );
        // Andorra is between 800 and 3,000 m up.
        for height in heights {
            let height = height.as_f64().unwrap();
            assert!((800.0..3000.0).contains(&height), "{costing}: {height}");
        }
    }
}

#[test]
fn errors_are_bodies_in_the_request_format() {
    let router = router();
    // OSRM format: Ferrostar's parser reads `code` and `message`.
    let far = r#"{"format": "osrm", "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211}, {"lat": 10.0, "lon": 10.0}]}"#;
    let response = router.route(far.as_bytes());
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.content_type, ERROR_CONTENT_TYPE);
    let body = json(&response);
    assert!(
        body["code"].is_string() && body.get("error_code").is_none(),
        "{body}"
    );

    // Valhalla JSON.
    let response = router.route(b"{\"locations\": [");
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(json(&response)["error_code"], json!(100));

    // A method other than GET or POST, and an unknown action.
    let response = router.handle(&HttpRequest {
        method: &Method::PUT,
        ..post("/route", FERROSTAR)
    });
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(json(&response)["error_code"], json!(101));
    for target in ["/", "/spaceship", "/route/extra"] {
        let response = router.handle(&post(target, "{}"));
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{target}");
        assert_eq!(json(&response)["error_code"], json!(106), "{target}");
    }

    // Actions Valinor doesn't serve yet.
    let response = router.handle(&post(
        "/sources_to_targets",
        r#"{"costing": "auto", "sources": [{"lat": 42.5078, "lon": 1.5211}], "targets": [{"lat": 42.5355, "lon": 1.583}]}"#,
    ));
    assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json(&response)["error_code"], json!(107));
}

#[test]
fn answers_status_locate_and_gpx() {
    let router = router();
    for target in ["/status", "/status?verbose=false", "/status?verbose=true"] {
        let response = router.handle(&HttpRequest {
            method: &Method::GET,
            ..post(target, "")
        });
        assert_eq!(response.status, StatusCode::OK, "{target}");
        assert_eq!(response.content_type, JSON_CONTENT_TYPE);
        let body = json(&response);
        assert_eq!(body["version"], json!("3.9.1"));
        assert!(
            body["tileset_last_modified"]
                .as_u64()
                .is_some_and(|t| t > 0)
        );
        assert!(
            body["available_actions"]
                .as_array()
                .is_some_and(|a| a.contains(&json!("route")))
        );
    }
    let response = router.handle(&HttpRequest {
        method: &Method::GET,
        ..post("/status?format=pbf", "")
    });
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, PBF_CONTENT_TYPE);
    let status = Api::decode(response.body.as_slice())
        .unwrap()
        .status
        .unwrap();
    assert_eq!(status.version, "3.9.1");

    let response = router.handle(&post(
        "/locate",
        r#"{"locations": [{"lat": 42.5078, "lon": 1.5211}], "costing": "auto"}"#,
    ));
    assert_eq!(response.status, StatusCode::OK);
    assert!(
        json(&response)[0]["edges"]
            .as_array()
            .is_some_and(|e| !e.is_empty())
    );

    let response = router.handle(&post(
        "/route",
        r#"{"format": "gpx", "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211}, {"lat": 42.5355, "lon": 1.583}]}"#,
    ));
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.content_type, GPX_CONTENT_TYPE);
    assert_eq!(response.content_disposition, Some(GPX_CONTENT_DISPOSITION));
}

#[test]
fn interrupted_requests_have_no_response() {
    let router = router();
    assert_eq!(
        router.handle_interruptible(&post("/route", FERROSTAR), &|| true),
        Err(Interrupted)
    );
    // Only path search and trip building are interrupted.
    let response = router
        .handle_interruptible(&post("/status", ""), &|| false)
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
}

/// A captured trace request's body (from `valinor-tyr`'s trace fixtures).
fn trace_body(case: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valinor-tyr/fixtures/traces/andorra")
        .join(case)
        .join("request.json");
    let recorded: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    recorded["request"].to_string()
}

#[test]
fn interrupted_traces_have_no_response() {
    let router = router();
    for (target, case) in [
        ("/trace_route", "andorra-0000-clean"),
        ("/trace_attributes", "andorra-0033-clean"),
        ("/trace_route", "andorra-0086-edge_walk_exact"),
    ] {
        let body = trace_body(case);
        assert_eq!(
            router.handle_interruptible(&post(target, &body), &|| true),
            Err(Interrupted),
            "{case}"
        );
        let response = router.handle(&post(target, &body));
        assert_eq!(response.status, StatusCode::OK, "{case}");
    }
}

#[test]
fn matches_traces_on_many_threads_at_once() {
    let config = RouterConfig {
        workspaces: core::num::NonZeroUsize::new(2).unwrap(),
        ..RouterConfig::default()
    };
    let router = Arc::new(Router::open(config, &[tiles()]).unwrap());
    let route = trace_body("andorra-0000-clean");
    let attributes = trace_body("andorra-0033-clean");
    let expected = (
        router.handle(&post("/trace_route", &route)),
        router.handle(&post("/trace_attributes", &attributes)),
    );
    assert_eq!(expected.0.status, StatusCode::OK);
    assert_eq!(expected.1.status, StatusCode::OK);
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let (router, route, attributes) = (router.clone(), route.clone(), attributes.clone());
            std::thread::spawn(move || {
                (0..3)
                    .map(|_| {
                        (
                            router.handle(&post("/trace_route", &route)),
                            router.handle(&post("/trace_attributes", &attributes)),
                        )
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for thread in threads {
        for responses in thread.join().unwrap() {
            assert_eq!(responses, expected);
        }
    }
    assert!(router.match_workspaces.created() <= 2);
    // Map matching doesn't take path search's workspaces.
    assert_eq!(router.workspaces.created(), 0);
}

#[test]
fn serves_many_threads_at_once() {
    let config = RouterConfig {
        workspaces: core::num::NonZeroUsize::new(2).unwrap(),
        ..RouterConfig::default()
    };
    let router = Arc::new(Router::open(config, &[tiles()]).unwrap());
    let expected = router.route(FERROSTAR.as_bytes());
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let router = router.clone();
            std::thread::spawn(move || {
                (0..4)
                    .map(|_| router.route(FERROSTAR.as_bytes()))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    for thread in threads {
        for response in thread.join().unwrap() {
            assert_eq!(response, expected);
        }
    }
    assert!(router.workspaces.created() <= 2);
}

/// A temporary directory, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "valinor-router-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    /// Copies fixture tiles (by path relative to the tile directory) into this directory.
    fn with_tiles(self, from: &Path, tiles: &[&str]) -> Self {
        for tile in tiles {
            let to = self.0.join(tile);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(from.join(tile), to).unwrap();
        }
        self
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ALL_TILES: [&str; 7] = [
    "0/003/015.gph",
    "1/047/701.gph",
    "2/000/762/485.gph",
    "2/000/762/486.gph",
    "2/000/763/925.gph",
    "2/000/763/926.gph",
    "2/000/763/927.gph",
];

#[test]
fn opens_sets_from_one_build_as_one() {
    // A tarball and a directory of the same build, and two halves of a region that overlap.
    let router = Router::open(
        RouterConfig::default(),
        &[fixtures().join("andorra-tiles.tar"), tiles()],
    )
    .unwrap();
    assert_eq!(router.tiles().build_id(), FIXTURE_BUILD_ID);
    assert_eq!(router.tiles().tile_ids().count(), ALL_TILES.len());
    assert_eq!(
        router.route(FERROSTAR.as_bytes()),
        self::router().route(FERROSTAR.as_bytes())
    );

    let west = TempDir::new("west").with_tiles(&tiles(), &ALL_TILES[..5]);
    let east = TempDir::new("east").with_tiles(&tiles(), &ALL_TILES[3..]);
    let halves = Router::open(RouterConfig::default(), &[&west.0, &east.0]).unwrap();
    assert_eq!(halves.tiles().tile_ids().count(), ALL_TILES.len());
    assert_eq!(
        halves.route(FERROSTAR.as_bytes()),
        self::router().route(FERROSTAR.as_bytes())
    );
}

#[test]
fn refuses_sets_from_different_builds() {
    // Mainline stamped a new build ID into every tile when it added predicted speeds
    // to this one (see tools/oracle/README.md).
    let newer = fixtures().join("andorra-tiles-with-traffic");
    let error = Router::open(RouterConfig::default(), &[tiles(), newer.clone()])
        .err()
        .unwrap();
    let OpenError::MixedBuilds {
        first_path,
        expected,
        path,
        tile,
        found,
        ..
    } = &error
    else {
        panic!("{error}");
    };
    assert_eq!(first_path, &tiles());
    assert_eq!(path, &newer);
    assert_eq!(*tile, GraphId::try_from_components(0, 3015, 0).unwrap());
    assert_eq!(expected.0, FIXTURE_BUILD_ID);
    assert_ne!(found.0, FIXTURE_BUILD_ID);
    assert!(error.to_string().contains("53174"), "{error}");

    // Within one set too.
    let mixed = TempDir::new("mixed").with_tiles(&tiles(), &ALL_TILES[1..]);
    std::fs::create_dir_all(mixed.0.join("0/003")).unwrap();
    std::fs::copy(newer.join("0/003/015.gph"), mixed.0.join("0/003/015.gph")).unwrap();
    assert!(matches!(
        Router::open(RouterConfig::default(), &[&mixed.0]),
        Err(OpenError::MixedBuilds { .. })
    ));
}

#[test]
fn refuses_what_it_cannot_open() {
    let none: [PathBuf; 0] = [];
    assert!(matches!(
        Router::open(RouterConfig::default(), &none),
        Err(OpenError::NoTileSets)
    ));
    assert!(matches!(
        Router::open(RouterConfig::default(), &[tiles().join("missing.tar")]),
        Err(OpenError::Io { .. })
    ));
    let empty = TempDir::new("empty");
    assert!(matches!(
        Router::open(RouterConfig::default(), &[&empty.0]),
        Err(OpenError::Empty { .. })
    ));
    // Not a tarball.
    assert!(matches!(
        Router::open(RouterConfig::default(), &[tiles().join("0/003/015.gph")]),
        Err(OpenError::Tiles { .. })
    ));

    // A tile without a checksum, as tiles built before Valhalla 3.9 are.
    let old = TempDir::new("old").with_tiles(&tiles(), &ALL_TILES);
    let path = old.0.join(ALL_TILES[0]);
    let mut bytes = std::fs::read(&path).unwrap();
    let header = DirectoryGraphTileProvider::new(old.0.clone(), core::num::NonZeroUsize::MIN)
        .tile_header(GraphId::try_from_components(0, 3015, 0).unwrap())
        .unwrap();
    let checksum = (u64::from(header.build_id().value()) << 48) | header.tile_hash().value();
    let position = bytes
        .windows(8)
        .position(|w| w == checksum.to_le_bytes())
        .unwrap();
    bytes[position..position + 8].fill(0);
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        Router::open(RouterConfig::default(), &[&old.0]),
        Err(OpenError::NoBuildId { tile, .. }) if tile == GraphId::try_from_components(0, 3015, 0).unwrap()
    ));
}

/// A region cut from the fixture tiles: a tile that no set has is the edge of the graph,
/// as in mainline, not an error (see `docs/offline-regions.md`).
#[test]
fn tiles_missing_from_every_set_are_dead_ends() {
    let full = router();
    // Drop the two local (level 2) tiles south of 42.5°N, keeping the highways there
    // (levels 0 and 1), as a region that ends at 42.5°N would.
    let region = TempDir::new("region").with_tiles(
        &tiles(),
        &[
            ALL_TILES[0],
            ALL_TILES[1],
            ALL_TILES[4],
            ALL_TILES[5],
            ALL_TILES[6],
        ],
    );
    let cut = Router::open(RouterConfig::default(), &[&region.0]).unwrap();
    assert_eq!(cut.tiles().tile_ids().count(), 5);

    let mut routed = 0;
    let mut no_edges = 0;
    // Locations across the fixture, north and south of the cut. A short search cutoff keeps
    // locations in the dropped tiles from snapping to a highway kilometers away.
    for lon in [1.45, 1.48, 1.5211, 1.55, 1.583, 1.62] {
        for lat in [42.43, 42.46, 42.48, 42.5078, 42.53, 42.56] {
            let body = format!(
                r#"{{"costing": "auto", "locations": [{{"lat": 42.5078, "lon": 1.5211}}, {{"lat": {lat}, "lon": {lon}, "search_cutoff": 150}}]}}"#
            );
            let response = cut.route(body.as_bytes());
            // Never a server error: the graph ends, or the route goes around.
            assert!(
                response.status == StatusCode::OK || response.status == StatusCode::BAD_REQUEST,
                "{lat},{lon}: {} {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            );
            if response.status == StatusCode::OK {
                routed += 1;
            } else {
                let code = json(&response)["error_code"].as_u64();
                // No edges near the location (171), a location in a tile not connected to the
                // origin's (170: the connectivity map only knows the tiles that are there),
                // or no path in what is left (442).
                assert!(
                    matches!(code, Some(170 | 171 | 442)),
                    "{lat},{lon}: {code:?}"
                );
                // Count the locations that only the cut lost: the whole graph routes them.
                if code == Some(171) && full.route(body.as_bytes()).status == StatusCode::OK {
                    no_edges += 1;
                }
            }
        }
    }
    assert!(
        routed > 0 && no_edges > 0,
        "{routed} routed, {no_edges} without edges"
    );

    // Locate reports no edges there rather than failing.
    let response = cut.handle(&post(
        "/locate",
        r#"{"locations": [{"lat": 42.5, "lon": 1.42}], "costing": "auto"}"#,
    ));
    assert_eq!(
        response.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
}

/// Tiles that arrive in a directory after the router opened: once refreshed, the router
/// answers as if they had been there at open (including Loki's connectivity check).
#[test]
fn tiles_that_arrive_are_routed_after_a_refresh() {
    let full = router();
    let kept = [
        ALL_TILES[0],
        ALL_TILES[1],
        ALL_TILES[4],
        ALL_TILES[5],
        ALL_TILES[6],
    ];
    let region = TempDir::new("arrive").with_tiles(&tiles(), &kept);
    let cut = Router::open(RouterConfig::default(), &[&region.0]).unwrap();
    let bodies: Vec<String> = [1.45, 1.48, 1.5211, 1.55]
        .into_iter()
        .flat_map(|lon| [42.43, 42.46, 42.48].map(|lat| (lat, lon)))
        .map(|(lat, lon)| {
            format!(
                r#"{{"costing": "auto", "locations": [{{"lat": 42.5078, "lon": 1.5211}}, {{"lat": {lat}, "lon": {lon}, "search_cutoff": 150}}]}}"#
            )
        })
        .collect();
    let differing = |router: &Router| {
        bodies
            .iter()
            .filter(|body| router.route(body.as_bytes()).body != full.route(body.as_bytes()).body)
            .count()
    };
    let before = differing(&cut);
    assert!(before > 0);

    // The southern tiles arrive, as a download puts them: written aside, renamed into place.
    for tile in [ALL_TILES[2], ALL_TILES[3]] {
        let to = region.0.join(tile);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        let part = to.with_extension("part");
        std::fs::copy(tiles().join(tile), &part).unwrap();
        std::fs::rename(&part, &to).unwrap();
    }
    // The tiles alone aren't enough: Loki's connectivity map doesn't know them yet.
    cut.tiles().refresh();
    let unconnected = bodies
        .iter()
        .filter(|body| json(&cut.route(body.as_bytes()))["error_code"].as_u64() == Some(170))
        .count();
    assert!(unconnected > 0, "{before} differed before");

    cut.refresh_tiles().unwrap();
    assert_eq!(differing(&cut), 0);
}

/// The paths of this process's mappings of deleted files under `dir` (Linux's
/// `/proc/self/maps` marks them `(deleted)`): their space isn't freed until they're unmapped.
#[cfg(target_os = "linux")]
fn deleted_mappings(dir: &Path) -> Vec<String> {
    let dir = dir.to_string_lossy().into_owned();
    std::fs::read_to_string("/proc/self/maps")
        .unwrap()
        .lines()
        .filter(|line| line.contains(&dir) && line.ends_with("(deleted)"))
        .map(str::to_owned)
        .collect()
}

/// What deleting tile files (an app evicting downloaded tiles) does to an open router, as
/// things stand: a refresh forgets only which tiles were missing. A tile that was mapped
/// stays mapped, so its disk space isn't freed and routes still use it, until the router is
/// dropped; a tile that was never mapped reads as missing.
#[cfg(target_os = "linux")]
#[test]
fn deleted_tiles_stay_mapped_until_the_router_is_dropped() {
    let region = TempDir::new("evict").with_tiles(&tiles(), &ALL_TILES);
    let router = Router::open_for_build(
        RouterConfig::default(),
        &[&region.0],
        Some(FIXTURE_BUILD_ID),
    )
    .unwrap();
    let ok = |router: &Router| router.route(FERROSTAR.as_bytes()).status.as_u16() == 200;
    assert!(ok(&router));

    for tile in ALL_TILES {
        std::fs::remove_file(region.0.join(tile)).unwrap();
    }
    router.refresh_tiles().unwrap();
    let mapped = deleted_mappings(&region.0);
    assert!(
        !mapped.is_empty(),
        "a refresh unmapped the deleted tiles; update this test and the app's eviction"
    );
    // The route still finds its (deleted) tiles.
    assert!(ok(&router));

    // A router opened now sees no tiles.
    let fresh = Router::open_for_build(
        RouterConfig::default(),
        &[&region.0],
        Some(FIXTURE_BUILD_ID),
    )
    .unwrap();
    assert!(!ok(&fresh));
    drop(router);
    assert!(deleted_mappings(&region.0).is_empty());
}
