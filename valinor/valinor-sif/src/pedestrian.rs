//! Pedestrian costing (mainline's `PedestrianCost` in `src/sif/pedestriancost.cc`),
//! including hiking (`max_hiking_difficulty`) and the wheelchair and blind types.

use enumset::{EnumSet, enum_set};
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo, NodeType, SacScale};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{Access, GraphId, RoadUse, Surface};
use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::options::{
    HasAlleyFactor, HasDrivewayFactor, HasElevatorPenalty, HasMaxDistance, HasMaxGrade,
    HasMaxHikingDifficulty, HasModeFactor, HasSidewalkFactor, HasStepPenalty,
    HasTransitStartEndMaxDistance, HasTransitTransferMaxDistance, HasTransportType, HasUseHills,
    HasWalkingSpeed, HasWalkwayFactor,
};

use crate::Costing;
use crate::cost::{
    Cost, InternalTurn, MAX_FERRY_SPEED_KPH, MAX_GRADE_FACTOR, SEC_PER_HOUR, TravelMode,
};
use crate::costing::{CostingError, EdgeCostResult, edge_speed, partial_from_full};
use crate::dynamic::{
    BaseSetup, DisallowMask, DynamicCost, EdgeAccess, LabelPred, TimeContext, TransitionPred,
    UNLIMITED_TRANSITIONS, flag, float_to_u32, get, node_type, options_of, road_use, stop_impact,
};
use crate::label::Label;

/// Crossing penalties by stop impact (`kCrossingCosts`).
const CROSSING_COSTS: [u8; 8] = [0, 0, 1, 1, 2, 3, 5, 15];

/// How much to avoid roundabouts (`kRoundaboutFactor`).
const ROUNDABOUT_FACTOR: f32 = 2.0;

/// Speed factors by SAC scale (`kSacScaleSpeedFactor`).
const SAC_SCALE_SPEED_FACTOR: [f32; 7] = [1.0, 1.11, 1.25, 1.54, 2.5, 4.0, 6.67];

/// Cost factors by SAC scale (`kSacScaleCostFactor`).
const SAC_SCALE_COST_FACTOR: [f32; 7] = [0.0, 0.25, 0.75, 1.25, 2.0, 2.5, 3.0];

/// Speed factors by weighted grade (`kGradeBasedSpeedFactor`).
const GRADE_BASED_SPEED_FACTOR: [f32; 16] = [
    1.33, 1.22, 1.08, 0.97, 0.88, 0.92, 1.00, 1.10, 1.20, 1.33, 1.43, 1.57, 1.83, 2.03, 2.23, 2.50,
];

/// How strongly to avoid each weighted grade (`kAvoidHillsStrength`).
const AVOID_HILLS_STRENGTH: [f32; 16] = [
    2.0, 1.0, 0.5, 0.2, 0.1, 0.0, 0.0, 0.0, 0.1, 0.3, 0.4, 0.5, 0.7, 1.0, 3.0, 5.0,
];

/// Bike share rental cost and penalty (`kDefaultBssCost` and `kDefaultBssPenalty`).
const DEFAULT_BSS_COST: f32 = 120.0;
const DEFAULT_BSS_PENALTY: f32 = 0.0;

/// The kind of pedestrian (mainline's `PedestrianType`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PedestrianType {
    Foot = 0,
    Wheelchair = 1,
    Blind = 2,
}

/// Pedestrian costing (mainline's `PedestrianCost`).
#[derive(Clone, Debug)]
pub struct PedestrianCost {
    base: DynamicCost,
    pedestrian_type: PedestrianType,
    max_distance: u32,
    mode_factor: f32,
    transit_start_end_max_distance: u32,
    transit_transfer_max_distance: u32,
    minimal_allowed_surface: Surface,
    max_grade: u32,
    max_hiking_difficulty: u8,
    speed: f32,
    speedfactor: f32,
    walkway_factor: f32,
    sidewalk_factor: f32,
    alley_factor: f32,
    driveway_factor: f32,
    step_penalty: f32,
    elevator_penalty: f32,
    grade_penalty: [f32; 16],
}

