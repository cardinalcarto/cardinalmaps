//! Mainline Valhalla's error codes.
//!
//! The table mirrors `error_codes` in mainline's `src/exceptions.cc` at 3.9.1, row for row.
//! (The tests check it against the copy in `fixtures/exceptions-3.9.1.cc`,
//! which is copied verbatim from <https://github.com/valhalla/valhalla> under the MIT license.)
//! Each code carries the message mainline reports,
//! the HTTP status it responds with,
//! the OSRM-format error it returns instead for `format=osrm`,
//! and the key mainline uses for its statsd error counter.

use core::fmt;
use http::StatusCode;
use thiserror::Error;

/// The HTTP statuses that mainline's error codes use.
///
/// Mainline writes the reason phrase itself (`HTTP_400` and so on in `exceptions.cc`)
/// rather than deriving it from the code,
/// so we keep both here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpStatus {
    /// 400 Bad Request.
    BadRequest,
    /// 404 Not Found.
    NotFound,
    /// 405 Method Not Allowed.
    MethodNotAllowed,
    /// 500 Internal Server Error.
    InternalServerError,
    /// 501 Not Implemented.
    NotImplemented,
    /// 503 Service Unavailable.
    ServiceUnavailable,
}

impl HttpStatus {
    /// The HTTP status code.
    #[inline]
    pub const fn status_code(self) -> StatusCode {
        match self {
            HttpStatus::BadRequest => StatusCode::BAD_REQUEST,
            HttpStatus::NotFound => StatusCode::NOT_FOUND,
            HttpStatus::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            HttpStatus::InternalServerError => StatusCode::INTERNAL_SERVER_ERROR,
            HttpStatus::NotImplemented => StatusCode::NOT_IMPLEMENTED,
            HttpStatus::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// The reason phrase mainline sends,
    /// both in the HTTP status line and in the `status` field of Valhalla JSON errors.
    #[inline]
    pub const fn reason(self) -> &'static str {
        match self {
            HttpStatus::BadRequest => "Bad Request",
            HttpStatus::NotFound => "Not Found",
            HttpStatus::MethodNotAllowed => "Method Not Allowed",
            HttpStatus::InternalServerError => "Internal Server Error",
            HttpStatus::NotImplemented => "Not Implemented",
            HttpStatus::ServiceUnavailable => "Service Unavailable",
        }
    }
}

/// The OSRM-format errors that mainline returns.
///
/// Each is a fixed `code` and `message` pair (the `OSRM_*` constants in `exceptions.cc`).
/// Two of them share the `InvalidUrl` code:
/// [`OsrmError::SerializationFailed`] is mainline's `OSRM_SERVER_ERROR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OsrmError {
    /// `InvalidUrl`: "URL string is invalid."
    InvalidUrl,
    /// `InvalidService`: "Service name is invalid."
    InvalidService,
    /// `InvalidOptions`: "Options are invalid."
    InvalidOptions,
    /// `InvalidValue`: "The successfully parsed query parameters are invalid."
    InvalidValue,
    /// `NoRoute`: "Impossible route between points"
    NoRoute,
    /// `NoSegment`: "One of the supplied input coordinates could not snap to street segment."
    NoSegment,
    /// `ServiceUnavailable`: "The service is shutting down."
    ShuttingDown,
    /// `InvalidUrl`: "Failed to serialize route."
    SerializationFailed,
    /// `DistanceExceeded`: "Path distance exceeds the max distance limit."
    DistanceExceeded,
    /// `PerimeterExceeded`: "Perimeter of avoid polygons exceeds the max limit."
    PerimeterExceeded,
    /// `BreakageDistanceExceeded`: "All coordinates are too far away from each other"
    BreakageDistanceExceeded,
}

