//! Valhalla-compatible errors and their serialization.
//!
//! [`ValhallaError`] is the error half of the stage contract in `docs/routing.md`:
//! every stage returns one when a request fails,
//! and the stage that fails serializes it for the client,
//! the way mainline's workers call `serialize_error` (`src/worker.cc`).

use crate::error_code::{ErrorCode, OsrmError};
use alloc::borrow::Cow;
use http::StatusCode;
use serde::Serialize;
use thiserror::Error;
use valhalla_proto::prost::Message;
use valhalla_proto::{Api, CodedDescription, Info, Statistic, StatisticType, options};

/// The `Content-Type` mainline sends with every error response.
///
/// This includes PBF-format errors, whose body is a serialized [`Api`].
/// That is a mainline quirk (`serialize_error` always uses `JSON_MIME`),
/// which we keep so clients see the same headers.
pub const ERROR_CONTENT_TYPE: &str = "application/json;charset=utf-8";

/// An error with one of mainline's codes, plus any request-specific detail.
///
/// This mirrors mainline's `valhalla_exception_t`
/// constructed from a code and an optional `extra` string.
/// The code determines everything else (HTTP status, OSRM error, and so on),
/// so that is all we store.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("Valhalla error {code}: {}", self.message())]
pub struct ValhallaError {
    code: ErrorCode,
    detail: Option<Box<str>>,
}

impl ValhallaError {
    /// An error with the code's base message.
    #[inline]
    pub const fn new(code: ErrorCode) -> Self {
        Self { code, detail: None }
    }

    /// An error with request-specific detail appended to the code's base message.
    ///
    /// The message becomes `"<base message>: <detail>"`,
    /// for example `No costing method found: 'spaceship'` for
    /// `ValhallaError::with_detail(ErrorCode::UnknownCosting, "'spaceship'")`.
    /// As in mainline, an empty detail is the same as none.
    pub fn with_detail(code: ErrorCode, detail: impl Into<String>) -> Self {
        let detail: String = detail.into();
        Self {
            code,
            detail: (!detail.is_empty()).then(|| detail.into_boxed_str()),
        }
    }

    /// Mainline's error code.
    #[inline]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// The request-specific detail, if any.
    #[inline]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// The full message, as in the `error` field of a Valhalla JSON error.
    pub fn message(&self) -> Cow<'static, str> {
        match &self.detail {
            None => Cow::Borrowed(self.code.message()),
            Some(detail) => Cow::Owned(format!("{}: {detail}", self.code.message())),
        }
    }

    /// The HTTP status mainline responds with.
    #[inline]
    pub const fn http_status(&self) -> StatusCode {
        self.code.http_status().status_code()
    }

    /// The error mainline returns instead for OSRM-format requests.
    #[inline]
    pub const fn osrm_error(&self) -> OsrmError {
        self.code.osrm_error()
    }

    /// The body of a Valhalla JSON error response.
    pub fn valhalla_json(&self) -> ValhallaJsonError {
        ValhallaJsonError {
            error_code: self.code.code(),
            error: self.message(),
            status_code: self.code.http_status().status_code().as_u16(),
            status: self.code.http_status().reason(),
        }
    }

    /// The body of an OSRM-format error response.
    #[inline]
    pub const fn osrm_json(&self) -> OsrmJsonError {
        OsrmJsonError {
            code: self.code.osrm_error().code(),
            message: self.code.osrm_error().message(),
        }
    }

    /// Serializes this error for the client, as mainline's `serialize_error` does.
    ///
    /// Like mainline, this first records the error in the request:
    /// it appends a [`CodedDescription`] to `info.errors`
    /// and an error counter to `info.statistics`
    /// (keyed `<action>.<warn|error>.<loki|thor|odin>.<statsd key>`).
    /// The body then depends on the requested format:
    ///
    /// - `osrm`: the fixed OSRM error (see [`ValhallaError::osrm_json`]).
    /// - `pbf`: an [`Api`] holding only the request's `info`, including the error just recorded.
    /// - Anything else, including formats like `gpx` and values we do not recognize:
    ///   a Valhalla JSON error (see [`ValhallaError::valhalla_json`]).
    ///
    /// Send the body with [`ValhallaError::http_status`] and [`ERROR_CONTENT_TYPE`].
    pub fn serialize(&self, request: &mut Api) -> Vec<u8> {
        let (action, format) = request
            .options
            .as_ref()
            .map_or((0, 0), |options| (options.action, options.format));
        // Mainline uses the protobuf enum name, which is empty for values it does not know.
        let action = options::Action::try_from(action).map_or("", |action| action.as_str_name());
        let level = if self.http_status().is_server_error() {
            "error"
        } else {
            "warn"
        };

        let info = request.info.get_or_insert_with(Info::default);
        info.errors.push(CodedDescription {
            description: self.message().into_owned(),
            code: u64::from(self.code.code()),
        });
        info.statistics.push(Statistic {
            key: format!(
                "{action}.{level}.{}.{}",
                self.code.statsd_worker(),
                self.code.statsd_key()
            ),
            value: 1.0,
            frequency: 0.0,
            r#type: StatisticType::Count.into(),
        });

        match options::Format::try_from(format) {
            Ok(options::Format::Osrm) => to_json(&self.osrm_json()),
            Ok(options::Format::Pbf) => Api {
                info: request.info.clone(),
                ..Api::default()
            }
            .encode_to_vec(),
            _ => to_json(&self.valhalla_json()),
        }
    }
}