impl PedestrianCost {
    /// Pedestrian costing from a request's options (mainline's `CreatePedestrianCost`).
    pub fn new(costing: &CostingProto) -> Self {
        let mut base = DynamicCost::new(
            costing,
            BaseSetup {
                mode: TravelMode::Pedestrian,
                access_mask: enum_set!(Access::Pedestrian),
                penalize_uturns: false,
            },
        );
        for limits in &mut base.hierarchy_limits {
            limits.max_up_transitions = UNLIMITED_TRANSITIONS;
        }
        base.allow_transit_connections = false;
        base.use_hierarchy_limits = false;

        let o = options_of(costing);
        let transport_type = match &o.has_transport_type {
            Some(HasTransportType::TransportType(t)) => t.as_str(),
            None => "",
        };
        let (pedestrian_type, access_mask, minimal_allowed_surface) = match transport_type {
            "wheelchair" => (
                PedestrianType::Wheelchair,
                enum_set!(Access::Wheelchair),
                Surface::Compacted,
            ),
            "blind" => (
                PedestrianType::Blind,
                enum_set!(Access::Pedestrian),
                Surface::Path,
            ),
            _ => (
                PedestrianType::Foot,
                enum_set!(Access::Pedestrian),
                Surface::Path,
            ),
        };
        base.access_mask = access_mask;

        let max_hiking_difficulty = if pedestrian_type == PedestrianType::Foot {
            let difficulty: u32 = get!(
                o,
                has_max_hiking_difficulty,
                HasMaxHikingDifficulty::MaxHikingDifficulty
            );
            // Mainline casts to its uint8_t enum.
            difficulty.to_le_bytes()[0]
        } else {
            SacScale::None as u8
        };

        let speed: f32 = get!(o, has_walking_speed, HasWalkingSpeed::WalkingSpeed);
        let use_hills: f32 = get!(o, has_use_hills, HasUseHills::UseHills);
        let avoid_hills = 1.0 - use_hills;
        let mut grade_penalty = [0.0; 16];
        for (penalty, strength) in grade_penalty.iter_mut().zip(AVOID_HILLS_STRENGTH) {
            *penalty = avoid_hills * strength;
        }
        debug_assert_eq!(grade_penalty.len(), MAX_GRADE_FACTOR + 1);

        Self {
            base,
            pedestrian_type,
            max_distance: get!(o, has_max_distance, HasMaxDistance::MaxDistance),
            mode_factor: get!(o, has_mode_factor, HasModeFactor::ModeFactor),
            transit_start_end_max_distance: get!(
                o,
                has_transit_start_end_max_distance,
                HasTransitStartEndMaxDistance::TransitStartEndMaxDistance
            ),
            transit_transfer_max_distance: get!(
                o,
                has_transit_transfer_max_distance,
                HasTransitTransferMaxDistance::TransitTransferMaxDistance
            ),
            minimal_allowed_surface,
            max_grade: get!(o, has_max_grade, HasMaxGrade::MaxGrade),
            max_hiking_difficulty,
            speed,
            speedfactor: (SEC_PER_HOUR * 0.001) / speed,
            walkway_factor: get!(o, has_walkway_factor, HasWalkwayFactor::WalkwayFactor),
            sidewalk_factor: get!(o, has_sidewalk_factor, HasSidewalkFactor::SidewalkFactor),
            alley_factor: get!(o, has_alley_factor, HasAlleyFactor::AlleyFactor),
            driveway_factor: get!(o, has_driveway_factor, HasDrivewayFactor::DrivewayFactor),
            step_penalty: get!(o, has_step_penalty, HasStepPenalty::StepPenalty),
            elevator_penalty: get!(o, has_elevator_penalty, HasElevatorPenalty::ElevatorPenalty),
            grade_penalty,
        }
    }

    /// The kind of pedestrian.
    #[inline]
    pub fn pedestrian_type(&self) -> PedestrianType {
        self.pedestrian_type
    }