impl OsrmError {
    /// Every OSRM error mainline uses.
    pub const ALL: &'static [OsrmError] = &[
        OsrmError::InvalidUrl,
        OsrmError::InvalidService,
        OsrmError::InvalidOptions,
        OsrmError::InvalidValue,
        OsrmError::NoRoute,
        OsrmError::NoSegment,
        OsrmError::ShuttingDown,
        OsrmError::SerializationFailed,
        OsrmError::DistanceExceeded,
        OsrmError::PerimeterExceeded,
        OsrmError::BreakageDistanceExceeded,
    ];

    /// The OSRM error code (the `code` field), for example `NoSegment`.
    #[inline]
    pub const fn code(self) -> &'static str {
        match self {
            OsrmError::InvalidUrl | OsrmError::SerializationFailed => "InvalidUrl",
            OsrmError::InvalidService => "InvalidService",
            OsrmError::InvalidOptions => "InvalidOptions",
            OsrmError::InvalidValue => "InvalidValue",
            OsrmError::NoRoute => "NoRoute",
            OsrmError::NoSegment => "NoSegment",
            OsrmError::ShuttingDown => "ServiceUnavailable",
            OsrmError::DistanceExceeded => "DistanceExceeded",
            OsrmError::PerimeterExceeded => "PerimeterExceeded",
            OsrmError::BreakageDistanceExceeded => "BreakageDistanceExceeded",
        }
    }

    /// The OSRM error message (the `message` field).
    ///
    /// Mainline never appends request details to these,
    /// unlike the `error` field of Valhalla JSON errors.
    #[inline]
    pub const fn message(self) -> &'static str {
        match self {
            OsrmError::InvalidUrl => "URL string is invalid.",
            OsrmError::InvalidService => "Service name is invalid.",
            OsrmError::InvalidOptions => "Options are invalid.",
            OsrmError::InvalidValue => "The successfully parsed query parameters are invalid.",
            OsrmError::NoRoute => "Impossible route between points",
            OsrmError::NoSegment => {
                "One of the supplied input coordinates could not snap to street segment."
            }
            OsrmError::ShuttingDown => "The service is shutting down.",
            OsrmError::SerializationFailed => "Failed to serialize route.",
            OsrmError::DistanceExceeded => "Path distance exceeds the max distance limit.",
            OsrmError::PerimeterExceeded => "Perimeter of avoid polygons exceeds the max limit.",
            OsrmError::BreakageDistanceExceeded => {
                "All coordinates are too far away from each other"
            }
        }
    }
}

/// A numeric code that is not in mainline's error table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("look up Valhalla error code {0}: it is not in mainline's table")]
pub struct UnknownErrorCode(pub u16);

/// Defines [`ErrorCode`] and its lookups from one table,
/// so that each code's attributes stay on one line, as they do in mainline.
macro_rules! error_codes {
    ($($variant:ident = $code:literal, $message:literal, $http:ident, $osrm:ident, $statsd:literal;)+) => {
        /// One of mainline Valhalla's numeric error codes.
        ///
        /// The variants follow mainline's `error_codes` table in `src/exceptions.cc` (3.9.1).
        /// Several messages appear under more than one code
        /// (the hundreds digit tells which service raised it:
        /// 1xx Loki, 2xx Odin, 3xx Skadi, 4xx Thor, and 5xx Tyr),
        /// so the later duplicates carry a service prefix.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        pub enum ErrorCode {
            $(
                #[doc = concat!("`", stringify!($code), "`: \"", $message, "\"")]
                $variant = $code,
            )+
        }

        impl ErrorCode {
            /// Every code in mainline's table, in ascending order.
            pub const ALL: &'static [ErrorCode] = &[$(ErrorCode::$variant),+];

            /// The base message mainline reports for this code.
            ///
            /// Mainline appends request-specific detail to some of these;
            /// see [`ValhallaError::with_detail`](crate::ValhallaError::with_detail).
            #[inline]
            pub const fn message(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant => $message,)+
                }
            }

            /// The HTTP status mainline responds with.
            #[inline]
            pub const fn http_status(self) -> HttpStatus {
                match self {
                    $(ErrorCode::$variant => HttpStatus::$http,)+
                }
            }

            /// The error mainline returns instead for OSRM-format requests.
            #[inline]
            pub const fn osrm_error(self) -> OsrmError {
                match self {
                    $(ErrorCode::$variant => OsrmError::$osrm,)+
                }
            }

            /// The suffix of mainline's statsd error counter key, for example `no_edges_near`.
            #[inline]
            pub const fn statsd_key(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant => $statsd,)+
                }
            }
        }

        impl TryFrom<u16> for ErrorCode {
            type Error = UnknownErrorCode;

            #[inline]
            fn try_from(code: u16) -> Result<Self, Self::Error> {
                match code {
                    $($code => Ok(ErrorCode::$variant),)+
                    other => Err(UnknownErrorCode(other)),
                }
            }
        }
    };
}

