//! Which attributes trip building writes (mainline's `baldr::AttributesController`).
//!
//! Every attribute has a default (most are on); a request's `filter_attributes`
//! turn the named ones on (`filter_action: include`) or off (`exclude`),
//! and `elevation_interval` alone decides `edge.elevation`.
//! Names that aren't attributes are ignored, as mainline only logs them.
//!
//! The table lists mainline's attributes in its order.
//! Mainline's table names `edge.speed_type` twice (on, then off) and gives two pairs of
//! attributes the same name (`edge.freeflow_speed_forward` and `edge.freeflow_speed_backward`);
//! a C++ map built from such a list keeps the first entry of each name,
//! so `edge.speed_type` is on by default, and each repeated name is listed once here.

use valhalla_proto::FilterAction;
use valhalla_proto::Options;

/// Declares the attributes, their names, and their defaults, in mainline's order.
macro_rules! attributes {
    ($($attribute:ident = $name:literal => $default:literal,)*) => {
        /// An attribute trip building may write (mainline's `kEdge…`, `kNode…`, and other keys).
        ///
        /// Every attribute is listed, for the names a request can give,
        /// though trip building for routes reads only some of them.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(usize)]
        #[allow(dead_code, reason = "the names of attributes other actions write")]
        pub(crate) enum Attribute {
            $($attribute,)*
        }

        /// The name and default of each [`Attribute`], indexed by its discriminant.
        const ATTRIBUTES: &[(&str, bool)] = &[$(($name, $default),)*];
    };
}