    /// The largest SAC scale allowed (0 to 6; always 0 unless walking).
    #[inline]
    pub fn max_hiking_difficulty(&self) -> u8 {
        self.max_hiking_difficulty
    }

    /// The walking speed, in km/h.
    #[inline]
    pub fn walking_speed(&self) -> f32 {
        self.speed
    }

    /// The maximum grade option (mainline reads it but doesn't use it,
    /// as grades are too noisy).
    #[inline]
    pub fn max_grade(&self) -> u32 {
        self.max_grade
    }

    /// The mode factor for multimodal routes (mainline's `GetModeFactor`).
    #[inline]
    pub fn mode_factor(&self) -> f32 {
        self.mode_factor
    }

    /// The longest walk between transit stops, in meters (mainline's `GetMaxTransferDistanceMM`).
    #[inline]
    pub fn max_transfer_distance(&self) -> u32 {
        self.transit_transfer_max_distance
    }

    /// The longest walk at either end of a multimodal route, in meters.
    #[inline]
    pub fn transit_start_end_max_distance(&self) -> u32 {
        self.transit_start_end_max_distance
    }

    /// The cost of renting a bike share bike (mainline's `BSSCost`).
    #[inline]
    pub fn bss_cost(&self) -> Cost {
        Cost::new(DEFAULT_BSS_COST, DEFAULT_BSS_PENALTY)
    }

    /// The edge's SAC scale as its numeric value.
    ///
    /// The one value the field holds that Valhalla doesn't define (7, from corrupt data)
    /// counts as harder than any.
    #[inline]
    fn sac_scale(edge: &DirectedEdge) -> u8 {
        edge.sac_scale().map_or(u8::MAX, |s| s as u8)
    }

    /// The per-use edge factor (mainline's `use_factor_` table; 0 for none).
    #[inline]
    fn use_factor(&self, edge_use: RoadUse) -> f32 {
        match edge_use {
            RoadUse::Footway => self.walkway_factor,
            RoadUse::Sidewalk => self.sidewalk_factor,
            RoadUse::Alley => self.alley_factor,
            RoadUse::Driveway => self.driveway_factor,
            RoadUse::Track => self.base.track_factor,
            RoadUse::LivingStreet => self.base.living_street_factor,
            RoadUse::ServiceRoad => self.base.service_factor,
            _ => 0.0,
        }
    }

    /// Mainline's `PedestrianCost::base_transition_cost`, which differs from the shared one:
    /// private access applies at gates only, there are no toll booths,
    /// transit connections have no maneuver penalty, and service roads count when internal.
    fn base_transition_cost<P: TransitionPred + ?Sized>(
        &self,
        node: &NodeInfo,
        edge: &DirectedEdge,
        pred: &P,
        idx: u8,
        edge_destonly: bool,
        pred_destonly: bool,
    ) -> Cost {
        let b = &self.base;
        let ntype = node_type(node);
        let edge_use = road_use(edge);
        let pred_use = pred.pred_use();
        let mut c = Cost::ZERO;
        c += b.country_crossing_cost * flag(ntype == NodeType::BorderControl);
        c += b.gate_cost * flag(ntype == NodeType::Gate) * flag(!node.tagged_access());
        c += b.private_access_cost * flag(ntype == NodeType::Gate) * flag(node.private_access());
        c += b.bike_share_cost * flag(ntype == NodeType::BikeShare);
        c += b.ferry_transition_cost
            * flag(edge_use == RoadUse::Ferry && pred_use != RoadUse::Ferry);
        c += b.rail_ferry_transition_cost
            * flag(edge_use == RoadUse::RailFerry && pred_use != RoadUse::RailFerry);

        c.cost += b.destination_only_penalty
            * flag((edge_destonly || edge.dest_only()) && !(pred_destonly || pred.pred_destonly()));
        c.cost += b.alley_penalty * flag(edge_use == RoadUse::Alley && pred_use != RoadUse::Alley);
        c.cost += b.maneuver_penalty
            * flag(
                !edge.is_link()
                    && edge_use != RoadUse::EgressConnection
                    && edge_use != RoadUse::PlatformConnection
                    && !edge.name_consistency().contains(idx),
            );
        c.cost += b.living_street_penalty
            * flag(edge_use == RoadUse::LivingStreet && pred_use != RoadUse::LivingStreet);
        c.cost += b.track_penalty * flag(edge_use == RoadUse::Track && pred_use != RoadUse::Track);
        c.cost += b.service_penalty
            * flag(edge_use == RoadUse::ServiceRoad && pred_use != RoadUse::ServiceRoad);
        c.cost *= flag(!b.shortest);
        c
    }