error_codes! {
    JsonParseFailed = 100, "Failed to parse json request", BadRequest, InvalidUrl, "json_parse_failed";
    WrongHttpMethod = 101, "Try a POST or GET request instead", MethodNotAllowed, InvalidUrl, "wrong_http_method";
    ShuttingDown = 102, "The service is shutting down", ServiceUnavailable, ShuttingDown, "shutting_down";
    PbfParseFailed = 103, "Failed to parse pbf request", BadRequest, InvalidUrl, "pbf_parse_failed";
    UnknownAction = 106, "Try any of", NotFound, InvalidService, "wrong_action";
    NotImplemented = 107, "Not Implemented", NotImplemented, InvalidService, "empty_action";
    LocationsRequired = 110, "Insufficiently specified required parameter 'locations'", BadRequest, InvalidOptions, "locations_parse_failed";
    TimeRequired = 111, "Insufficiently specified required parameter 'time'", BadRequest, InvalidOptions, "time_parse_failed";
    MatrixLocationsRequired = 112, "Insufficiently specified required parameter 'locations' or 'sources & targets'", BadRequest, InvalidOptions, "matrix_locations_parse_failed";
    ContoursRequired = 113, "Insufficiently specified required parameter 'contours'", BadRequest, InvalidOptions, "contours_parse_failed";
    ShapeOrPolylineRequired = 114, "Insufficiently specified required parameter 'shape' or 'encoded_polyline'", BadRequest, InvalidOptions, "shape_parse_failed";
    ActionRequired = 115, "Insufficiently specified required parameter 'action'", BadRequest, InvalidOptions, "action_parse_failed";
    NotEnoughLocations = 120, "Insufficient number of locations provided", BadRequest, InvalidOptions, "not_enough_locations";
    NotEnoughSources = 121, "Insufficient number of sources provided", BadRequest, InvalidOptions, "not_enough_sources";
    NotEnoughTargets = 122, "Insufficient number of targets provided", BadRequest, InvalidOptions, "not_enough_targets";
    NotEnoughShape = 123, "Insufficient shape provided", BadRequest, InvalidOptions, "not_enough_shape";
    CostingRequired = 124, "No edge/node costing provided", BadRequest, InvalidOptions, "costing_required";
    UnknownCosting = 125, "No costing method found", BadRequest, InvalidOptions, "wrong_costing";
    ShapeRequired = 126, "No shape provided", BadRequest, InvalidOptions, "shape_required";
    RecostingCostingRequired = 127, "Recostings require a valid costing parameter", BadRequest, InvalidOptions, "recosting_parse_failed";
    DuplicateRecostingNames = 128, "Recostings require a unique 'name' field for each recosting", BadRequest, InvalidOptions, "no_recosting_duplicate_names";
    LocationParseFailed = 130, "Failed to parse location", BadRequest, InvalidValue, "location_parse_failed";
    SourceParseFailed = 131, "Failed to parse source", BadRequest, InvalidValue, "source_parse_failed";
    TargetParseFailed = 132, "Failed to parse target", BadRequest, InvalidValue, "target_parse_failed";
    AvoidParseFailed = 133, "Failed to parse avoid", BadRequest, InvalidValue, "avoid_parse_failed";
    ShapeParseFailed = 134, "Failed to parse shape", BadRequest, InvalidValue, "shape_parse_failed";
    TraceParseFailed = 135, "Failed to parse trace", BadRequest, InvalidValue, "trace_parse_failed";
    TraceDurationMismatch = 136, "durations size not compatible with trace size", BadRequest, InvalidValue, "trace_duration_mismatch";
    PolygonParseFailed = 137, "Failed to parse polygon", BadRequest, InvalidValue, "polygon_parse_failed";
    MultimodalNotSupported = 140, "Action does not support multimodal costing", BadRequest, InvalidValue, "no_multimodal";
    MultimodalArriveByNotImplemented = 141, "Arrive by for multimodal not implemented yet", NotImplemented, InvalidValue, "no_arrive_by_multimodal";
    IsochroneArriveByNotImplemented = 142, "Arrive by not implemented for isochrones", NotImplemented, InvalidValue, "no_arrive_by_isochrones";
    ClosuresConflict = 143, "ignore_closures in costing and exclude_closures in search_filter cannot both be specified", BadRequest, InvalidValue, "closures_conflict";
    ExpansionNotSupported = 144, "Action does not support expansion", BadRequest, InvalidValue, "no_action_for_expansion";
    TooManyLocations = 150, "Exceeded max locations", BadRequest, InvalidValue, "too_many_locations";
    TimeTooLarge = 151, "Exceeded max time", BadRequest, InvalidValue, "too_large_time";
    TooManyContours = 152, "Exceeded max contours", BadRequest, InvalidValue, "too_many_contours";
    TooManyShapePoints = 153, "Too many shape points", BadRequest, InvalidValue, "too_large_shape";
    PathDistanceExceeded = 154, "Path distance exceeds the max distance limit", BadRequest, DistanceExceeded, "too_large_distance";
    FirstLastWalkingDistanceExceeded = 155, "Outside the valid walking distance at the beginning or end of a multimodal route", BadRequest, InvalidUrl, "too_large_first_last_walking_distance";
    TransferWalkingDistanceExceeded = 156, "Outside the valid walking distance between stops of a multimodal route", BadRequest, InvalidUrl, "too_large_in_between_walking_distance";
    TooManyAvoidLocations = 157, "Exceeded max avoid locations", BadRequest, InvalidValue, "too_many_avoids";
    TraceOptionOutOfBounds = 158, "Input trace option is out of bounds", BadRequest, InvalidValue, "trace_option_invalid";
    MissingTimestamps = 159, "use_timestamps set with no timestamps present", BadRequest, InvalidValue, "missing_timestamps";
    MissingDepartAtDate = 160, "Date and time required for origin for date_type of depart at", BadRequest, InvalidOptions, "missing_depart_date";
    MissingArriveByDate = 161, "Date and time required for destination for date_type of arrive by", BadRequest, InvalidOptions, "missing_arrive_date";
    DateParseFailed = 162, "Date and time is invalid.  Format is YYYY-MM-DDTHH:MM", BadRequest, InvalidValue, "date_parse_failed";
    InvalidDateType = 163, "Invalid date_type", BadRequest, InvalidValue, "wrong_date_type";
    InvalidShapeFormat = 164, "Invalid shape format", BadRequest, InvalidValue, "wrong_shape_format";
    MissingInvariantDate = 165, "Date and time required for destination for date_type of invariant", BadRequest, InvalidOptions, "missing_invariant_date";
    MaxDistanceExceeded = 166, "Exceeded max distance", BadRequest, InvalidValue, "too_large_distance";
    ExcludePolygonsPerimeterExceeded = 167, "Exceeded maximum circumference for exclude_polygons", BadRequest, PerimeterExceeded, "too_large_polygon";
    InvalidExpansionProperty = 168, "Invalid expansion property type", BadRequest, InvalidOptions, "invalid_expansion_property";
    UnconnectedRegions = 170, "Locations are in unconnected regions. Go check/edit the map at osm.org", BadRequest, NoRoute, "impossible_route";
    NoSuitableEdges = 171, "No suitable edges near location", BadRequest, NoSegment, "no_edges_near";
    BreakageDistanceExceeded = 172, "Exceeded breakage distance for all pairs", BadRequest, BreakageDistanceExceeded, "too_large_breakage_distance";
    LineFeatureParseFailed = 173, "Failed to parse line feature", BadRequest, InvalidValue, "polygon_parse_failed";
    InvalidTileCoordinates = 174, "Invalid tile coordinates", BadRequest, InvalidValue, "tile_coords_invalid";
    TileZoomExceeded = 175, "Exceeded max zoom level of", BadRequest, InvalidValue, "tile_zoom_invalid";
    ExcludePolygonsVerticesExceeded = 176, "Exceeded maximum number of vertices for exclude_polygons", BadRequest, PerimeterExceeded, "too_large_polygon";
    PolygonCrossesAntimeridian = 177, "Polygon crosses antimeridian", BadRequest, PerimeterExceeded, "polygon_parse_failed";
    LokiUnknown = 199, "Unknown", InternalServerError, InvalidUrl, "unknown";
    OdinIntermediateParseFailed = 200, "Failed to parse intermediate request format", InternalServerError, InvalidUrl, "pbf_parse_failed";
    TripLegParseFailed = 201, "Failed to parse TripLeg", InternalServerError, InvalidUrl, "trip_parse_failed";
    DirectionsBuildingFailed = 202, "Could not build directions for TripLeg", InternalServerError, InvalidUrl, "directions_building_failed";
    OdinShuttingDown = 203, "The service is shutting down", ServiceUnavailable, ShuttingDown, "shutting_down";
    TripPathNoNodes = 210, "Trip path does not have any nodes", BadRequest, InvalidUrl, "no_nodes";
    TripPathOneNode = 211, "Trip path has only one node", BadRequest, InvalidUrl, "one_node";
    TripNotEnoughLocations = 212, "Trip must have at least 2 locations", BadRequest, InvalidOptions, "not_enough_locations";
    NoShapeOrInvalidNodeCount = 213, "Error - No shape or invalid node count", BadRequest, InvalidUrl, "shape_parse_failed";
    TurnDegreeOutOfRange = 220, "Turn degree out of range for cardinal direction", BadRequest, InvalidUrl, "wrong_turn_degree";
    InvalidManeuverTypeForTurn = 230, "Invalid DirectionsLeg_Maneuver_Type in method FormTurnInstruction", BadRequest, InvalidUrl, "wrong_maneuver_form_turn";
    InvalidManeuverTypeForRelativeTwo = 231, "Invalid DirectionsLeg_Maneuver_Type in method FormRelativeTwoDirection", BadRequest, InvalidUrl, "wrong_maneuver_form_relative_two";
    InvalidManeuverTypeForRelativeThree = 232, "Invalid DirectionsLeg_Maneuver_Type in method FormRelativeThreeDirection", BadRequest, InvalidUrl, "wrong_maneuver_form_relative_three";
    LineFeatureEdgeWalkFailed = 233, "Failed to edge walk line feature", BadRequest, InvalidValue, "polygon_parse_failed";
    TooManyCostFactorEdges = 234, "Exceeded maximum allowed cost factor edges", BadRequest, PerimeterExceeded, "too_many_cost_factor_edges";
    OdinUnknown = 299, "Unknown", InternalServerError, InvalidUrl, "unknown";
    HeightShapeOrPolylineRequired = 312, "Insufficiently specified required parameter 'shape' or 'encoded_polyline'", BadRequest, InvalidOptions, "shape_parse_failed";
    ResampleDistanceTooSmall = 313, "'resample_distance' must be >= ", BadRequest, InvalidUrl, "wrong_resample_distance";
    HeightTooManyShapePoints = 314, "Too many shape points", BadRequest, InvalidValue, "too_many_shape_points";
    ThorUnknownAction = 400, "Unknown action", BadRequest, InvalidService, "wrong_action";
    ThorIntermediateParseFailed = 401, "Failed to parse intermediate request format", InternalServerError, SerializationFailed, "options_parse_failed";
    ThorShuttingDown = 402, "The service is shutting down", ServiceUnavailable, ShuttingDown, "shutting_down";
    CorrelatedLocationParseFailed = 420, "Failed to parse correlated location", BadRequest, InvalidValue, "candidate_parse_failed";
    ThorLocationParseFailed = 421, "Failed to parse location", BadRequest, InvalidValue, "location_parse_failed";
    ThorSourceParseFailed = 422, "Failed to parse source", BadRequest, InvalidValue, "source_parse_failed";
    ThorTargetParseFailed = 423, "Failed to parse target", BadRequest, InvalidValue, "target_parse_failed";
    ThorShapeParseFailed = 424, "Failed to parse shape", BadRequest, InvalidValue, "shape_parse_failed";
    CostMatrixIterationsExceeded = 430, "Exceeded max iterations in CostMatrix::SourceToTarget", BadRequest, InvalidUrl, "too_many_iterations_cost_matrix";
    TransitStopTooFar = 440, "Cannot reach destination - too far from a transit stop", BadRequest, InvalidUrl, "transit_unreachable";
    LocationUnreachable = 441, "Location is unreachable", BadRequest, InvalidUrl, "matrix_element_unreachable";
    NoPathFound = 442, "No path could be found for input", BadRequest, NoRoute, "no_path";
    ExactRouteMatchFailed = 443, "Exact route match algorithm failed to find path", BadRequest, NoSegment, "shape_match_failed";
    MapMatchFailed = 444, "Map Match algorithm failed to find path", BadRequest, NoSegment, "map_match_failed";
    InvalidShapeMatch = 445, "Shape match algorithm specification in api request is incorrect. Please see documentation for valid shape_match input.", BadRequest, InvalidUrl, "wrong_match_type";
    RemoteTarChanged = 446, "Remote tar file has changed, service is unavailable", InternalServerError, SerializationFailed, "remote_tar_changed";
    ThorUnknown = 499, "Unknown", InternalServerError, InvalidUrl, "unknown";
    LegCountMismatch = 503, "Leg count mismatch", BadRequest, InvalidUrl, "wrong_number_of_legs";
    GeoTiffNotSupported = 504, "This service does not support GeoTIFF serialization.", BadRequest, InvalidValue, "unknown";
    SerializationUnknown = 599, "Unknown serialization error", BadRequest, InvalidValue, "unknown";
}

