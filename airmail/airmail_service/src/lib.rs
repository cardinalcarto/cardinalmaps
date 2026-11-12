// Only `poi_status` maps a file, with `#[allow(unsafe_code)]`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

use std::sync::Arc;

use axum::{Router, routing::get};

pub mod api;
pub mod error;
pub mod poi_status;

/// The service's routes.
pub fn router(state: Arc<api::AppState>) -> Router {
    Router::new()
        .route("/v1/search", get(api::search))
        .route("/v1/place", get(api::place))
        .route("/health", get(api::health))
        .route("/openapi.yaml", get(api::openapi))
        .fallback(api::not_found)
        .with_state(state)
}