    /// The fixed costs for steps and elevators, shared by both directions,
    /// or `None` if the transition has none.
    #[inline]
    fn fixed_transition(&self, edge: &DirectedEdge) -> Option<Cost> {
        match road_use(edge) {
            RoadUse::Steps => Some(Cost::new(self.step_penalty, 0.0)),
            RoadUse::Elevator => Some(Cost::new(self.elevator_penalty, 0.0)),
            _ => None,
        }
    }

    /// The penalty for riding an elevator node from `prev` to `levels`
    /// (each an edge's levels): one penalty per level traversed when both edges are on
    /// a single level, and one penalty otherwise.
    fn elevator_levels_cost(
        &self,
        levels: Option<(usize, f32, f32)>,
        prev: Option<(usize, f32, f32)>,
    ) -> Cost {
        #[expect(clippy::float_cmp, reason = "mainline compares exactly")]
        let traversed = match (levels, prev) {
            (Some((1, lo, hi)), Some((1, prev_lo, prev_hi))) if lo == hi && prev_lo == prev_hi => {
                // `std::abs(float)` converted to unsigned int.
                float_to_u32((prev_lo - lo).abs())
            }
            _ => 1,
        };
        #[expect(clippy::cast_precision_loss, reason = "level counts are small")]
        let traversed = traversed as f32;
        Cost::new(self.elevator_penalty * traversed, 0.0)
    }

    /// The crossing part of a transition, shared by both directions.
    #[inline]
    fn crossing(&self, mut c: Cost, edge: &DirectedEdge, idx: u8) -> Cost {
        if edge.edge_to_right().contains(idx) && edge.edge_to_left().contains(idx) {
            let seconds = f32::from(CROSSING_COSTS[usize::from(stop_impact(edge, idx) & 7)]);
            c.secs += seconds;
            c.cost += if self.base.shortest { 0.0 } else { seconds };
        }
        c
    }

    /// Mainline's `EdgeCost`, applying the user's cost factor for `factor_edge` if given.
    fn edge_cost_inner<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        factor_edge: Option<GraphId>,
        tile: &T,
        seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        let b = &self.base;
        #[expect(clippy::cast_precision_loss, reason = "edge lengths fit in 24 bits")]
        let length = edge.length() as f32;
        let edge_use = road_use(edge);

        // Ferries use the ferry's speed.
        if edge_use == RoadUse::Ferry {
            let (speed, flow_sources) =
                edge_speed(tile, edge, edge_id, b.flow_mask, seconds_of_week, false)?;
            #[expect(clippy::cast_precision_loss, reason = "speeds are small")]
            let sec = length * (SEC_PER_HOUR * 0.001) / speed as f32;
            return Ok(EdgeCostResult {
                cost: Cost::new(sec * b.ferry_factor, sec),
                flow_sources,
            });
        }

        let sac = usize::from(Self::sac_scale(edge)).min(SAC_SCALE_SPEED_FACTOR.len() - 1);
        let grade = usize::from(edge.weighted_grade() & 0xf);
        let sec = length
            * self.speedfactor
            * SAC_SCALE_SPEED_FACTOR[sac]
            * GRADE_BASED_SPEED_FACTOR[grade];

        if b.shortest {
            return Ok(EdgeCostResult {
                cost: Cost::new(length, sec),
                flow_sources: EnumSet::empty(),
            });
        }