impl ErrorCode {
    /// The numeric code, for example 171.
    #[inline]
    pub const fn code(self) -> u16 {
        self as u16
    }

    /// The service that mainline attributes this code to in its statsd keys.
    ///
    /// This follows `serialize_error` in mainline's `src/worker.cc`,
    /// which groups by numeric range rather than by the service that raised the error:
    /// below 200 and 3xx are Loki, 400 through 500 are Thor, and everything else is Odin.
    #[inline]
    pub(crate) const fn statsd_worker(self) -> &'static str {
        let code = self.code();
        if code < 200 || (code >= 300 && code < 400) {
            "loki"
        } else if code >= 400 && code <= 500 {
            "thor"
        } else {
            "odin"
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Mainline's `exceptions.cc` at 3.9.1, the source of the table above.
    const MAINLINE_EXCEPTIONS: &str = include_str!("../fixtures/exceptions-3.9.1.cc");

    /// One row of mainline's `error_codes` table.
    #[derive(Debug, PartialEq)]
    struct MainlineRow<'a> {
        code: u16,
        message: &'a str,
        http_code: u16,
        http_message: &'a str,
        osrm_body: String,
        statsd_key: &'a str,
    }

    /// Reads the `HTTP_*` and `OSRM_*` string constants.
    fn mainline_constants() -> HashMap<&'static str, &'static str> {
        MAINLINE_EXCEPTIONS
            .lines()
            .filter_map(|line| line.strip_prefix("constexpr const char* "))
            .map(|rest| {
                let (name, value) = rest.split_once(" = ").expect("constant assignment");
                let value = value.strip_suffix(';').expect("terminated constant");
                let value = value
                    .strip_prefix("R\"(")
                    .and_then(|v| v.strip_suffix(")\""))
                    .or_else(|| value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
                    .expect("string literal");
                (name, value)
            })
            .collect()
    }

    /// Reads rows like
    /// `{171, {171, "No suitable edges near location", 400, HTTP_400, OSRM_NO_SEGMENT, "no_edges_near"}},`.
    fn mainline_rows() -> Vec<MainlineRow<'static>> {
        let constants = mainline_constants();
        let start = MAINLINE_EXCEPTIONS
            .find("error_codes{")
            .expect("error code table");
        let end = start
            + MAINLINE_EXCEPTIONS[start..]
                .find("};")
                .expect("end of error code table");
        MAINLINE_EXCEPTIONS[start..end]
            .lines()
            .skip(1)
            .map(|line| {
                let line = line.trim();
                let (key, rest) = line
                    .strip_prefix('{')
                    .and_then(|l| l.split_once(", {"))
                    .expect("row key");
                let (code, rest) = rest.split_once(", \"").expect("row code");
                assert_eq!(key, code, "row key and code differ");
                let (message, rest) = rest.split_once("\", ").expect("row message");
                let rest = rest.strip_suffix("\"}},").expect("row end");
                let mut fields = rest.split(", ");
                let http_code = fields.next().expect("http code");
                let http_name = fields.next().expect("http message");
                let osrm_name = fields.next().expect("osrm error");
                let statsd_key = fields
                    .next()
                    .and_then(|f| f.strip_prefix('"'))
                    .expect("statsd key");
                assert!(fields.next().is_none(), "unexpected extra fields in {line}");
                MainlineRow {
                    code: code.parse().expect("numeric code"),
                    message,
                    http_code: http_code.parse().expect("numeric http code"),
                    http_message: constants[http_name],
                    osrm_body: constants[osrm_name].to_string(),
                    statsd_key,
                }
            })
            .collect()
    }

