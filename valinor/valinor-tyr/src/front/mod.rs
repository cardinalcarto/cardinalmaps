//! The front: parsing requests into `Api.options`, as mainline's `ParseApi` does.
//!
//! In mainline, the HTTP server hands Loki's worker the raw request,
//! and `ParseApi` (`src/worker.cc`) turns it into the `options` every later stage reads,
//! filling in defaults and rejecting what it cannot use.
//! This module does the same, step for step,
//! so that the `options` match mainline's exactly
//! (see "Match tolerances" in `docs/routing.md`).
//!
//! - [`parse_http_request`] is the entry point for HTTP requests:
//!   the method, the action from the path, GET `?json=` or a POST body
//!   (JSON, or PBF with `Content-Type: application/x-protobuf`), and the other query parameters.
//! - [`parse_json_request`] parses a JSON request for a known action
//!   (mainline's `ParseApi(const std::string&, Options::Action, Api&)`).
//! - [`from_json`] is the shared core, for a document that is already parsed.
//!
//! The costing options are parsed by [`crate::costing_options`].
//!
//! # What the front does not do
//!
//! Loki's worker checks more before it searches,
//! and those checks belong to Loki (they need its configuration):
//! the action must be one the service offers (106),
//! routes need two locations (120) and a costing (124),
//! and location counts and distances must be within the service limits (150 and 154).
//! Loki also fills in per-location defaults
//! (radius, heading tolerance, search cutoff, and so on; see the tests),
//! and caps `alternates`.

mod http;
pub mod json;
mod locations;
mod options;
mod values;

pub use http::{HttpRequest, PBF_CONTENT_TYPE, parse_action, parse_http_request};
pub use json::{JsonNumber, JsonParseError, JsonValue};
pub use options::from_json;
pub use values::{LOCALES, is_iso_valid};

use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_response::{ErrorCode, ValhallaError};

/// Parses a JSON request for `action` into `api.options`
/// (mainline's `ParseApi(const std::string&, Options::Action, Api&)`).
///
/// An empty request is an empty object.
/// If `api` already has options, an empty request validates them instead (as a PBF request).
///
/// # Errors
///
/// 100 if the JSON does not parse, and otherwise whatever [`from_json`] raises.
pub fn parse_json_request(json: &[u8], action: Action, api: &mut Api) -> Result<(), ValhallaError> {
    let mut document = if json.is_empty() {
        JsonValue::empty_object()
    } else {
        JsonValue::parse(json).map_err(|_| ValhallaError::new(ErrorCode::JsonParseFailed))?
    };
    from_json(&mut document, Some(action), api)
}

#[cfg(test)]
mod tests;
