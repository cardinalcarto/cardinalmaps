//! Serves the Who's On First places in a bounding box, with what the airmail
//! indexer needs of each, so a phone can index the admin areas of the map
//! it downloaded. See README.md for the API.

#![forbid(unsafe_code)]

pub mod geojson;
pub mod places;
pub mod simplify;
pub mod spatialite;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tower_http::compression::CompressionLayer;

use places::{Bbox, Database, Limits, Place};
use simplify::Simplification;

/// What requests may cost.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// The largest bbox, in square degrees.
    pub max_area: f64,
    /// The most places in a response, ancestors included.
    pub max_places: usize,
    /// How long a query may take.
    pub timeout: Duration,
    /// How many queries run at once; the rest wait.
    pub concurrency: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { max_area: 100.0, max_places: 50_000, timeout: Duration::from_secs(30), concurrency: 4 }
    }
}

struct Inner {
    path: PathBuf,
    config: Config,
    /// Open connections not in use.
    idle: Mutex<Vec<Database>>,
    permits: Semaphore,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

impl AppState {
    /// Opens the database at `path`, failing now if it isn't a WOF SpatiaLite
    /// database.
    pub fn open(path: PathBuf, config: Config) -> anyhow::Result<Self> {
        let db = Database::open(&path)?;
        Ok(Self(Arc::new(Inner {
            path,
            config,
            idle: Mutex::new(vec![db]),
            permits: Semaphore::new(config.concurrency.max(1)),
        })))
    }

    /// Runs `f` on a connection on a blocking thread.
    async fn with_db<T: Send + 'static>(&self, f: impl FnOnce(&Database) -> T + Send + 'static) -> Result<T, ApiError> {
        let _permit = self.0.permits.acquire().await.map_err(|_| ApiError::internal("shutting down"))?;
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let db = inner.idle.lock().unwrap().pop();
            let db = match db {
                Some(db) => db,
                None => Database::open(&inner.path).map_err(|e| ApiError::internal(e.to_string()))?,
            };
            let result = f(&db);
            inner.idle.lock().unwrap().push(db);
            Ok(result)
        })
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/places", get(places))
        .layer(CompressionLayer::new().gzip(true).zstd(true).no_br().no_deflate())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Result<&'static str, ApiError> {
    state.with_db(|db| db.check()).await?.map_err(|e| ApiError::internal(e.to_string()))?;
    Ok("ok")
}

#[derive(Debug, Deserialize)]
pub struct PlacesQuery {
    /// `west,south,east,north` in degrees.
    bbox: String,
    /// The simplification tolerance, relative to each shape's size.
    tolerance: Option<f64>,
    /// The most vertices of each shape.
    max_vertices: Option<usize>,
}

#[derive(Serialize)]
struct PlacesResponse<'a> {
    bbox: [f64; 4],
    tolerance: f64,
    max_vertices: usize,
    places: &'a [Place],
}

async fn places(State(state): State<AppState>, Query(query): Query<PlacesQuery>) -> Result<Response, ApiError> {
    let bbox = parse_bbox(&query.bbox).map_err(ApiError::bad_request)?;
    let config = state.0.config;
    if bbox.area() > config.max_area {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("the bbox is {:.1} square degrees; the most is {}", bbox.area(), config.max_area),
        ));
    }
    let tolerance = query.tolerance.unwrap_or(simplify::DEFAULT_TOLERANCE);
    if !(simplify::MIN_TOLERANCE..=simplify::MAX_TOLERANCE).contains(&tolerance) {
        return Err(ApiError::bad_request(format!(
            "tolerance must be from {} to {}",
            simplify::MIN_TOLERANCE,
            simplify::MAX_TOLERANCE
        )));
    }
    let max_vertices = query.max_vertices.unwrap_or(simplify::DEFAULT_MAX_VERTICES);
    if !(simplify::MIN_MAX_VERTICES..=simplify::MAX_MAX_VERTICES).contains(&max_vertices) {
        return Err(ApiError::bad_request(format!(
            "max_vertices must be from {} to {}",
            simplify::MIN_MAX_VERTICES,
            simplify::MAX_MAX_VERTICES
        )));
    }
    let simplification = Simplification { tolerance, max_vertices };
    let start = Instant::now();
    let limits = Limits { max_places: config.max_places, deadline: start + config.timeout };
    let body = state
        .with_db(move |db| {
            let places = db.places(bbox, simplification, limits)?;
            let body = serde_json::to_vec(&PlacesResponse {
                bbox: [bbox.west, bbox.south, bbox.east, bbox.north],
                tolerance,
                max_vertices,
                places: &places,
            })
            .expect("places serialize");
            Ok::<_, places::Error>((places.len(), body))
        })
        .await?;
    let (count, body) = body.map_err(|e| match e {
        places::Error::Sqlite(e) => ApiError::internal(e.to_string()),
        e @ (places::Error::TooManyPlaces(_) | places::Error::TooSlow) => {
            ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, e.to_string())
        }
    })?;
    log::info!("bbox={} places={count} bytes={} in {} ms", query.bbox, body.len(), start.elapsed().as_millis());
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
            // A deployment serves one database; responses change only with it.
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400")),
        ],
        body,
    )
        .into_response())
}

/// Parses `west,south,east,north`. West may be greater than east, for a box
/// across the antimeridian.
pub fn parse_bbox(bbox: &str) -> Result<Bbox, String> {
    let values: Vec<f64> = bbox
        .split(',')
        .map(|v| v.trim().parse::<f64>().map_err(|_| format!("bbox: {v:?} isn't a number")))
        .collect::<Result<_, _>>()?;
    let [west, south, east, north] = values[..] else {
        return Err("bbox must be west,south,east,north".into());
    };
    if !values.iter().all(|v| v.is_finite()) {
        return Err("bbox must be finite".into());
    }
    if !(-180.0..=180.0).contains(&west) || !(-180.0..=180.0).contains(&east) {
        return Err("bbox longitudes must be from -180 to 180".into());
    }
    if !(-90.0..=90.0).contains(&south) || !(-90.0..=90.0).contains(&north) || south > north {
        return Err("bbox latitudes must be from -90 to 90, south first".into());
    }
    Ok(Bbox { west, south, east, north })
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        let message = message.into();
        log::error!("{message}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, axum::Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bboxes() {
        assert_eq!(
            parse_bbox("-122.4,47.5,-122.2,47.7"),
            Ok(Bbox { west: -122.4, south: 47.5, east: -122.2, north: 47.7 })
        );
        let across = parse_bbox("179,-1,-179,1").unwrap();
        assert!((across.area() - 4.0).abs() < 1e-9);
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("1,2,3,x").is_err());
        assert!(parse_bbox("0,10,1,5").is_err());
        assert!(parse_bbox("0,0,181,1").is_err());
        assert!(parse_bbox("0,0,NaN,1").is_err());
    }
}
