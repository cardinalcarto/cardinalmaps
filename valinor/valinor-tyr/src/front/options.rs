//! The request options (mainline's `from_json` in `src/worker.cc`).
//!
//! This follows mainline step by step, in the same order,
//! because the order decides which error a bad request gets
//! and which warnings come first.

use crate::costing_options::{
    TIME_DEPENDENT_FLOW_MASK, costing_name, parse_costing, parse_costing_name, parse_costings,
    parse_filter_action,
};
use crate::front::json::{JsonNumber, JsonValue};
use crate::front::locations::{
    LocationList, add_date_to_locations, clamp_longitude, parse_locations,
};
use crate::front::values::{LOCALES, decode_polyline, is_iso_valid};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use valhalla_proto::costing::Type as CostingType;
use valhalla_proto::costing::options::{HasFlowMask, HasIgnoreClosures};
use valhalla_proto::costing::{HasName, HasOptions};
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{HasTime, Type as LocationType};
use valhalla_proto::options::{
    Action, DateTimeType, Format, HasAlternates, HasBreakageDistance, HasDateTime, HasDenoise,
    HasEncodedPolyline, HasExpansionAction, HasGeneralize, HasGpsAccuracy, HasHeightPrecision,
    HasId, HasInterpolationDistance, HasLanguage, HasMatrixLocations, HasResampleDistance,
    HasRoundaboutExits, HasSearchRadius, HasShapeFormat, HasTurnPenaltyFactor, HasVerbose,
    ReverseTimeTracking, Units,
};
use valhalla_proto::tile::{HasX, HasY, HasZ};
use valhalla_proto::{
    Api, CodedDescription, Contour, Costing, DirectionsType, LatLng, LinearFeatureCost, Location,
    Options, Ring, ShapeFormat, ShapeMatch, contour,
};
use valhalla_response::{ErrorCode, ValhallaError};

/// Mainline's warning messages (`warning_codes` in `src/exceptions.cc`) for the codes the front adds.
fn warning_message(code: u64) -> Option<&'static str> {
    Some(match code {
        100 => r#"auto_shorter costing is deprecated, use "shortest" costing option instead"#,
        101 => r#"hov costing is deprecated, use "include_hov2" costing option instead"#,
        102 => r#"auto_data_fix is deprecated, use the "ignore_*" costing options instead"#,
        103 => r#"best_paths has been deprecated, use "alternates" instead"#,
        104 => "jsonp has been deprecated, use CORS instead",
        203 => {
            r#""waiting_time" is set on a location of type "via" or "through", ignoring waiting_time"#
        }
        204 => r#""exclude_polygons" received invalid input, ignoring exclude_polygons"#,
        211 => "This action doesn't support requested format, using json instead",
        212 => "Invalid layer name in exclude_layers array: ",
        _ => return None,
    })
}

/// Adds a warning, as mainline's `add_warning` does: its message followed by `extra`.
pub(crate) fn add_warning(warnings: &mut Vec<CodedDescription>, code: u64, extra: &str) {
    if let Some(message) = warning_message(code) {
        warnings.push(CodedDescription {
            description: format!("{message}{extra}"),
            code,
        });
    }
}

/// The unknown error (199) Loki's worker reports when a rapidjson assertion fails,
/// with the assertion as its detail.
fn assertion(assertion: &str) -> ValhallaError {
    ValhallaError::with_detail(ErrorCode::LokiUnknown, assertion)
}

