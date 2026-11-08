//! Locations (`parse_location` and `parse_locations` in mainline's `src/worker.cc`).

use crate::front::json::{JsonValue, double_to_i32, double_to_u32};
use crate::front::options::add_warning;
use alloc::vec::Vec;
use valhalla_proto::costing::HasFilterClosures;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{
    HasAccuracy, HasHeading, HasHeadingTolerance, HasMinimumReachability, HasNodeSnapTolerance,
    HasPreferredLayer, HasRadius, HasSearchCutoff, HasStreetSideCutoff, HasStreetSideMaxDistance,
    HasStreetSideTolerance, HasTime, PreferredSide, Type as LocationType,
};
use valhalla_proto::options::{Action, DateTimeType, HasDateTime};
use valhalla_proto::search_filter::{
    HasExcludeClosures, HasLevel, HasMaxRoadClass, HasMinRoadClass,
};
use valhalla_proto::{CodedDescription, LatLng, Location, Options, RoadClass, SearchFilter};
use valhalla_response::{ErrorCode, ValhallaError};

/// The level that means "any level" (`baldr::kMaxLevel`).
pub(crate) const MAX_LEVEL: f32 = f32::MAX;

/// Which of the request's location lists to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocationList {
    Locations,
    Shape,
    Trace,
    Sources,
    Targets,
    /// `exclude_locations`, read from this JSON key
    /// (`avoid_locations` is the deprecated name).
    ExcludeLocations(&'static str),
}

impl LocationList {
    fn key(self) -> &'static str {
        match self {
            LocationList::Locations => "/locations",
            LocationList::Shape => "/shape",
            LocationList::Trace => "/trace",
            LocationList::Sources => "/sources",
            LocationList::Targets => "/targets",
            LocationList::ExcludeLocations(key) => key,
        }
    }

    fn list(self, options: &mut Options) -> &mut Vec<Location> {
        match self {
            LocationList::Locations => &mut options.locations,
            LocationList::Shape => &mut options.shape,
            LocationList::Trace => &mut options.trace,
            LocationList::Sources => &mut options.sources,
            LocationList::Targets => &mut options.targets,
            LocationList::ExcludeLocations(_) => &mut options.exclude_locations,
        }
    }
}

/// Why a location could not be parsed.
enum LocationError {
    /// A specific mainline error, passed on as is.
    Valhalla(ValhallaError),
    /// Anything else, reported with the list's generic parse error.
    Invalid,
}

/// Mainline's `Location_Type_Enum_Parse`.
fn parse_location_type(name: &str) -> Option<LocationType> {
    Some(match name {
        "break" => LocationType::KBreak,
        "through" => LocationType::KThrough,
        "break_through" => LocationType::KBreakThrough,
        "via" => LocationType::KVia,
        _ => return None,
    })
}

/// Mainline's `PreferredSide_Enum_Parse`.
fn parse_preferred_side(name: &str) -> Option<PreferredSide> {
    Some(match name {
        "either" => PreferredSide::Either,
        "same" => PreferredSide::Same,
        "opposite" => PreferredSide::Opposite,
        _ => return None,
    })
}

/// Mainline's `RoadClass_Enum_Parse`.
pub(crate) fn parse_road_class(name: &str) -> Option<RoadClass> {
    Some(match name {
        "motorway" => RoadClass::KMotorway,
        "trunk" => RoadClass::KTrunk,
        "primary" => RoadClass::KPrimary,
        "secondary" => RoadClass::KSecondary,
        "tertiary" => RoadClass::KTertiary,
        "unclassified" => RoadClass::KUnclassified,
        "residential" => RoadClass::KResidential,
        "service_other" => RoadClass::KServiceOther,
        _ => return None,
    })
}

/// `midgard::circular_range_clamp(value, -180, 180)`.
///
/// Like mainline's, this subtracts whole turns counted as an `int`, so NaN stays NaN.
pub(crate) fn clamp_longitude(value: f64) -> f64 {
    const LOWER: f64 = -180.0;
    const UPPER: f64 = 180.0;
    const RANGE: f64 = UPPER - LOWER;
    if (LOWER..=UPPER).contains(&value) {
        return value;
    }
    let without_turns = |d: f64| d - f64::from(double_to_i32(d / RANGE)) * RANGE;
    if value < LOWER {
        UPPER - without_turns(LOWER - value)
    } else {
        LOWER + without_turns(value - UPPER)
    }
}

fn ll_lat(ll: Option<&LatLng>) -> Option<f64> {
    ll?.has_lat.map(|HasLat::Lat(lat)| lat)
}

fn ll_lng(ll: Option<&LatLng>) -> Option<f64> {
    ll?.has_lng.map(|HasLng::Lng(lng)| lng)
}

