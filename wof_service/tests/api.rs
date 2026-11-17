//! The API against a tiny Who's On First database built here, with the
//! tables and R-trees of the real one.

use std::io::Read;
use std::path::Path;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use geo::{BoundingRect, Geometry, Point, Polygon, Rect, coord};
use rusqlite::{Connection, params};
use serde_json::Value;
use tower::ServiceExt;
use wof_service::{AppState, Config, router, spatialite};

const COUNTRY: u64 = 85633793;
const REGION: u64 = 1001;
const LOCALITY: u64 = 1002;
const LABELLED: u64 = 1003;
const FAR_POINT: u64 = 1004;
const CAMPUS: u64 = 1005;
const ABBR_ONLY: u64 = 1006;
const L_SHAPED: u64 = 1007;
const UNSHARDED: u64 = 1008;
const FAR_ANCESTOR: u64 = 1010;
const BUFFERED: u64 = 1011;
const ACROSS: u64 = 1012;

fn rect(w: f64, s: f64, e: f64, n: f64) -> Polygon<f64> {
    Rect::new(coord! { x: w, y: s }, coord! { x: e, y: n }).to_polygon()
}

struct Fixture(Connection);

impl Fixture {
    fn new(path: &Path) -> Self {
        let db = Connection::open(path).unwrap();
        db.execute_batch(
            "CREATE TABLE place (source TEXT NOT NULL, id TEXT NOT NULL, class TEXT NOT NULL, type TEXT NOT NULL);
             CREATE TABLE property (source TEXT NOT NULL, id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL);
             CREATE TABLE geometry (source TEXT NOT NULL, id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'default',
                 geom GEOMETRY NOT NULL DEFAULT '');
             CREATE UNIQUE INDEX geometry_idx_unique ON geometry(source, id, role);
             CREATE VIRTUAL TABLE idx_geometry_geom USING rtree(pkid, xmin, xmax, ymin, ymax);
             CREATE TABLE shard (source TEXT NOT NULL, id TEXT NOT NULL, role TEXT NOT NULL DEFAULT 'default',
                 element INTEGER NOT NULL, geom POLYGON NOT NULL DEFAULT '');
             CREATE VIRTUAL TABLE idx_shard_geom USING rtree(pkid, xmin, xmax, ymin, ymax);
             CREATE TABLE hierarchy (parent_source TEXT NOT NULL, parent_id TEXT NOT NULL, child_source TEXT NOT NULL,
                 child_id TEXT NOT NULL, depth INTEGER NOT NULL, branch TEXT NOT NULL);
             CREATE TABLE name (source TEXT NOT NULL, id TEXT NOT NULL, lang TEXT NOT NULL COLLATE NOCASE,
                 tag TEXT NOT NULL COLLATE NOCASE, abbr INTEGER NOT NULL, name TEXT NOT NULL COLLATE NOCASE);",
        )
        .unwrap();
        Self(db)
    }

    fn place(&self, id: u64, placetype: &str, name: &str) -> &Self {
        self.0
            .execute("INSERT INTO place VALUES ('wof', ?1, 'common', ?2)", params![id.to_string(), placetype])
            .unwrap();
        // Every place is its own ancestor at depth 0, as in the real database.
        self.0
            .execute("INSERT INTO hierarchy VALUES ('wof', ?1, 'wof', ?1, 0, 'wof:0')", params![id.to_string()])
            .unwrap();
        self.name(id, "und", "default", false, name)
    }

    fn name(&self, id: u64, lang: &str, tag: &str, abbr: bool, name: &str) -> &Self {
        self.0
            .execute(
                "INSERT INTO name VALUES ('wof', ?1, ?2, ?3, ?4, ?5)",
                params![id.to_string(), lang, tag, i64::from(abbr), name],
            )
            .unwrap();
        self
    }

    fn property(&self, id: u64, key: &str, value: &str) -> &Self {
        self.0.execute("INSERT INTO property VALUES ('wof', ?1, ?2, ?3)", params![id.to_string(), key, value]).unwrap();
        self
    }