/// `std::min` (which returns the first argument unless the second is smaller).
fn cpp_min(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

/// `std::max` (which returns the first argument unless it is smaller).
fn cpp_max(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// Which actions support which formats (mainline's `is_format_supported`).
fn is_format_supported(action: i32, format: i32) -> bool {
    let (Ok(action), Ok(format)) = (Action::try_from(action), Format::try_from(format)) else {
        return false;
    };
    match format {
        Format::Json => true,
        Format::Gpx => matches!(
            action,
            Action::Route | Action::OptimizedRoute | Action::TraceRoute
        ),
        Format::Osrm => matches!(
            action,
            Action::Route
                | Action::OptimizedRoute
                | Action::TraceRoute
                | Action::TraceAttributes
                | Action::Locate
                | Action::Status
                | Action::SourcesToTargets
                | Action::Expansion
        ),
        Format::Pbf => matches!(
            action,
            Action::Route
                | Action::OptimizedRoute
                | Action::TraceRoute
                | Action::Centroid
                | Action::TraceAttributes
                | Action::Status
                | Action::SourcesToTargets
                | Action::Isochrone
                | Action::Expansion
        ),
        // Mainline's image is built without GeoTIFF support.
        Format::Geotiff => false,
        Format::Mvt => action == Action::Tile,
    }
}

/// Mainline's `Options_Format_Enum_Parse` (which cannot produce `mvt`).
fn parse_format(name: &str) -> Option<Format> {
    Some(match name {
        "json" => Format::Json,
        "gpx" => Format::Gpx,
        "osrm" => Format::Osrm,
        "pbf" => Format::Pbf,
        "geotiff" => Format::Geotiff,
        _ => return None,
    })
}

/// Parses a request's JSON into `api.options` (mainline's `from_json`).
///
/// If `api` already has options and the document is an empty object, this is a PBF request:
/// the options are kept and validated (with mainline's defaults filled in),
/// and only the response fields and `info` are cleared.
/// Otherwise `api` starts over and everything comes from the JSON.
/// `action` is the action from the request path, if it named one.
///
/// On failure, `api` holds what was parsed before the error, as in mainline.
///
/// # Errors
///
/// Returns the error mainline raises for the first problem it finds,
/// for example 125 for an unknown costing, 130 for a bad location,
/// or 162 for an invalid date and time.
pub fn from_json(
    doc: &mut JsonValue,
    action: Option<Action>,
    api: &mut Api,
) -> Result<(), ValhallaError> {
    let pbf = match (&api.options, &*doc) {
        (Some(_), JsonValue::Object(members)) => members.is_empty(),
        // rapidjson's `ObjectEmpty` asserts that the document is an object.
        (Some(_), _) => return Err(assertion("IsObject()")),
        (None, _) => false,
    };
    if pbf {
        api.trip = None;
        api.directions = None;
        api.status = None;
        api.info = None;
    } else {
        *api = Api::default();
    }

    let options = api.options.get_or_insert_default();
    if let Some(action) = action {
        options.action = action.into();
    }
    let warnings = &mut api.info.get_or_insert_default().warnings;
    Parser {
        doc,
        action,
        pbf,
        options,
        warnings,
    }
    .parse()
}

/// The state of one `from_json` call.
struct Parser<'a> {
    doc: &'a mut JsonValue,
    /// The action from the request path (mainline's `action` argument).
    action: Option<Action>,
    pbf: bool,
    options: &'a mut Options,
    warnings: &'a mut Vec<CodedDescription>,
}

impl Parser<'_> {
    fn is_action(&self, action: Action) -> bool {
        self.options.action == action as i32
    }

    #[expect(
        clippy::too_many_lines,
        reason = "this mirrors mainline's `from_json` step by step, which is easier to check as one"
    )]
    fn parse(mut self) -> Result<(), ValhallaError> {
        let doc = &*self.doc;
        let options = &mut *self.options;

        // Matrix responses are verbose by default, for backwards compatibility.
        let verbose = match options.has_verbose {
            Some(HasVerbose::Verbose(verbose)) => verbose,
            None => options.action == Action::SourcesToTargets as i32,
        };
        options.has_verbose = Some(HasVerbose::Verbose(doc.get_or("/verbose", verbose)));

        if let Some(action) = doc
            .get::<String>("/filters/action")
            .as_deref()
            .and_then(parse_filter_action)
        {
            options.filter_action = action.into();
        }
        if let Some(attributes) = doc.get_array("/filters/attributes") {
            for attribute in attributes {
                let JsonValue::String(attribute) = attribute else {
                    return Err(assertion("IsString()"));
                };
                // `edge.tagged_names` was renamed; mainline still accepts the old name.
                let attribute = if attribute == "edge.tagged_names" {
                    "edge.tagged_values"
                } else {
                    attribute
                };
                options.filter_attributes.push(attribute.into());
            }
        }
        if let Some(generalize) = doc.get::<f32>("/generalize") {
            options.has_generalize = Some(HasGeneralize::Generalize(generalize));
        }

        // Tiles need only their coordinates and layers.
        if self.is_action(Action::Tile) {
            self.parse_tile()?;
            self.options.format = Format::Mvt.into();
            return Ok(());
        }

        // Move anything nested in the deprecated `directions_options` up to the top level.
        if let Some(deprecated) = self.doc.pointer("/directions_options").cloned() {
            for key in ["units", "narrative", "format", "language"] {
                if let Some(child) = deprecated.member(key) {
                    self.doc.add_member(key.into(), child.clone());
                }
            }
            self.doc.remove_member("directions_options");
        }
        let doc = &*self.doc;
        let options = &mut *self.options;

        if let Some(format) = doc
            .get::<String>("/format")
            .as_deref()
            .and_then(parse_format)
        {
            options.format = format.into();
        }
        if let Some(id) = doc.get::<String>("/id") {
            options.has_id = Some(HasId::Id(id));
        }
        // JSONP is deprecated: CORS should be used instead.
        if doc.get::<String>("/jsonp").is_some() {
            add_warning(self.warnings, 104, "");
        }
        if !is_format_supported(options.action, options.format) {
            options.format = Format::Json.into();
            add_warning(self.warnings, 211, "");
        }

        if let Some(units) = doc.get::<String>("/units")
            && (units == "miles" || units == "mi")
        {
            options.units = Units::Miles.into();
        }
        // Note that this ignores a PBF request's value.
        options.reverse = doc.get_or("/reverse", false);

        if let Some(language) = doc.get::<String>("/language")
            && LOCALES.contains(&language.as_str())
        {
            options.has_language = Some(HasLanguage::Language(language));
        }
        if options.has_language.is_none() {
            options.has_language = Some(HasLanguage::Language("en-US".into()));
        }

        // `narrative` is deprecated.
        if doc.get::<bool>("/narrative") == Some(false) {
            options.directions_type = DirectionsType::None.into();
        }
        if let Some(directions_type) = doc.get::<String>("/directions_type").and_then(|name| {
            Some(match name.as_str() {
                "none" => DirectionsType::None,
                "maneuvers" => DirectionsType::Maneuvers,
                "instructions" => DirectionsType::Instructions,
                _ => return None,
            })
        }) {
            options.directions_type = directions_type.into();
        }
        if let Some(strategy) = doc
            .get::<String>("/reverse_time_tracking")
            .and_then(|name| match name.as_str() {
                "disabled" => Some(ReverseTimeTracking::RttDisabled),
                "heuristic" => Some(ReverseTimeTracking::RttHeuristic),
                _ => None,
            })
        {
            options.reverse_time_tracking = strategy.into();
        }

        // The costing defaults to none, which is only valid for /locate.
        let default_costing = if self.pbf {
            // An invalid costing has an empty name, which parses as none.
            CostingType::try_from(options.costing_type).map_or("", costing_name)
        } else {
            "none"
        };
        let mut costing_str: String = doc.get_or("/costing", default_costing.into());
        self.rewrite_deprecated_costing(&mut costing_str);
        let doc = &*self.doc;
        let options = &mut *self.options;

        let costing_type = parse_costing_name(&costing_str).ok_or_else(|| {
            ValhallaError::with_detail(ErrorCode::UnknownCosting, format!("'{costing_str}'"))
        })?;
        options.costing_type = costing_type.into();

        parse_date_time(doc, options)?;

        if let Some(shape_format) = doc.get::<String>("/shape_format") {
            let shape_format = match shape_format.as_str() {
                "polyline6" => ShapeFormat::Polyline6,
                "polyline5" => ShapeFormat::Polyline5,
                "geojson" => ShapeFormat::Geojson,
                "no_shape" if self.action != Some(Action::Height) => ShapeFormat::NoShape,
                _ => return Err(ErrorCode::InvalidShapeFormat.into()),
            };
            options.has_shape_format = Some(HasShapeFormat::ShapeFormat(shape_format.into()));
        } else if self.action == Some(Action::SourcesToTargets)
            && options.has_shape_format.is_none()
        {
            options.has_shape_format =
                Some(HasShapeFormat::ShapeFormat(ShapeFormat::NoShape.into()));
        }

        if let Some(linear_references) = doc.get::<bool>("/linear_references") {
            options.linear_references = linear_references;
        }
        if let Some(admin_crossings) = doc.get::<bool>("/admin_crossings") {
            options.admin_crossings = admin_crossings;
        }

        // Whatever the costing, check whether it ignores closures.
        let mut ignore_closures = if costing_str == "multimodal" {
            None
        } else {
            doc.get::<bool>(&format!("/costing_options/{costing_str}/ignore_closures"))
        };
        // A PBF request's costing takes precedence.
        // (Mainline takes the first match in hash map order; we go by key.)
        let mut keys: Vec<&i32> = options.costings.keys().collect();
        keys.sort_unstable();
        for key in keys {
            let costing = &options.costings[key];
            if costing.r#type == options.costing_type
                && let Some(HasOptions::Options(costing_options)) = &costing.has_options
                && let Some(HasIgnoreClosures::IgnoreClosures(ignore)) =
                    costing_options.has_ignore_closures
            {
                ignore_closures = Some(ignore);
                break;
            }
        }

        parse_costings(doc, "/costing_options", options, self.warnings)?;

        // Whether any location list had a date and time.
        let mut had_date_time = false;

        // Map matching shapes, and the encoded polylines of height requests.
        if let Some(encoded) = doc.get::<String>("/encoded_polyline") {
            options.has_encoded_polyline = Some(HasEncodedPolyline::EncodedPolyline(encoded));
        }
        if let Some(HasEncodedPolyline::EncodedPolyline(encoded)) = &options.has_encoded_polyline {
            // Only height requests support polyline5 input.
            let precision = if options.action == Action::Height as i32
                && options.has_shape_format
                    == Some(HasShapeFormat::ShapeFormat(ShapeFormat::Polyline5.into()))
            {
                1e-5
            } else {
                1e-6
            };
            let points = decode_polyline(encoded.as_bytes(), precision)
                .map_err(|_| assertion("Bad encoded polyline"))?;
            options.shape = points
                .into_iter()
                .map(|(lng, lat)| Location {
                    ll: Some(LatLng {
                        has_lat: Some(HasLat::Lat(lat)),
                        has_lng: Some(HasLng::Lng(lng)),
                    }),
                    r#type: LocationType::KVia.into(),
                    has_time: Some(HasTime::Time(-1.0)),
                    ..Location::default()
                })
                .collect();
            if let Some(first) = options.shape.first_mut() {
                first.r#type = LocationType::KBreak.into();
            }
            if let Some(last) = options.shape.last_mut() {
                last.r#type = LocationType::KBreak.into();
            }
            add_date_to_locations(options, LocationList::Shape);
        } else {
            had_date_time |= parse_locations(
                doc,
                options,
                self.warnings,
                LocationList::Shape,
                ErrorCode::ShapeParseFailed,
                ignore_closures,
            )?;
            if options.shape.is_empty() {
                had_date_time |= parse_locations(
                    doc,
                    options,
                    self.warnings,
                    LocationList::Trace,
                    ErrorCode::TraceParseFailed,
                    ignore_closures,
                )?;
            }
        }

        // Durations between shape points, from a begin time (default 0), give the shape's times.
        let begin_time = doc.get::<u32>("/begin_time").map_or(0.0, f64::from);
        if let Some(durations) = doc.get_array("/durations") {
            // Mainline indexes past the end of an empty shape here (undefined behavior);
            // we report the mismatch instead.
            if options.shape.is_empty() || durations.len() != options.shape.len() - 1 {
                return Err(ErrorCode::TraceDurationMismatch.into());
            }
            let mut time = begin_time;
            options.shape[0].has_time = Some(HasTime::Time(time));
            for (duration, location) in durations.iter().zip(&mut options.shape[1..]) {
                let JsonValue::Number(duration) = duration else {
                    return Err(assertion("IsNumber()"));
                };
                time += duration.as_f64();
                location.has_time = Some(HasTime::Time(time));
            }
        }

        options.use_timestamps = doc.get_or("/use_timestamps", options.use_timestamps);
        options.prioritize_bidirectional = doc.get_or(
            "/prioritize_bidirectional",
            options.prioritize_bidirectional,
        );
        if options.use_timestamps && !options.shape.iter().any(|s| s.has_time.is_some()) {
            return Err(ErrorCode::MissingTimestamps.into());
        }

        // Elevation sampling for routes and trace attributes, in [0, 1000].
        let elevation_interval = doc
            .get::<f32>("/elevation_interval")
            .unwrap_or(options.elevation_interval);
        options.elevation_interval = cpp_max(cpp_min(elevation_interval, 1000.0), 0.0);

        options.range = doc.get_or("/range", options.range);
        if let Some(precision) = doc.get::<u32>("/height_precision")
            && precision <= 2
        {
            options.has_height_precision = Some(HasHeightPrecision::HeightPrecision(precision));
        }

        parse_recostings(doc, options, self.warnings)?;

        for (list, error) in [
            (LocationList::Locations, ErrorCode::LocationParseFailed),
            (LocationList::Sources, ErrorCode::SourceParseFailed),
            (LocationList::Targets, ErrorCode::TargetParseFailed),
        ] {
            had_date_time |=
                parse_locations(doc, options, self.warnings, list, error, ignore_closures)?;
        }

        // Without a time, routes and map matching use neither predicted nor live speeds.
        let first_shape_time = options.shape.first().map(|s| match s.has_time {
            Some(HasTime::Time(time)) => time,
            None => 0.0,
        });
        #[expect(
            clippy::float_cmp,
            reason = "comparing against mainline's sentinel exactly"
        )]
        let untimed_shape = first_shape_time.is_none_or(|time| time == -1.0);
        if options.date_time_type == DateTimeType::NoTime as i32 && !had_date_time && untimed_shape
        {
            for costing in options.costings.values_mut() {
                let costing_options = costing_options_mut(costing);
                let mask = match costing_options.has_flow_mask {
                    Some(HasFlowMask::FlowMask(mask)) => mask,
                    None => 0,
                };
                // Mainline narrows the mask to 8 bits first.
                costing_options.has_flow_mask = Some(HasFlowMask::FlowMask(
                    mask & 0xFF & !TIME_DEPENDENT_FLOW_MASK,
                ));
            }
        }

        // rapidjson's `HasMember` asserts that the document is an object.
        if !doc.is_object() {
            return Err(assertion("IsObject()"));
        }
        // `avoid_locations` is the deprecated name of `exclude_locations`.
        let exclude_key = if doc.member("avoid_locations").is_some() {
            "/avoid_locations"
        } else {
            "/exclude_locations"
        };
        parse_locations(
            doc,
            options,
            self.warnings,
            LocationList::ExcludeLocations(exclude_key),
            ErrorCode::AvoidParseFailed,
            ignore_closures,
        )?;

        // `matrix_locations` only applies to one-to-many and many-to-one matrices.
        let one_to_many = options.sources.len() == 1 || options.targets.len() == 1;
        if let Some(matrix_locations) = doc.get::<i32>("/matrix_locations")
            && one_to_many
        {
            #[expect(clippy::cast_sign_loss, reason = "mainline stores the int unsigned")]
            let matrix_locations = matrix_locations as u32;
            options.has_matrix_locations =
                Some(HasMatrixLocations::MatrixLocations(matrix_locations));
        } else if options.has_matrix_locations.is_none() || !one_to_many {
            options.has_matrix_locations = Some(HasMatrixLocations::MatrixLocations(u32::MAX));
        }

        if let Some(distance) = doc.get::<u32>("/expansion_max_distance") {
            options.expansion_max_distance = distance;
        }

        parse_exclude_polygons(doc, options, self.warnings)?;
        parse_linear_cost_factors(doc, options, self.warnings)?;

        if let Some(distance) = doc.get::<f64>("/resample_distance") {
            options.has_resample_distance = Some(HasResampleDistance::ResampleDistance(distance));
        }

        // The `action` member is the expansion action.
        if let Some(name) = doc.get::<String>("/action") {
            let action = match name.as_str() {
                "route" => Action::Route,
                "isochrone" => Action::Isochrone,
                "sources_to_targets" => Action::SourcesToTargets,
                _ => {
                    return Err(ValhallaError::with_detail(
                        ErrorCode::ExpansionNotSupported,
                        name,
                    ));
                }
            };
            options.has_expansion_action = Some(HasExpansionAction::ExpansionAction(action.into()));
        }
        let expansion_action = match options.has_expansion_action {
            Some(HasExpansionAction::ExpansionAction(action)) => action,
            None => Action::NoAction.into(),
        };
        if options.action == Action::Expansion as i32 && expansion_action == Action::NoAction as i32
        {
            return Err(ValhallaError::with_detail(
                ErrorCode::ActionRequired,
                "action",
            ));
        }

        if let Some(JsonValue::Array(properties)) = doc.pointer("/expansion_properties") {
            for property in properties {
                let JsonValue::String(property) = property else {
                    return Err(assertion("IsString()"));
                };
                let property =
                    valhalla_proto::options::ExpansionProperties::from_str_name(property)
                        .ok_or_else(|| {
                            ValhallaError::with_detail(
                                ErrorCode::InvalidExpansionProperty,
                                property.clone(),
                            )
                        })?;
                options.expansion_properties.push(property.into());
            }
        }

        options.skip_opposites = doc.get_or("/skip_opposites", options.skip_opposites);
        options.dedupe = doc.get_or("/dedupe", options.dedupe);

        parse_contours(doc, &mut options.contours)?;
        options.polygons = doc.get_or("/polygons", options.polygons);

        let denoise = match options.has_denoise {
            Some(HasDenoise::Denoise(denoise)) => denoise,
            None => 1.0,
        };
        let denoise: f32 = doc.get_or("/denoise", denoise);
        options.has_denoise = Some(HasDenoise::Denoise(cpp_max(cpp_min(denoise, 1.0), 0.0)));

        options.show_locations = doc.get_or("/show_locations", options.show_locations);

        if let Some(shape_match) = doc.get::<String>("/shape_match") {
            let shape_match = match shape_match.as_str() {
                "edge_walk" => ShapeMatch::EdgeWalk,
                "map_snap" => ShapeMatch::MapSnap,
                "walk_or_snap" => ShapeMatch::WalkOrSnap,
                _ => return Err(ErrorCode::InvalidShapeMatch.into()),
            };
            options.shape_match = shape_match.into();
        }

        if let Some(v) = doc.get::<f32>("/trace_options/gps_accuracy") {
            options.has_gps_accuracy = Some(HasGpsAccuracy::GpsAccuracy(v));
        }
        if let Some(v) = doc.get::<f32>("/trace_options/search_radius") {
            options.has_search_radius = Some(HasSearchRadius::SearchRadius(v));
        }
        if let Some(v) = doc.get::<f32>("/trace_options/turn_penalty_factor") {
            options.has_turn_penalty_factor = Some(HasTurnPenaltyFactor::TurnPenaltyFactor(v));
        }
        if let Some(v) = doc.get::<f32>("/trace_options/breakage_distance") {
            options.has_breakage_distance = Some(HasBreakageDistance::BreakageDistance(v));
        }
        if let Some(v) = doc.get::<f32>("/trace_options/interpolation_distance") {
            options.has_interpolation_distance =
                Some(HasInterpolationDistance::InterpolationDistance(v));
        }

        // `best_paths` is the deprecated name of `alternates` (plus one).
        if doc.get::<u32>("/best_paths").is_some() {
            add_warning(self.warnings, 103, "");
        }
        let best_paths = doc.get_or::<u32>("/best_paths", 1).max(1);
        let alternates = match options.has_alternates {
            Some(HasAlternates::Alternates(alternates)) => alternates,
            None => best_paths - 1,
        };
        options.has_alternates = Some(HasAlternates::Alternates(
            doc.get_or("/alternates", alternates),
        ));
        // Routes through more than two locations have no alternates.
        if options.action != Action::TraceAttributes as i32 && options.locations.len() > 2 {
            options.has_alternates = Some(HasAlternates::Alternates(0));
        }

        options.guidance_views = doc.get_or("/guidance_views", options.guidance_views);
        options.banner_instructions =
            doc.get_or("/banner_instructions", options.banner_instructions);
        options.voice_instructions = doc.get_or("/voice_instructions", options.voice_instructions);
        options.turn_lanes = doc.get_or("/turn_lanes", options.turn_lanes);
        let roundabout_exits = match options.has_roundabout_exits {
            Some(HasRoundaboutExits::RoundaboutExits(exits)) => exits,
            None => true,
        };
        options.has_roundabout_exits = Some(HasRoundaboutExits::RoundaboutExits(
            doc.get_or("/roundabout_exits", roundabout_exits),
        ));

        // Mainline finally writes the language and format it used back into the document,
        // which changes nothing we keep.
        Ok(())
    }

    /// Rewrites the deprecated costings (`auto_shorter`, `hov`, and `auto_data_fix`)
    /// into auto costing with the options they stood for, as mainline does in the document.
    fn rewrite_deprecated_costing(&mut self, costing_str: &mut String) {
        let (warning, options_key, flags): (u64, &str, &[&str]) = match costing_str.as_str() {
            "auto_shorter" => (100, "/costing_options/auto_shorter", &["shortest"]),
            "hov" => (101, "/costing_options/hov", &["include_hov2"]),
            "auto_data_fix" => (
                102,
                "/costing_options/auto_data_fix",
                &[
                    "ignore_restrictions",
                    "ignore_oneways",
                    "ignore_access",
                    "ignore_closures",
                ],
            ),
            _ => return,
        };
        add_warning(self.warnings, warning, "");
        *costing_str = "auto".into();
        self.doc
            .set_pointer("/costing", JsonValue::String("auto".into()));
        if let Some(json_options) = self.doc.pointer(options_key).cloned() {
            self.doc.set_pointer("/costing_options/auto", json_options);
        }
        for flag in flags {
            self.doc.set_pointer(
                &format!("/costing_options/auto/{flag}"),
                JsonValue::Bool(true),
            );
        }
    }

    /// The tile coordinates and layers of a `/tile` request (`parse_xyz` and `parse_exclude_layers`).
    fn parse_tile(&mut self) -> Result<(), ValhallaError> {
        let doc = &*self.doc;
        // Read the PBF request's values without creating the message (as protobuf's getters do).
        let current = self.options.tile_xyz.unwrap_or_default();
        let z = match current.has_z {
            Some(HasZ::Z(z)) => z,
            None => u32::MAX,
        };
        let z: u32 = doc.get_or("/tile/z", z);
        let mut valid = z != u32::MAX && z <= 30;
        if valid {
            self.options.tile_xyz.get_or_insert_default().has_z = Some(HasZ::Z(z));
            let x = match current.has_x {
                Some(HasX::X(x)) => x,
                None => u32::MAX,
            };
            let y = match current.has_y {
                Some(HasY::Y(y)) => y,
                None => u32::MAX,
            };
            let x: u32 = doc.get_or("/tile/x", x);
            let y: u32 = doc.get_or("/tile/y", y);
            valid = x != u32::MAX && y != u32::MAX;
            if valid {
                let max_coord = 1u64 << z;
                valid = u64::from(x) < max_coord && u64::from(y) < max_coord;
                let tile = self.options.tile_xyz.get_or_insert_default();
                tile.has_x = Some(HasX::X(x));
                tile.has_y = Some(HasY::Y(y));
            }
        }
        if !valid {
            return Err(ErrorCode::InvalidTileCoordinates.into());
        }

        if let Some(JsonValue::Array(layers)) = doc.pointer("/tile_options/exclude_layers") {
            const SUPPORTED: [&str; 5] = [
                "edges",
                "nodes",
                "shortcuts",
                "access_restrictions",
                "incidents",
            ];
            for layer in layers {
                match layer {
                    JsonValue::String(name) if SUPPORTED.contains(&name.as_str()) => self
                        .options
                        .tile_options
                        .get_or_insert_default()
                        .exclude_layers
                        .push(name.clone()),
                    JsonValue::String(name) => add_warning(self.warnings, 212, name),
                    _ => add_warning(self.warnings, 212, "not a string"),
                }
            }
        }
        Ok(())
    }
}