impl From<ErrorCode> for ValhallaError {
    #[inline]
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}

/// The body of a Valhalla JSON error response.
///
/// Fields serialize in the order mainline 3.9.1 writes them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValhallaJsonError {
    /// Mainline's numeric error code.
    pub error_code: u16,
    /// The full error message.
    pub error: Cow<'static, str>,
    /// The HTTP status code.
    pub status_code: u16,
    /// The HTTP reason phrase.
    pub status: &'static str,
}

/// The body of an OSRM-format error response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OsrmJsonError {
    /// The OSRM error code, for example `NoSegment`.
    pub code: &'static str,
    /// The OSRM error message.
    pub message: &'static str,
}

fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    // Both bodies are structs of strings and integers,
    // for which serde_json serialization cannot fail.
    serde_json::to_vec(value).expect("error bodies always serialize")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use std::path::Path;

    const RESPONSES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/error-responses");

    /// Reads a captured mainline response: `{"status": <HTTP status>, "body": <parsed body>}`.
    fn captured(name: &str) -> (u16, Value) {
        let text = fs::read_to_string(Path::new(RESPONSES_DIR).join(name)).expect("fixture");
        let mut response: Value = serde_json::from_str(&text).expect("fixture is JSON");
        let status = response["status"]
            .as_u64()
            .and_then(|s| u16::try_from(s).ok())
            .expect("numeric status");
        (status, response["body"].take())
    }

    fn request(format: options::Format) -> Api {
        Api {
            options: Some(valhalla_proto::Options {
                action: options::Action::Route.into(),
                format: format.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Serializes as mainline would for a route request in the given format.
    fn serialize(error: &ValhallaError, format: options::Format) -> (u16, Vec<u8>) {
        (
            error.http_status().as_u16(),
            error.serialize(&mut request(format)),
        )
    }

    /// The captured Andorra error cases, with the error each one raised in mainline.
    fn cases() -> [(&'static str, ValhallaError); 3] {
        [
            ("error_no_data", ErrorCode::NoSuitableEdges.into()),
            ("error_one_location", ErrorCode::NotEnoughLocations.into()),
            (
                "error_unknown_costing",
                ValhallaError::with_detail(ErrorCode::UnknownCosting, "'spaceship'"),
            ),
        ]
    }

    #[test]
    fn valhalla_json_matches_mainline() {
        for (case, error) in cases() {
            let (status, body) = serialize(&error, options::Format::Json);
            let body: Value = serde_json::from_slice(&body).expect("valid JSON");
            assert_eq!(
                (status, body),
                captured(&format!("{case}-json.json")),
                "{case}"
            );
        }
    }

    #[test]
    fn osrm_json_matches_mainline() {
        for (case, error) in cases() {
            let (status, body) = serialize(&error, options::Format::Osrm);
            let body: Value = serde_json::from_slice(&body).expect("valid JSON");
            assert_eq!(
                (status, body),
                captured(&format!("{case}-osrm.json")),
                "{case}"
            );
        }
    }

    #[test]
    fn json_key_order_matches_mainline() {
        let (_, body) = serialize(&ErrorCode::NoSuitableEdges.into(), options::Format::Json);
        assert_eq!(
            String::from_utf8(body).expect("UTF-8"),
            r#"{"error_code":171,"error":"No suitable edges near location","status_code":400,"status":"Bad Request"}"#
        );
    }

    #[test]
    fn osrm_body_is_mainline_constant() {
        // OSRM_NO_SEGMENT in exceptions.cc, byte for byte.
        let (_, body) = serialize(&ErrorCode::NoSuitableEdges.into(), options::Format::Osrm);
        assert_eq!(
            String::from_utf8(body).expect("UTF-8"),
            r#"{"code":"NoSegment","message":"One of the supplied input coordinates could not snap to street segment."}"#
        );
    }

    #[test]
    fn other_formats_get_valhalla_json() {
        for format in [
            options::Format::Gpx,
            options::Format::Geotiff,
            options::Format::Mvt,
        ] {
            let (_, body) = serialize(&ErrorCode::NoSuitableEdges.into(), format);
            let body: Value = serde_json::from_slice(&body).expect("valid JSON");
            assert_eq!(body["error_code"], 171, "{format:?}");
        }

        let mut unknown_format = request(options::Format::Json);
        if let Some(options) = unknown_format.options.as_mut() {
            options.format = 99;
        }
        let body = ValhallaError::new(ErrorCode::NoSuitableEdges).serialize(&mut unknown_format);
        let body: Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(body["error_code"], 171);
    }

    #[test]
    fn pbf_body_carries_only_info() {
        let mut request = request(options::Format::Pbf);
        request.info = Some(Info {
            warnings: vec![CodedDescription {
                description: "an earlier warning".to_string(),
                code: 200,
            }],
            is_service: true,
            ..Default::default()
        });
        let error = ValhallaError::with_detail(ErrorCode::UnknownCosting, "'spaceship'");
        let body = error.serialize(&mut request);

        let decoded = Api::decode(body.as_slice()).expect("valid Api");
        assert_eq!(
            decoded,
            Api {
                info: request.info.clone(),
                ..Default::default()
            }
        );
        let info = decoded.info.expect("info");
        assert_eq!(info.warnings.len(), 1);
        assert!(info.is_service);
        assert_eq!(
            info.errors,
            [CodedDescription {
                description: "No costing method found: 'spaceship'".to_string(),
                code: 125,
            }]
        );
        assert_eq!(
            info.statistics,
            [Statistic {
                key: "route.warn.loki.wrong_costing".to_string(),
                value: 1.0,
                frequency: 0.0,
                r#type: StatisticType::Count.into(),
            }]
        );
    }

    #[test]
    fn records_error_for_every_format() {
        let mut request = request(options::Format::Json);
        ValhallaError::new(ErrorCode::OdinUnknown).serialize(&mut request);
        let info = request.info.expect("info");
        assert_eq!(info.errors[0].code, 299);
        // 5xx statuses are counted as errors rather than warnings.
        assert_eq!(info.statistics[0].key, "route.error.odin.unknown");
    }

    #[test]
    fn missing_options_default_to_json_and_no_action() {
        let mut request = Api::default();
        let body = ValhallaError::new(ErrorCode::NotImplemented).serialize(&mut request);
        let body: Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(body["error_code"], 107);
        assert_eq!(body["status_code"], 501);
        assert_eq!(body["status"], "Not Implemented");
        let info = request.info.expect("info");
        assert_eq!(info.statistics[0].key, "no_action.error.loki.empty_action");
    }

    #[test]
    fn empty_detail_is_no_detail() {
        assert_eq!(
            ValhallaError::with_detail(ErrorCode::NoSuitableEdges, ""),
            ValhallaError::new(ErrorCode::NoSuitableEdges)
        );
        assert_eq!(
            ValhallaError::with_detail(ErrorCode::NoSuitableEdges, "").message(),
            "No suitable edges near location"
        );
    }

    #[test]
    fn display_includes_code_and_message() {
        assert_eq!(
            ValhallaError::with_detail(ErrorCode::UnknownCosting, "'spaceship'").to_string(),
            "Valhalla error 125: No costing method found: 'spaceship'"
        );
    }
}