attributes! {
    EdgeNames = "edge.names" => true,
    EdgeLength = "edge.length" => true,
    EdgeSpeed = "edge.speed" => true,
    EdgeSpeedType = "edge.speed_type" => true,
    EdgeSpeedsFaded = "edge.speeds_faded" => true,
    EdgeSpeedsNonFaded = "edge.speeds_non_faded" => true,
    EdgeRoadClass = "edge.road_class" => true,
    EdgeBeginHeading = "edge.begin_heading" => true,
    EdgeEndHeading = "edge.end_heading" => true,
    EdgeBeginShapeIndex = "edge.begin_shape_index" => true,
    EdgeEndShapeIndex = "edge.end_shape_index" => true,
    EdgeTraversability = "edge.traversability" => true,
    EdgeUse = "edge.use" => true,
    EdgeToll = "edge.toll" => true,
    EdgeUnpaved = "edge.unpaved" => true,
    EdgeTunnel = "edge.tunnel" => true,
    EdgeBridge = "edge.bridge" => true,
    EdgeRoundabout = "edge.roundabout" => true,
    EdgeInternalIntersection = "edge.internal_intersection" => true,
    EdgeDriveOnRight = "edge.drive_on_right" => true,
    EdgeSurface = "edge.surface" => true,
    EdgeSignExitNumber = "edge.sign.exit_number" => true,
    EdgeSignExitBranch = "edge.sign.exit_branch" => true,
    EdgeSignExitToward = "edge.sign.exit_toward" => true,
    EdgeSignExitName = "edge.sign.exit_name" => true,
    EdgeSignGuideBranch = "edge.sign.guide_branch" => true,
    EdgeSignGuideToward = "edge.sign.guide_toward" => true,
    EdgeSignJunctionName = "edge.sign.junction_name" => true,
    EdgeSignGuidanceViewJunction = "edge.sign.guidance_view_junction" => true,
    EdgeSignGuidanceViewSignboard = "edge.sign.guidance_view_signboard" => true,
    EdgeTravelMode = "edge.travel_mode" => true,
    EdgeVehicleType = "edge.vehicle_type" => true,
    EdgePedestrianType = "edge.pedestrian_type" => true,
    EdgeBicycleType = "edge.bicycle_type" => true,
    EdgeTransitType = "edge.transit_type" => true,
    EdgeTransitRouteInfoOnestopId = "edge.transit_route_info.onestop_id" => true,
    EdgeTransitRouteInfoBlockId = "edge.transit_route_info.block_id" => true,
    EdgeTransitRouteInfoTripId = "edge.transit_route_info.trip_id" => true,
    EdgeTransitRouteInfoShortName = "edge.transit_route_info.short_name" => true,
    EdgeTransitRouteInfoLongName = "edge.transit_route_info.long_name" => true,
    EdgeTransitRouteInfoHeadsign = "edge.transit_route_info.headsign" => true,
    EdgeTransitRouteInfoColor = "edge.transit_route_info.color" => true,
    EdgeTransitRouteInfoTextColor = "edge.transit_route_info.text_color" => true,
    EdgeTransitRouteInfoDescription = "edge.transit_route_info.description" => true,
    EdgeTransitRouteInfoOperatorOnestopId = "edge.transit_route_info.operator_onestop_id" => true,
    EdgeTransitRouteInfoOperatorName = "edge.transit_route_info.operator_name" => true,
    EdgeTransitRouteInfoOperatorUrl = "edge.transit_route_info.operator_url" => true,
    EdgeId = "edge.id" => true,
    EdgeWayId = "edge.way_id" => true,
    EdgeBeginOsmNodeId = "edge.begin_osm_node_id" => true,
    EdgeEndOsmNodeId = "edge.end_osm_node_id" => true,
    EdgeWeightedGrade = "edge.weighted_grade" => true,
    EdgeMaxUpwardGrade = "edge.max_upward_grade" => true,
    EdgeMaxDownwardGrade = "edge.max_downward_grade" => true,
    EdgeMeanElevation = "edge.mean_elevation" => true,
    EdgeLaneCount = "edge.lane_count" => true,
    EdgeLaneConnectivity = "edge.lane_connectivity" => true,
    EdgeCycleLane = "edge.cycle_lane" => true,
    EdgeBicycleNetwork = "edge.bicycle_network" => true,
    EdgeElevation = "edge.elevation" => false,
    EdgeSacScale = "edge.sac_scale" => true,
    EdgeShoulder = "edge.shoulder" => true,
    EdgeSidewalk = "edge.sidewalk" => true,
    EdgeDensity = "edge.density" => true,
    EdgeSpeedLimit = "edge.speed_limit" => true,
    EdgeConditionalSpeedLimits = "edge.conditional_speed_limits" => true,
    EdgeTruckSpeed = "edge.truck_speed" => true,
    EdgeTruckRoute = "edge.truck_route" => true,
    EdgeDefaultSpeed = "edge.default_speed" => true,
    EdgeDestinationOnly = "edge.destination_only" => true,
    EdgeDestinationOnlyHgv = "edge.destination_only_hgv" => false,
    EdgeIsUrban = "edge.is_urban" => false,
    EdgeTaggedValues = "edge.tagged_values" => true,
    EdgeIndoor = "edge.indoor" => true,
    EdgeLandmarks = "edge.landmarks" => true,
    EdgeCountryCrossing = "edge.country_crossing" => true,
    EdgeForward = "edge.forward" => true,
    EdgeLevels = "edge.levels" => true,
    EdgeTrafficSignal = "edge.traffic_signal" => true,
    EdgeHovType = "edge.hov_type" => false,
    EdgeRamp = "edge.ramp" => false,
    EdgeDismount = "edge.dismount" => false,
    EdgeUseSidepath = "edge.use_sidepath" => false,
    EdgeSidewalkLeft = "edge.sidewalk_left" => false,
    EdgeSidewalkRight = "edge.sidewalk_right" => false,
    EdgeBssConnection = "edge.bss_connection" => false,
    EdgeLit = "edge.lit" => false,
    EdgeNotThru = "edge.not_thru" => false,
    EdgePartComplexRestriction = "edge.part_of_complex_restriction" => false,
    EdgeLayer = "edge.layer" => false,
    EdgeShortcut = "edge.is_shortcut" => false,
    EdgeLeavesTile = "edge.leaves_tile" => false,
    EdgeCurvature = "edge.curvature" => false,
    EdgeSpeedFwd = "edge.speed_forward" => false,
    EdgeDeadendFwd = "edge.deadend_forward" => false,
    EdgeLaneCountFwd = "edge.lanecount_forward" => false,
    EdgeTruckSpeedFwd = "edge.truck_speed_forward" => false,
    EdgeSignalFwd = "edge.traffic_signal_forward" => false,
    EdgeStopSignFwd = "edge.stop_sign_forward" => false,
    EdgeYieldFwd = "edge.yield_sign_forward" => false,
    EdgeAccessFwd = "edge.access_forward" => false,
    EdgeLiveSpeedFwd = "edge.live_speed_forward" => false,
    // Also mainline's `kEdgeConstrainedSpeedFwd`.
    EdgeFreeflowSpeedFwd = "edge.freeflow_speed_forward" => false,
    EdgeSpeedBwd = "edge.speed_backward" => false,
    EdgeDeadendBwd = "edge.deadend_backward" => false,
    EdgeLaneCountBwd = "edge.lanecount_backward" => false,
    EdgeTruckSpeedBwd = "edge.truck_speed_backward" => false,
    EdgeSignalBwd = "edge.traffic_signal_backward" => false,
    EdgeStopSignBwd = "edge.stop_sign_backward" => false,
    EdgeYieldBwd = "edge.yield_sign_backward" => false,
    EdgeAccessBwd = "edge.access_backward" => false,
    EdgeLiveSpeedBwd = "edge.live_speed_backward" => false,
    // Also mainline's `kEdgeConstrainedSpeedBwd`.
    EdgeFreeflowSpeedBwd = "edge.freeflow_speed_backward" => false,
    Incidents = "incidents" => false,
    NodeIntersectingEdgeBeginHeading = "node.intersecting_edge.begin_heading" => true,
    NodeIntersectingEdgeFromEdgeNameConsistency =
        "node.intersecting_edge.from_edge_name_consistency" => true,
    NodeIntersectingEdgeToEdgeNameConsistency =
        "node.intersecting_edge.to_edge_name_consistency" => true,
    NodeIntersectingEdgeDriveability = "node.intersecting_edge.driveability" => true,
    NodeIntersectingEdgeCyclability = "node.intersecting_edge.cyclability" => true,
    NodeIntersectingEdgeWalkability = "node.intersecting_edge.walkability" => true,
    NodeIntersectingEdgeUse = "node.intersecting_edge.use" => true,
    NodeIntersectingEdgeRoadClass = "node.intersecting_edge.road_class" => true,
    NodeIntersectingEdgeLaneCount = "node.intersecting_edge.lane_count" => true,
    NodeIntersectingEdgeSignInfo = "node.intersecting_edge.sign_info" => true,
    NodeElapsedTime = "node.elapsed_time" => true,
    NodeAdminIndex = "node.admin_index" => true,
    NodeType = "node.type" => true,
    NodeTrafficSignal = "node.traffic_signal" => true,
    NodeFork = "node.fork" => true,
    NodeTransitPlatformInfoType = "node.transit_platform_info.type" => true,
    NodeTransitPlatformInfoOnestopId = "node.transit_platform_info.onestop_id" => true,
    NodeTransitPlatformInfoName = "node.transit_platform_info.name" => true,
    NodeTransitPlatformInfoStationOnestopId =
        "node.transit_platform_info.station_onestop_id" => true,
    NodeTransitPlatformInfoStationName = "node.transit_platform_info.station_name" => true,
    NodeTransitPlatformInfoArrivalDateTime =
        "node.transit_platform_info.arrival_date_time" => true,
    NodeTransitPlatformInfoDepartureDateTime =
        "node.transit_platform_info.departure_date_time" => true,
    NodeTransitPlatformInfoIsParentStop = "node.transit_platform_info.is_parent_stop" => true,
    NodeTransitPlatformInfoAssumedSchedule =
        "node.transit_platform_info.assumed_schedule" => true,
    NodeTransitPlatformInfoLatLon = "node.transit_platform_info.lat_lon" => true,
    NodeTransitStationInfoOnestopId = "node.transit_station_info.onestop_id" => true,
    NodeTransitStationInfoName = "node.transit_station_info.name" => true,
    NodeTransitStationInfoLatLon = "node.transit_station_info.lat_lon" => true,
    NodeTransitEgressInfoOnestopId = "node.transit_egress_info.onestop_id" => true,
    NodeTransitEgressInfoName = "node.transit_egress_info.name" => true,
    NodeTransitEgressInfoLatLon = "node.transit_egress_info.lat_lon" => true,
    NodeTimeZone = "node.time_zone" => true,
    NodeTransitionTime = "node.transition_time" => true,
    NodeDriveOnRight = "node.drive_on_right" => false,
    NodeElevation = "node.elevation" => false,
    NodeTaggedAccess = "node.tagged_access" => false,
    NodePrivateAccess = "node.private_access" => false,
    NodeCashOnlyToll = "node.cash_only_toll" => false,
    NodeModeChangeAllowed = "node.mode_change_allowed" => false,
    NodeNamedIntersection = "node.named_intersection" => false,
    NodeIsTransit = "node.is_transit" => false,
    NodeAccess = "node.access" => false,
    OsmChangeset = "osm_changeset" => true,
    AdminCountryCode = "admin.country_code" => true,
    AdminCountryText = "admin.country_text" => true,
    AdminStateCode = "admin.state_code" => true,
    AdminStateText = "admin.state_text" => true,
    Shape = "shape" => true,
    MatchedPoint = "matched.point" => true,
    MatchedType = "matched.type" => true,
    MatchedEdgeIndex = "matched.edge_index" => true,
    MatchedBeginRouteDiscontinuity = "matched.begin_route_discontinuity" => true,
    MatchedEndRouteDiscontinuity = "matched.end_route_discontinuity" => true,
    MatchedDistanceAlongEdge = "matched.distance_along_edge" => true,
    MatchedDistanceFromTracePoint = "matched.distance_from_trace_point" => true,
    ConfidenceScore = "confidence_score" => true,
    RawScore = "raw_score" => true,
    ShapeAttributesTime = "shape_attributes.time" => false,
    ShapeAttributesLength = "shape_attributes.length" => false,
    ShapeAttributesSpeed = "shape_attributes.speed" => false,
    ShapeAttributesSpeedLimit = "shape_attributes.speed_limit" => false,
    ShapeAttributesClosure = "shape_attributes.closure" => false,
    ShapeAttributesCongestion = "shape_attributes.congestion" => false,
    IncidentId = "incident.id" => false,
    IncidentType = "incident.type" => false,
    IncidentDescription = "incident.description" => false,
    IncidentSubType = "incident.sub_type" => false,
    IncidentSubTypeDescription = "incident.sub_type_description" => false,
    IncidentStartTime = "incident.start_time" => false,
    IncidentEndTime = "incident.end_time" => false,
    IncidentImpact = "incident.impact" => false,
    IncidentRoadClosed = "incident.road_closed" => false,
    IncidentCongestionValue = "incident.congestion_value" => false,
    IncidentCreationTime = "incident.creation_time" => false,
    IncidentLongDescription = "incident.long_description" => false,
    IncidentClearLanes = "incident.clear_lanes" => false,
    IncidentNumLanesBlocked = "incident.num_lanes_blocked" => false,
    IncidentLength = "incident.length" => false,
    IncidentIso31661Alpha2 = "incident.iso_3166_1_alpha2" => false,
    IncidentIso31661Alpha3 = "incident.iso_3166_1_alpha3" => false,
}