/// The options of a costing, created if needed (protobuf's `mutable_options`).
fn costing_options_mut(costing: &mut Costing) -> &mut valhalla_proto::costing::Options {
    let HasOptions::Options(options) = costing
        .has_options
        .get_or_insert_with(|| HasOptions::Options(valhalla_proto::costing::Options::default()));
    options
}

/// `date_time.type` and `date_time.value`, and the checks on them.
fn parse_date_time(doc: &JsonValue, options: &mut Options) -> Result<(), ValhallaError> {
    // The JSON types are offset by one from the protobuf ones (which start at no time).
    if let Some(json_type) = doc.get::<u32>("/date_time/type") {
        let date_time_type = json_type.wrapping_add(1);
        if let Ok(date_time_type) = i32::try_from(date_time_type)
            && DateTimeType::try_from(date_time_type).is_ok()
        {
            options.date_time_type = date_time_type;
        }
    }
    let costing = options.costing_type;
    let is_transit =
        costing == CostingType::Multimodal as i32 || costing == CostingType::Transit as i32;
    let date_time_type = options.date_time_type;
    if date_time_type != DateTimeType::NoTime as i32 {
        // Only a PBF request can carry a type that is out of range.
        if date_time_type >= 5 {
            return Err(ErrorCode::InvalidDateType.into());
        }
        let value = if date_time_type == DateTimeType::Current as i32 {
            "current".to_string()
        } else {
            let current = match &options.has_date_time {
                Some(HasDateTime::DateTime(value)) => value.clone(),
                None => String::new(),
            };
            doc.get_or("/date_time/value", current)
        };
        if value.is_empty() {
            match DateTimeType::try_from(date_time_type) {
                Ok(DateTimeType::DepartAt) => return Err(ErrorCode::MissingDepartAtDate.into()),
                Ok(DateTimeType::ArriveBy) => return Err(ErrorCode::MissingArriveByDate.into()),
                Ok(DateTimeType::Invariant) => return Err(ErrorCode::MissingInvariantDate.into()),
                _ => {}
            }
        }
        if value != "current" && !is_iso_valid(&value) {
            return Err(ErrorCode::DateParseFailed.into());
        }
        options.has_date_time = Some(HasDateTime::DateTime(value));
    } else if is_transit {
        // Transit without a time leaves now.
        options.date_time_type = DateTimeType::Current.into();
        options.has_date_time = Some(HasDateTime::DateTime("current".into()));
    }

    let date_time_type = options.date_time_type;
    if date_time_type == DateTimeType::ArriveBy as i32
        || date_time_type == DateTimeType::Invariant as i32
    {
        if is_transit {
            return Err(ErrorCode::MultimodalArriveByNotImplemented.into());
        }
        if options.action == Action::Isochrone as i32 {
            return Err(ErrorCode::IsochroneArriveByNotImplemented.into());
        }
    }
    Ok(())
}

