//! The HTTP side: every request goes to the router, as mainline's httpd hands every request
//! to its workers.
//!
//! What clients see follows mainline's server (`valhalla_service`, with `prime_server`):
//!
//! - Valhalla's paths at the root (`/route`, `/status`, ...), with `GET ?json=` or a POST body.
//!   Unknown paths are Loki's error 106 (404), and methods other than GET and POST are 101 (405),
//!   as the router answers them; methods `prime_server` doesn't parse at all
//!   (anything but OPTIONS, HEAD, GET, POST, PUT, DELETE, TRACE, and CONNECT) get its
//!   plain 501.
//! - `Access-Control-Allow-Origin: *` on every response, as mainline's workers add it.
//! - The router's content type, and `Content-Disposition` for GPX.
//!
//! Routing is synchronous, so it runs on Tokio's blocking pool, at most `workers` requests at
//! once (a semaphore with as many permits as the router has path search workspaces).
//! A request that isn't answered within the timeout (waiting for a worker included) is a 504,
//! and its path search is interrupted, as it is when the client goes away.
//! `/status` is cheap and is the deployment's health probe, so it doesn't wait for a worker.
//!
//! Public transit, when configured, has its own paths under `/transit`; see [`crate::transit`].

use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_DISPOSITION, CONTENT_TYPE, HeaderValue,
};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing::{debug, error, warn};
use valinor_router::{HttpRequest, HttpResponse, Router};

/// The methods `prime_server`'s HTTP parser knows; it answers any other with a plain 501.
const KNOWN_METHODS: [Method; 8] = [
    Method::OPTIONS,
    Method::HEAD,
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::TRACE,
    Method::CONNECT,
];

/// `prime_server`'s body for a method it doesn't parse.
const UNSUPPORTED_METHOD: &str = "The HTTP request method is not supported";

/// The body of a 504 (the router didn't answer in time).
const TIMED_OUT: &str = "The server didn't respond in time";

/// What every request shares.
pub struct AppState {
    router: Arc<Router>,
    workers: Arc<Semaphore>,
    timeout: Duration,
    transit: Option<Arc<crate::transit::Transit>>,
}

impl AppState {
    /// `workers` should be the router's number of workspaces, so that no worker waits for one.
    pub fn new(router: Router, workers: usize, timeout: Duration) -> Self {
        Self {
            router: Arc::new(router),
            workers: Arc::new(Semaphore::new(workers)),
            timeout,
            transit: None,
        }
    }

    /// Serves transit too, when `transit` is given.
    #[must_use]
    pub fn with_transit(mut self, transit: Option<crate::transit::Transit>) -> Self {
        self.transit = transit.map(Arc::new);
        self
    }

    pub(crate) fn transit(&self) -> Option<&Arc<crate::transit::Transit>> {
        self.transit.as_ref()
    }

    pub(crate) fn workers_semaphore(&self) -> &Arc<Semaphore> {
        &self.workers
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }

    #[cfg(test)]
    pub fn workers(&self) -> &Arc<Semaphore> {
        &self.workers
    }
}

/// The server: every path and method goes to [`handle`].
pub fn app(state: Arc<AppState>, max_body_bytes: usize) -> axum::Router {
    crate::transit::routes(&state)
        .fallback(handle)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        // Outermost, so that the body limit's 413 has it too.
        .layer(axum::middleware::map_response(with_cors))
        .with_state(state)
}

/// Sets the request's interrupt flag when dropped: when the request times out,
/// and when the client goes away (axum drops the handler's future).
struct InterruptOnDrop(Arc<AtomicBool>);

impl Drop for InterruptOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn handle(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let start = Instant::now();
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_owned(), |p| p.as_str().to_owned());
    let response = if KNOWN_METHODS.contains(&method) {
        route(&state, method.clone(), target.clone(), &headers, body).await
    } else {
        bare(StatusCode::NOT_IMPLEMENTED, UNSUPPORTED_METHOD)
    };
    let status = response.status();
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    if status.is_server_error() {
        warn!("{method} {target}: {status} in {elapsed:.1} ms");
    } else {
        debug!("{method} {target}: {status} in {elapsed:.1} ms");
    }
    response
}

async fn route(
    state: &AppState,
    method: Method,
    target: String,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers.get(CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
    let path = target.split('?').next().unwrap_or_default();
    if path == "/status" {
        let request = HttpRequest {
            method: &method,
            target: target.as_bytes(),
            content_type: content_type.as_deref(),
            body: &body,
        };
        return to_response(state.router.handle(&request));
    }

    let deadline = tokio::time::Instant::now() + state.timeout;
    let Ok(Ok(permit)) =
        tokio::time::timeout_at(deadline, state.workers.clone().acquire_owned()).await
    else {
        return timed_out();
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let guard = InterruptOnDrop(interrupted.clone());
    let router = state.router.clone();
    let work = tokio::task::spawn_blocking(move || {
        // The permit is held until the work is done, even if the client has gone,
        // so that at most `workers` searches ever run.
        let _permit = permit;
        let request = HttpRequest {
            method: &method,
            target: target.as_bytes(),
            content_type: content_type.as_deref(),
            body: &body,
        };
        router.handle_interruptible(&request, &|| interrupted.load(Ordering::Relaxed))
    });
    let result = tokio::time::timeout_at(deadline, work).await;
    // From here on, an unfinished search has nobody to answer.
    drop(guard);
    match result {
        Ok(Ok(Ok(response))) => to_response(response),
        // Only the guard interrupts, and only once the request has timed out or gone.
        Ok(Ok(Err(_))) | Err(_) => timed_out(),
        Ok(Err(join_error)) => {
            error!("a routing worker failed: {join_error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn timed_out() -> Response {
    bare(StatusCode::GATEWAY_TIMEOUT, TIMED_OUT)
}

/// A response with a body but no `Content-Type`, as `prime_server` sends its own errors.
fn bare(status: StatusCode, body: &'static str) -> Response {
    let mut response = Body::from(body).into_response();
    *response.status_mut() = status;
    response
}

fn to_response(response: HttpResponse) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(response.content_type),
    );
    if let Some(disposition) = response.content_disposition {
        headers.insert(CONTENT_DISPOSITION, HeaderValue::from_static(disposition));
    }
    (response.status, headers, response.body).into_response()
}

async fn with_cors(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    response
}