        let mut factor = 1.0 + SAC_SCALE_COST_FACTOR[sac] + self.grade_penalty[grade];
        let use_factor = self.use_factor(edge_use);
        if use_factor != 0.0 {
            factor *= use_factor;
        } else if edge.has_sidewalk_left() || edge.has_sidewalk_right() {
            factor *= self.sidewalk_factor;
        } else if edge.roundabout() {
            factor *= ROUNDABOUT_FACTOR;
        }

        factor *= flag(edge.is_lit()) + flag(!edge.is_lit()) * b.unlit_factor;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline's float *= double"
        )]
        {
            factor = (f64::from(factor) * b.edge_factor(factor_edge)) as f32;
        }

        Ok(EdgeCostResult {
            cost: Cost::new(sec * factor, sec),
            flow_sources: EnumSet::empty(),
        })
    }
}

/// The levels of an edge: how many ranges, and the first range.
///
/// # Errors
///
/// Fails if the edge's info or levels are corrupt.
fn edge_levels<T: GraphTile + ?Sized>(
    tile: &T,
    edge: &DirectedEdge,
) -> Result<Option<(usize, f32, f32)>, CostingError> {
    let info = tile.get_edge_info(edge)?;
    let Some(levels) = info.levels()? else {
        return Ok(None);
    };
    let mut ranges = levels.ranges();
    let Some(first) = ranges.next().transpose()? else {
        return Ok(Some((0, 0.0, 0.0)));
    };
    // Mainline only cares whether there is exactly one range.
    let count = 1 + usize::from(ranges.next().transpose()?.is_some());
    Ok(Some((count, first.0, first.1)))
}

/// The levels of the edge with the given ID, read through `provider`.
fn levels_of_edge_id<P: GraphTileProvider>(
    provider: &P,
    edge_id: GraphId,
) -> Result<Option<(usize, f32, f32)>, CostingError> {
    provider.with_tile_containing(edge_id, |tile| {
        let edge = tile
            .get_directed_edge(edge_id)
            .map_err(|_| CostingError::MissingEdge(edge_id))?;
        edge_levels(tile, edge)
    })?
}

impl Costing for PedestrianCost {
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
        // Mainline's distance check is `(!allow_transit_connections_ && distance) > max_distance_`,
        // a boolean compared with the maximum, so it only rejects anything when
        // `max_distance` is 0 (and then rejects edges that move away from the origin).
        let over_distance = u32::from(
            !b.allow_transit_connections && pred.path_distance().wrapping_add(edge.length()) != 0,
        ) > self.max_distance;
        if !b.is_accessible(edge)
            || edge.surface() > self.minimal_allowed_surface
            || edge.is_shortcut()
            || b.is_user_avoid_edge(edge_id)
            || Self::sac_scale(edge) > self.max_hiking_difficulty
            || (!pred.deadend()
                && pred.opp_local_idx() == edge.local_edge_index()
                && pred.mode() == TravelMode::Pedestrian)
            || over_distance
            || b.check_exclusions(true, edge, pred)
        {
            return false;
        }