/// Named costings to recost a path with (`parse_recostings`).
fn parse_recostings(
    doc: &JsonValue,
    options: &mut Options,
    warnings: &mut Vec<CodedDescription>,
) -> Result<(), ValhallaError> {
    let mut names = BTreeSet::new();
    let mut check_name = |costing: &Costing| -> Result<(), ValhallaError> {
        match &costing.has_name {
            None => Err(ErrorCode::RecostingCostingRequired.into()),
            Some(HasName::Name(name)) if !names.insert(name.clone()) => {
                Err(ErrorCode::DuplicateRecostingNames.into())
            }
            Some(_) => Ok(()),
        }
    };
    if let Some(JsonValue::Array(recostings)) = doc.pointer("/recostings") {
        for i in 0..recostings.len() {
            options.recostings.push(Costing::default());
            let recosting = options
                .recostings
                .last_mut()
                .ok_or(ErrorCode::LokiUnknown)?;
            parse_costing(doc, &format!("/recostings/{i}"), recosting, warnings, None)?;
            check_name(recosting)?;
        }
    } else {
        for recosting in &mut options.recostings {
            check_name(recosting)?;
            let costing_type = CostingType::try_from(recosting.r#type).ok();
            // Mainline would fail an assertion on a type it does not know.
            let costing_type = costing_type.ok_or_else(|| assertion("Unknown costing"))?;
            parse_costing(doc, "/recostings", recosting, warnings, Some(costing_type))?;
        }
    }
    Ok(())
}

/// Isochrone contours (`parse_contours`).
fn parse_contours(doc: &JsonValue, contours: &mut Vec<Contour>) -> Result<(), ValhallaError> {
    if let Some(json_contours) = doc.get_array("/contours") {
        for json_contour in json_contours {
            contours.push(Contour {
                has_time: json_contour.get::<f32>("/time").map(contour::HasTime::Time),
                has_distance: json_contour
                    .get::<f32>("/distance")
                    .map(contour::HasDistance::Distance),
                has_color: json_contour
                    .get::<String>("/color")
                    .map(contour::HasColor::Color),
            });
        }
    }
    // Each contour needs a time or a distance.
    if contours
        .iter()
        .any(|c| c.has_time.is_none() && c.has_distance.is_none())
    {
        return Err(ErrorCode::TimeRequired.into());
    }
    Ok(())
}

/// A failure inside one of mainline's `try` blocks, with what its `what()` would say.
struct Failure(String);

impl Failure {
    /// A failed rapidjson assertion.
    fn assertion(assertion: &str) -> Self {
        Failure(assertion.into())
    }
}

/// rapidjson's `GetDouble` on a value that must be a number.
fn get_double(value: &JsonValue) -> Result<f64, Failure> {
    match value {
        JsonValue::Number(number) => Ok(number.as_f64()),
        _ => Err(Failure::assertion("IsNumber()")),
    }
}

/// rapidjson's `GetFloat` on a value that must be a number.
#[expect(
    clippy::cast_possible_truncation,
    reason = "this is rapidjson's conversion"
)]
fn get_float(value: &JsonValue) -> Result<f32, Failure> {
    get_double(value).map(|d| d as f32)
}

