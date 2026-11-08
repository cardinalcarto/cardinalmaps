//! What every costing shares: mainline's `sif::DynamicCost` base class.
//!
//! [`DynamicCost`] holds the options common to all costings (penalties, factors,
//! the `ignore_*` and `exclude_*` flags, hierarchy limits, and user avoid edges)
//! and implements the checks and costs that mainline's base class provides:
//! access, exclusions, access restrictions (with time domains and local traffic exemptions),
//! complex turn restrictions, U-turn penalties, and the base transition cost.
//! The costings in this crate wrap one and add their own rules.

use alloc::borrow::Cow;
use std::collections::HashMap;

use enumset::EnumSet;
use valhalla_graphtile::graph_tile::{
    AccessRestriction, AccessRestrictionType, DirectedEdge, GraphTile, NodeInfo, NodeType,
    RestrictionType, SectionDecodingError, TimeDomain, TurnType,
};
use valhalla_graphtile::{Access, GraphId, RoadClass, RoadUse};
use valhalla_proto::avoid_edge::{HasId, HasPercentAlong};
use valhalla_proto::costing::options::{
    HasAlleyPenalty, HasBikeShareCost, HasBikeSharePenalty, HasClosureFactor,
    HasCountryCrossingCost, HasCountryCrossingPenalty, HasDestinationOnlyPenalty, HasFerryCost,
    HasFlowMask, HasGateCost, HasGatePenalty, HasIgnoreClosures, HasManeuverPenalty,
    HasPrivateAccessPenalty, HasRailFerryCost, HasRestrictionProbability, HasServiceFactor,
    HasServicePenalty, HasSpeedPenaltyFactor, HasTollBoothCost, HasTollBoothPenalty, HasTopSpeed,
    HasUseDistance, HasUseFerry, HasUseLivingStreets, HasUseRailFerry, HasUseTracks,
};
use valhalla_proto::costing::{HasFilterClosures, HasOptions, Options, Type as CostingType};
use valhalla_proto::{Costing as CostingProto, HierarchyLimits};

use crate::cost::{
    Cost, DISABLE_FIXED_SPEED, INV_MEDIAN_SPEED, InternalTurn, MAX_FERRY_PENALTY,
    SHORT_INTERNAL_LENGTH, TC_NAME_INCONSISTENT_UTURN, TC_UNFAVORABLE_PENCIL_POINT_UTURN,
    TC_UNFAVORABLE_UTURN, TravelMode,
};
use crate::datetime::{ConditionalTime, is_conditional_active_in};
use crate::label::{Label, LabelStore};

/// Reads a oneof option's value, or the protobuf default when it is unset,
/// as mainline's generated accessors do.
macro_rules! get {
    ($options:expr, $field:ident, $variant:path) => {
        match $options.$field {
            Some($variant(value)) => value,
            None => Default::default(),
        }
    };
}
pub(crate) use get;

/// The options of a costing, or empty options when there are none
/// (mainline's `costing.options()`, which returns the default message).
pub(crate) fn options_of(costing: &CostingProto) -> Cow<'_, Options> {
    match &costing.has_options {
        Some(HasOptions::Options(options)) => Cow::Borrowed(options),
        None => Cow::Owned(Options::default()),
    }
}

/// What Loki may rule out when it checks an edge without a predecessor
/// (mainline's `kDisallow*` masks for `DynamicCost::Allowed(edge, tile, mask)`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct DisallowMask(u16);

impl DisallowMask {
    /// Rule out nothing beyond the costing's own rules (`kDisallowNone`).
    pub const NONE: Self = Self(0);
    /// Rule out edges that start a complex restriction (`kDisallowStartRestriction`).
    pub const START_RESTRICTION: Self = Self(0x1);
    /// Rule out edges that end a complex restriction (`kDisallowEndRestriction`).
    pub const END_RESTRICTION: Self = Self(0x2);
    /// Rule out edges with simple turn restrictions (`kDisallowSimpleRestriction`).
    pub const SIMPLE_RESTRICTION: Self = Self(0x4);
    /// Rule out closed edges, even if the costing allows them (`kDisallowClosure`).
    pub const CLOSURE: Self = Self(0x8);
    /// Rule out shortcuts (`kDisallowShortcut`).
    pub const SHORTCUT: Self = Self(0x10);

    /// Whether every bit of `other` is set.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// A mask from mainline's bits (bits mainline doesn't define are kept and ignored).
    #[inline]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The raw bits, as mainline's mask.
    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl core::ops::BitOr for DisallowMask {
    type Output = Self;

    #[inline]
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The time at which an edge is reached, for timed restrictions
/// (mainline's `current_time` and `tz_index` arguments).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct TimeContext {
    /// Seconds since the Unix epoch, or 0 for a route without a time.
    ///
    /// Mainline passes its `TimeInfo::local_time` here.
    pub current_time: u64,
    /// The time zone index of the node the edge leaves (see [`crate::datetime::time_zone`]).
    pub tz_index: u32,
}

impl TimeContext {
    /// No time: timed restrictions are ignored (but recorded in
    /// [`EdgeAccess::restriction_idx`]).
    pub const NONE: Self = Self {
        current_time: 0,
        tz_index: 0,
    };

    /// A time in a time zone.
    #[inline]
    pub const fn new(current_time: u64, tz_index: u32) -> Self {
        Self {
            current_time,
            tz_index,
        }
    }

    #[inline]
    const fn has_time(self) -> bool {
        self.current_time != 0
    }
}

/// No timed restriction was seen (mainline's `kInvalidRestriction`).
pub const INVALID_RESTRICTION: u8 = u8::MAX;

/// What an access check passes back besides its answer
/// (mainline's `restriction_idx`, `destonly_access_restr_mask`, and `edge_destonly`
/// in/out arguments).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EdgeAccess {
    /// The index (among the edge's restrictions for the costing's modes)
    /// of the last timed or destination restriction seen,
    /// or [`INVALID_RESTRICTION`].
    /// Thor stores it on the label so trip building can report the restriction.
    pub restriction_idx: u8,
    /// Restriction types that local traffic is exempt from.
    ///
    /// Set it to the predecessor label's mask before the check;
    /// the check leaves the mask for the new label.
    pub destonly_access_restr_mask: u8,
    /// Whether the edge is destination-only,
    /// either by its flag or by an active conditional `destination` restriction.
    pub edge_destonly: bool,
}

impl EdgeAccess {
    /// The starting state for a check from a predecessor with the given exemption mask.
    #[inline]
    pub const fn new(pred_destonly_access_restr_mask: u8) -> Self {
        Self {
            restriction_idx: INVALID_RESTRICTION,
            destonly_access_restr_mask: pred_destonly_access_restr_mask,
            edge_destonly: false,
        }
    }
}

impl Default for EdgeAccess {
    #[inline]
    fn default() -> Self {
        Self::new(0)
    }
}

/// A user's cost factor over part of an edge (mainline's `cost_edge_t`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct CostRange {
    start: f64,
    end: f64,
    factor: f64,
}

/// The cost factors for one edge (mainline's `custom_cost_t`).
#[derive(Clone, Debug, PartialEq)]
struct CustomCost {
    ranges: Vec<CostRange>,
    avg_factor: f64,
    ignore_restrictions: bool,
}

