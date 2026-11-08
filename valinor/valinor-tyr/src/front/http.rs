//! HTTP requests as Loki's worker receives them, and mainline's `ParseApi` for them.
//!
//! Mainline's HTTP server (`prime_server`) hands Loki's worker the raw request.
//! `ParseApi` (in `src/worker.cc`) then:
//!
//! 1. rejects every method but `GET` and `POST` (error 101);
//! 2. reads the action from the path (`/route` and so on);
//! 3. for a `Content-Type: application/x-protobuf` body, decodes an `Api` (error 103 on failure)
//!    and validates its options as if the JSON were empty;
//! 4. otherwise parses the `json` query parameter, or failing that the body,
//!    as JSON (error 100 on failure; no JSON at all is an empty object);
//! 5. adds every other query parameter to the document as a string member
//!    (or an array of strings when it repeats), and parses the options from it.
//!
//! # How `prime_server` splits the target
//!
//! The whole request target is URL-decoded (`%XX` only; `+` stays a plus) *before*
//! it is split into the path and the query.
//! So an encoded `&`, `=`, or `?` inside a value (such as `?json=...`) splits it anyway,
//! and a decoded NUL ends the target.
//! Without a `?`, the whole path is also read as a query string.
//! We reproduce all of this, since it decides what mainline sees.
//!
//! # Deviations
//!
//! - `prime_server` looks up the `Content-Type` header case-sensitively
//!   (and exactly as `Content-Type`).
//!   The caller passes us the header value however it found it,
//!   so we treat any spelling of the header name alike.
//! - Decoded bytes that are not UTF-8 are replaced with U+FFFD
//!   (mainline keeps them as raw bytes).

use crate::front::json::JsonValue;
use crate::front::options::from_json;
use alloc::string::String;
use alloc::vec::Vec;
use http::Method;
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;
use valhalla_response::{ErrorCode, ValhallaError};

/// The `Content-Type` that marks a PBF request body.
pub const PBF_CONTENT_TYPE: &str = "application/x-protobuf";

/// An HTTP request, as much of it as the front reads.
#[derive(Debug, Clone, Copy)]
pub struct HttpRequest<'a> {
    /// The request method.
    pub method: &'a Method,
    /// The request target as sent, for example `/route?json=%7B%7D`.
    pub target: &'a [u8],
    /// The `Content-Type` header's value, if there is one.
    pub content_type: Option<&'a [u8]>,
    /// The request body.
    pub body: &'a [u8],
}

/// Parses an HTTP request into `api.options`, as mainline's `ParseApi` does for services.
///
/// The request starts over: anything already in `api` is replaced.
/// On failure, `api` holds whatever was parsed before the error
/// (in particular `options.format`, which picks the error body),
/// as mainline's does when it serializes the error.
///
/// # Errors
///
/// Returns mainline's error for the request:
/// 101 for a method other than `GET` or `POST`, 103 for an undecodable PBF body,
/// 100 for invalid JSON, and otherwise whatever [`from_json`] raises.
pub fn parse_http_request(request: &HttpRequest<'_>, api: &mut Api) -> Result<(), ValhallaError> {
    if request.method != Method::GET && request.method != Method::POST {
        return Err(ErrorCode::WrongHttpMethod.into());
    }

    *api = Api::default();
    // Mainline marks this as a service request here,
    // but the parsing below starts over and clears `info` (including this flag) again,
    // so it never survives. We keep that behavior.
    api.info.get_or_insert_default().is_service = true;

    let target = url_decode(request.target);
    let (path, query) = split_target(&target);
    let action = path.strip_prefix('/').and_then(parse_action);

    if request.content_type == Some(PBF_CONTENT_TYPE.as_bytes()) {
        *api =
            Api::decode(request.body).map_err(|_| ValhallaError::new(ErrorCode::PbfParseFailed))?;
        let mut empty = JsonValue::empty_object();
        return from_json(&mut empty, action, api);
    }

    let json = query
        .iter()
        .find(|(key, _)| key == "json")
        .and_then(|(_, values)| values.first())
        .filter(|value| !value.is_empty());
    let mut document = match json {
        Some(json) => JsonValue::parse(json.as_bytes()),
        None if !request.body.is_empty() => JsonValue::parse(request.body),
        None => Ok(JsonValue::empty_object()),
    }
    .map_err(|_| ValhallaError::new(ErrorCode::JsonParseFailed))?;

    for (key, values) in query {
        if key == "json" || key.is_empty() || values.first().is_none_or(String::is_empty) {
            continue;
        }
        if !document.is_object() {
            // rapidjson's `AddMember` asserts that the document is an object,
            // and Loki's worker reports the failed assertion as an unknown error.
            return Err(ValhallaError::with_detail(
                ErrorCode::LokiUnknown,
                "IsObject()",
            ));
        }
        let value = if values.len() == 1 {
            JsonValue::String(values.into_iter().next().unwrap_or_default())
        } else {
            JsonValue::Array(values.into_iter().map(JsonValue::String).collect())
        };
        document.add_member(key, value);
    }

    from_json(&mut document, action, api)
}

