//! Bicycle costing (mainline's `BicycleCost` in `src/sif/bicyclecost.cc`).

use enumset::{EnumSet, enum_set};
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{Access, GraphId, RoadUse, Surface};
use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::options::{
    HasAvoidBadSurfaces, HasCyclingSpeed, HasTransportType, HasUseHills, HasUseRoads,
};

use crate::Costing;
use crate::cost::{Cost, InternalTurn, MAX_SPEED_KPH, TravelMode, speed_factor};
use crate::costing::{CostingError, EdgeCostResult, partial_from_full};
use crate::dynamic::{
    BaseSetup, DisallowMask, DynamicCost, EdgeAccess, LabelPred, TimeContext, TransitionPred,
    UNLIMITED_TRANSITIONS, float_to_u32, get, has_bit, options_of, road_use, stop_impact,
    turn_type,
};
use crate::label::Label;

// Turn costs, modified by the stop impact (`kTC*` in bicyclecost.cc).
const TC_STRAIGHT: f32 = 0.15;
const TC_FAVORABLE_SLIGHT: f32 = 0.2;
const TC_FAVORABLE: f32 = 0.3;
const TC_FAVORABLE_SHARP: f32 = 0.5;
const TC_CROSSING: f32 = 0.75;
const TC_UNFAVORABLE_SLIGHT: f32 = 0.4;
const TC_UNFAVORABLE: f32 = 1.0;
const TC_UNFAVORABLE_SHARP: f32 = 1.5;
const TC_REVERSE: f32 = 5.0;

const RIGHT_SIDE_TURN_COSTS: [f32; 8] = [
    TC_STRAIGHT,
    TC_FAVORABLE_SLIGHT,
    TC_FAVORABLE,
    TC_FAVORABLE_SHARP,
    TC_REVERSE,
    TC_UNFAVORABLE_SHARP,
    TC_UNFAVORABLE,
    TC_UNFAVORABLE_SLIGHT,
];
const LEFT_SIDE_TURN_COSTS: [f32; 8] = [
    TC_STRAIGHT,
    TC_UNFAVORABLE_SLIGHT,
    TC_UNFAVORABLE,
    TC_UNFAVORABLE_SHARP,
    TC_REVERSE,
    TC_FAVORABLE_SHARP,
    TC_FAVORABLE,
    TC_FAVORABLE_SLIGHT,
];

// Turn stress penalties (`kTP*`).
const TP_STRAIGHT: f32 = 0.0;
const TP_FAVORABLE_SLIGHT: f32 = 0.25;
const TP_FAVORABLE: f32 = 0.75;
const TP_FAVORABLE_SHARP: f32 = 1.0;
const TP_UNFAVORABLE_SLIGHT: f32 = 0.75;
const TP_UNFAVORABLE: f32 = 1.75;
const TP_UNFAVORABLE_SHARP: f32 = 2.25;
const TP_REVERSE: f32 = 4.0;

const RIGHT_SIDE_TURN_PENALTIES: [f32; 8] = [
    TP_STRAIGHT,
    TP_FAVORABLE_SLIGHT,
    TP_FAVORABLE,
    TP_FAVORABLE_SHARP,
    TP_REVERSE,
    TP_UNFAVORABLE_SHARP,
    TP_UNFAVORABLE,
    TP_UNFAVORABLE_SLIGHT,
];
const LEFT_SIDE_TURN_PENALTIES: [f32; 8] = [
    TP_STRAIGHT,
    TP_UNFAVORABLE_SLIGHT,
    TP_UNFAVORABLE,
    TP_UNFAVORABLE_SHARP,
    TP_REVERSE,
    TP_FAVORABLE_SHARP,
    TP_FAVORABLE,
    TP_FAVORABLE_SLIGHT,
];

/// Extra stress on designated truck routes (`kTruckStress`).
const TRUCK_STRESS: f32 = 0.5;
/// The cost factor for steps (`kBicycleStepsFactor`).
const BICYCLE_STEPS_FACTOR: f32 = 8.0;
/// The speed when dismounted (`kDismountSpeed`), which mainline converts to 5 km/h.
const DISMOUNT_SPEED: f32 = 5.1;

