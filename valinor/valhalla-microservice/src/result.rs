//! What a worker does with a job, and how HTTP responses are written.

use crate::http_protocol::HttpRequestInfo;
use crate::interrupt::Interrupted;
use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, StatusCode};
use serde::Serialize;
use valhalla_proto::Api;
use valhalla_response::{ERROR_CONTENT_TYPE, ValhallaError};

/// The `Content-Type` mainline uses for JSON responses.
const JSON_CONTENT_TYPE: &str = "application/json;charset=utf-8";
/// The `Content-Type` mainline uses for PBF responses.
const PBF_CONTENT_TYPE: &str = "application/x-protobuf";

/// The result of a worker computation.
///
/// These correspond to `prime_server`'s `worker_t::result_t`:
/// either an intermediate result for the next stage, or a final response for the client.
#[derive(Debug)]
pub enum WorkerResult {
    /// Pass the request on to the next stage (`prime_server`'s `intermediate` results).
    ///
    /// The service sends the same request info it received, followed by this message,
    /// to the downstream proxy.
    /// (The message is boxed because an `Api` is large compared to the other variants.)
    Forward(Box<Api>),
    /// An HTTP response to be delivered to the client through the loopback.
    ///
    /// No further processing will be done downstream.
    /// Use helpers like [`WorkerResult::json`] and [`WorkerResult::error`] wherever possible
    /// to avoid footguns, like forgetting to set headers.
    HttpResponse {
        /// The response status.
        status_code: StatusCode,
        /// A map of HTTP headers.
        ///
        /// If you are constructing this variant on your own (or implementing a new helper),
        /// be careful to set `Content-Type`, and any other headers specific to this type of response.
        ///
        /// `Content-Length` and `Access-Control-Allow-Origin` are always set during serialization,
        /// clobbering any values here;
        /// `Connection` is set from the request when it asked for it and is not already present.
        headers: HeaderMap,
        /// The response body.
        body: Vec<u8>,
    },
    /// The job was interrupted, so nothing is sent.
    ///
    /// This is what `prime_server` does when a work function bails through its interrupt callback:
    /// the server has already answered the client (with a timeout, or because it disconnected).
    Interrupted(Interrupted),
}

impl WorkerResult {
    /// Forwards the request to the next stage.
    #[inline]
    pub fn forward(api: Api) -> WorkerResult {
        WorkerResult::Forward(Box::new(api))
    }

    /// A JSON HTTP response.
    pub fn json<T: Serialize>(status_code: StatusCode, value: T) -> WorkerResult {
        #[expect(clippy::missing_panics_doc)]
        let body = serde_json::to_vec(&value).expect("Programming error: either Serialize is incorrectly implemented, or the structure contains a map with non-string keys.");
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE));

        WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        }
    }

    /// A PBF HTTP response, with a serialized [`Api`] as the body.
    pub fn pbf(status_code: StatusCode, body: Vec<u8>) -> WorkerResult {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(PBF_CONTENT_TYPE));

        WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        }
    }

    /// An error response, serialized for the request's format as mainline does.
    ///
    /// This records the error in the request's `info` (see [`ValhallaError::serialize`]).
    pub fn error(error: &ValhallaError, request: &mut Api) -> WorkerResult {
        let (status_code, headers, body) = error_response(error, request);
        WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        }
    }
}

impl From<Interrupted> for WorkerResult {
    #[inline]
    fn from(interrupted: Interrupted) -> Self {
        WorkerResult::Interrupted(interrupted)
    }
}

/// The status, headers, and body of an error response (see [`WorkerResult::error`]).
pub(crate) fn error_response(
    error: &ValhallaError,
    request: &mut Api,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let body = error.serialize(request);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(ERROR_CONTENT_TYPE));
    (error.http_status(), headers, body)
}