/// rapidjson's `operator[]` on an object, which asserts that the member exists.
fn get_member<'a>(value: &'a JsonValue, name: &str) -> Result<&'a JsonValue, Failure> {
    match value {
        JsonValue::Object(_) => value.member(name).ok_or(Failure::assertion("false")),
        _ => Err(Failure::assertion("IsObject()")),
    }
}

/// rapidjson's `GetArray`, which asserts that the value is an array.
fn get_elements(value: &JsonValue) -> Result<&[JsonValue], Failure> {
    match value {
        JsonValue::Array(elements) => Ok(elements),
        _ => Err(Failure::assertion("IsArray()")),
    }
}

/// rapidjson's `operator[]` on an array, which asserts that the index is in range.
fn get_element(value: &JsonValue, index: usize) -> Result<&JsonValue, Failure> {
    get_elements(value)?
        .get(index)
        .ok_or(Failure::assertion("index < data_.a.size"))
}

/// A ring of `[lon, lat]` pairs from JSON (`parse_ring`), appended to `ring`.
fn parse_json_ring(ring: &mut Ring, coordinates: &[JsonValue]) -> Result<(), Failure> {
    for pair in coordinates {
        if get_elements(pair)?.len() < 2 {
            return Err(Failure(
                "Polygon coordinates must consist of [Lon, Lat] arrays.".into(),
            ));
        }
        let lon = clamp_longitude(get_double(get_element(pair, 0)?)?);
        let lat = get_double(get_element(pair, 1)?)?;
        if lat < -90.0 || lat > 90.0 {
            return Err(Failure(
                "Latitude must be in the range [-90, 90] degrees".into(),
            ));
        }
        ring.coords.push(LatLng {
            has_lat: Some(HasLat::Lat(lat)),
            has_lng: Some(HasLng::Lng(lon)),
        });
    }
    Ok(())
}