const ROAD_SURFACE_SPEED_FACTORS: [f32; 8] = [1.0, 1.0, 0.9, 0.6, 0.5, 0.3, 0.2, 0.0];
const HYBRID_SURFACE_SPEED_FACTORS: [f32; 8] = [1.0, 1.0, 1.0, 0.8, 0.6, 0.4, 0.25, 0.0];
const CROSS_SURFACE_SPEED_FACTORS: [f32; 8] = [1.0, 1.0, 1.0, 0.8, 0.7, 0.5, 0.4, 0.0];
const MOUNTAIN_SURFACE_SPEED_FACTORS: [f32; 8] = [1.0, 1.0, 1.0, 1.0, 0.9, 0.75, 0.55, 0.0];

/// Surface factors past the worst unpenalized surface (`kSurfaceFactors`).
const SURFACE_FACTORS: [f32; 4] = [1.0, 2.5, 4.5, 7.0];

/// Road class stress (`kRoadClassFactor`).
const ROAD_CLASS_FACTOR: [f32; 8] = [1.0, 0.4, 0.2, 0.1, 0.05, 0.05, 0.0, 0.5];

/// Speed factors by weighted grade (`kGradeBasedSpeedFactor`).
const GRADE_BASED_SPEED_FACTOR: [f32; 16] = [
    2.2, 2.0, 1.9, 1.7, 1.4, 1.2, 1.0, 0.95, 0.85, 0.75, 0.65, 0.55, 0.5, 0.45, 0.4, 0.3,
];

/// Turn penalty factors by shoulder and cycle lane (`kCycleLaneTransitionFactor`).
const CYCLE_LANE_TRANSITION_FACTOR: [f32; 8] = [1.0, 0.5, 0.25, 0.1, 0.4, 0.5, 0.25, 0.1];

/// The turn penalty factor onto a living street (`kLivingStreetTransitionFactor`).
const LIVING_STREET_TRANSITION_FACTOR: f32 = 0.15;

/// How strongly to avoid each weighted grade (`kAvoidHillsStrength`).
const AVOID_HILLS_STRENGTH: [f32; 16] = [
    2.0, 1.0, 0.5, 0.2, 0.1, 0.0, 0.05, 0.1, 0.3, 0.8, 2.0, 3.0, 4.5, 6.5, 10.0, 12.0,
];

/// Edge speed above which roads are penalized, before `use_roads` (`kSpeedPenaltyThreshold`).
const SPEED_PENALTY_THRESHOLD: u32 = 40;

/// How much to favor bicycle networks (`kBicycleNetworkFactor`).
const BICYCLE_NETWORK_FACTOR: f32 = 0.95;

/// Bike share rental cost and penalty (`kDefaultBssCost` and `kDefaultBssPenalty`).
const DEFAULT_BSS_COST: f32 = 120.0;
const DEFAULT_BSS_PENALTY: f32 = 0.0;

/// The kind of bicycle (mainline's `BicycleType`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BicycleType {
    Road = 0,
    /// A cyclocross bike: a road bike with wider tires.
    Cross = 1,
    /// A hybrid or city bike (the default).
    Hybrid = 2,
    Mountain = 3,
}

impl BicycleType {
    /// The worst surface each type may ride (`kWorstAllowedSurface`).
    const fn worst_allowed_surface(self) -> Surface {
        match self {
            Self::Road => Surface::Compacted,
            Self::Cross => Surface::Gravel,
            Self::Hybrid => Surface::Dirt,
            Self::Mountain => Surface::Path,
        }
    }

    const fn surface_speed_factors(self) -> &'static [f32; 8] {
        match self {
            Self::Road => &ROAD_SURFACE_SPEED_FACTORS,
            Self::Hybrid => &HYBRID_SURFACE_SPEED_FACTORS,
            Self::Cross => &CROSS_SURFACE_SPEED_FACTORS,
            Self::Mountain => &MOUNTAIN_SURFACE_SPEED_FACTORS,
        }
    }
}

/// Bicycle costing (mainline's `BicycleCost`): distance-based costs with preferences
/// for bike infrastructure, speeds by bike type, surface, and grade.
#[derive(Clone, Debug)]
pub struct BicycleCost {
    base: DynamicCost,
    use_roads: f32,
    avoid_roads: f32,
    road_factor: f32,
    sidepath_factor: f32,
    livingstreet_factor: f32,
    track_factor: f32,
    avoid_bad_surfaces: f32,
    speed: f32,
    bicycle_type: BicycleType,
    minimal_surface_penalized: Surface,
    worst_allowed_surface: Surface,
    cyclelane_factor: [f32; 8],
    path_cyclelane_factor: [f32; 4],
    speedpenalty: [f32; MAX_SPEED_KPH as usize + 1],
    speed_penalty_threshold: u32,
    grade_penalty: [f32; 16],
}