impl CustomCost {
    /// Sorts the ranges, computes the average factor, and returns the smallest factor
    /// (mainline's `sort_and_find_smallest`).
    fn sort_and_find_smallest(&mut self) -> f64 {
        if self.ranges.is_empty() {
            return 1.0;
        }
        // std::sort is not stable, but ranges with equal starts only differ
        // in their order of summation below, which mainline leaves unspecified too.
        self.ranges.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut uncovered = 1.0;
        let mut avg = 0.0;
        let mut min_factor: f64 = 1.0;
        for range in &self.ranges {
            uncovered -= range.end - range.start;
            avg += (range.end - range.start) * range.factor;
            min_factor = min_factor.min(range.factor);
        }
        avg += uncovered * 1.0;
        self.avg_factor = avg.max(f64::EPSILON);
        min_factor.max(f64::EPSILON)
    }
}

/// The routing graph's hierarchy levels (mainline's `TileHierarchy::levels()`).
const HIERARCHY_LEVEL_COUNT: u32 = 3;

/// Unlimited upward transitions (`kUnlimitedTransitions`).
pub const UNLIMITED_TRANSITIONS: u32 = u32::MAX;

/// Whether expansion on a hierarchy level should stop: its upward transitions are used up
/// and the search is farther than `dist` meters from the destination
/// (mainline's `StopExpanding(limits, dist)`).
#[inline]
pub fn stop_expanding(limits: &HierarchyLimits, dist: f32) -> bool {
    limits.up_transition_count > limits.max_up_transitions && dist > limits.expand_within_dist
}

/// Whether expansion on a hierarchy level should stop, for bidirectional searches
/// where the distance doesn't matter (mainline's `StopExpanding(limits)`).
#[inline]
pub fn stop_expanding_bidirectional(limits: &HierarchyLimits) -> bool {
    limits.up_transition_count > limits.max_up_transitions
}

/// The predecessor fields that the base transition cost reads,
/// which both labels (forward) and opposing edges (reverse) provide.
pub(crate) trait TransitionPred {
    fn pred_use(&self) -> RoadUse;
    fn pred_toll(&self) -> bool;
    fn pred_destonly(&self) -> bool;
    fn pred_classification(&self) -> RoadClass;
}

/// A [`Label`] as a [`TransitionPred`].
pub(crate) struct LabelPred<'a, L>(pub(crate) &'a L);

impl<L: Label> TransitionPred for LabelPred<'_, L> {
    #[inline]
    fn pred_use(&self) -> RoadUse {
        self.0.road_use()
    }
    #[inline]
    fn pred_toll(&self) -> bool {
        self.0.toll()
    }
    #[inline]
    fn pred_destonly(&self) -> bool {
        self.0.destonly()
    }
    #[inline]
    fn pred_classification(&self) -> RoadClass {
        self.0.classification()
    }
}

impl TransitionPred for DirectedEdge {
    #[inline]
    fn pred_use(&self) -> RoadUse {
        road_use(self)
    }
    #[inline]
    fn pred_toll(&self) -> bool {
        self.toll()
    }
    #[inline]
    fn pred_destonly(&self) -> bool {
        self.dest_only()
    }
    #[inline]
    fn pred_classification(&self) -> RoadClass {
        self.classification()
    }
}

/// An edge's use, with [`RoadUse::Other`] for values Valhalla doesn't define
/// (which a parsed tile never has).
#[inline]
pub(crate) fn road_use(edge: &DirectedEdge) -> RoadUse {
    edge.road_use().unwrap_or(RoadUse::Other)
}

/// A node's type, with [`NodeType::StreetIntersection`] for values Valhalla doesn't define
/// (which a parsed tile never has).
#[inline]
pub(crate) fn node_type(node: &NodeInfo) -> NodeType {
    node.node_type().unwrap_or(NodeType::StreetIntersection)
}

/// The turn type onto `edge` from local index `idx` (always defined for indexes up to 7).
#[inline]
pub(crate) fn turn_type(edge: &DirectedEdge, idx: u8) -> TurnType {
    edge.turn_type(idx).unwrap_or(TurnType::Straight)
}

/// The stop impact onto `edge` from local index `idx` (always defined for indexes up to 7).
#[inline]
pub(crate) fn stop_impact(edge: &DirectedEdge, idx: u8) -> u8 {
    edge.stop_impact(idx).unwrap_or(0)
}

/// Whether a mask of local edge indexes contains `idx`.
#[inline]
pub(crate) const fn has_bit(mask: u8, idx: u8) -> bool {
    idx < 8 && mask & (1 << idx) != 0
}

/// The access bits a costing checks, as mainline's `uint32_t` access mask.
#[inline]
fn bits(access: EnumSet<Access>) -> u16 {
    access.as_repr()
}

/// What a costing configures in [`DynamicCost::new`] beyond the options.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BaseSetup {
    pub(crate) mode: TravelMode,
    pub(crate) access_mask: EnumSet<Access>,
    pub(crate) penalize_uturns: bool,
}

/// The state and logic every costing shares (mainline's `sif::DynamicCost`).
///
/// Costings build one from the request's options and wrap it;
/// see [`crate::Costing`] for the full interface.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mainline's flags, read on hot paths"
)]
pub struct DynamicCost {
    pub(crate) pass: u32,
    pub(crate) allow_transit_connections: bool,
    pub(crate) allow_destination_only: bool,
    pub(crate) project_on_bss_connection: bool,
    pub(crate) travel_mode: TravelMode,
    pub(crate) access_mask: EnumSet<Access>,
    pub(crate) hierarchy_limits: Vec<HierarchyLimits>,
    /// User avoid edges by raw graph ID, with the percent along of the avoided location.
    user_exclude_edges: HashMap<u64, f32>,

    pub(crate) ferry_factor: f32,
    pub(crate) rail_ferry_factor: f32,
    pub(crate) track_factor: f32,
    pub(crate) living_street_factor: f32,
    pub(crate) service_factor: f32,
    pub(crate) closure_factor: f32,
    pub(crate) unlit_factor: f32,
    pub(crate) speed_penalty_factor: f32,

    pub(crate) country_crossing_cost: Cost,
    pub(crate) gate_cost: Cost,
    pub(crate) private_access_cost: Cost,
    pub(crate) toll_booth_cost: Cost,
    pub(crate) ferry_transition_cost: Cost,
    pub(crate) bike_share_cost: Cost,
    pub(crate) rail_ferry_transition_cost: Cost,

    pub(crate) maneuver_penalty: f32,
    pub(crate) alley_penalty: f32,
    pub(crate) destination_only_penalty: f32,
    pub(crate) living_street_penalty: f32,
    pub(crate) track_penalty: f32,
    pub(crate) service_penalty: f32,

    pub(crate) flow_mask: u8,
    pub(crate) restriction_probability: u8,
    pub(crate) shortest: bool,
    pub(crate) distance_factor: f32,
    pub(crate) inv_distance_factor: f32,