/// Validates a ring from a PBF request (`parse_ring`'s first branch).
fn validate_pbf_ring(ring: &mut Ring) -> Result<(), Failure> {
    if ring.coords.is_empty() {
        // Mainline then parses the (null) JSON instead, failing `GetArray`'s assertion.
        return Err(Failure::assertion("IsArray()"));
    }
    if ring.coords.len() < 2 {
        return Err(Failure(
            "Polygon coordinates must consist of [Lon, Lat] arrays.".into(),
        ));
    }
    for coord in &mut ring.coords {
        let (Some(HasLat::Lat(lat)), Some(HasLng::Lng(lng))) = (coord.has_lat, coord.has_lng)
        else {
            return Err(Failure(
                "Polygon coordinates must consist of [Lon, Lat] arrays.".into(),
            ));
        };
        coord.has_lng = Some(HasLng::Lng(clamp_longitude(lng)));
        if lat < -90.0 || lat > 90.0 {
            return Err(Failure(
                "Latitude must be in the range [-90, 90] degrees".into(),
            ));
        }
    }
    Ok(())
}

/// Areas to avoid: rings or `GeoJSON` polygons (with optional levels).
fn parse_exclude_polygons(
    doc: &JsonValue,
    options: &mut Options,
    warnings: &mut Vec<CodedDescription>,
) -> Result<(), ValhallaError> {
    // `avoid_polygons` is the deprecated name of `exclude_polygons`.
    let key = if doc.member("avoid_polygons").is_some() {
        "/avoid_polygons"
    } else {
        "/exclude_polygons"
    };
    let Some(exclude_polygons) = doc.pointer(key) else {
        if !options.exclude_polygons.is_empty() {
            // A PBF request's polygons: failures here escape to Loki's worker as unknown errors.
            for ring in &mut options.exclude_polygons {
                validate_pbf_ring(ring).map_err(|Failure(what)| assertion(&what))?;
            }
            if !options.exclude_levels.is_empty()
                && options.exclude_levels.len() != options.exclude_polygons.len()
            {
                return Err(assertion(
                    "Expected number of exclude levels and number of exclude polygons to be equal",
                ));
            }
        }
        return Ok(());
    };
    if !matches!(exclude_polygons, JsonValue::Array(_) | JsonValue::Object(_)) {
        add_warning(warnings, 204, "");
        return Ok(());
    }

    // An object must be a GeoJSON feature collection.
    // (Mainline looks for it under `exclude_polygons` even when the request used `avoid_polygons`.)
    let mut polygons = exclude_polygons;
    if !matches!(exclude_polygons, JsonValue::Array(_))
        && doc.get::<String>("/exclude_polygons/type").as_deref() == Some("FeatureCollection")
        && let Some(features) = doc.pointer("/exclude_polygons/features")
    {
        polygons = features;
    }

    let result = (|| -> Result<(), Failure> {
        for polygon in get_elements(polygons)? {
            let empty = match polygon {
                JsonValue::Array(elements) => elements.is_empty(),
                JsonValue::Object(members) => members.is_empty(),
                _ => false,
            };
            if empty {
                continue;
            }
            options.exclude_polygons.push(Ring::default());
            options
                .exclude_levels
                .push(valhalla_proto::Levels::default());
            let (Some(ring), Some(levels)) = (
                options.exclude_polygons.last_mut(),
                options.exclude_levels.last_mut(),
            ) else {
                continue;
            };
            match polygon {
                JsonValue::Array(coordinates) => parse_json_ring(ring, coordinates)?,
                JsonValue::Object(_) => {
                    // Only the first ring of a GeoJSON polygon.
                    let coordinates = polygon.pointer("/geometry/coordinates/0");
                    let is_polygon = matches!(
                        polygon.pointer("/geometry/type"),
                        Some(JsonValue::String(t)) if t == "Polygon"
                    );
                    match coordinates {
                        Some(JsonValue::Array(coordinates)) if is_polygon => {
                            parse_json_ring(ring, coordinates)?;
                        }
                        _ => {
                            return Err(Failure(
                                "Expected object to have coordinates member".into(),
                            ));
                        }
                    }
                    if let Some(JsonValue::Array(json_levels)) =
                        polygon.pointer("/properties/levels")
                    {
                        for level in json_levels {
                            // rapidjson's `IsFloat`: only doubles (not integers) in float range.
                            if let JsonValue::Number(JsonNumber::Double(d)) = level
                                && (-3.402_823_4e38..=3.402_823_4e38).contains(d)
                            {
                                #[expect(
                                    clippy::cast_possible_truncation,
                                    reason = "this is rapidjson's GetFloat"
                                )]
                                levels.levels.push(*d as f32);
                            }
                        }
                    }
                }
                // Neither an array nor an object: mainline adds an empty ring.
                _ => {}
            }
        }
        Ok(())
    })();
    result.map_err(|Failure(what)| ValhallaError::with_detail(ErrorCode::PolygonParseFailed, what))
}