    #[test]
    fn table_matches_mainline() {
        let mainline = mainline_rows();
        let ours: Vec<_> = ErrorCode::ALL
            .iter()
            .map(|&code| MainlineRow {
                code: code.code(),
                message: code.message(),
                http_code: code.http_status().status_code().as_u16(),
                http_message: code.http_status().reason(),
                osrm_body: osrm_body(code.osrm_error()),
                statsd_key: code.statsd_key(),
            })
            .collect();
        assert_eq!(ours.len(), mainline.len(), "number of codes");
        for (ours, mainline) in ours.iter().zip(&mainline) {
            assert_eq!(ours, mainline);
        }
    }

    fn osrm_body(error: OsrmError) -> String {
        format!(
            r#"{{"code":"{}","message":"{}"}}"#,
            error.code(),
            error.message()
        )
    }

    #[test]
    fn every_osrm_constant_is_modeled() {
        let mut mainline: Vec<_> = mainline_constants()
            .into_iter()
            .filter(|(name, _)| name.starts_with("OSRM_"))
            .map(|(_, body)| body.to_string())
            .collect();
        let mut ours: Vec<_> = OsrmError::ALL.iter().map(|&e| osrm_body(e)).collect();
        mainline.sort();
        ours.sort();
        assert_eq!(ours, mainline);
    }