    pub(crate) ignore_restrictions: bool,
    pub(crate) ignore_non_vehicular_restrictions: bool,
    pub(crate) ignore_turn_restrictions: bool,
    pub(crate) ignore_oneways: bool,
    pub(crate) ignore_access: bool,
    pub(crate) ignore_closures: bool,
    pub(crate) ignore_construction: bool,
    pub(crate) top_speed: u32,
    pub(crate) fixed_speed: u32,
    pub(crate) filter_closures: bool,
    pub(crate) penalize_uturns: bool,

    pub(crate) exclude_unpaved: bool,
    pub(crate) exclude_bridges: bool,
    pub(crate) exclude_tunnels: bool,
    pub(crate) exclude_tolls: bool,
    pub(crate) exclude_highways: bool,
    pub(crate) exclude_ferries: bool,
    pub(crate) has_excludes: bool,
    pub(crate) default_hierarchy_limits: bool,
    pub(crate) use_hierarchy_limits: bool,
    pub(crate) exclude_cash_only_tolls: bool,

    pub(crate) include_hot: bool,
    pub(crate) include_hov2: bool,
    pub(crate) include_hov3: bool,
    pub(crate) is_hgv: bool,

    /// User cost factors by raw graph ID.
    linear_cost_edges: HashMap<u64, CustomCost>,
    pub(crate) min_linear_cost_factor: f64,
}

// Track and living street preferences (`set_use_tracks` and friends in dynamiccost.cc).
const MAX_TRACK_PENALTY: f32 = 300.0;
const MIN_TRACK_FACTOR: f32 = 0.8;
const MAX_TRACK_FACTOR: f32 = 4.0;
const MAX_LIVING_STREET_PENALTY: f32 = 500.0;
const MIN_LIVING_STREET_FACTOR: f32 = 0.8;
const MAX_LIVING_STREET_FACTOR: f32 = 3.0;
const MIN_LIT_FACTOR: f32 = 1.0;
const DEFAULT_CLOSURE_FACTOR: f32 = 9.0;
const DEFAULT_SPEED_PENALTY_FACTOR: f32 = 0.05;
/// Mainline's `kDefaultFlowMask`.
pub(crate) const DEFAULT_FLOW_MASK: u8 = 15;
/// Mainline's `kCurrentFlowMask`.
pub(crate) const CURRENT_FLOW_MASK: u8 = 8;

/// `static_cast<uint32_t>` of a float, as x86-64 does it for in-range values
/// (negative and NaN values become 0 here; mainline leaves them undefined).
#[inline]
pub(crate) fn float_to_u32(value: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "truncation towards zero is what the C++ conversion does"
    )]
    let converted = value as u32;
    converted
}

impl DynamicCost {
    /// The base state for a costing (mainline's `DynamicCost` constructor
    /// followed by `get_base_costs`, which every costing calls first).
    pub(crate) fn new(costing: &CostingProto, setup: BaseSetup) -> Self {
        let o = options_of(costing);
        let use_distance: f32 = get!(o, has_use_distance, HasUseDistance::UseDistance);
        let ignore_closures: bool = get!(o, has_ignore_closures, HasIgnoreClosures::IgnoreClosures);
        let filter_closures = matches!(
            costing.has_filter_closures,
            Some(HasFilterClosures::FilterClosures(true))
        );

        // User hierarchy limits for the levels that have them; sentinels for the rest
        // (Thor clamps them to its configuration).
        let hierarchy_limits = (0..HIERARCHY_LEVEL_COUNT)
            .map(|level| match o.hierarchy_limits.get(&level) {
                Some(limits) => HierarchyLimits {
                    up_transition_count: 0,
                    ..*limits
                },
                None => HierarchyLimits {
                    up_transition_count: 0,
                    max_up_transitions: UNLIMITED_TRANSITIONS,
                    expand_within_dist: f32::MAX,
                },
            })
            .collect();

        let mut base = Self {
            pass: 0,
            allow_transit_connections: false,
            allow_destination_only: true,
            project_on_bss_connection: false,
            travel_mode: setup.mode,
            access_mask: setup.access_mask,
            hierarchy_limits,
            user_exclude_edges: HashMap::new(),
            ferry_factor: 1.0,
            rail_ferry_factor: 1.0,
            track_factor: 1.0,
            living_street_factor: 1.0,
            service_factor: 1.0,
            closure_factor: DEFAULT_CLOSURE_FACTOR,
            unlit_factor: 1.0,
            speed_penalty_factor: DEFAULT_SPEED_PENALTY_FACTOR,
            country_crossing_cost: Cost::ZERO,
            gate_cost: Cost::ZERO,
            private_access_cost: Cost::ZERO,
            toll_booth_cost: Cost::ZERO,
            ferry_transition_cost: Cost::ZERO,
            bike_share_cost: Cost::ZERO,
            rail_ferry_transition_cost: Cost::ZERO,
            maneuver_penalty: 0.0,
            alley_penalty: 0.0,
            destination_only_penalty: 0.0,
            living_street_penalty: 0.0,
            track_penalty: 0.0,
            service_penalty: 0.0,
            flow_mask: DEFAULT_FLOW_MASK,
            restriction_probability: 0,
            shortest: o.shortest,
            distance_factor: use_distance * INV_MEDIAN_SPEED,
            inv_distance_factor: 1.0 - use_distance,
            ignore_restrictions: o.ignore_restrictions,
            ignore_non_vehicular_restrictions: o.ignore_non_vehicular_restrictions,
            ignore_turn_restrictions: o.ignore_restrictions || o.ignore_non_vehicular_restrictions,
            ignore_oneways: o.ignore_oneways,
            ignore_access: o.ignore_access,
            ignore_closures,
            ignore_construction: o.ignore_construction,
            // Set from the options with the fixed speed below.
            top_speed: 0,
            fixed_speed: o.fixed_speed,
            filter_closures: !ignore_closures && filter_closures,
            penalize_uturns: setup.penalize_uturns,
            exclude_unpaved: false,
            exclude_bridges: false,
            exclude_tunnels: false,
            exclude_tolls: false,
            exclude_highways: false,
            exclude_ferries: false,
            has_excludes: false,
            default_hierarchy_limits: true,
            use_hierarchy_limits: true,
            exclude_cash_only_tolls: false,
            include_hot: false,
            include_hov2: false,
            include_hov3: false,
            is_hgv: costing.r#type == i32::from(CostingType::Truck),
            linear_cost_edges: HashMap::new(),
            min_linear_cost_factor: 1.0,
        };
        base.set_cost_factor_edges(&o);
        base.get_base_costs(&o);
        base
    }

