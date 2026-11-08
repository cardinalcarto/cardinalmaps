//! The Loki worker's entry point: from the server's raw HTTP request to Loki's result.
//!
//! Mainline's HTTP server hands Loki's worker each request as it arrived
//! (`prime_server`'s `http_request_t`, serialized),
//! and the worker parses it into `options` (`ParseApi`, the front) before running the action.
//! So front and Loki share a worker, and the worker's input is bytes rather than an `Api`.
//! [`handle_http_request`] does both steps, and says what the worker should do next:
//! forward the request to Thor, answer it (`/locate`), or answer with an error.
//!
//! The function is synchronous and has no transport: the service binary
//! (`valinor-loki-service`) wraps it as a ZeroMQ worker, and an offline router can call it
//! directly.

use crate::Loki;
use http::Method;
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::Api;
use valhalla_proto::options::Format;
use valhalla_response::{ErrorCode, ValhallaError};
use valinor_tyr::front::{HttpRequest, parse_http_request};
use valinor_tyr::tyr::{GPX_CONTENT_TYPE, JSON_CONTENT_TYPE, PBF_CONTENT_TYPE};

/// A raw HTTP/1.x request, split into the parts the front reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawHttpRequest<'a> {
    /// The method.
    pub method: Method,
    /// The request target, for example `/route?json=...`.
    pub target: &'a [u8],
    /// The `Content-Type` header's value, if there is one.
    pub content_type: Option<&'a [u8]>,
    /// The body.
    pub body: &'a [u8],
}

/// Why a request couldn't be read as HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HttpParseError {
    /// The request line or headers don't end (no blank line).
    #[error("read the request: the headers do not end")]
    Unterminated,
    /// The request line isn't `METHOD TARGET VERSION`.
    #[error("read the request line")]
    RequestLine,
    /// The method isn't a valid token.
    #[error("read the request method")]
    Method,
}

impl<'a> RawHttpRequest<'a> {
    /// Splits a request into its request line, headers, and body.
    ///
    /// The body is the `Content-Length` bytes after the headers when that header is present
    /// (and no longer than what is there), and everything after the headers otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the headers don't end or the request line is malformed.
    pub fn parse(raw: &'a [u8]) -> Result<Self, HttpParseError> {
        let end = find(raw, b"\r\n\r\n").ok_or(HttpParseError::Unterminated)?;
        let head = &raw[..end];
        let rest = &raw[end + 4..];
        let mut lines = head
            .split(|&b| b == b'\n')
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l));
        let request_line = lines.next().ok_or(HttpParseError::RequestLine)?;
        let mut parts = request_line.split(|&b| b == b' ').filter(|p| !p.is_empty());
        let (Some(method), Some(target), Some(_version)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(HttpParseError::RequestLine);
        };
        let method = Method::from_bytes(method).map_err(|_| HttpParseError::Method)?;
        let mut content_type = None;
        let mut content_length = None;
        for line in lines {
            let Some(colon) = line.iter().position(|&b| b == b':') else {
                continue;
            };
            let (name, value) = (&line[..colon], trim(&line[colon + 1..]));
            if name.eq_ignore_ascii_case(b"content-type") {
                content_type = Some(value);
            } else if name.eq_ignore_ascii_case(b"content-length") {
                content_length = core::str::from_utf8(value)
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok());
            }
        }
        let body = match content_length {
            Some(length) if length <= rest.len() => &rest[..length],
            _ => rest,
        };
        Ok(Self {
            method,
            target,
            content_type,
            body,
        })
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn trim(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |e| e + 1);
    &value[start..end.max(start)]
}

/// What the Loki worker does with a request.
#[derive(Debug)]
pub enum HttpOutcome {
    /// Send the correlated request on to Thor.
    Forward(Box<Api>),
    /// Answer the client (`/locate`) with this body, with status 200.
    Respond {
        /// The `Content-Type` for the request's format (mainline's `fmt_to_mime`).
        content_type: &'static str,
        /// Whether to send the body as an attachment (GPX).
        attachment: bool,
        /// The body.
        body: Vec<u8>,
        /// The request, for the response's bookkeeping.
        api: Box<Api>,
    },
    /// Answer the client with an error, serialized from the request as it was when it failed.
    Error {
        /// The error.
        error: ValhallaError,
        /// The request, whose format picks the error body.
        api: Box<Api>,
    },
}

/// Parses the server's raw HTTP request with the front and runs Loki on it
/// (what mainline's `loki_worker_t::work` does).
///
/// A request that isn't HTTP at all is error 199, as mainline reports the parser's exception.
pub fn handle_http_request<P: GraphTileProvider>(loki: &Loki<P>, raw: &[u8]) -> HttpOutcome {
    let mut api = Api::default();
    let request = match RawHttpRequest::parse(raw) {
        Ok(request) => request,
        Err(e) => {
            return HttpOutcome::Error {
                error: ValhallaError::with_detail(ErrorCode::LokiUnknown, e.to_string()),
                api: Box::new(api),
            };
        }
    };
    let front = HttpRequest {
        method: &request.method,
        target: request.target,
        content_type: request.content_type,
        body: request.body,
    };
    if let Err(error) = parse_http_request(&front, &mut api) {
        return HttpOutcome::Error {
            error,
            api: Box::new(api),
        };
    }
    match loki.handle(&mut api) {
        Ok(crate::LokiOutput::Forward) => HttpOutcome::Forward(Box::new(api)),
        Ok(crate::LokiOutput::Respond(body)) => {
            let format = api.options.as_ref().map_or(0, |o| o.format);
            let (content_type, attachment) = match Format::try_from(format) {
                Ok(Format::Pbf) => (PBF_CONTENT_TYPE, false),
                Ok(Format::Gpx) => (GPX_CONTENT_TYPE, true),
                _ => (JSON_CONTENT_TYPE, false),
            };
            HttpOutcome::Respond {
                content_type,
                attachment,
                body: body.into_bytes(),
                api: Box::new(api),
            }
        }
        Err(error) => HttpOutcome::Error {
            error,
            api: Box::new(api),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        let raw = b"POST /route?json=1 HTTP/1.1\r\nHost: x\r\ncontent-type:  application/json \r\nContent-Length: 2\r\n\r\n{}junk";
        let request = RawHttpRequest::parse(raw).unwrap();
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.target, b"/route?json=1");
        assert_eq!(request.content_type, Some(&b"application/json"[..]));
        assert_eq!(request.body, b"{}");

        let raw = b"GET /status HTTP/1.1\r\n\r\n";
        let request = RawHttpRequest::parse(raw).unwrap();
        assert_eq!(request.method, Method::GET);
        assert!(request.body.is_empty() && request.content_type.is_none());

        assert_eq!(
            RawHttpRequest::parse(b"GET /status HTTP/1.1\r\n"),
            Err(HttpParseError::Unterminated)
        );
        assert_eq!(
            RawHttpRequest::parse(b"GET\r\n\r\n"),
            Err(HttpParseError::RequestLine)
        );
        assert_eq!(
            RawHttpRequest::parse(b"G(T / HTTP/1.1\r\n\r\n"),
            Err(HttpParseError::Method)
        );
    }
}
