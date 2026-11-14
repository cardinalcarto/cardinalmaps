//! Public transit's own paths, beside Valhalla's.
//!
//! | Path | Purpose |
//! |---|---|
//! | `POST /transit/plan` | Plans a trip: the JSON of [`valinor_transit_plan::PlanRequestBody`] in, a `PlanResponse` out. |
//! | `POST /transit/nearby` | The routes a rider can catch near a place: [`valinor_transit_plan::NearbyRequestBody`] in, a `NearbyResponse` out. |
//! | `POST /transit/route` | One route and direction from one stop: [`valinor_transit_plan::RouteRequestBody`] in, a `RouteDetailResponse` out. |
//! | `GET /transit/vehicle/{feed}/{id}` | A vehicle's latest real-time state. |
//! | `PUT /transit/feeds/{feed}/{kind}` | Operator ingest of GTFS-RT bytes, authenticated by a shared secret. |
//!
//! The server fetches nothing itself. An operator's own job pushes each feed to the ingest
//! path as it is fetched (see the overnight decisions log for why this isn't a poller here),
//! and the server rebuilds its overlay on each push.
//! `kind` is one of `trip_updates`, `vehicle_positions`, `alerts`, or `combined`;
//! an empty body clears the feed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{FromRef, FromRequestParts, Path, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::Serialize;
use tracing::{error, warn};
use valinor_gtfs_rt::FeedKind;
use valinor_transit_plan::{FeedUpdate, IngestOutcome, JsonResponse, StateError, StreetPlanner};

use crate::app::AppState;

/// What every transit request shares.
pub struct Transit {
    planner: StreetPlanner,
    ingest_token: Option<String>,
}

impl Transit {
    /// Transit over a planner; `ingest_token` is the shared secret for the ingest path.
    ///
    /// An empty token configures no ingest: nothing an unset variable or flag set by mistake
    /// produces may grant access.
    pub fn new(planner: StreetPlanner, ingest_token: Option<String>) -> Self {
        Self {
            planner,
            ingest_token: ingest_token.filter(|token| !token.is_empty()),
        }
    }
}

/// The transit paths, if transit is configured; otherwise none, so they fall through as unknown.
pub fn routes(state: &Arc<AppState>) -> axum::Router<Arc<AppState>> {
    let router = axum::Router::new();
    if state.transit().is_none() {
        return router;
    }
    router
        .route("/transit/plan", post(plan))
        .route("/transit/nearby", post(nearby))
        .route("/transit/route", post(route))
        .route("/transit/vehicle/{feed}/{id}", get(vehicle))
        .route("/transit/feeds/{feed}/{kind}", put(ingest))
}

/// POSIX seconds now. The server is the only part of transit with a clock.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

fn json_response(answer: JsonResponse) -> Response {
    let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json;charset=utf-8"),
        )],
        answer.body,
    )
        .into_response()
}

fn error_response(status: u16, kind: &str, message: &str) -> Response {
    #[derive(Serialize)]
    struct Body<'a> {
        error: &'a str,
        kind: &'a str,
    }
    let body = serde_json::to_vec(&Body {
        error: message,
        kind,
    })
    .unwrap_or_default();
    json_response(JsonResponse { status, body })
}

fn timed_out() -> Response {
    error_response(504, "timed_out", "the server didn't respond in time")
}

