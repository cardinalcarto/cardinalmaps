//! Directions for route-like requests, as mainline's `odin_worker_t::work` builds them:
//! Odin narrates the trip, then Tyr serializes the response in the same process.

use http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use valhalla_microservice::WorkerResult;
use valhalla_proto::Api;
use valinor_tyr::tyr::{self, GPX_CONTENT_DISPOSITION, GPX_CONTENT_TYPE, Response};

/// Narrates the request's trip and serializes the response in the requested format.
///
/// An error from either step is serialized the way mainline's worker serializes it,
/// in the request's format.
pub fn directions(request: &mut Api) -> WorkerResult {
    let response = illuvatar::narrate(request).and_then(|()| tyr::serialize_directions(request));
    match response {
        Ok(response) => http_response(response),
        Err(error) => WorkerResult::error(&error, request),
    }
}

/// The worker's response for a serialized body.
pub(super) fn http_response(response: Response) -> WorkerResult {
    let mut headers = HeaderMap::new();
    // The content types are constants, so they are always valid header values.
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static(response.content_type),
    );
    // Mainline sends GPX as an attachment.
    if response.content_type == GPX_CONTENT_TYPE {
        headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static(GPX_CONTENT_DISPOSITION),
        );
    }
    WorkerResult::HttpResponse {
        status_code: StatusCode::OK,
        headers,
        body: response.body,
    }
}