    #[test]
    fn reasons_are_canonical() {
        for code in ErrorCode::ALL {
            let status = code.http_status();
            assert_eq!(
                status.status_code().canonical_reason(),
                Some(status.reason())
            );
        }
    }

    #[test]
    fn numeric_codes_round_trip() {
        for &code in ErrorCode::ALL {
            assert_eq!(ErrorCode::try_from(code.code()), Ok(code));
        }
        assert_eq!(ErrorCode::try_from(0), Err(UnknownErrorCode(0)));
        assert_eq!(ErrorCode::try_from(104), Err(UnknownErrorCode(104)));
        assert_eq!(ErrorCode::try_from(600), Err(UnknownErrorCode(600)));
    }

    #[test]
    fn all_is_sorted_and_unique() {
        assert!(ErrorCode::ALL.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(ErrorCode::ALL.len(), 100);
    }

    #[test]
    fn statsd_worker_follows_mainline_ranges() {
        assert_eq!(ErrorCode::NoSuitableEdges.statsd_worker(), "loki");
        assert_eq!(ErrorCode::LokiUnknown.statsd_worker(), "loki");
        assert_eq!(
            ErrorCode::OdinIntermediateParseFailed.statsd_worker(),
            "odin"
        );
        assert_eq!(ErrorCode::ResampleDistanceTooSmall.statsd_worker(), "loki");
        assert_eq!(ErrorCode::ThorUnknownAction.statsd_worker(), "thor");
        assert_eq!(ErrorCode::ThorUnknown.statsd_worker(), "thor");
        assert_eq!(ErrorCode::LegCountMismatch.statsd_worker(), "odin");
    }
}