/// Runs work on the blocking pool, at most `workers` at once, within the timeout.
///
/// The interrupt fires if the client goes away or the timeout passes.
async fn run_blocking(
    state: &AppState,
    work: impl FnOnce(&dyn Fn() -> bool) -> JsonResponse + Send + 'static,
) -> Response {
    let deadline = tokio::time::Instant::now() + state.timeout();
    let Ok(Ok(permit)) =
        tokio::time::timeout_at(deadline, state.workers_semaphore().clone().acquire_owned()).await
    else {
        return timed_out();
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let guard = InterruptOnDrop(interrupted.clone());
    let work = tokio::task::spawn_blocking(move || {
        // Held to the end, so at most `workers` searches run even for a client gone away.
        let _permit = permit;
        work(&|| interrupted.load(Ordering::Relaxed))
    });
    let result = tokio::time::timeout_at(deadline, work).await;
    drop(guard);
    match result {
        Ok(Ok(answer)) => json_response(answer),
        Ok(Err(join_error)) => {
            error!("a transit worker failed: {join_error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(_) => timed_out(),
    }
}

struct InterruptOnDrop(Arc<AtomicBool>);

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn plan(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(transit) = state.transit().cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    run_blocking(&state, move |interrupt| {
        transit.planner.plan_json(&body, unix_now(), interrupt)
    })
    .await
}

async fn nearby(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(transit) = state.transit().cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    run_blocking(&state, move |interrupt| {
        transit.planner.nearby_json(&body, unix_now(), interrupt)
    })
    .await
}

async fn route(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let Some(transit) = state.transit().cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    run_blocking(&state, move |interrupt| {
        transit.planner.route_json(&body, unix_now(), interrupt)
    })
    .await
}

async fn vehicle(
    State(state): State<Arc<AppState>>,
    Path((feed, id)): Path<(String, String)>,
) -> Response {
    let Some(transit) = state.transit().cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    run_blocking(&state, move |_| {
        transit.planner.vehicle_json(&feed, &id, unix_now())
    })
    .await
}

/// Parses the feed kind in an ingest path.
fn feed_kind(kind: &str) -> Option<FeedKind> {
    match kind {
        "trip_updates" => Some(FeedKind::TripUpdates),
        "vehicle_positions" => Some(FeedKind::VehiclePositions),
        "alerts" => Some(FeedKind::Alerts),
        "combined" => Some(FeedKind::Combined),
        _ => None,
    }
}

/// Whether a request carries the right shared secret, comparing without an early exit.
fn authorized(transit: &Transit, headers: &HeaderMap) -> bool {
    let Some(token) = transit.ingest_token.as_deref() else {
        return false;
    };
    let expected = format!("Bearer {token}");
    let Some(given) = headers.get(AUTHORIZATION) else {
        return false;
    };
    let given = given.as_bytes();
    if given.len() != expected.len() {
        return false;
    }
    given
        .iter()
        .zip(expected.as_bytes())
        .fold(0_u8, |sum, (a, b)| sum | (a ^ b))
        == 0
}

#[derive(Serialize)]
struct IngestBody {
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    built_at: Option<i64>,
}

/// Checks the shared secret from the headers alone, so a body is never read for a stranger.
struct IngestAuthorized;

impl<S> FromRequestParts<S> for IngestAuthorized
where
    Arc<AppState>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Response;

    #[expect(
        clippy::unused_async_trait_impl,
        reason = "the trait is async; a token check needs no I/O"
    )]
    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = Arc::<AppState>::from_ref(state);
        let Some(transit) = state.transit() else {
            return Err(StatusCode::NOT_FOUND.into_response());
        };
        if transit.ingest_token.is_none() {
            return Err(error_response(
                403,
                "ingest_disabled",
                "no ingest token is configured",
            ));
        }
        if !authorized(transit, &parts.headers) {
            return Err(error_response(
                401,
                "unauthorized",
                "a valid ingest token is required",
            ));
        }
        Ok(Self)
    }
}

async fn ingest(
    _authorized: IngestAuthorized,
    State(state): State<Arc<AppState>>,
    Path((feed, kind)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let Some(transit) = state.transit().cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(kind) = feed_kind(&kind) else {
        return error_response(400, "invalid_request", "unknown feed kind");
    };
    let work = tokio::task::spawn_blocking(move || {
        let now = unix_now();
        let update = FeedUpdate {
            feed: &feed,
            kind,
            bytes: (!body.is_empty()).then_some(body.as_ref()),
            received_at: now,
            stale_after: None,
        };
        // In one step, so that queries keep serving the old snapshot until this one is ready.
        let state = transit.planner.state();
        let result = match state.ingest_and_refresh(update, now) {
            Ok(result) => result,
            Err(StateError::UnknownFeed(feed)) => {
                return error_response(404, "unknown_feed", &format!("unknown feed {feed}"));
            }
            Err(error) => return error_response(500, "internal", &error.to_string()),
        };
        let built_at = match &result.snapshot {
            Ok(snapshot) => Some(snapshot.built_at()),
            Err(error) => {
                warn!("couldn't rebuild the transit overlay: {error}");
                None
            }
        };
        let (outcome, error) = match result.outcome {
            IngestOutcome::Accepted => ("accepted", None),
            IngestOutcome::Cleared => ("cleared", None),
            IngestOutcome::Undecodable(error) => ("undecodable", Some(error.to_string())),
        };
        json_response(JsonResponse {
            status: 200,
            body: serde_json::to_vec(&IngestBody {
                outcome,
                error,
                built_at,
            })
            .unwrap_or_default(),
        })
    });
    match work.await {
        Ok(response) => response,
        Err(join_error) => {
            error!("a transit worker failed: {join_error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