/// Mainline's `Options_Action_Enum_Parse`.
pub fn parse_action(name: &str) -> Option<Action> {
    Some(match name {
        "route" => Action::Route,
        "locate" => Action::Locate,
        "sources_to_targets" => Action::SourcesToTargets,
        "optimized_route" => Action::OptimizedRoute,
        "isochrone" => Action::Isochrone,
        "trace_route" => Action::TraceRoute,
        "trace_attributes" => Action::TraceAttributes,
        "height" => Action::Height,
        "transit_available" => Action::TransitAvailable,
        "expansion" => Action::Expansion,
        "centroid" => Action::Centroid,
        "status" => Action::Status,
        "tile" => Action::Tile,
        _ => return None,
    })
}

/// `curl_unescape`: decodes `%XX` (with two hex digits), leaving everything else alone.
///
/// Like the `std::string` that `prime_server` builds from curl's C string,
/// the result ends at the first decoded NUL.
fn url_decode(encoded: &[u8]) -> String {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        let byte = encoded[i];
        let escaped = (byte == b'%')
            .then(|| encoded.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| core::str::from_utf8(hex).ok())
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        let byte = if let Some(escaped) = escaped {
            i += 3;
            escaped
        } else {
            i += 1;
            byte
        };
        if byte == 0 {
            break;
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded)
        .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned())
}

/// A query: each key with its values, in order of first appearance.
///
/// `prime_server` uses an unordered map; the order only matters for which duplicate
/// a lookup finds, and these keys are unique.
type Query = Vec<(String, Vec<String>)>;

fn query_entry<'q>(query: &'q mut Query, key: &str) -> &'q mut Vec<String> {
    let position = query.iter().position(|(k, _)| k == key).unwrap_or_else(|| {
        query.push((key.into(), Vec::new()));
        query.len() - 1
    });
    &mut query[position].1
}

/// `prime_server`'s `split_path_query`, applied to the decoded target.
fn split_target(target: &str) -> (&str, Query) {
    let query_begin = target.find('?');
    // Without a `?`, `prime_server`'s index arithmetic wraps around to the start of the path.
    let query_string = query_begin.map_or(target, |begin| &target[begin + 1..]);
    let path = query_begin.map_or(target, |begin| &target[..begin]);

    let mut query = Query::new();
    let bytes = query_string.as_bytes();
    let mut key_start = 0;
    let mut value_start = 0;
    let mut key: Option<String> = None;
    for (i, &c) in bytes.iter().enumerate() {
        match c {
            b'&' => {
                match key.take() {
                    Some(key) => {
                        query_entry(&mut query, &key).push(query_string[value_start..i].into());
                    }
                    None => {
                        query_entry(&mut query, &query_string[key_start..i]).push(String::new());
                    }
                }
                key_start = i + 1;
            }
            b'=' if key.is_none() => {
                let name = &query_string[key_start..i];
                query_entry(&mut query, name);
                key = Some(name.into());
                value_start = i + 1;
            }
            _ => {}
        }
    }
    if key_start < bytes.len() {
        match key {
            Some(key) => query_entry(&mut query, &key).push(query_string[value_start..].into()),
            None => query_entry(&mut query, &query_string[key_start..]).push(String::new()),
        }
    }
    (path, query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(target: &str) -> (String, Query) {
        let (path, query) = split_target(target);
        (path.into(), query)
    }

    fn entry(key: &str, values: &[&str]) -> (String, Vec<String>) {
        (key.into(), values.iter().map(|&v| v.into()).collect())
    }

    #[test]
    fn splits_like_prime_server() {
        assert_eq!(
            query("/route?a=1&b&a=2&c=x=y&"),
            (
                "/route".into(),
                vec![
                    entry("a", &["1", "2"]),
                    entry("b", &[""]),
                    entry("c", &["x=y"])
                ]
            )
        );
        // Without a `?`, the path is read as a query too.
        assert_eq!(
            query("/route"),
            ("/route".into(), vec![entry("/route", &[""])])
        );
        assert_eq!(query("/route?"), ("/route".into(), vec![]));
        assert_eq!(query("/r?&&"), ("/r".into(), vec![entry("", &["", ""])]));
    }

    #[test]
    fn decodes_like_curl() {
        assert_eq!(url_decode(b"%7B%22a%22:1%7d+%zz%4"), "{\"a\":1}+%zz%4");
        assert_eq!(url_decode(b"a%00b"), "a");
    }

    #[test]
    fn action_names() {
        assert_eq!(parse_action("route"), Some(Action::Route));
        assert_eq!(parse_action("no_action"), None);
        assert_eq!(parse_action("Route"), None);
    }
}