    /// Mainline's `get_base_costs`.
    fn get_base_costs(&mut self, o: &Options) {
        self.alley_penalty = get!(o, has_alley_penalty, HasAlleyPenalty::AlleyPenalty);
        self.destination_only_penalty = get!(
            o,
            has_destination_only_penalty,
            HasDestinationOnlyPenalty::DestinationOnlyPenalty
        );
        self.maneuver_penalty = get!(o, has_maneuver_penalty, HasManeuverPenalty::ManeuverPenalty);
        let probability: u32 = get!(
            o,
            has_restriction_probability,
            HasRestrictionProbability::RestrictionProbability
        );
        // Mainline stores the option in a uint8_t.
        self.restriction_probability = probability.to_le_bytes()[0];

        let toll_booth_cost: f32 = get!(o, has_toll_booth_cost, HasTollBoothCost::TollBoothCost);
        let toll_booth_penalty: f32 = get!(
            o,
            has_toll_booth_penalty,
            HasTollBoothPenalty::TollBoothPenalty
        );
        self.toll_booth_cost = Cost::new(toll_booth_cost + toll_booth_penalty, toll_booth_cost);
        let crossing_cost: f32 = get!(
            o,
            has_country_crossing_cost,
            HasCountryCrossingCost::CountryCrossingCost
        );
        let crossing_penalty: f32 = get!(
            o,
            has_country_crossing_penalty,
            HasCountryCrossingPenalty::CountryCrossingPenalty
        );
        self.country_crossing_cost = Cost::new(crossing_cost + crossing_penalty, crossing_cost);
        let gate_cost: f32 = get!(o, has_gate_cost, HasGateCost::GateCost);
        let gate_penalty: f32 = get!(o, has_gate_penalty, HasGatePenalty::GatePenalty);
        self.gate_cost = Cost::new(gate_cost + gate_penalty, gate_cost);
        let private_access_penalty: f32 = get!(
            o,
            has_private_access_penalty,
            HasPrivateAccessPenalty::PrivateAccessPenalty
        );
        self.private_access_cost = Cost::new(gate_cost + private_access_penalty, gate_cost);
        let bss_cost: f32 = get!(o, has_bike_share_cost, HasBikeShareCost::BikeShareCost);
        let bss_penalty: f32 = get!(
            o,
            has_bike_share_penalty,
            HasBikeSharePenalty::BikeSharePenalty
        );
        self.bike_share_cost = Cost::new(bss_cost + bss_penalty, bss_cost);

        let ferry_cost: f32 = get!(o, has_ferry_cost, HasFerryCost::FerryCost);
        let (ferry_penalty, ferry_factor) =
            ferry_preference(get!(o, has_use_ferry, HasUseFerry::UseFerry));
        self.ferry_factor = ferry_factor;
        self.ferry_transition_cost = Cost::new(ferry_cost + ferry_penalty, ferry_cost);
        let rail_ferry_cost: f32 = get!(o, has_rail_ferry_cost, HasRailFerryCost::RailFerryCost);
        let (rail_ferry_penalty, rail_ferry_factor) =
            ferry_preference(get!(o, has_use_rail_ferry, HasUseRailFerry::UseRailFerry));
        self.rail_ferry_factor = rail_ferry_factor;
        self.rail_ferry_transition_cost =
            Cost::new(rail_ferry_cost + rail_ferry_penalty, rail_ferry_cost);

        self.set_use_tracks(get!(o, has_use_tracks, HasUseTracks::UseTracks));
        self.set_use_living_streets(get!(
            o,
            has_use_living_streets,
            HasUseLivingStreets::UseLivingStreets
        ));
        self.set_use_lit(o.use_lit);

        self.service_penalty = get!(o, has_service_penalty, HasServicePenalty::ServicePenalty);
        self.service_factor = get!(o, has_service_factor, HasServiceFactor::ServiceFactor);
        self.closure_factor = get!(o, has_closure_factor, HasClosureFactor::ClosureFactor);
        self.speed_penalty_factor = get!(
            o,
            has_speed_penalty_factor,
            HasSpeedPenaltyFactor::SpeedPenaltyFactor
        );

        let flow_mask: u32 = get!(o, has_flow_mask, HasFlowMask::FlowMask);
        // Mainline stores the option in a uint8_t.
        self.flow_mask = flow_mask.to_le_bytes()[0];
        self.fixed_speed = o.fixed_speed;
        let top_speed: f32 = get!(o, has_top_speed, HasTopSpeed::TopSpeed);
        self.top_speed = if self.fixed_speed == DISABLE_FIXED_SPEED {
            float_to_u32(top_speed)
        } else {
            self.fixed_speed
        };

        self.exclude_unpaved = o.exclude_unpaved;
        self.exclude_bridges = o.exclude_bridges;
        self.exclude_tunnels = o.exclude_tunnels;
        self.exclude_tolls = o.exclude_tolls;
        self.exclude_highways = o.exclude_highways;
        self.exclude_ferries = o.exclude_ferries;
        self.has_excludes = self.exclude_bridges
            || self.exclude_tunnels
            || self.exclude_tolls
            || self.exclude_highways
            || self.exclude_ferries;
        self.exclude_cash_only_tolls = o.exclude_cash_only_tolls;
        self.default_hierarchy_limits = o.hierarchy_limits.is_empty();
    }

    /// Mainline's `set_use_tracks`.
    pub(crate) fn set_use_tracks(&mut self, use_tracks: f32) {
        self.track_penalty = if use_tracks < 0.5 {
            MAX_TRACK_PENALTY * (1.0 - 2.0 * use_tracks)
        } else {
            0.0
        };
        self.track_factor = if use_tracks < 0.5 {
            MAX_TRACK_FACTOR - 2.0 * use_tracks * (MAX_TRACK_FACTOR - 1.0)
        } else {
            MIN_TRACK_FACTOR + 2.0 * (1.0 - use_tracks) * (1.0 - MIN_TRACK_FACTOR)
        };
    }

    /// Mainline's `set_use_living_streets`.
    pub(crate) fn set_use_living_streets(&mut self, use_living_streets: f32) {
        self.living_street_penalty = if use_living_streets < 0.5 {
            MAX_LIVING_STREET_PENALTY * (1.0 - 2.0 * use_living_streets)
        } else {
            0.0
        };
        self.living_street_factor = if use_living_streets < 0.5 {
            MAX_LIVING_STREET_FACTOR - 2.0 * use_living_streets * (MAX_LIVING_STREET_FACTOR - 1.0)
        } else {
            MIN_LIVING_STREET_FACTOR
                + 2.0 * (1.0 - use_living_streets) * (1.0 - MIN_LIVING_STREET_FACTOR)
        };
    }

    /// Mainline's `set_use_lit`.
    pub(crate) fn set_use_lit(&mut self, use_lit: f32) {
        self.unlit_factor = if use_lit < 0.5 {
            MIN_LIT_FACTOR + 2.0 * use_lit
        } else {
            (MIN_LIT_FACTOR - 5.0) + 12.0 * use_lit
        };
    }