/// Custom cost factors for linear features (`linear_cost_factors`).
fn parse_linear_cost_factors(
    doc: &JsonValue,
    options: &mut Options,
    warnings: &mut Vec<CodedDescription>,
) -> Result<(), ValhallaError> {
    let Some(features) = doc.pointer("/linear_cost_factors") else {
        return Ok(());
    };
    let JsonValue::Array(features) = features else {
        // Mainline reuses the exclude layers warning here.
        add_warning(warnings, 212, "");
        return Ok(());
    };
    let result = (|| -> Result<(), Failure> {
        for feature in features {
            let JsonValue::Object(_) = feature else {
                return Err(Failure::assertion("IsObject()"));
            };
            options.cost_factor_lines.push(LinearFeatureCost::default());
            let Some(line) = options.cost_factor_lines.last_mut() else {
                continue;
            };
            if feature.member("type").is_some() {
                parse_geojson_line(feature, line)?;
            } else {
                parse_encoded_line(feature, line)?;
            }
            if line.shape.is_empty() {
                // This is itself a 173 error, which mainline then wraps in another.
                return Err(Failure(
                    "Failed to parse line feature: feature coordinates are empty".into(),
                ));
            }
        }
        Ok(())
    })();
    result.map_err(|Failure(what)| {
        ValhallaError::with_detail(ErrorCode::LineFeatureParseFailed, what)
    })
}