/// A category of attributes: the attributes whose names start with it
/// (mainline's `kNodeCategory`, `kAdminCategory`, and so on).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Category {
    Admin,
    ShapeAttributes,
}

impl Category {
    const fn prefix(self) -> &'static str {
        match self {
            Self::Admin => "admin.",
            Self::ShapeAttributes => "shape_attributes.",
        }
    }
}

/// The attributes a request turns on (mainline's `AttributesController(options)`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AttributesController {
    enabled: Vec<bool>,
}

impl Default for AttributesController {
    /// The defaults (mainline's `AttributesController()`).
    fn default() -> Self {
        Self {
            enabled: ATTRIBUTES.iter().map(|&(_, default)| default).collect(),
        }
    }
}

impl AttributesController {
    /// The attributes a request asks for, with a non-strict filter
    /// (`AttributesController(options)`, as Thor builds it for routes).
    pub(crate) fn new(options: &Options) -> Self {
        let mut controller = Self::default();
        let value = match FilterAction::try_from(options.filter_action) {
            Ok(FilterAction::Include) => Some(true),
            Ok(FilterAction::Exclude) => Some(false),
            _ => None,
        };
        if let Some(value) = value {
            for name in &options.filter_attributes {
                if let Some(index) = ATTRIBUTES.iter().position(|&(n, _)| n == name)
                    && let Some(slot) = controller.enabled.get_mut(index)
                {
                    *slot = value;
                }
            }
        }
        controller.set(Attribute::EdgeElevation, options.elevation_interval > 0.0);
        controller
    }

