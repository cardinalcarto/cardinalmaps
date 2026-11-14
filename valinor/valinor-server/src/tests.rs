use super::*;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures")
}

fn state(workers: usize, timeout: Duration) -> Arc<app::AppState> {
    let router = Router::open(
        RouterConfig {
            workspaces: NonZeroUsize::new(workers).unwrap(),
            ..RouterConfig::default()
        },
        &[fixtures().join("andorra-tiles.tar")],
    )
    .unwrap();
    Arc::new(app::AppState::new(router, workers, timeout))
}

async fn send(
    state: &Arc<app::AppState>,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app::app(state.clone(), 1 << 20)
        .oneshot(request)
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    (parts.status, parts.headers, body.to_vec())
}

fn request(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

const ROUTE: &str = r#"{"costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211}, {"lat": 42.5355, "lon": 1.583}]}"#;

#[tokio::test]
async fn serves_valhalla_paths_as_mainline_does() {
    let state = state(2, Duration::from_secs(30));

    let (status, headers, body) = send(&state, request("GET", "/status", "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json;charset=utf-8");
    assert_eq!(headers["access-control-allow-origin"], "*");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["version"], "3.9.1");

    let (status, _, body) = send(&state, request("POST", "/route", ROUTE)).await;
    assert_eq!(status, StatusCode::OK);
    let posted: Value = serde_json::from_slice(&body).unwrap();
    assert!(posted["trip"]["legs"][0]["maneuvers"].is_array());

    // The same request as GET ?json= (curl-style percent-encoding).
    let query: String = ROUTE
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let (status, _, body) = send(&state, request("GET", &format!("/route?json={query}"), "")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(), posted);

    // GPX is an attachment.
    let gpx = ROUTE.replacen('{', r#"{"format": "gpx", "#, 1);
    let (status, headers, _) = send(&state, request("POST", "/route", &gpx)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/gpx+xml;charset=utf-8");
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=route.gpx"
    );
}

#[tokio::test]
async fn answers_bad_methods_and_paths_as_mainline_does() {
    let state = state(1, Duration::from_secs(30));
    for method in ["PUT", "DELETE", "OPTIONS", "TRACE"] {
        let (status, headers, body) = send(&state, request(method, "/route", ROUTE)).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_eq!(headers["access-control-allow-origin"], "*");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error_code"], 101, "{method}");
    }
    let (status, headers, body) = send(&state, request("PATCH", "/route", ROUTE)).await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert_eq!(body, b"The HTTP request method is not supported");
    assert!(!headers.contains_key("content-type"));

    for path in ["/", "/spaceship", "/route/extra"] {
        let (status, _, body) = send(&state, request("GET", path, "")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error_code"], 106, "{path}");
    }
}

#[tokio::test]
async fn limits_bodies() {
    let state = state(1, Duration::from_secs(30));
    let large = format!("{ROUTE}{}", " ".repeat(2 << 20));
    let (status, headers, _) = send(&state, request("POST", "/route", &large)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(headers["access-control-allow-origin"], "*");
}

#[tokio::test]
async fn times_out_waiting_for_a_worker_but_not_for_status() {
    let state = state(1, Duration::from_millis(50));
    // Every worker is busy.
    let _busy = state.workers().clone().acquire_owned().await.unwrap();
    let (status, headers, _) = send(&state, request("POST", "/route", ROUTE)).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(headers["access-control-allow-origin"], "*");
    // The health check doesn't wait for one.
    let (status, _, _) = send(&state, request("GET", "/status", "")).await;
    assert_eq!(status, StatusCode::OK);
}

fn cli(args: &[&str]) -> Cli {
    Cli::try_parse_from(core::iter::once("valinor-server").chain(args.iter().copied())).unwrap()
}

#[test]
fn takes_tiles_from_the_command_line_or_the_config() {
    let config =
        RouterConfig::from_valhalla_json(r#"{"mjolnir": {"tile_extract": "tiles.tar"}}"#).unwrap();
    // As the deployment passes them.
    let given = cli(&[
        "--config=/releases/1/routing/valhalla.json",
        "--tiles=/releases/1/routing/tiles.tar",
        "--listen=0.0.0.0:8002",
    ]);
    assert_eq!(
        tile_sets(&given, &config).unwrap(),
        vec![PathBuf::from("/releases/1/routing/tiles.tar")]
    );
    assert_eq!(given.listen, "0.0.0.0:8002".parse().unwrap());
    let several = cli(&[
        "--config", "c.json", "--tiles", "a.tar", "b", "--tiles", "c.tar",
    ]);
    assert_eq!(several.tiles.len(), 3);

    // Without --tiles, the config's extract next to the config.
    let fallback = cli(&["--config", "/releases/1/routing/valhalla.json"]);
    assert_eq!(
        tile_sets(&fallback, &config).unwrap(),
        vec![PathBuf::from("/releases/1/routing/tiles.tar")]
    );
    assert_eq!(fallback.listen, "0.0.0.0:8002".parse().unwrap());
    let bare = cli(&["--config", "valhalla.json"]);
    assert_eq!(
        tile_sets(&bare, &config).unwrap(),
        vec![PathBuf::from("tiles.tar")]
    );
    assert!(tile_sets(&fallback, &RouterConfig::default()).is_err());
    assert!(Cli::try_parse_from(["valinor-server"]).is_err());
}

#[test]
fn opens_the_configured_tiles() {
    let dir = fixtures();
    let config =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../valhalla-graphtile/fixtures/x.json");
    let given = cli(&[
        "--config",
        config.to_str().unwrap(),
        "--tiles",
        dir.join("andorra-tiles.tar").to_str().unwrap(),
    ]);
    // The config file doesn't exist.
    assert!(open(&given).is_err());
}

/// A small synthetic feed over the Andorra fixture, with service every day for years,
/// so that the server's real clock always falls inside its coverage.
fn transit_state(token: Option<&str>) -> Arc<app::AppState> {
    let source = valinor_gtfs::InMemorySource::new()
        .with(
            "agency.txt",
            "agency_name,agency_timezone,agency_url\nAndorra Bus,Europe/Andorra,https://b.test\n",
        )
        .with("routes.txt", "route_id,route_short_name,route_type\nL1,1,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\nA,Plaça,42.5078,1.5211\nB,Meritxell,42.5095,1.5345\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             DAILY,1,1,1,1,1,1,1,20260101,20361231\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nL1,DAILY,T1\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nT1,09:00:00,09:00:00,A,1\nT1,09:08:00,09:08:00,B,2\n",
        );
    let compiled = valinor_gtfs::compile(vec![valinor_gtfs::FeedInput {
        id: valinor_gtfs::FeedId::try_new("bus".to_owned()).unwrap(),
        source: Box::new(source),
    }])
    .unwrap();
    let timetable = valinor_gtfs::LoadedTimetable::from_bytes(compiled.bytes).unwrap();
    let planner = valinor_transit_plan::StreetPlanner::open_with_built_transfers(
        valinor_gtfs::TimetableSet::single(timetable).unwrap(),
        &[fixtures().join("andorra-tiles.tar")],
        valinor_transit_plan::StreetConfig::default(),
    )
    .unwrap();
    let router = Router::open(
        RouterConfig::default(),
        &[fixtures().join("andorra-tiles.tar")],
    )
    .unwrap();
    Arc::new(
        app::AppState::new(router, 2, Duration::from_secs(30)).with_transit(Some(
            transit::Transit::new(planner, token.map(str::to_owned)),
        )),
    )
}

const PLAN: &str =
    r#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.5100,"lon":1.5350}}"#;

#[tokio::test]
async fn plans_transit_on_its_own_path() {
    let state = transit_state(Some("secret"));
    let (status, headers, body) = send(&state, request("POST", "/transit/plan", PLAN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json;charset=utf-8");
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["journeys"].is_array());
    assert_eq!(body["realtime"]["level"], "schedule");

    // Malformed plans are 400s in transit's own error format.
    let (status, _, body) = send(&state, request("POST", "/transit/plan", "{nope")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["kind"], "invalid_request");

    // Valhalla's paths still answer alongside it.
    let (status, ..) = send(&state, request("GET", "/status", "")).await;
    assert_eq!(status, StatusCode::OK);
}

/// 2030-06-03 08:55 in Andorra, five minutes before the fixture's daily trip leaves `A`.
const BEFORE_THE_TRIP: i64 = 1_906_700_100;

#[tokio::test]
async fn finds_nearby_departures_and_a_routes_detail_on_their_own_paths() {
    let state = transit_state(None);
    let nearby = format!(
        r#"{{"location":{{"lat":42.5078,"lon":1.5205}},"time":{BEFORE_THE_TRIP},"max_walk_seconds":600}}"#
    );
    let (status, headers, body) = send(&state, request("POST", "/transit/nearby", &nearby)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/json;charset=utf-8");
    let body: Value = serde_json::from_slice(&body).unwrap();
    let page = &body["routes"][0]["pages"][0];
    assert_eq!(body["routes"][0]["route"]["route_id"], "L1");
    assert_eq!(page["stop"]["stop_id"], "A");
    assert_eq!(page["departures"][0]["trip_id"], "T1");
    assert_eq!(body["realtime"]["level"], "schedule");

    // The detail takes the identifiers the row's page gave.
    let route = format!(
        r#"{{"feed":"bus","route_id":"L1","headsigns":{},"stop_id":"A","time":{BEFORE_THE_TRIP},"walk_seconds":{}}}"#,
        page.get("headsigns").unwrap_or(&Value::Array(Vec::new())),
        page["walk_seconds"]
    );
    let (status, _, body) = send(&state, request("POST", "/transit/route", &route)).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["departures"][0]["trip_id"], "T1");
    assert_eq!(body["ride"]["stops"][1]["stop_id"], "B");

    // What isn't there is a 404 in transit's own error format, and a bad body a 400.
    let unknown = format!(
        r#"{{"feed":"bus","route_id":"L9","headsigns":[],"stop_id":"A","time":{BEFORE_THE_TRIP}}}"#
    );
    let (status, _, body) = send(&state, request("POST", "/transit/route", &unknown)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["kind"], "unknown_route");
    // A route is asked for by a page's headsigns, not by a direction.
    let old = format!(
        r#"{{"feed":"bus","route_id":"L1","direction_id":0,"stop_id":"A","time":{BEFORE_THE_TRIP}}}"#
    );
    let (status, _, body) = send(&state, request("POST", "/transit/route", &old)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["kind"], "invalid_request");
    let (status, _, body) = send(&state, request("POST", "/transit/nearby", "{}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["kind"], "invalid_request");
}

#[tokio::test]
async fn transit_paths_are_unknown_when_it_is_not_configured() {
    let state = state(1, Duration::from_secs(30));
    for path in ["/transit/plan", "/transit/nearby", "/transit/route"] {
        let (status, ..) = send(&state, request("POST", path, PLAN)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test]
async fn looks_up_vehicles_by_id() {
    let state = transit_state(Some("secret"));
    let (status, _, body) = send(
        &state,
        request("GET", "/transit/vehicle/bus/does-not-exist", ""),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert!(body["vehicle"].is_null());
}

fn ingest_request(token: Option<&str>, kind: &str, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!("/transit/feeds/bus/{kind}"));
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::test]
async fn ingest_needs_the_shared_secret() {
    let state = transit_state(Some("secret"));
    let (status, ..) = send(&state, ingest_request(None, "alerts", "x")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, ..) = send(&state, ingest_request(Some("wrong"), "alerts", "x")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, ..) = send(&state, ingest_request(Some("secret"), "nonsense", "x")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Bytes that aren't GTFS-RT degrade to the schedule and are reported.
    let (status, _, body) = send(
        &state,
        ingest_request(Some("secret"), "alerts", "not a feed"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["outcome"], "undecodable");

    // An empty body clears the feed.
    let (status, _, body) = send(&state, ingest_request(Some("secret"), "alerts", "")).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["outcome"], "cleared");
}

#[tokio::test]
async fn ingest_is_refused_without_a_configured_token() {
    let state = transit_state(None);
    let (status, ..) = send(&state, ingest_request(Some("x"), "alerts", "x")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn review_ingest_reads_the_body_before_checking_the_token() {
    let state = transit_state(Some("secret"));
    let large = "x".repeat(2 << 20);
    // No token at all: an unauthenticated client should get 401, not have its body buffered.
    let (status, ..) = send(&state, ingest_request(None, "alerts", &large)).await;
    println!("unauthenticated 2 MiB PUT: {status}");
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn review_an_empty_configured_token_refuses_ingest() {
    let parsed = cli(&["--config", "c.json", "--transit-ingest-token", ""]);
    println!("empty flag parses as {:?}", parsed.transit_ingest_token);
    let state = transit_state(Some(""));
    let request = Request::builder()
        .method("PUT")
        .uri("/transit/feeds/bus/alerts")
        .header("authorization", "Bearer ")
        .body(Body::from(""))
        .unwrap();
    let (status, ..) = send(&state, request).await;
    println!("empty token, header 'Bearer ': {status}");
    assert_ne!(status, StatusCode::OK);
}

/// An empty environment variable must behave like an unset one.
#[test]
fn an_empty_transit_feeds_variable_is_no_subset() {
    // SAFETY: this test binary reads no other environment concurrently for this variable.
    unsafe { std::env::set_var("TRANSIT_FEEDS", "") };
    let cli = Cli::try_parse_from(["valinor-server", "--config", "x"]).expect("parses");
    unsafe { std::env::remove_var("TRANSIT_FEEDS") };
    assert!(
        cli.transit_feeds.is_empty(),
        "an empty TRANSIT_FEEDS parsed as {:?}, which then fails with unknown feed",
        cli.transit_feeds
    );
}
