//! Response serialization, as mainline's `tyr` serializers do it.
//!
//! [`serialize_directions`] is mainline's `tyr::serializeDirections`:
//! it turns a request that has been through Odin
//! (an `Api` with `options`, `trip`, and `directions`)
//! into the response body for the requested format.
//! Errors are serialized by [`ValhallaError::serialize`] instead.

// TODO(N5): drop these allows once the serializers that use the helpers land.
#[allow(dead_code, reason = "used by the serializers that follow")]
mod numbers;
#[allow(dead_code, reason = "used by the serializers that follow")]
mod polyline;
#[allow(dead_code, reason = "used by the serializers that follow")]
mod trip;
#[allow(dead_code, reason = "used by the serializers that follow")]
mod writer;

use valhalla_proto::Api;
use valhalla_proto::options::Format;
use valhalla_response::{ErrorCode, ValhallaError};

/// The `Content-Type` of JSON responses (mainline's `JSON_MIME`).
pub const JSON_CONTENT_TYPE: &str = "application/json;charset=utf-8";
/// The `Content-Type` of PBF responses (mainline's `PBF_MIME`).
pub const PBF_CONTENT_TYPE: &str = "application/x-protobuf";
/// The `Content-Type` of GPX responses (mainline's `GPX_MIME`).
pub const GPX_CONTENT_TYPE: &str = "application/gpx+xml;charset=utf-8";

/// A serialized response body and its `Content-Type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The `Content-Type` to send.
    pub content_type: &'static str,
    /// The body.
    pub body: Vec<u8>,
}

impl Response {
    #[allow(dead_code, reason = "used by the serializers that follow")]
    fn json(body: String) -> Self {
        Self {
            content_type: JSON_CONTENT_TYPE,
            body: body.into_bytes(),
        }
    }
}

/// Serializes a route-like response in the requested format,
/// as mainline's `tyr::serializeDirections` does.
///
/// The request may be modified, as mainline's serializers modify it
/// (for example, the PBF serializer clears the fields it does not send).
///
/// # Errors
///
/// Fails with [`ErrorCode::OdinUnknown`] (299), as mainline's Odin worker does,
/// when the request is inconsistent (for example, when its directions have no matching trip),
/// or when the format has no route serializer.
/// (Mainline terminates on formats without a route serializer;
/// the front never lets those through for route-like actions.)
pub fn serialize_directions(api: &mut Api) -> Result<Response, ValhallaError> {
    let format = api.options.as_ref().map_or(0, |options| options.format);
    match Format::try_from(format) {
        Ok(Format::Json | Format::Osrm | Format::Gpx | Format::Pbf) => {
            Err(unknown("this format's serializer is not implemented yet"))
        }
        Ok(Format::Geotiff | Format::Mvt) | Err(_) => {
            Err(unknown("the format has no route serializer"))
        }
    }
}

/// The error mainline's Odin worker reports for an exception during serialization.
fn unknown(detail: impl Into<String>) -> ValhallaError {
    ValhallaError::with_detail(ErrorCode::OdinUnknown, detail)
}