    fn parent(&self, id: u64, parent: u64) -> &Self {
        self.0
            .execute(
                "INSERT INTO hierarchy VALUES ('wof', ?1, 'wof', ?2, 1, 'wof:0')",
                params![parent.to_string(), id.to_string()],
            )
            .unwrap();
        self
    }

    fn geometry(&self, id: u64, role: &str, geometry: Geometry<f64>) -> &Self {
        self.0
            .execute(
                "INSERT INTO geometry VALUES ('wof', ?1, ?2, ?3)",
                params![id.to_string(), role, spatialite::encode(&geometry)],
            )
            .unwrap();
        let rowid = self.0.last_insert_rowid();
        let r = geometry.bounding_rect().unwrap();
        self.0
            .execute(
                "INSERT INTO idx_geometry_geom VALUES (?1, ?2, ?3, ?4, ?5)",
                params![rowid, r.min().x, r.max().x, r.min().y, r.max().y],
            )
            .unwrap();
        self
    }

    fn shard(&self, id: u64, role: &str, element: i64, polygon: Polygon<f64>) -> &Self {
        let r = polygon.bounding_rect().unwrap();
        self.0
            .execute(
                "INSERT INTO shard VALUES ('wof', ?1, ?2, ?3, ?4)",
                params![id.to_string(), role, element, spatialite::encode(&Geometry::Polygon(polygon))],
            )
            .unwrap();
        let rowid = self.0.last_insert_rowid();
        self.0
            .execute(
                "INSERT INTO idx_shard_geom VALUES (?1, ?2, ?3, ?4, ?5)",
                params![rowid, r.min().x, r.max().x, r.min().y, r.max().y],
            )
            .unwrap();
        self
    }

    /// A place with a boundary, sharded whole as one piece.
    fn area(&self, id: u64, placetype: &str, name: &str, shape: Polygon<f64>) -> &Self {
        self.place(id, placetype, name)
            .geometry(id, "boundary", Geometry::Polygon(shape.clone()))
            .shard(id, "boundary", 0, shape)
    }
}

fn point(x: f64, y: f64) -> Geometry<f64> {
    Geometry::Point(Point::new(x, y))
}