/// Writes an HTTP response as `prime_server`'s `http_response_t::from_info` and `generic` do,
/// plus the CORS header that mainline's workers add to every response.
///
/// The status line uses the request's HTTP version.
/// Headers follow in map order (names are lowercase, which HTTP treats the same),
/// and `Content-Length` comes last.
pub(crate) fn serialize_http(
    request_info: &HttpRequestInfo,
    status_code: StatusCode,
    mut headers: HeaderMap,
    body: &[u8],
) -> Vec<u8> {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    // `prime_server` emplaces these, so a header the worker set wins,
    // and Keep-Alive wins over Close if the request somehow had both.
    if request_info.connection_keep_alive() {
        headers
            .entry(CONNECTION)
            .or_insert(HeaderValue::from_static("Keep-Alive"));
    }
    if request_info.connection_close() {
        headers
            .entry(CONNECTION)
            .or_insert(HeaderValue::from_static("Close"));
    }

    let mut response = Vec::with_capacity(256 + body.len());
    response.extend_from_slice(request_info.http_version_string().as_bytes());
    response.push(b' ');
    response.extend_from_slice(status_code.as_str().as_bytes());
    response.push(b' ');
    response.extend_from_slice(status_code.canonical_reason().unwrap_or("").as_bytes());
    response.extend_from_slice(b"\r\n");
    for (name, value) in &headers {
        if name == CONTENT_LENGTH {
            continue;
        }
        response.extend_from_slice(name.as_str().as_bytes());
        response.extend_from_slice(b": ");
        response.extend_from_slice(value.as_bytes());
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"content-length: ");
    response.extend_from_slice(body.len().to_string().as_bytes());
    response.extend_from_slice(b"\r\n\r\n");
    response.extend_from_slice(body);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::transmute;

    const REQ_INFO_BYTES: [u8; 12] = [
        0x00, 0x00, 0x00, 0x00, 0xf5, 0x76, 0xb1, 0x68, 0x01, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn test_serialize_http() {
        const BODY: &[u8] = br#"{"version":"3.5.1","tileset_last_modified":1756439278,"available_actions":["status","centroid","expansion","transit_available","trace_attributes","trace_route","isochrone","optimized_route","sources_to_targets","height","route","locate"]}"#;
        let req_info: HttpRequestInfo = transmute!(REQ_INFO_BYTES);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE));
        // A stale length must not survive serialization.
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("1"));

        let result = serialize_http(&req_info, StatusCode::OK, headers, BODY);

        let result_utf8 = String::from_utf8(result).expect("Expected a UTF-8 result.");

        if !cfg!(miri) {
            insta::assert_snapshot!(result_utf8);
        }
    }

    #[test]
    fn connection_headers_follow_request() {
        let mut keep_alive = REQ_INFO_BYTES;
        // HTTP/1.0 with Connection: Keep-Alive.
        keep_alive[8] = 0b1000;
        let info: HttpRequestInfo = transmute!(keep_alive);
        let response = serialize_http(&info, StatusCode::OK, HeaderMap::new(), b"");
        assert_eq!(
            String::from_utf8(response).expect("UTF-8"),
            "HTTP/1.0 200 OK\r\naccess-control-allow-origin: *\r\nconnection: Keep-Alive\r\ncontent-length: 0\r\n\r\n"
        );

        let mut close = REQ_INFO_BYTES;
        // HTTP/1.1 with Connection: Close.
        close[8] = 0b1_0001;
        let info: HttpRequestInfo = transmute!(close);
        let response = serialize_http(&info, StatusCode::BAD_REQUEST, HeaderMap::new(), b"{}");
        assert_eq!(
            String::from_utf8(response).expect("UTF-8"),
            "HTTP/1.1 400 Bad Request\r\naccess-control-allow-origin: *\r\nconnection: Close\r\ncontent-length: 2\r\n\r\n{}"
        );
    }

    #[test]
    fn error_response_uses_mainline_status_and_body() {
        let mut request = Api::default();
        let error = ValhallaError::new(valhalla_response::ErrorCode::NoSuitableEdges);
        let WorkerResult::HttpResponse {
            status_code,
            headers,
            body,
        } = WorkerResult::error(&error, &mut request)
        else {
            panic!("expected an HTTP response");
        };
        let info: HttpRequestInfo = transmute!(REQ_INFO_BYTES);
        let response = serialize_http(&info, status_code, headers, &body);
        assert_eq!(
            String::from_utf8(response).expect("UTF-8"),
            "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json;charset=utf-8\r\naccess-control-allow-origin: *\r\ncontent-length: 101\r\n\r\n{\"error_code\":171,\"error\":\"No suitable edges near location\",\"status_code\":400,\"status\":\"Bad Request\"}"
        );
    }
}