impl BicycleCost {
    /// Bicycle costing from a request's options (mainline's `CreateBicycleCost`).
    #[expect(
        clippy::too_many_lines,
        reason = "follows mainline's constructor field by field"
    )]
    pub fn new(costing: &CostingProto) -> Self {
        let mut base = DynamicCost::new(
            costing,
            BaseSetup {
                mode: TravelMode::Bicycle,
                access_mask: enum_set!(Access::Bicycle),
                penalize_uturns: false,
            },
        );
        for limits in &mut base.hierarchy_limits {
            limits.max_up_transitions = UNLIMITED_TRANSITIONS;
        }
        base.use_hierarchy_limits = false;

        let o = options_of(costing);
        let transport_type = match &o.has_transport_type {
            Some(HasTransportType::TransportType(t)) => t.as_str(),
            None => "",
        };
        let bicycle_type = match transport_type {
            "cross" => BicycleType::Cross,
            "road" => BicycleType::Road,
            "mountain" => BicycleType::Mountain,
            _ => BicycleType::Hybrid,
        };

        let speed: f32 = get!(o, has_cycling_speed, HasCyclingSpeed::CyclingSpeed);
        let avoid_bad_surfaces: f32 = get!(
            o,
            has_avoid_bad_surfaces,
            HasAvoidBadSurfaces::AvoidBadSurfaces
        );
        let minimal_surface_penalized = bicycle_type.worst_allowed_surface();
        #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
        let worst_allowed_surface = if avoid_bad_surfaces == 1.0 {
            minimal_surface_penalized
        } else {
            Surface::Path
        };

        let use_roads: f32 = get!(o, has_use_roads, HasUseRoads::UseRoads);
        let avoid_roads = 1.0 - use_roads;
        let road_factor = if use_roads >= 0.5 {
            1.5 - use_roads
        } else {
            2.0 - use_roads * 2.0
        };
        let cyclelane_factor = [
            1.0,
            0.9 + use_roads * 0.05,
            0.4 + use_roads * 0.45,
            0.15 + use_roads * 0.6,
            0.7 + use_roads * 0.2,
            0.9 + use_roads * 0.05,
            0.4 + use_roads * 0.45,
            0.15 + use_roads * 0.6,
        ];
        let path_cyclelane_factor = [
            0.2 + use_roads,
            0.2 + use_roads,
            0.1 + use_roads * 0.9,
            use_roads * 0.8,
        ];
        let speed_penalty_threshold = SPEED_PENALTY_THRESHOLD + float_to_u32(use_roads * 30.0);

        // Mainline mixes doubles into these: `0.25` and `50.0` are double literals.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "assigned to a float in mainline"
        )]
        let penalty_avoid_roads = (f64::from((1.0 - use_roads) * 0.75) + 0.25) as f32;
        let mut speedpenalty = [0.0; MAX_SPEED_KPH as usize + 1];
        for (s, penalty) in speedpenalty.iter_mut().enumerate().skip(1) {
            #[expect(clippy::cast_precision_loss, reason = "speeds are at most 252")]
            let sf = s as f32;
            let base_pen: f32 = if s <= 40 {
                sf / 40.0
            } else if s <= 65 {
                (sf / 25.0) - 0.6
            } else {
                #[expect(clippy::cast_possible_truncation, reason = "assigned to a float")]
                let pen = (f64::from(sf) / 50.0 + f64::from(0.7f32)) as f32;
                pen
            };
            *penalty = (base_pen - 1.0) * penalty_avoid_roads + 1.0;
        }

        let use_hills: f32 = get!(o, has_use_hills, HasUseHills::UseHills);
        let avoid_hills = 1.0 - use_hills;
        let mut grade_penalty = [0.0; 16];
        for (penalty, strength) in grade_penalty.iter_mut().zip(AVOID_HILLS_STRENGTH) {
            *penalty = avoid_hills * strength;
        }

        Self {
            base,
            use_roads,
            avoid_roads,
            road_factor,
            sidepath_factor: 3.0 * (1.0 - use_roads),
            livingstreet_factor: 0.2 + use_roads * 0.8,
            track_factor: 0.5 + use_roads,
            avoid_bad_surfaces,
            speed,
            bicycle_type,
            minimal_surface_penalized,
            worst_allowed_surface,
            cyclelane_factor,
            path_cyclelane_factor,
            speedpenalty,
            speed_penalty_threshold,
            grade_penalty,
        }
    }

    /// The kind of bicycle.
    #[inline]
    pub fn bicycle_type(&self) -> BicycleType {
        self.bicycle_type
    }

    /// The cycling speed on smooth, flat roads, in km/h.
    #[inline]
    pub fn cycling_speed(&self) -> f32 {
        self.speed
    }

    /// The speed above which roads are penalized (mainline computes it but doesn't use it).
    #[inline]
    pub fn speed_penalty_threshold(&self) -> u32 {
        self.speed_penalty_threshold
    }

    /// The cost of renting a bike share bike (mainline's `BSSCost`).
    #[inline]
    pub fn bss_cost(&self) -> Cost {
        Cost::new(DEFAULT_BSS_COST, DEFAULT_BSS_PENALTY)
    }

    /// The shoulder and cycle lane index into the cycle lane tables.
    #[inline]
    fn lane_index(edge: &DirectedEdge) -> usize {
        usize::from(edge.has_shoulder()) * 4 + edge.cycle_lane() as usize
    }

    /// Mainline's `EdgeCost`, applying the user's cost factor for `factor_edge` if given.
    fn edge_cost_inner(&self, edge: &DirectedEdge, factor_edge: Option<GraphId>) -> EdgeCostResult {
        let b = &self.base;
        #[expect(clippy::cast_precision_loss, reason = "edge lengths fit in 24 bits")]
        let length = edge.length() as f32;
        let edge_use = road_use(edge);
        let result = |cost, sec| EdgeCostResult {
            cost: Cost::new(cost, sec),
            flow_sources: EnumSet::empty(),
        };

        // Steps cost as if ridden at 1 km/h, so they're generally avoided.
        if edge_use == RoadUse::Steps {
            let sec = length * speed_factor(1);
            return result(
                if b.shortest {
                    length
                } else {
                    sec * BICYCLE_STEPS_FACTOR
                },
                sec,
            );
        }

        // Ferries use the edge's speed.
        if edge_use == RoadUse::Ferry {
            let sec = length * speed_factor(u32::from(edge.speed()));
            return result(
                if b.shortest {
                    length
                } else {
                    sec * b.ferry_factor
                },
                sec,
            );
        }

        let mut roadway_stress: f32 = 1.0;
        let mut accommodation_factor: f32 = 1.0;
        match edge_use {
            RoadUse::Cycleway | RoadUse::Footway | RoadUse::Path => {
                accommodation_factor = self.path_cyclelane_factor[edge.cycle_lane() as usize];
            }
            RoadUse::MountainBike if self.bicycle_type == BicycleType::Mountain => {
                accommodation_factor = 0.3 + self.use_roads;
            }
            RoadUse::LivingStreet => roadway_stress = self.livingstreet_factor,
            RoadUse::Track => roadway_stress = self.track_factor,
            _ => {
                accommodation_factor = self.cyclelane_factor[Self::lane_index(edge)];
                if edge.lane_count() > 1 {
                    roadway_stress +=
                        (f32::from(edge.lane_count()) - 1.0) * 0.05 * self.road_factor;
                }
                if edge.truck_route() {
                    roadway_stress += TRUCK_STRESS;
                }
                roadway_stress += self.road_factor
                    * ROAD_CLASS_FACTOR[usize::from(edge.classification().discriminant())];
                // The edge's own speed, not a measured one.
                // Mainline reads past the table for speeds above 252 kph (which tiles don't
                // hold); this uses its last entry (see "Known divergences" in docs/routing.md).
                let speed = usize::from(edge.speed()).min(self.speedpenalty.len() - 1);
                roadway_stress *= self.speedpenalty[speed];
            }
        }

        if edge.use_sidepath() {
            accommodation_factor += self.sidepath_factor;
        }
        // Favor bicycle networks slightly.
        if edge.is_bike_network() {
            accommodation_factor *= BICYCLE_NETWORK_FACTOR;
        }

        let grade = usize::from(edge.weighted_grade() & 0xf);
        let mut factor = 1.0 + self.grade_penalty[grade] + (accommodation_factor * roadway_stress);

        if edge.surface() >= self.minimal_surface_penalized {
            // Mainline reads past the table for impassable surfaces on road bikes
            // (which it never allows, but can cost at an origin or destination);
            // this uses its last entry (see "Known divergences" in docs/routing.md).
            let index = (edge.surface() as usize - self.minimal_surface_penalized as usize)
                .min(SURFACE_FACTORS.len() - 1);
            factor += self.avoid_bad_surfaces * SURFACE_FACTORS[index];
        }

        let bike_speed = if edge.dismount() {
            float_to_u32(DISMOUNT_SPEED)
        } else {
            float_to_u32(
                (self.speed
                    * self.bicycle_type.surface_speed_factors()[edge.surface() as usize]
                    * GRADE_BASED_SPEED_FACTOR[grade])
                    + 0.5,
            )
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline's float *= double"
        )]
        {
            factor = (f64::from(factor) * b.edge_factor(factor_edge)) as f32;
        }

        let sec = length * speed_factor(bike_speed);
        result(if b.shortest { length } else { sec * factor }, sec)
    }

    /// The bicycle part of a transition (shared by both directions).
    fn bike_transition<P: TransitionPred + ?Sized>(
        &self,
        mut c: Cost,
        idx: u8,
        node: &NodeInfo,
        edge: &DirectedEdge,
        pred: &P,
    ) -> Cost {
        let edge_use = road_use(edge);
        let mut class_factor = ROAD_CLASS_FACTOR[usize::from(edge.classification().discriminant())];
        let bike_accom = match edge_use {
            RoadUse::Cycleway | RoadUse::Footway | RoadUse::Path => {
                // These are "service/other" roads, but shouldn't be penalized as such.
                class_factor = 0.1;
                0.05
            }
            RoadUse::LivingStreet => LIVING_STREET_TRANSITION_FACTOR,
            _ => CYCLE_LANE_TRANSITION_FACTOR[Self::lane_index(edge)],
        };

        let mut seconds: f32 = 0.0;
        let mut turn_stress: f32 = 1.0;
        let stopimpact = stop_impact(edge, idx);
        if stopimpact > 0 {
            let turn = turn_type(edge, idx) as usize;
            turn_stress += if node.drive_on_right() {
                RIGHT_SIDE_TURN_PENALTIES[turn]
            } else {
                LEFT_SIDE_TURN_PENALTIES[turn]
            };
            let mut turn_cost = if node.drive_on_right() {
                RIGHT_SIDE_TURN_COSTS[turn]
            } else {
                LEFT_SIDE_TURN_COSTS[turn]
            };
            if turn_cost < TC_CROSSING
                && edge.edge_to_right().contains(idx)
                && edge.edge_to_left().contains(idx)
            {
                turn_cost = TC_CROSSING;
            }
            seconds += f32::from(stopimpact) * turn_cost;
        }

        turn_stress *= (class_factor * self.avoid_roads) + self.use_roads + 1.0;

        let mut penalty: f32 = 0.0;
        let pred_class = pred.pred_classification();
        if (edge.classification() as u8) < (pred_class as u8) && edge_use != RoadUse::LivingStreet {
            penalty +=
                10.0 * f32::from(pred_class.discriminant() - edge.classification().discriminant());
            // A traffic signal reduces the stress (a double in mainline).
            let signal: f64 = if node.is_traffic_signal() { 0.4 } else { 1.0 };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline's float += double"
            )]
            {
                turn_stress = (f64::from(turn_stress) + signal) as f32;
            }
            penalty *= (bike_accom * self.avoid_roads) + self.use_roads;
        }

        c.cost += if self.base.shortest {
            0.0
        } else {
            seconds * (turn_stress + 1.0) + penalty
        };
        c.secs += seconds;
        c
    }
}

