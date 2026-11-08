//! The stage chain in one process: what mainline's whole server does for a request.

use crate::config::RouterConfig;
use crate::pool::WorkspacePool;
use crate::tiles::{OpenError, SharedTiles, TileSets};
use http::{Method, StatusCode};
use illuvatar::Odin;
use std::path::Path;
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_response::{ERROR_CONTENT_TYPE, ErrorCode, ValhallaError};
use valinor_loki::{ConnectivityMap, HttpOutcome, Loki, handle_request};
use valinor_thor::Thor;
use valinor_tyr::front::HttpRequest;
use valinor_tyr::tyr::{self, GPX_CONTENT_DISPOSITION, GPX_CONTENT_TYPE, Response};

/// A response, as mainline's server sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// The status: 200, or the error's status.
    pub status: StatusCode,
    /// The `Content-Type`.
    pub content_type: &'static str,
    /// The `Content-Disposition`, which mainline sends only for GPX (an attachment).
    pub content_disposition: Option<&'static str>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn ok(response: Response) -> Self {
        Self {
            status: StatusCode::OK,
            content_type: response.content_type,
            content_disposition: (response.content_type == GPX_CONTENT_TYPE)
                .then_some(GPX_CONTENT_DISPOSITION),
            body: response.body,
        }
    }

    /// An error, serialized from the request as it was when it failed
    /// (its format picks the body), as mainline's workers serialize errors.
    fn error(error: &ValhallaError, api: &mut Api) -> Self {
        Self {
            status: error.http_status(),
            content_type: ERROR_CONTENT_TYPE,
            content_disposition: None,
            body: error.serialize(api),
        }
    }
}

/// A request was interrupted, so there is no response to send
/// (as mainline's workers send nothing for an interrupted request).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("handle the request: it was interrupted")]
pub struct Interrupted;

/// Valhalla's whole server in one process: the front, Loki, Thor, Odin, and Tyr.
///
/// It is [`Send`] and [`Sync`]: one router serves every thread.
/// Each request runs on its caller's thread; Loki, Odin, and Tyr hold nothing per request,
/// and path searches take a workspace from a bounded pool
/// (see [`RouterConfig::workspaces`]).
pub struct Router {
    loki: Loki<SharedTiles>,
    thor: Thor<SharedTiles>,
    odin: Odin,
    workspaces: WorkspacePool,
}

impl Router {
    /// Opens a router over tile sets: mainline `build_extract` tarballs or tile directories,
    /// which serve as one tile set (any of them can supply a tile).
    ///
    /// The sets must come from one build; see [`crate::tiles`] for the check.
    ///
    /// # Errors
    ///
    /// See [`OpenError`].
    pub fn open<P: AsRef<Path>>(config: RouterConfig, tile_sets: &[P]) -> Result<Self, OpenError> {
        let sets = TileSets::open(tile_sets)?;
        let connectivity = ConnectivityMap::new(sets.tile_ids());
        let last_modified = sets.last_modified();
        let tiles = SharedTiles::new(sets, config.tile_cache_size);
        Ok(Self {
            loki: Loki::new(config.loki, tiles.clone(), Some(connectivity))
                .with_tileset_last_modified(last_modified),
            thor: Thor::new(config.thor, tiles),
            odin: config.odin,
            workspaces: WorkspacePool::new(config.workspaces.get()),
        })
    }

    /// The tile sets.
    #[inline]
    pub fn tiles(&self) -> &TileSets {
        self.loki.provider().sets()
    }

    /// Answers a `route` request body (Valhalla JSON, as Ferrostar's request generator makes it),
    /// as `POST /route` would.
    pub fn route(&self, body: &[u8]) -> HttpResponse {
        self.handle(&HttpRequest {
            method: &Method::POST,
            target: b"/route",
            content_type: Some(b"application/json"),
            body,
        })
    }

    /// Answers an HTTP request as mainline's server does: every action Loki and Thor serve,
    /// and mainline's errors for the rest (in the request's format).
    pub fn handle(&self, request: &HttpRequest<'_>) -> HttpResponse {
        match self.handle_interruptible(request, &|| false) {
            Ok(response) => response,
            // `never` never interrupts.
            Err(Interrupted) => HttpResponse::error(
                &ValhallaError::new(ErrorCode::ThorUnknown),
                &mut Api::default(),
            ),
        }
    }

    /// [`Router::handle`], giving up when `interrupt` returns true.
    ///
    /// Path search checks it every 5000 iterations and trip building before each leg,
    /// as mainline checks its interrupt;
    /// location search and narration (which take milliseconds) don't.
    ///
    /// # Errors
    ///
    /// [`Interrupted`] if the request was interrupted: there is nothing to send.
    pub fn handle_interruptible(
        &self,
        request: &HttpRequest<'_>,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<HttpResponse, Interrupted> {
        match handle_request(&self.loki, request) {
            HttpOutcome::Forward(mut api) => self.thor_and_odin(&mut api, interrupt),
            HttpOutcome::Respond {
                content_type, body, ..
            } => Ok(HttpResponse::ok(Response { content_type, body })),
            HttpOutcome::Error { error, mut api } => Ok(HttpResponse::error(&error, &mut api)),
        }
    }

    /// Thor's worker (`thor_worker_t::work`), then Odin's (`odin_worker_t::work`).
    fn thor_and_odin(
        &self,
        api: &mut Api,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<HttpResponse, Interrupted> {
        if interrupt() {
            return Err(Interrupted);
        }
        let action = api.options.as_ref().map_or(0, |o| o.action);
        match Action::try_from(action) {
            Ok(Action::Route) => {
                if let Err(error) = self.path_and_trip(api, interrupt) {
                    if interrupt() {
                        return Err(Interrupted);
                    }
                    return Ok(HttpResponse::error(&error, api));
                }
                let response = self
                    .odin
                    .narrate(api)
                    .and_then(|()| tyr::serialize_directions(api));
                Ok(match response {
                    Ok(response) => HttpResponse::ok(response),
                    Err(error) => HttpResponse::error(&error, api),
                })
            }
            // Thor passes status on to Odin, which serializes it.
            Ok(Action::Status) => Ok(match tyr::serialize_status(api) {
                Ok(response) => HttpResponse::ok(response),
                Err(error) => HttpResponse::error(&error, api),
            }),
            // Mainline's Thor serves these too; 107 is its code for an action
            // it means to support (here: what Valinor doesn't have yet).
            Ok(
                Action::OptimizedRoute
                | Action::SourcesToTargets
                | Action::Isochrone
                | Action::TraceRoute
                | Action::TraceAttributes
                | Action::Expansion
                | Action::Centroid,
            ) => Ok(HttpResponse::error(&ErrorCode::NotImplemented.into(), api)),
            // `thor_worker_t::work`: "this should never happen" (Loki forwards nothing else).
            _ => Ok(HttpResponse::error(
                &ErrorCode::ThorUnknownAction.into(),
                api,
            )),
        }
    }

    /// Path search, then trip building, with a workspace from the pool.
    fn path_and_trip(
        &self,
        api: &mut Api,
        interrupt: &dyn Fn() -> bool,
    ) -> Result<(), ValhallaError> {
        let now = jiff::Timestamp::now();
        let paths = {
            let mut workspace = self.workspaces.take();
            self.thor
                .route_interruptible(workspace.get(), api, now, interrupt)?
        };
        // The workspace is back in the pool: trip building doesn't search.
        self.thor
            .build_trip_interruptible(api, &paths, now, interrupt)
    }
}

#[cfg(test)]
mod tests;