fn shape_point(lng: f64, lat: f64) -> Location {
    Location {
        ll: Some(LatLng {
            has_lat: Some(HasLat::Lat(lat)),
            has_lng: Some(HasLng::Lng(lng)),
        }),
        ..Location::default()
    }
}

/// A linear feature as a `GeoJSON` feature (`parse_line_geojson`).
fn parse_geojson_line(feature: &JsonValue, line: &mut LinearFeatureCost) -> Result<(), Failure> {
    let geometry = get_member(feature, "geometry")?;
    let coordinates = get_member(geometry, "coordinates")?;
    for pair in get_elements(coordinates)? {
        let lng = get_float(get_element(pair, 0)?)?;
        let lat = get_float(get_element(pair, 1)?)?;
        line.shape.push(shape_point(f64::from(lng), f64::from(lat)));
    }
    let properties = get_member(feature, "properties")?;
    if let Some(factor) = get_object_member(properties, "factor")? {
        line.cost_factor = f64::from(get_float(factor)?);
    } else if let Some(ignore) = get_object_member(properties, "ignore_access_restrictions")? {
        line.ignore_access_restrictions = get_bool(ignore)?;
        line.cost_factor = 1.0;
    }
    Ok(())
}

/// rapidjson's `GetObject().HasMember(name)` followed by `operator[]`.
fn get_object_member<'a>(
    value: &'a JsonValue,
    name: &str,
) -> Result<Option<&'a JsonValue>, Failure> {
    match value {
        JsonValue::Object(_) => Ok(value.member(name)),
        _ => Err(Failure::assertion("IsObject()")),
    }
}

/// rapidjson's `GetBool`, which asserts that the value is a boolean.
fn get_bool(value: &JsonValue) -> Result<bool, Failure> {
    match value {
        JsonValue::Bool(b) => Ok(*b),
        _ => Err(Failure::assertion("IsBool()")),
    }
}

/// A linear feature as an encoded polyline and a factor (`parse_line`).
fn parse_encoded_line(feature: &JsonValue, line: &mut LinearFeatureCost) -> Result<(), Failure> {
    let JsonValue::String(shape) = get_member(feature, "shape")? else {
        return Err(Failure::assertion("IsString()"));
    };
    // `std::string(GetString())` stops at the first NUL.
    let shape = shape.split('\0').next().unwrap_or_default();
    let points = decode_polyline(shape.as_bytes(), 1e-6)
        .map_err(|_| Failure("Bad encoded polyline".into()))?;
    line.shape
        .extend(points.into_iter().map(|(lng, lat)| shape_point(lng, lat)));
    if let Some(factor) = feature.member("factor") {
        line.cost_factor = f64::from(get_float(factor)?);
    } else if let Some(ignore) = feature.member("ignore_access_restrictions") {
        line.cost_factor = 1.0;
        line.ignore_access_restrictions = get_bool(ignore)?;
    }
    Ok(())
}
