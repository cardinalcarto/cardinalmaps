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