    fn set(&mut self, attribute: Attribute, value: bool) {
        if let Some(slot) = self.enabled.get_mut(attribute as usize) {
            *slot = value;
        }
    }

    /// Whether an attribute is on (mainline's `controller(key)`).
    #[inline]
    pub(crate) fn get(&self, attribute: Attribute) -> bool {
        self.enabled
            .get(attribute as usize)
            .copied()
            .unwrap_or(false)
    }

    /// Whether any attribute of a category is on (`category_attribute_enabled`).
    pub(crate) fn category_enabled(&self, category: Category) -> bool {
        let prefix = category.prefix();
        ATTRIBUTES
            .iter()
            .zip(&self.enabled)
            .any(|(&(name, _), &on)| on && name.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(action: FilterAction, attributes: &[&str]) -> Options {
        Options {
            filter_action: action.into(),
            filter_attributes: attributes.iter().map(|&a| a.to_owned()).collect(),
            ..Options::default()
        }
    }

    #[test]
    fn names_are_unique_and_match_the_enum() {
        let mut names: Vec<&str> = ATTRIBUTES.iter().map(|&(n, _)| n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ATTRIBUTES.len());
        assert_eq!(
            ATTRIBUTES.len(),
            Attribute::IncidentIso31661Alpha3 as usize + 1
        );
        assert_eq!(ATTRIBUTES[Attribute::Shape as usize].0, "shape");
        assert_eq!(
            ATTRIBUTES[Attribute::EdgeSpeedType as usize].0,
            "edge.speed_type"
        );
    }

    #[test]
    fn defaults_and_filters_as_mainline() {
        let defaults = AttributesController::new(&Options::default());
        assert!(defaults.get(Attribute::EdgeNames));
        assert!(defaults.get(Attribute::EdgeSpeedType));
        assert!(!defaults.get(Attribute::EdgeElevation));
        assert!(!defaults.get(Attribute::ShapeAttributesSpeed));
        assert!(defaults.category_enabled(Category::Admin));
        assert!(!defaults.category_enabled(Category::ShapeAttributes));

        // Ferrostar's annotations.
        let ferrostar = AttributesController::new(&options(
            FilterAction::Include,
            &[
                "shape_attributes.speed",
                "shape_attributes.speed_limit",
                "shape_attributes.time",
                "shape_attributes.length",
                "not.an.attribute",
            ],
        ));
        assert!(ferrostar.get(Attribute::ShapeAttributesSpeed));
        assert!(ferrostar.get(Attribute::ShapeAttributesLength));
        assert!(!ferrostar.get(Attribute::ShapeAttributesClosure));
        assert!(ferrostar.category_enabled(Category::ShapeAttributes));
        // A non-strict include keeps the defaults.
        assert!(ferrostar.get(Attribute::EdgeNames));

        let exclude = AttributesController::new(&options(
            FilterAction::Exclude,
            &[
                "admin.country_code",
                "admin.country_text",
                "admin.state_code",
                "admin.state_text",
                "edge.names",
            ],
        ));
        assert!(!exclude.category_enabled(Category::Admin));
        assert!(!exclude.get(Attribute::EdgeNames));

        // Elevation only follows the interval, even when it's included.
        let mut elevation = options(FilterAction::Include, &["edge.elevation"]);
        assert!(!AttributesController::new(&elevation).get(Attribute::EdgeElevation));
        elevation.elevation_interval = 30.0;
        assert!(AttributesController::new(&elevation).get(Attribute::EdgeElevation));
    }
}