        // No transit connections (except in multimodal routes).
        if !b.allow_transit_connections
            && matches!(
                road_use(edge),
                RoadUse::PlatformConnection
                    | RoadUse::EgressConnection
                    | RoadUse::TransitConnection
            )
        {
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
        // No distance check here, and transit connections are never allowed
        // (mainline assumes reverse searches aren't multimodal).
        // Mainline checks the hiking difficulty of `edge`, not `opp_edge`.
        if !b.is_accessible(opp_edge)
            || opp_edge.surface() > self.minimal_allowed_surface
            || opp_edge.is_shortcut()
            || b.is_user_avoid_edge(opp_edge_id)
            || Self::sac_scale(edge) > self.max_hiking_difficulty
            || (!pred.deadend()
                && pred.opp_local_idx() == edge.local_edge_index()
                && pred.mode() == TravelMode::Pedestrian)
            || matches!(
                road_use(opp_edge),
                RoadUse::TransitConnection
                    | RoadUse::EgressConnection
                    | RoadUse::PlatformConnection
            )
            || b.check_exclusions(false, opp_edge, pred)
        {
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
        tile: &T,
        seconds_of_week: Option<u32>,
    ) -> Result<EdgeCostResult, CostingError> {
        self.edge_cost_inner(edge, edge_id, Some(edge_id), tile, seconds_of_week)
    }

    #[inline]
    fn partial_edge_cost<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        edge_id: GraphId,
        tile: &T,
        seconds_of_week: Option<u32>,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, CostingError> {
        let full = self.edge_cost_inner(edge, edge_id, None, tile, seconds_of_week)?;
        Ok(partial_from_full(&self.base, full, edge_id, start, end))
    }

    #[inline]
    fn transition_cost<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        edge: &DirectedEdge,
        tile: &T,
        node: &NodeInfo,
        pred: &L,
        provider: &P,
        edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        if let Some(cost) = self.fixed_transition(edge) {
            return Ok(cost);
        }
        if node_type(node) == NodeType::Elevator {
            let levels = edge_levels(tile, edge)?;
            let pred_levels = levels_of_edge_id(provider, pred.edge_id())?;
            return Ok(self.elevator_levels_cost(levels, pred_levels));
        }
        let idx = pred.opp_local_idx();
        let c = self.base_transition_cost(node, edge, &LabelPred(pred), idx, edge_destonly, false);
        Ok(self.crossing(c, edge, idx))
    }

    #[inline]
    fn transition_cost_reverse<T: GraphTile, L: Label, P: GraphTileProvider>(
        &self,
        idx: u8,
        node: &NodeInfo,
        opp_edge: &DirectedEdge,
        opp_pred_edge: &DirectedEdge,
        pred: &L,
        tile: &T,
        provider: &P,
        _has_measured_speed: bool,
        _internal_turn: InternalTurn,
        opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        if let Some(cost) = self.fixed_transition(opp_pred_edge) {
            return Ok(cost);
        }
        if node_type(node) == NodeType::Elevator {
            let levels = edge_levels(tile, opp_edge)?;
            let pred_levels = levels_of_edge_id(provider, pred.edge_id())?;
            return Ok(self.elevator_levels_cost(levels, pred_levels));
        }
        let c = self.base_transition_cost(
            node,
            opp_pred_edge,
            opp_edge,
            idx,
            pred.destonly(),
            opp_edge_destonly,
        );
        Ok(self.crossing(c, opp_pred_edge, idx))
    }

    #[inline]
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _tile: &T,
        disallow: DisallowMask,
    ) -> bool {
        self.base.base_edge_filter(edge, disallow)
            && (road_use(edge) as u8) < RoadUse::RailFerry as u8
            && Self::sac_scale(edge) <= self.max_hiking_difficulty
            && (!edge.has_bss_connection() || self.base.project_on_bss_connection)
    }

    #[inline]
    fn astar_cost_factor(&self) -> f32 {
        let min_linear = self.base.min_linear_cost_factor;
        if self.base.pass == 0 {
            let b = &self.base;
            let mut factor: f32 = 1.0;
            for f in [
                self.walkway_factor,
                self.sidewalk_factor,
                self.alley_factor,
                self.driveway_factor,
                b.track_factor,
                b.living_street_factor,
                b.service_factor,
            ] {
                if f < 1.0 {
                    factor *= f;
                }
            }
            #[expect(clippy::cast_possible_truncation, reason = "mainline returns a float")]
            let factor = (f64::from(self.speedfactor * factor) * min_linear) as f32;
            factor
        } else {
            #[expect(clippy::cast_precision_loss, reason = "a small constant")]
            let ferry = (SEC_PER_HOUR * 0.001) / MAX_FERRY_SPEED_KPH as f32;
            #[expect(clippy::cast_possible_truncation, reason = "mainline returns a float")]
            let factor = (f64::from(ferry) * min_linear) as f32;
            factor
        }
    }

    #[inline]
    fn allow_multi_pass(&self) -> bool {
        true
    }

    #[inline]
    fn travel_type(&self) -> u8 {
        self.pedestrian_type as u8
    }
}