fn lat_lng(lat: f64, lng: f64) -> LatLng {
    LatLng {
        has_lat: Some(HasLat::Lat(lat)),
        has_lng: Some(HasLng::Lng(lng)),
    }
}

/// One location (mainline's `parse_location`).
///
/// `r_loc` is the location's JSON, or null for a PBF request.
#[expect(
    clippy::cast_sign_loss,
    reason = "mainline stores signed JSON values in unsigned fields the same way"
)]
#[expect(
    clippy::too_many_lines,
    reason = "this follows mainline's function option by option, which is easier to check as one"
)]
fn parse_location(
    location: &mut Location,
    r_loc: &JsonValue,
    action: i32,
    warnings: &mut Vec<CodedDescription>,
    ignore_closures: Option<bool>,
    is_last: bool,
) -> Result<(), LocationError> {
    let lat = ll_lat(location.ll.as_ref())
        .or_else(|| r_loc.get::<f64>("/lat"))
        .ok_or(LocationError::Invalid)?;
    // Note that NaN passes this check, as it does in mainline.
    if lat < -90.0 || lat > 90.0 {
        return Err(LocationError::Invalid);
    }
    let lon = ll_lng(location.ll.as_ref())
        .or_else(|| r_loc.get::<f64>("/lon"))
        .ok_or(LocationError::Invalid)?;
    location.ll = Some(lat_lng(lat, clamp_longitude(lon)));

    // Trace attributes does not support legs or breaks at discontinuities.
    let stop_type = r_loc.get::<String>("/type");
    if action == Action::TraceAttributes as i32 {
        location.r#type = LocationType::KVia.into();
    } else if let Some(stop_type) = stop_type {
        // An unknown type is a break.
        location.r#type = parse_location_type(&stop_type)
            .unwrap_or(LocationType::KBreak)
            .into();
    } else if action == Action::TraceRoute as i32 && location.has_time.is_none() {
        location.r#type = LocationType::KVia.into();
    }

    if let Some(name) = r_loc.get::<String>("/name") {
        location.name = name;
    }
    if let Some(street) = r_loc.get::<String>("/street") {
        location.street = street;
    }
    if let Some(date_time) = r_loc.get::<String>("/date_time") {
        location.date_time = date_time;
    }
    if let Some(heading) = r_loc.get::<i32>("/heading") {
        location.has_heading = Some(HasHeading::Heading(heading as u32));
    }
    if let Some(tolerance) = r_loc.get::<i32>("/heading_tolerance") {
        location.has_heading_tolerance =
            Some(HasHeadingTolerance::HeadingTolerance(tolerance as u32));
    }
    if let Some(layer) = r_loc.get::<i32>("/preferred_layer") {
        location.has_preferred_layer = Some(HasPreferredLayer::PreferredLayer(layer));
    }
    if let Some(tolerance) = r_loc.get::<f32>("/node_snap_tolerance") {
        // A float stored in an unsigned field, converted as mainline's x86-64 builds do.
        location.has_node_snap_tolerance = Some(HasNodeSnapTolerance::NodeSnapTolerance(
            double_to_u32(f64::from(tolerance)),
        ));
    }
    if let Some(reachability) = r_loc.get::<u32>("/minimum_reachability") {
        location.has_minimum_reachability =
            Some(HasMinimumReachability::MinimumReachability(reachability));
    }
    if let Some(radius) = r_loc.get::<u32>("/radius") {
        location.has_radius = Some(HasRadius::Radius(radius));
    }
    if let Some(accuracy) = r_loc.get::<u32>("/accuracy") {
        location.has_accuracy = Some(HasAccuracy::Accuracy(accuracy));
    }
    let time = match location.has_time {
        Some(HasTime::Time(time)) => time,
        None => -1.0,
    };
    location.has_time = Some(HasTime::Time(r_loc.get_or("/time", time)));
    location.skip_ranking_candidates =
        !r_loc.get_or("/rank_candidates", !location.skip_ranking_candidates);
    if let Some(side) = r_loc
        .get::<String>("/preferred_side")
        .as_deref()
        .and_then(parse_preferred_side)
    {
        location.preferred_side = side.into();
    }

    let display_lat =
        ll_lat(location.display_ll.as_ref()).or_else(|| r_loc.get::<f64>("/display_lat"));
    let display_lon =
        ll_lng(location.display_ll.as_ref()).or_else(|| r_loc.get::<f64>("/display_lon"));
    location.display_ll = match (display_lat, display_lon) {
        (Some(lat), Some(lon)) if (-90.0..=90.0).contains(&lat) => {
            Some(lat_lng(lat, clamp_longitude(lon)))
        }
        _ => None,
    };

    if let Some(cutoff) = r_loc.get::<u32>("/search_cutoff") {
        location.has_search_cutoff = Some(HasSearchCutoff::SearchCutoff(cutoff));
    }
    if let Some(tolerance) = r_loc.get::<u32>("/street_side_tolerance") {
        location.has_street_side_tolerance =
            Some(HasStreetSideTolerance::StreetSideTolerance(tolerance));
    }
    if let Some(distance) = r_loc.get::<u32>("/street_side_max_distance") {
        location.has_street_side_max_distance =
            Some(HasStreetSideMaxDistance::StreetSideMaxDistance(distance));
    }
    if let Some(cutoff) = r_loc
        .get::<String>("/street_side_cutoff")
        .as_deref()
        .and_then(parse_road_class)
    {
        location.has_street_side_cutoff =
            Some(HasStreetSideCutoff::StreetSideCutoff(cutoff.into()));
    }

    let mut exclude_closures = None;
    if let Some(search_filter) = r_loc.pointer("/search_filter") {
        // JSON
        let filter = location.search_filter.get_or_insert_default();
        let min: String = search_filter.get_or("/min_road_class", "service_other".into());
        if let Some(min) = parse_road_class(&min) {
            filter.has_min_road_class = Some(HasMinRoadClass::MinRoadClass(min.into()));
        }
        let max: String = search_filter.get_or("/max_road_class", "motorway".into());
        if let Some(max) = parse_road_class(&max) {
            filter.has_max_road_class = Some(HasMaxRoadClass::MaxRoadClass(max.into()));
        }
        filter.exclude_tunnel = search_filter.get_or("/exclude_tunnel", false);
        filter.exclude_bridge = search_filter.get_or("/exclude_bridge", false);
        filter.exclude_toll = search_filter.get_or("/exclude_toll", false);
        filter.exclude_ramp = search_filter.get_or("/exclude_ramp", false);
        filter.exclude_ferry = search_filter.get_or("/exclude_ferry", false);
        filter.has_level = Some(HasLevel::Level(search_filter.get_or("/level", MAX_LEVEL)));
        exclude_closures = search_filter.get::<bool>("/exclude_closures");
    } else if let Some(filter) = location.search_filter.as_mut() {
        // PBF: drop road classes that are not valid.
        if let Some(HasMinRoadClass::MinRoadClass(min)) = filter.has_min_road_class
            && RoadClass::try_from(min).is_err()
        {
            filter.has_min_road_class = None;
        }
        if let Some(HasMaxRoadClass::MaxRoadClass(max)) = filter.has_max_road_class
            && RoadClass::try_from(max).is_err()
        {
            filter.has_max_road_class = None;
        }
        if let Some(HasExcludeClosures::ExcludeClosures(exclude)) = filter.has_exclude_closures {
            exclude_closures = Some(exclude);
        }
    }

    // Both may contradict each other, so mainline refuses them together.
    if ignore_closures.is_some() && exclude_closures.is_some() {
        return Err(LocationError::Valhalla(ErrorCode::ClosuresConflict.into()));
    }
    let filter: &mut SearchFilter = location.search_filter.get_or_insert_default();
    // `ignore_closures` takes precedence.
    filter.has_exclude_closures = Some(HasExcludeClosures::ExcludeClosures(
        ignore_closures.map_or(exclude_closures.unwrap_or(true), |ignore| !ignore),
    ));
    if filter.has_min_road_class.is_none() {
        filter.has_min_road_class = Some(HasMinRoadClass::MinRoadClass(
            RoadClass::KServiceOther.into(),
        ));
    }
    if filter.has_max_road_class.is_none() {
        filter.has_max_road_class =
            Some(HasMaxRoadClass::MaxRoadClass(RoadClass::KMotorway.into()));
    }
    if filter.has_level.is_none() {
        filter.has_level = Some(HasLevel::Level(MAX_LEVEL));
    }

    let waiting_secs: f32 = r_loc.get_or("/waiting", 0.0);
    let location_type = LocationType::try_from(location.r#type);
    if matches!(
        location_type,
        Ok(LocationType::KBreak | LocationType::KBreakThrough)
    ) {
        // Origins and destinations don't wait.
        let index = location
            .correlation
            .as_ref()
            .map_or(0, |c| c.original_index);
        location.waiting_secs = if index == 0 || is_last || waiting_secs < 0.0 {
            0.0
        } else {
            waiting_secs
        };
    } else if waiting_secs != 0.0 {
        add_warning(warnings, 203, "");
    }
    Ok(())
}

/// One of the request's location lists (mainline's `parse_locations`).
///
/// Returns whether any location had a `date_time`, including one pushed down from the request's.
///
/// # Errors
///
/// 143 if a location's `search_filter.exclude_closures` conflicts with `ignore_closures`,
/// and `parse_error` for anything else wrong with a location.
pub(crate) fn parse_locations(
    doc: &JsonValue,
    options: &mut Options,
    warnings: &mut Vec<CodedDescription>,
    list: LocationList,
    parse_error: ErrorCode,
    ignore_closures: Option<bool>,
) -> Result<bool, ValhallaError> {
    let action = options.action;
    let mut locations = core::mem::take(list.list(options));

    let mut filter_closures = true;
    let mut location_had_time = false;
    let result = (|| {
        if let Some(json_locations) = doc.get_array(list.key()) {
            let last_index = json_locations.len().checked_sub(1);
            for (index, r_loc) in json_locations.iter().enumerate() {
                let mut location = Location::default();
                location.correlation.get_or_insert_default().original_index =
                    u32::try_from(index).map_err(|_| LocationError::Invalid)?;
                parse_location(
                    &mut location,
                    r_loc,
                    action,
                    warnings,
                    ignore_closures,
                    Some(index) == last_index,
                )?;
                location_had_time |= !location.date_time.is_empty();
                filter_closures &= exclude_closures(&location);
                locations.push(location);
            }
        } else if !locations.is_empty() {
            // A PBF request.
            let last_index = locations.len() - 1;
            for (index, location) in locations.iter_mut().enumerate() {
                location.correlation.get_or_insert_default().original_index =
                    u32::try_from(index).map_err(|_| LocationError::Invalid)?;
                parse_location(
                    location,
                    &JsonValue::Null,
                    action,
                    warnings,
                    ignore_closures,
                    index == last_index,
                )?;
                location_had_time |= !location.date_time.is_empty();
                filter_closures &= exclude_closures(location);
            }
        }
        Ok(())
    })();
    *list.list(options) = locations;
    result.map_err(|error| match error {
        LocationError::Valhalla(error) => error,
        LocationError::Invalid => ValhallaError::new(parse_error),
    })?;

    let locations = list.list(options);
    if locations.is_empty() {
        return Ok(false);
    }
    // The first and last locations are breaks no matter what.
    if let Some(first) = locations.first_mut() {
        first.r#type = LocationType::KBreak.into();
    }
    if let Some(last) = locations.last_mut() {
        last.r#type = LocationType::KBreak.into();
    }

    // Push the request's date and time down into the locations.
    let mut had_date_time = location_had_time;
    if !location_had_time {
        had_date_time = add_date_to_locations(options, list);
    }

    // If any location lets closures through, the costings must too,
    // so that Loki's per-location search filter can decide.
    for costing in options.costings.values_mut() {
        costing.has_filter_closures = Some(HasFilterClosures::FilterClosures(filter_closures));
    }
    Ok(had_date_time)
}

fn exclude_closures(location: &Location) -> bool {
    matches!(
        location
            .search_filter
            .as_ref()
            .and_then(|f| f.has_exclude_closures),
        Some(HasExcludeClosures::ExcludeClosures(true))
    )
}

/// Pushes the request's date and time down into a list of locations
/// (mainline's `add_date_to_locations`), returning whether any location now has one.
pub(crate) fn add_date_to_locations(options: &mut Options, list: LocationList) -> bool {
    let date_time = options
        .has_date_time
        .clone()
        .map(|HasDateTime::DateTime(date_time)| date_time);
    let date_time_type = DateTimeType::try_from(options.date_time_type);
    let is_matrix = options.action == Action::SourcesToTargets as i32;
    let locations = list.list(options);
    if let Some(date_time) = date_time
        && !locations.is_empty()
    {
        if is_matrix {
            let node = if date_time_type == Ok(DateTimeType::ArriveBy) {
                LocationList::Targets
            } else {
                LocationList::Sources
            };
            if node == list {
                let value = if date_time_type == Ok(DateTimeType::Current) {
                    "current".into()
                } else {
                    date_time
                };
                for location in locations.iter_mut() {
                    location.date_time.clone_from(&value);
                }
            }
        } else {
            match date_time_type {
                Ok(DateTimeType::Current) => locations[0].date_time = "current".into(),
                Ok(DateTimeType::DepartAt) => locations[0].date_time = date_time,
                Ok(DateTimeType::ArriveBy) => {
                    if let Some(last) = locations.last_mut() {
                        last.date_time = date_time;
                    }
                }
                Ok(DateTimeType::Invariant) => {
                    for location in locations.iter_mut() {
                        location.date_time.clone_from(&date_time);
                    }
                }
                _ => {}
            }
        }
    }
    locations
        .iter()
        .any(|location| !location.date_time.is_empty())
}