    /// Ingests the user's avoid edges and per-edge cost factors
    /// (mainline's `SetCostFactorEdges`).
    ///
    /// The costing's constructor calls this with its own options;
    /// call it again if cost factor edges were resolved later,
    /// before the first [`astar_cost_factor`](crate::Costing::astar_cost_factor).
    pub fn set_cost_factor_edges(&mut self, options: &Options) {
        self.linear_cost_edges.clear();
        self.min_linear_cost_factor = 1.0;

        for edge in &options.exclude_edges {
            let id = match edge.has_id {
                Some(HasId::Id(id)) => id,
                None => 0,
            };
            let percent_along = match edge.has_percent_along {
                Some(HasPercentAlong::PercentAlong(p)) => p,
                None => 0.0,
            };
            self.user_exclude_edges.entry(id).or_insert(percent_along);
        }

        for e in &options.cost_factor_edges {
            // A factor of 0 avoids the edge.
            if e.factor == 0.0 {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "mainline stores the start in a float"
                )]
                self.user_exclude_edges
                    .entry(e.id)
                    .or_insert(e.start as f32);
                continue;
            }
            let cost_edge = self
                .linear_cost_edges
                .entry(e.id)
                .or_insert_with(|| CustomCost {
                    ranges: Vec::new(),
                    avg_factor: 1.0,
                    ignore_restrictions: false,
                });
            cost_edge.ranges.push(CostRange {
                start: e.start,
                end: e.end,
                factor: e.factor,
            });
            cost_edge.ignore_restrictions = e.ignore_access_restrictions;
        }

        for cost_factors in self.linear_cost_edges.values_mut() {
            self.min_linear_cost_factor = self
                .min_linear_cost_factor
                .min(cost_factors.sort_and_find_smallest());
        }
    }

    /// Adds edges to avoid, each with the percent along of the avoided location
    /// (mainline's `AddUserAvoidEdges`).
    pub fn add_user_avoid_edges(&mut self, edges: impl IntoIterator<Item = (GraphId, f32)>) {
        for (id, percent_along) in edges {
            self.user_exclude_edges
                .entry(id.value())
                .or_insert(percent_along);
        }
    }

    /// Whether the user asked to avoid the edge (mainline's `IsUserAvoidEdge`).
    #[inline]
    pub fn is_user_avoid_edge(&self, edge_id: GraphId) -> bool {
        !self.user_exclude_edges.is_empty()
            && self.user_exclude_edges.contains_key(&edge_id.value())
    }

    /// Whether to avoid the edge as an origin at `percent_along`:
    /// the avoided location is ahead of the origin (mainline's `AvoidAsOriginEdge`).
    #[inline]
    pub fn avoid_as_origin_edge(&self, edge_id: GraphId, percent_along: f32) -> bool {
        self.user_exclude_edges
            .get(&edge_id.value())
            .is_some_and(|&avoid| avoid >= percent_along)
    }

    /// Whether to avoid the edge as a destination at `percent_along`:
    /// the avoided location is behind the destination (mainline's `AvoidAsDestinationEdge`).
    #[inline]
    pub fn avoid_as_destination_edge(&self, edge_id: GraphId, percent_along: f32) -> bool {
        self.user_exclude_edges
            .get(&edge_id.value())
            .is_some_and(|&avoid| avoid <= percent_along)
    }

    /// The user's average cost factor for an edge, or 1 (mainline's `EdgeFactor`).
    ///
    /// `None` (mainline's invalid graph ID) means no factor, as [`partial_edge_factor`]
    /// applies the factors itself.
    #[inline]
    pub(crate) fn edge_factor(&self, edge_id: Option<GraphId>) -> f64 {
        if self.linear_cost_edges.is_empty() {
            return 1.0;
        }
        edge_id
            .and_then(|id| self.linear_cost_edges.get(&id.value()))
            .map_or(1.0, |c| c.avg_factor)
    }

    /// The user's cost factor over a fraction of an edge, or 1 (mainline's `PartialEdgeFactor`).
    #[inline]
    pub fn partial_edge_factor(&self, edge_id: GraphId, start: f32, end: f32) -> f64 {
        #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
        if self.linear_cost_edges.is_empty() || start == end {
            return 1.0;
        }
        let Some(custom) = self.linear_cost_edges.get(&edge_id.value()) else {
            return 1.0;
        };
        let (start, end) = (f64::from(start), f64::from(end));
        let mut partial_factor = 0.0;
        let mut uncovered = 1.0;
        for range in &custom.ranges {
            if range.end <= start || range.start >= end {
                continue;
            }
            let fraction = (range.end - start.max(range.start)) / (end - start);
            partial_factor += fraction * range.factor;
            uncovered -= fraction;
        }
        partial_factor + uncovered
    }

    /// Whether the user's cost factors for the edge ignore its access restrictions.
    #[inline]
    fn ignores_restrictions(&self, edge_id: GraphId) -> bool {
        !self.linear_cost_edges.is_empty()
            && self
                .linear_cost_edges
                .get(&edge_id.value())
                .is_some_and(|c| c.ignore_restrictions)
    }

    /// The search pass (0 first; mainline's `pass`).
    #[inline]
    pub fn pass(&self) -> u32 {
        self.pass
    }

    /// Sets the search pass; later passes relax some rules (mainline's `set_pass`).
    #[inline]
    pub fn set_pass(&mut self, pass: u32) {
        self.pass = pass;
    }

    /// The access modes this costing checks (mainline's `access_mode`).
    #[inline]
    pub fn access_mode(&self) -> EnumSet<Access> {
        self.access_mask
    }

    /// The travel mode (mainline's `travel_mode`).
    #[inline]
    pub fn travel_mode(&self) -> TravelMode {
        self.travel_mode
    }

    /// Whether destination-only edges may be entered from edges that aren't
    /// (mainline's `set_allow_destination_only`; bidirectional searches turn it off).
    #[inline]
    pub fn set_allow_destination_only(&mut self, allow: bool) {
        self.allow_destination_only = allow;
    }

    /// Whether transit connections may be used (mainline's `SetAllowTransitConnections`).
    #[inline]
    pub fn set_allow_transit_connections(&mut self, allow: bool) {
        self.allow_transit_connections = allow;
    }

    /// Whether Loki may correlate locations to bike share connections
    /// (mainline's `set_project_on_bss_connection`).
    #[inline]
    pub fn set_project_on_bss_connection(&mut self, project: bool) {
        self.project_on_bss_connection = project;
    }

    /// Whether the costing is for heavy goods vehicles (mainline's `is_hgv`).
    #[inline]
    pub fn is_hgv(&self) -> bool {
        self.is_hgv
    }

    /// The speed sources the costing uses, as mainline's mask (`flow_mask`).
    #[inline]
    pub fn flow_mask(&self) -> u8 {
        self.flow_mask
    }

    /// The top speed, in km/h (the fixed speed when there is one).
    #[inline]
    pub fn top_speed(&self) -> u32 {
        self.top_speed
    }

    /// The fixed speed, in km/h, or 0 for none.
    #[inline]
    pub fn fixed_speed(&self) -> u32 {
        self.fixed_speed
    }

    /// The hierarchy limits by level (mainline's `GetHierarchyLimits`).
    #[inline]
    pub fn hierarchy_limits(&self) -> &[HierarchyLimits] {
        &self.hierarchy_limits
    }

    /// The hierarchy limits by level, for Thor to count transitions in.
    #[inline]
    pub fn hierarchy_limits_mut(&mut self) -> &mut [HierarchyLimits] {
        &mut self.hierarchy_limits
    }

    /// Replaces the hierarchy limits (mainline's `SetHierarchyLimits`).
    #[inline]
    pub fn set_hierarchy_limits(&mut self, limits: Vec<HierarchyLimits>) {
        self.hierarchy_limits = limits;
    }

    /// Relaxes the hierarchy limits for another pass (mainline's `RelaxHierarchyLimits`).
    ///
    /// Unlimited levels (as pedestrian and bicycle costings use) are left as they are.
    pub fn relax_hierarchy_limits(&mut self, using_bidirectional: bool) {
        let relax_factor: f32 = if using_bidirectional { 8.0 } else { 16.0 };
        let expansion_within_factor: f32 = if using_bidirectional { 2.0 } else { 4.0 };
        for limits in &mut self.hierarchy_limits {
            if limits.max_up_transitions != UNLIMITED_TRANSITIONS {
                #[expect(clippy::cast_precision_loss, reason = "mainline multiplies as a float")]
                let relaxed = limits.max_up_transitions as f32 * relax_factor;
                limits.max_up_transitions = float_to_u32(relaxed);
                limits.expand_within_dist *= expansion_within_factor;
            }
        }
    }

    /// Whether the request gave no hierarchy limits (mainline's `DefaultHierarchyLimits`).
    #[inline]
    pub fn default_hierarchy_limits(&self) -> bool {
        self.default_hierarchy_limits
    }

    /// Records whether the hierarchy limits are the defaults
    /// (mainline's `SetDefaultHierarchyLimits`).
    #[inline]
    pub fn set_default_hierarchy_limits(&mut self, default: bool) {
        self.default_hierarchy_limits = default;
    }

    /// Whether path search should apply hierarchy limits at all
    /// (mainline's `UseHierarchyLimits`; false for pedestrians and bicycles).
    #[inline]
    pub fn use_hierarchy_limits(&self) -> bool {
        self.use_hierarchy_limits
    }

    /// Whether the edge is closed by live traffic (mainline's `IsClosed`).
    ///
    /// Valinor has no live traffic, so no edge is ever closed
    /// (mainline's check with no traffic tile).
    #[inline]
    pub fn is_closed(&self, _edge: &DirectedEdge) -> bool {
        let live_closure = false;
        !self.ignore_closures && (self.flow_mask & CURRENT_FLOW_MASK) != 0 && live_closure
    }

    /// Whether Loki should filter out closed edges (mainline's `filter_closures_`).
    ///
    /// Without live traffic nothing is closed, so this changes nothing here.
    #[inline]
    pub fn filter_closures(&self) -> bool {
        self.filter_closures
    }

    /// A rough time estimate in seconds for `distance_meters` at the top or fixed speed
    /// (mainline's `BeeLineTimeEstimate`).
    pub fn bee_line_time_estimate(&self, distance_meters: f64, factor: f64) -> u32 {
        const KPH_TO_METERS_PER_SEC: f64 = 1000.0 / 3600.0;
        let estimate = if self.fixed_speed == DISABLE_FIXED_SPEED {
            (distance_meters / (f64::from(self.top_speed) * KPH_TO_METERS_PER_SEC)) * factor
        } else {
            distance_meters / (f64::from(self.fixed_speed) * KPH_TO_METERS_PER_SEC) * factor * 0.85
        };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the C++ conversion truncates"
        )]
        let estimate = estimate as u32;
        estimate
    }

    // Access

    /// Mainline's `IsAccessible`: forward access for the mode, or anything when ignoring access,
    /// or reverse access when ignoring oneways, or construction when ignoring it.
    #[inline]
    pub fn is_accessible(&self, edge: &DirectedEdge) -> bool {
        let forward = bits(edge.forward_access());
        let mask = bits(self.access_mask);
        (forward & mask) != 0
            || (self.ignore_access && forward != 0)
            || (self.ignore_oneways && (bits(edge.reverse_access()) & mask) != 0)
            || (self.ignore_construction && road_use(edge) == RoadUse::Construction)
    }

    /// Mainline's base `Allowed(edge, tile, disallow_mask)`, Loki's edge filter
    /// before each costing adds its own rules.
    #[inline]
    pub(crate) fn base_edge_filter(&self, edge: &DirectedEdge, disallow: DisallowMask) -> bool {
        let mask = if self.ignore_access {
            EnumSet::all()
        } else {
            self.access_mask
        };
        let mask = bits(mask);
        let accessible = (bits(edge.forward_access()) & mask) != 0
            || (self.ignore_oneways && (bits(edge.reverse_access()) & mask) != 0);
        let assumed_restricted = (disallow.contains(DisallowMask::START_RESTRICTION)
            && !edge.start_restriction().is_empty())
            || (disallow.contains(DisallowMask::END_RESTRICTION)
                && !edge.end_restriction().is_empty())
            || (disallow.contains(DisallowMask::SIMPLE_RESTRICTION)
                && !edge.restrictions().is_empty())
            || (disallow.contains(DisallowMask::SHORTCUT) && edge.is_shortcut());
        accessible
            && !assumed_restricted
            && (road_use(edge) != RoadUse::Construction || self.ignore_construction)
    }

    /// Mainline's base `Allowed(node)`: node access for the mode (or ignoring access),
    /// and no cash-only toll when those are excluded.
    #[inline]
    pub fn node_allowed(&self, node: &NodeInfo) -> bool {
        ((bits(node.access()) & bits(self.access_mask)) != 0 || self.ignore_access)
            && !(self.exclude_cash_only_tolls && node.cash_only_toll())
    }

    /// Mainline's `CheckExclusions<FORWARD>`: whether the edge enters an excluded
    /// bridge, tunnel, toll, highway, or ferry (in the direction of travel)
    /// from an edge that isn't one.
    #[inline]
    pub(crate) fn check_exclusions<L: Label>(
        &self,
        forward: bool,
        edge: &DirectedEdge,
        pred: &L,
    ) -> bool {
        let drive_onto = |condition: bool, pred_condition: bool| {
            forward == condition && pred_condition != condition
        };
        let is_ferry = |u: RoadUse| matches!(u, RoadUse::Ferry | RoadUse::RailFerry);
        self.has_excludes
            && ((self.exclude_bridges && drive_onto(edge.is_bridge(), pred.bridge()))
                || (self.exclude_tunnels && drive_onto(edge.is_tunnel(), pred.tunnel()))
                || (self.exclude_tolls && drive_onto(edge.toll(), pred.toll()))
                || (self.exclude_highways
                    && drive_onto(
                        edge.classification() == RoadClass::Motorway,
                        pred.classification() == RoadClass::Motorway,
                    ))
                || (self.exclude_ferries
                    && drive_onto(is_ferry(road_use(edge)), is_ferry(pred.road_use()))))
    }

    /// The access restrictions of an edge that apply to `access_mode`, in tile order
    /// (mainline's `GetAccessRestrictions(edgeid.id(), access_mode)`).
    #[inline]
    fn restrictions_for<T: GraphTile>(
        tile: &T,
        edge_id: GraphId,
        access_mode: EnumSet<Access>,
    ) -> impl Iterator<Item = &AccessRestriction> {
        // Edge indexes within a tile fit in 21 bits.
        let index = u32::try_from(edge_id.feature_index()).unwrap_or(u32::MAX);
        tile.edge_access_restrictions(index)
            .iter()
            .filter(move |r| !r.affected_access_modes().is_disjoint(access_mode))
    }

    /// Mainline's `GetExemptedAccessRestrictions`: the restriction types the edge's
    /// local traffic is exempt from, and whether the edge is destination-only
    /// (by its flag or an active conditional `destination` restriction).
    ///
    /// Thor calls this for the origin and destination edges of a search.
    pub fn exempted_access_restrictions<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        edge_id: GraphId,
        time: TimeContext,
    ) -> (u8, bool) {
        let mut edge_destonly = if self.is_hgv {
            edge.dest_only_hgv()
        } else {
            edge.dest_only()
        };
        if self.ignore_restrictions || edge.access_restrictions().is_disjoint(self.access_mask) {
            return (0, edge_destonly);
        }
        let mut mask = 0;
        for restriction in Self::restrictions_for(tile, edge_id, self.access_mask) {
            let restriction_type = restriction.restriction_type().ok();
            if restriction_type == Some(AccessRestrictionType::DestinationAllowed)
                && time.has_time()
                && is_conditional_active_in(
                    ConditionalTime::from(TimeDomain::from_raw(restriction.value())),
                    time.current_time,
                    time.tz_index,
                )
            {
                edge_destonly = true;
            }
            if restriction.except_destination() && !self.allow_destination_only {
                mask |= restriction.mask();
            }
        }
        (mask, edge_destonly)
    }

    /// Mainline's `EvaluateRestrictions`: the edge's access restrictions for `access_mode`,
    /// at the given time, including local traffic exemptions and the costing's
    /// own restriction checks (`mode_allowed`, mainline's `ModeSpecificAllowed`).
    #[inline]
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    pub(crate) fn evaluate_restrictions<T: GraphTile>(
        &self,
        access_mode: EnumSet<Access>,
        edge: &DirectedEdge,
        tile: &T,
        edge_id: GraphId,
        time: TimeContext,
        access: &mut EdgeAccess,
        pred_destonly: bool,
        mode_allowed: impl Fn(&AccessRestriction) -> bool,
    ) -> bool {
        if self.ignore_restrictions || edge.access_restrictions().is_disjoint(access_mode) {
            return true;
        }
        if self.ignores_restrictions(edge_id) {
            return true;
        }

        let mut time_allowed = false;
        let mut tmp_mask = 0u8;
        for (i, restriction) in Self::restrictions_for(tile, edge_id, access_mode).enumerate() {
            let raw_type = restriction.raw_restriction_type();
            let access_type = AccessRestrictionType::try_from(raw_type).ok();
            if !self.ignore_non_vehicular_restrictions
                && matches!(
                    access_type,
                    Some(
                        AccessRestrictionType::TimedAllowed
                            | AccessRestrictionType::TimedDenied
                            | AccessRestrictionType::DestinationAllowed
                    )
                )
            {
                // Mainline stores the index in a uint8_t.
                access.restriction_idx = i.to_le_bytes()[0];
                if access_type == Some(AccessRestrictionType::TimedAllowed) {
                    time_allowed = true;
                }
                if !time.has_time() {
                    // No time: ignore timed restrictions (but note them above).
                    continue;
                }
                let condition = ConditionalTime::from(TimeDomain::from_raw(restriction.value()));
                if is_conditional_active_in(condition, time.current_time, time.tz_index) {
                    match access_type {
                        Some(AccessRestrictionType::TimedAllowed) => {
                            access.destonly_access_restr_mask = tmp_mask;
                            return true;
                        }
                        Some(AccessRestrictionType::TimedDenied) => return false,
                        _ => {}
                    }
                    // An active conditional destination restriction makes the edge
                    // destination-only.
                    access.edge_destonly = true;
                    if !self.allow_destination_only && !pred_destonly {
                        return false;
                    }
                }
            }

            // Mainline checks the type against its 32-entry mask table;
            // types past it have no mask.
            if restriction.except_destination() && raw_type < 32 {
                let mask = restriction.mask();
                tmp_mask |= mask;
                if (access.destonly_access_restr_mask & mask) != 0 || self.allow_destination_only {
                    continue;
                }
            }

            if !mode_allowed(restriction) {
                return false;
            }
        }
        access.destonly_access_restr_mask = tmp_mask;

        // Timed allowed restrictions mean the edge is only usable at those times,
        // and no time matched.
        !time_allowed || !time.has_time()
    }

    /// Whether a complex restriction ends on (forward) or starts on (reverse) this edge
    /// along the path to `pred` (mainline's `DynamicCost::Restricted`).
    ///
    /// `labels` is where [`Label::predecessor`] indexes point.
    /// When a restriction triggers, `reset` is called with the edges of the restriction
    /// before the last (as mainline resets their edge status to unreached),
    /// so that other paths through them stay possible.
    ///
    /// # Errors
    ///
    /// Fails if the tile's complex restrictions are corrupt.
    #[expect(clippy::too_many_arguments, reason = "mainline's signature")]
    pub fn restricted<T: GraphTile, S: LabelStore + ?Sized>(
        &self,
        edge: &DirectedEdge,
        pred: &S::Label,
        labels: &S,
        tile: &T,
        edge_id: GraphId,
        forward: bool,
        time: TimeContext,
        mut reset: impl FnMut(GraphId),
    ) -> Result<bool, SectionDecodingError> {
        if self.ignore_turn_restrictions {
            return Ok(false);
        }
        let ends_here = if forward {
            edge.end_restriction()
        } else {
            edge.start_restriction()
        };
        if ends_here.is_disjoint(self.access_mask) {
            return Ok(false);
        }

        for restriction in tile.get_complex_restrictions(forward, edge_id, self.access_mask) {
            let restriction = restriction?;
            let rtype = restriction.restriction_type().ok();
            if matches!(
                rtype,
                Some(RestrictionType::NoProbable | RestrictionType::OnlyProbable)
            ) && (self.restriction_probability == 0
                || self.restriction_probability > restriction.probability())
            {
                continue;
            }

            // Walk the vias back from the predecessor;
            // at the origin label, mainline stays put.
            let mut current: &S::Label = pred;
            let mut matched = true;
            for via in restriction.vias() {
                if via.value() != current.edge_id().value() {
                    matched = false;
                    break;
                }
                if let Some(index) = current.predecessor() {
                    if let Some(label) = labels.label(index) {
                        current = label;
                    } else {
                        // A dangling index (mainline would read out of bounds).
                        matched = false;
                        break;
                    }
                }
            }
            if !matched {
                continue;
            }
            let end = if forward {
                restriction.raw_from_graph_id()
            } else {
                restriction.raw_to_graph_id()
            };
            if current.edge_id().value() != end {
                continue;
            }

            if time.has_time()
                && let Some(date_time) = restriction.date_time()
            {
                if is_conditional_active_in(
                    ConditionalTime::from(date_time),
                    time.current_time,
                    time.tz_index,
                ) {
                    Self::reset_restriction_edges(
                        pred,
                        labels,
                        restriction.vias().len(),
                        &mut reset,
                    );
                    return Ok(true);
                }
                continue;
            } else if !time.has_time() && restriction.has_date_time() {
                return Ok(false);
            }
            Self::reset_restriction_edges(pred, labels, restriction.vias().len(), &mut reset);
            return Ok(true);
        }
        Ok(false)
    }

    /// Calls `reset` with the edges that matched a restriction's vias:
    /// the predecessor and the labels before it, one per via.
    fn reset_restriction_edges<S: LabelStore + ?Sized>(
        pred: &S::Label,
        labels: &S,
        via_count: usize,
        reset: &mut impl FnMut(GraphId),
    ) {
        let mut current = Some(pred);
        for _ in 0..via_count {
            let Some(label) = current else { return };
            reset(label.edge_id());
            current = match label.predecessor() {
                None => Some(label),
                Some(index) => labels.label(index),
            };
        }
    }

    /// The internal turn made onto `edge` from the edge with local index `idx`
    /// (mainline's `TurnType`).
    ///
    /// Only costings that penalize U-turns (auto) report internal turns.
    /// In a reverse search, pass the opposing predecessor edge, whose turn type is read instead.
    #[inline]
    pub fn turn_type(
        &self,
        idx: u8,
        node: &NodeInfo,
        edge: &DirectedEdge,
        opp_pred_edge: Option<&DirectedEdge>,
    ) -> InternalTurn {
        if !self.penalize_uturns || !edge.is_intersection_internal() {
            return InternalTurn::NoTurn;
        }
        let turn = turn_type(opp_pred_edge.unwrap_or(edge), idx);
        let short = edge.length() <= SHORT_INTERNAL_LENGTH;
        if node.drive_on_right() {
            if short && matches!(turn, TurnType::SharpLeft | TurnType::Left) {
                return InternalTurn::LeftTurn;
            }
        } else if short && matches!(turn, TurnType::SharpRight | TurnType::Right) {
            return InternalTurn::RightTurn;
        }
        InternalTurn::NoTurn
    }

    /// Mainline's `AddUturnPenalty`.
    #[expect(
        clippy::too_many_arguments,
        clippy::fn_params_excessive_bools,
        reason = "mainline's signature"
    )]
    #[inline]
    pub(crate) fn add_uturn_penalty(
        idx: u8,
        node: &NodeInfo,
        edge: &DirectedEdge,
        has_reverse: bool,
        has_left: bool,
        has_right: bool,
        penalize_internal_uturns: bool,
        internal_turn: InternalTurn,
        seconds: &mut f32,
    ) {
        let name_consistent = edge.name_consistency().contains(idx);
        let turn = turn_type(edge, idx);
        let to_right = edge.edge_to_right().contains(idx);
        let to_left = edge.edge_to_left().contains(idx);
        if node.drive_on_right() {
            if has_reverse && !name_consistent {
                *seconds += TC_NAME_INCONSISTENT_UTURN;
            } else if has_reverse
                || (penalize_internal_uturns && internal_turn == InternalTurn::LeftTurn && has_left)
            {
                *seconds += TC_UNFAVORABLE_UTURN;
            } else if turn == TurnType::SharpLeft
                && to_right
                && !to_left
                && edge.is_named()
                && name_consistent
            {
                *seconds *= TC_UNFAVORABLE_PENCIL_POINT_UTURN;
            }
        } else if has_reverse && !name_consistent {
            *seconds += TC_NAME_INCONSISTENT_UTURN;
        } else if has_reverse
            || (penalize_internal_uturns && internal_turn == InternalTurn::RightTurn && has_right)
        {
            *seconds += TC_UNFAVORABLE_UTURN;
        } else if turn == TurnType::SharpRight
            && !to_right
            && to_left
            && edge.is_named()
            && name_consistent
        {
            *seconds *= TC_UNFAVORABLE_PENCIL_POINT_UTURN;
        }
    }

    /// Mainline's `SpeedPenalty`, with no live traffic:
    /// a factor for edges faster than the top speed.
    #[inline]
    pub(crate) fn speed_penalty(&self, edge_speed: u32) -> f32 {
        // Mainline re-reads the speed without live traffic when live traffic was used,
        // which never happens here.
        if edge_speed > self.top_speed {
            #[expect(clippy::cast_precision_loss, reason = "speeds are small")]
            let excess = (edge_speed - self.top_speed) as f32;
            excess * self.speed_penalty_factor
        } else {
            0.0
        }
    }

    /// Mainline's `base_transition_cost` (in `DynamicCost`):
    /// country crossings, gates, private access, bike share, toll booths, and ferries
    /// (with time), and destination-only, alley, maneuver, living street, track,
    /// and service road penalties (cost only).
    #[inline]
    pub(crate) fn base_transition_cost<P: TransitionPred + ?Sized>(
        &self,
        node: &NodeInfo,
        edge: &DirectedEdge,
        pred: &P,
        idx: u8,
        edge_destonly: bool,
        pred_destonly: bool,
    ) -> Cost {
        let ntype = node_type(node);
        let edge_use = road_use(edge);
        let pred_use = pred.pred_use();
        let mut c = Cost::ZERO;
        c += self.country_crossing_cost * f32::from(u8::from(ntype == NodeType::BorderControl));
        c += self.gate_cost
            * f32::from(u8::from(ntype == NodeType::Gate))
            * f32::from(u8::from(!node.tagged_access()));
        c += self.private_access_cost
            * f32::from(u8::from(matches!(
                ntype,
                NodeType::Gate | NodeType::Bollard
            )))
            * f32::from(u8::from(node.private_access()));
        c += self.bike_share_cost * f32::from(u8::from(ntype == NodeType::BikeShare));
        c += self.toll_booth_cost
            * f32::from(u8::from(
                ntype == NodeType::TollBooth || (edge.toll() && !pred.pred_toll()),
            ));
        c += self.ferry_transition_cost
            * f32::from(u8::from(
                edge_use == RoadUse::Ferry && pred_use != RoadUse::Ferry,
            ));
        c += self.rail_ferry_transition_cost
            * f32::from(u8::from(
                edge_use == RoadUse::RailFerry && pred_use != RoadUse::RailFerry,
            ));

        let is_destonly = edge_destonly
            || if self.is_hgv {
                edge.dest_only_hgv()
            } else {
                edge.dest_only()
            };
        c.cost += self.destination_only_penalty
            * flag(is_destonly && !(pred_destonly || pred.pred_destonly()));
        c.cost +=
            self.alley_penalty * flag(edge_use == RoadUse::Alley && pred_use != RoadUse::Alley);
        c.cost +=
            self.maneuver_penalty * flag(!edge.is_link() && !edge.name_consistency().contains(idx));
        c.cost += self.living_street_penalty
            * flag(edge_use == RoadUse::LivingStreet && pred_use != RoadUse::LivingStreet);
        c.cost +=
            self.track_penalty * flag(edge_use == RoadUse::Track && pred_use != RoadUse::Track);
        c.cost += self.service_penalty
            * flag(edge_use == RoadUse::ServiceRoad && pred_use != RoadUse::ServiceRoad)
            * flag(!edge.is_intersection_internal());

        // Shortest ignores penalties in favor of length.
        c.cost *= flag(!self.shortest);
        c
    }
}

/// A boolean as 0 or 1, as C++ multiplies by one.
#[inline]
pub(crate) fn flag(value: bool) -> f32 {
    f32::from(u8::from(value))
}

/// The ferry penalty and factor for a `use_ferry` (or `use_rail_ferry`) preference
/// (from mainline's `get_base_costs`).
fn ferry_preference(use_ferry: f32) -> (f32, f32) {
    if use_ferry < 0.5 {
        // Mainline truncates the penalty to whole seconds.
        #[expect(clippy::cast_precision_loss, reason = "the penalty is at most 6 hours")]
        let penalty = float_to_u32(MAX_FERRY_PENALTY * (1.0 - use_ferry * 2.0)) as f32;
        (penalty, 10.0 - use_ferry * 18.0)
    } else {
        (0.0, 1.5 - use_ferry)
    }
}