/// Builds the fixture database in `dir` and returns its path.
fn build(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("wof.spatial.db");
    let f = Fixture::new(&path);
    f.area(COUNTRY, "country", "Testland", rect(0.0, 0.0, 10.0, 10.0))
        .property(COUNTRY, "wof:lang_x_spoken", "eng, fra")
        .property(COUNTRY, "alpha2", "TL")
        .geometry(COUNTRY, "label_position", point(5.0, 5.0));
    f.area(REGION, "region", "Westshire", rect(0.0, 0.0, 5.0, 5.0)).parent(REGION, COUNTRY);
    f.area(LOCALITY, "locality", "Alpha", rect(1.0, 1.0, 2.0, 2.0))
        .name(LOCALITY, "eng", "preferred", false, "Alpha City")
        .name(LOCALITY, "eng", "variant", false, "Old Alpha")
        .name(LOCALITY, "eng", "colloquial", false, "The A")
        .name(LOCALITY, "und", "default", true, "AC")
        .property(LOCALITY, "wof:population", "12345.0")
        .property(LOCALITY, "wof:population_rank", "7")
        .property(LOCALITY, "wof:country", "TL")
        .property(LOCALITY, "wof:repo", "not wanted")
        .geometry(LOCALITY, "label_position", point(1.4, 1.6))
        .geometry(LOCALITY, "centroid", point(1.5, 1.5))
        .parent(LOCALITY, REGION)
        .parent(LOCALITY, COUNTRY)
        .parent(LOCALITY, FAR_ANCESTOR);
    // Only points: the label position wins over the centroid.
    f.place(LABELLED, "locality", "Beta")
        .geometry(LABELLED, "label_position", point(3.0, 3.0))
        .geometry(LABELLED, "centroid", point(9.0, 9.0))
        .parent(LABELLED, COUNTRY);
    // The real database has geometries that are empty text.
    f.0.execute("INSERT INTO geometry VALUES ('wof', ?1, 'reversegeo_position', '')", [LABELLED.to_string()]).unwrap();
    // The centroid is in the box, but the indexer places it by its label.
    f.place(FAR_POINT, "locality", "Gamma").geometry(FAR_POINT, "centroid", point(1.0, 1.0)).geometry(
        FAR_POINT,
        "reversegeo_position",
        point(8.0, 8.0),
    );
    f.area(CAMPUS, "campus", "Delta University", rect(1.2, 1.2, 1.3, 1.3));
    f.area(ABBR_ONLY, "locality", "unused", rect(1.2, 1.2, 1.3, 1.3));
    f.0.execute("DELETE FROM name WHERE id = ?1", [ABBR_ONLY.to_string()]).unwrap();
    f.name(ABBR_ONLY, "und", "default", true, "EPS");
    // An L whose bounds cover the box, but whose arms both miss it.
    let vertical = rect(0.0, 0.0, 0.4, 4.0);
    let horizontal = rect(0.0, 3.6, 4.0, 4.0);
    let l = Geometry::Polygon(Polygon::new(
        geo::LineString::from(vec![(0.0, 0.0), (0.4, 0.0), (0.4, 3.6), (4.0, 3.6), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)]),
        vec![],
    ));
    f.place(L_SHAPED, "locality", "Zeta")
        .geometry(L_SHAPED, "boundary", l)
        .shard(L_SHAPED, "boundary", 0, vertical)
        .shard(L_SHAPED, "boundary", 1, horizontal);
    // A boundary with no shards is tested whole.
    f.place(UNSHARDED, "neighbourhood", "Eta").geometry(
        UNSHARDED,
        "boundary",
        Geometry::Polygon(rect(3.0, 3.0, 4.0, 4.0)),
    );
    f.area(FAR_ANCESTOR, "macroregion", "Faraway", rect(50.0, 50.0, 60.0, 60.0));
    // Only its buffer meets the box.
    f.place(BUFFERED, "locality", "Theta")
        .geometry(BUFFERED, "boundary", Geometry::Polygon(rect(6.0, 6.0, 7.0, 7.0)))
        .shard(BUFFERED, "boundary", 0, rect(6.0, 6.0, 7.0, 7.0))
        .shard(BUFFERED, "buffer", 0, rect(1.0, 1.0, 7.0, 7.0));
    f.area(ACROSS, "locality", "Dateline", rect(179.0, 0.0, 180.0, 1.0));
    path
}

async fn get(app: &axum::Router, uri: &str, encoding: Option<&str>) -> (StatusCode, Vec<u8>, Option<String>) {
    let mut request = Request::get(uri);
    if let Some(encoding) = encoding {
        request = request.header(header::ACCEPT_ENCODING, encoding);
    }
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();
    let status = response.status();
    let content_encoding = response.headers().get(header::CONTENT_ENCODING).map(|v| v.to_str().unwrap().to_string());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
    (status, body, content_encoding)
}

fn ids(response: &Value) -> Vec<u64> {
    response["places"].as_array().unwrap().iter().map(|p| p["id"].as_u64().unwrap()).collect()
}

fn place(response: &Value, id: u64) -> &Value {
    response["places"].as_array().unwrap().iter().find(|p| p["id"] == id).unwrap()
}