impl Costing for BicycleCost {
    #[inline]
    fn base(&self) -> &DynamicCost {
        &self.base
    }

    #[inline]
    fn base_mut(&mut self) -> &mut DynamicCost {
        &mut self.base
    }

    #[inline]
    fn allowed<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        pred: &L,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        let b = &self.base;
        let local = edge.local_edge_index();
        if !b.is_accessible(edge)
            || edge.is_shortcut()
            || (!pred.deadend()
                && pred.opp_local_idx() == local
                && pred.mode() == TravelMode::Bicycle)
            || (!b.ignore_turn_restrictions && has_bit(pred.restrictions(), local))
            || b.is_user_avoid_edge(edge_id)
            || b.check_exclusions(true, edge, pred)
        {
            return false;
        }
        if matches!(
            road_use(edge),
            RoadUse::TransitConnection | RoadUse::EgressConnection | RoadUse::PlatformConnection
        ) {
            return false;
        }
        if edge.surface() > self.worst_allowed_surface {
            return false;
        }
        access.edge_destonly = edge.dest_only();
        b.evaluate_restrictions(
            b.access_mask,
            edge,
            tile,
            edge_id,
            time,
            access,
            pred.destonly(),
            |_| true,
        )
    }

    #[inline]
    fn allowed_reverse<T: GraphTile, L: Label>(
        &self,
        edge: &DirectedEdge,
        pred: &L,
        opp_edge: &DirectedEdge,
        opp_edge_id: GraphId,
        tile: &T,
        time: TimeContext,
        access: &mut EdgeAccess,
    ) -> bool {
        let b = &self.base;
        if !b.is_accessible(opp_edge)
            || opp_edge.is_shortcut()
            || matches!(
                road_use(opp_edge),
                RoadUse::TransitConnection
                    | RoadUse::EgressConnection
                    | RoadUse::PlatformConnection
            )
            || (!pred.deadend()
                && pred.opp_local_idx() == edge.local_edge_index()
                && pred.mode() == TravelMode::Bicycle)
            || (!b.ignore_turn_restrictions
                && opp_edge.restrictions().contains(pred.opp_local_idx()))
            || b.is_user_avoid_edge(opp_edge_id)
            || b.check_exclusions(false, opp_edge, pred)
        {
            return false;
        }
        // Mainline checks the surface of `edge`, not `opp_edge`.
        if edge.surface() > self.worst_allowed_surface {
            return false;
        }
        access.edge_destonly = opp_edge.dest_only();
        b.evaluate_restrictions(
            b.access_mask,
            opp_edge,
            tile,
            opp_edge_id,
            time,
            access,
            pred.destonly(),
            |_| true,
        )
    }

    #[inline]
    fn edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        _tile: &T,
        _seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        // Bicycle costs don't depend on time.
        Ok(self.edge_cost_inner(edge, Some(edge_id)))
    }

    #[inline]
    fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _edge_id: GraphId,
        factor_edge: Option<GraphId>,
        _tile: &T,
        _seconds_of_week: Option<u32>,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, CostingError> {
        let full = self.edge_cost_inner(edge, None);
        Ok(partial_from_full(&self.base, full, factor_edge, start, end))
    }

    #[inline]
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        edge: &DirectedEdge,
        _tile: &T,
        node: &NodeInfo,
        pred: &L,
        _provider: &P,
        edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        let idx = pred.opp_local_idx();
        let pred = LabelPred(pred);
        let c = self
            .base
            .base_transition_cost(node, edge, &pred, idx, edge_destonly, false);
        Ok(self.bike_transition(c, idx, node, edge, &pred))
    }

    #[inline]
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        idx: u8,
        node: &NodeInfo,
        opp_edge: &DirectedEdge,
        opp_pred_edge: &DirectedEdge,
        pred: &L,
        _tile: &T,
        _provider: &P,
        _has_measured_speed: bool,
        _internal_turn: InternalTurn,
        opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        let c = self.base.base_transition_cost(
            node,
            opp_pred_edge,
            opp_edge,
            idx,
            pred.destonly(),
            opp_edge_destonly,
        );
        Ok(self.bike_transition(c, idx, node, opp_pred_edge, opp_edge))
    }

    #[inline]
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _tile: &T,
        disallow: DisallowMask,
    ) -> bool {
        #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
        let surface_ok =
            self.avoid_bad_surfaces != 1.0 || edge.surface() <= self.worst_allowed_surface;
        self.base.base_edge_filter(edge, disallow)
            && !edge.has_bss_connection()
            && road_use(edge) != RoadUse::Steps
            && surface_ok
    }

    #[inline]
    fn astar_cost_factor(&self) -> f32 {
        // Assume a top speed of twice the cycling speed.
        let factor = speed_factor(float_to_u32(2.0 * self.speed));
        #[expect(clippy::cast_possible_truncation, reason = "mainline returns a float")]
        let factor = (f64::from(factor) * self.base.min_linear_cost_factor) as f32;
        factor
    }

    #[inline]
    fn travel_type(&self) -> u8 {
        self.bicycle_type as u8
    }
}