#[tokio::test]
async fn serves_places_in_a_bbox() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(build(dir.path()), Config::default()).unwrap());

    let (status, body, encoding) = get(&app, "/v1/places?bbox=0.5,0.5,3.5,3.5", Some("gzip")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(encoding.as_deref(), Some("gzip"));
    let mut json = String::new();
    flate2::read::GzDecoder::new(&body[..]).read_to_string(&mut json).unwrap();
    let response: Value = serde_json::from_str(&json).unwrap();

    // Not FAR_POINT, CAMPUS, ABBR_ONLY, L_SHAPED, BUFFERED or ACROSS.
    assert_eq!(ids(&response), vec![REGION, LOCALITY, LABELLED, UNSHARDED, FAR_ANCESTOR, COUNTRY]);
    assert_eq!(response["bbox"], serde_json::json!([0.5, 0.5, 3.5, 3.5]));

    let alpha = place(&response, LOCALITY);
    assert_eq!(alpha["placetype"], "locality");
    assert_eq!(alpha["population"], 12345);
    assert_eq!(alpha["population_rank"], 7);
    assert_eq!(alpha["country"], "TL");
    assert_eq!(alpha["country_id"], COUNTRY);
    assert_eq!(alpha["label"], serde_json::json!([1.4, 1.6]));
    assert_eq!(alpha["bbox"], serde_json::json!([1.0, 1.0, 2.0, 2.0]));
    assert_eq!(alpha["geometry"]["type"], "Polygon");
    let mut ancestors: Vec<u64> = alpha["ancestors"].as_array().unwrap().iter().map(|a| a.as_u64().unwrap()).collect();
    ancestors.sort_unstable();
    assert_eq!(ancestors, vec![REGION, FAR_ANCESTOR, COUNTRY]);
    let names = alpha["names"].as_array().unwrap();
    assert!(names.contains(&serde_json::json!(["eng", "variant", false, "Old Alpha"])));
    assert!(names.contains(&serde_json::json!(["eng", "colloquial", false, "The A"])));
    assert!(names.contains(&serde_json::json!(["und", "default", true, "AC"])));
    assert!(alpha.get("ancestor_only").is_none());
    assert!(alpha.get("lang_x_spoken").is_none());

    let country = place(&response, COUNTRY);
    assert_eq!(country["lang_x_spoken"], serde_json::json!(["eng", "fra"]));
    assert_eq!(country["country_id"], COUNTRY);

    let beta = place(&response, LABELLED);
    assert_eq!(beta["geometry"], Value::Null);
    assert_eq!(beta["label"], serde_json::json!([3.0, 3.0]));
    assert_eq!(beta["bbox"], serde_json::json!([3.0, 3.0, 3.0, 3.0]));

    let far = place(&response, FAR_ANCESTOR);
    assert_eq!(far["ancestor_only"], true);
    assert_eq!(far["geometry"], Value::Null);
    assert_eq!(far["bbox"], serde_json::json!([50.0, 50.0, 60.0, 60.0]));
}

#[tokio::test]
async fn crosses_the_antimeridian() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(build(dir.path()), Config::default()).unwrap());
    let (status, body, encoding) = get(&app, "/v1/places?bbox=179.5,0.2,-179.5,0.8", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(encoding, None);
    let response: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(ids(&response), vec![ACROSS]);
}

#[tokio::test]
async fn compresses_with_zstd() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(build(dir.path()), Config::default()).unwrap());
    let (status, _, encoding) = get(&app, "/v1/places?bbox=0,0,10,10", Some("zstd")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(encoding.as_deref(), Some("zstd"));
}

#[tokio::test]
async fn refuses_bad_and_oversized_requests() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config { max_area: 50.0, max_places: 3, ..Config::default() };
    let app = router(AppState::open(build(dir.path()), config).unwrap());

    let (status, body, _) = get(&app, "/v1/places?bbox=1,2,3", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(serde_json::from_slice::<Value>(&body).unwrap()["error"].is_string());
    assert_eq!(get(&app, "/v1/places?bbox=0,0,1,1&tolerance=0.5", None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(get(&app, "/v1/places?bbox=0,0,1,1&max_vertices=3", None).await.0, StatusCode::BAD_REQUEST);
    // 100 square degrees.
    assert_eq!(get(&app, "/v1/places?bbox=0,0,10,10", None).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    // More than three places.
    assert_eq!(get(&app, "/v1/places?bbox=0.5,0.5,3.5,3.5", None).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(get(&app, "/v1/places?bbox=4.5,4.5,4.9,4.9", None).await.0, StatusCode::OK);
}

#[tokio::test]
async fn reports_health() {
    let dir = tempfile::tempdir().unwrap();
    let app = router(AppState::open(build(dir.path()), Config::default()).unwrap());
    let (status, body, _) = get(&app, "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"ok");
}

#[test]
fn refuses_other_databases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.db");
    Connection::open(&path).unwrap().execute_batch("CREATE TABLE t (x)").unwrap();
    assert!(AppState::open(path, Config::default()).is_err());
}
