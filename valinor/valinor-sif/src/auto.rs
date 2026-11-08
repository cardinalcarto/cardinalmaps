//! Auto costing (mainline's `AutoCost` in `src/sif/autocost.cc`).

use std::sync::LazyLock;

use enumset::{EnumSet, enum_set};
use valhalla_graphtile::graph_tile::{
    AccessRestriction, AccessRestrictionType, DirectedEdge, GraphTile, HovType, NodeInfo, TurnType,
};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_graphtile::{Access, GraphId, RoadUse, Surface};
use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::options::{
    HasAlleyFactor, HasHeight, HasLength, HasUseHighways, HasUseTolls, HasWeight, HasWidth,
};

use crate::Costing;
use crate::cost::{
    Cost, DENSITY_FACTOR, DISABLE_FIXED_SPEED, InternalTurn, MAX_HIGHWAY_BIAS_FACTOR,
    TRANS_DENSITY_FACTOR, TravelMode, speed_factor,
};
use crate::costing::{CostingError, EdgeCostResult, edge_speed, partial_from_full};
use crate::dynamic::{
    BaseSetup, DEFAULT_FLOW_MASK, DisallowMask, DynamicCost, EdgeAccess, LabelPred, TimeContext,
    TransitionPred, get, has_bit, options_of, road_use, stop_impact, turn_type,
};
use crate::label::Label;

// Turn costs (`kTC*` in autocost.cc).
const TC_STRAIGHT: f32 = 0.5;
const TC_SLIGHT: f32 = 0.75;
const TC_FAVORABLE: f32 = 1.0;
const TC_FAVORABLE_SHARP: f32 = 1.5;
const TC_CROSSING: f32 = 2.0;
const TC_UNFAVORABLE: f32 = 2.5;
const TC_UNFAVORABLE_SHARP: f32 = 3.5;
const TC_REVERSE: f32 = 9.5;
const TC_RAMP: f32 = 1.5;
const TC_ROUNDABOUT: f32 = 0.5;

/// How much to favor turn channels (`kTurnChannelFactor`).
const TURN_CHANNEL_FACTOR: f32 = 0.6;

/// Turn costs by turn type, driving on the right (`kRightSideTurnCosts`).
const RIGHT_SIDE_TURN_COSTS: [f32; 8] = [
    TC_STRAIGHT,
    TC_SLIGHT,
    TC_FAVORABLE,
    TC_FAVORABLE_SHARP,
    TC_REVERSE,
    TC_UNFAVORABLE_SHARP,
    TC_UNFAVORABLE,
    TC_SLIGHT,
];
/// Turn costs by turn type, driving on the left (`kLeftSideTurnCosts`).
const LEFT_SIDE_TURN_COSTS: [f32; 8] = [
    TC_STRAIGHT,
    TC_SLIGHT,
    TC_UNFAVORABLE,
    TC_UNFAVORABLE_SHARP,
    TC_REVERSE,
    TC_FAVORABLE_SHARP,
    TC_FAVORABLE,
    TC_SLIGHT,
];

/// Highway factors by road class (`kHighwayFactor`).
const HIGHWAY_FACTOR: [f32; 8] = [1.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// Surface factors by surface (`kSurfaceFactor`).
///
/// Mainline's table has no entry for impassable surfaces and reads past it,
/// which is undefined. Auto never enters an impassable edge ([`Costing::allowed`] rejects them),
/// but Loki's filter lets locations correlate to one, so Thor can cost one at an origin
/// or destination; this treats it like `kPath` (see "Known divergences" in docs/routing.md).
const SURFACE_FACTOR: [f32; 8] = [0.0, 0.0, 0.0, 0.1, 0.2, 0.5, 1.0, 1.0];

/// Auto costing (mainline's `AutoCost`): driving a car,
/// generally by shortest time, with hierarchies that keep routes off residential shortcuts.
#[derive(Clone, Debug)]
pub struct AutoCost {
    base: DynamicCost,
    highway_factor: f32,
    alley_factor: f32,
    toll_factor: f32,
    surface_factor: f32,
    height: f32,
    width: f32,
    length: f32,
    weight: f32,
}

impl AutoCost {
    /// Auto costing from a request's options (mainline's `CreateAutoCost`).
    pub fn new(costing: &CostingProto) -> Self {
        Self::with_access(costing, enum_set!(Access::Auto | Access::HOV))
    }

    /// Auto costing with a different access mask (mainline's second constructor argument,
    /// which bus and taxi costings use).
    pub(crate) fn with_access(costing: &CostingProto, access_mask: EnumSet<Access>) -> Self {
        let base = DynamicCost::new(
            costing,
            BaseSetup {
                mode: TravelMode::Drive,
                access_mask,
                penalize_uturns: true,
            },
        );
        let o = options_of(costing);

        let use_highways: f32 = get!(o, has_use_highways, HasUseHighways::UseHighways);
        let highway_factor = if use_highways >= 0.5 {
            let f = 0.5 - use_highways;
            f * f * f
        } else {
            let f = 1.0 - use_highways * 2.0;
            MAX_HIGHWAY_BIAS_FACTOR * (f * f)
        };

        let use_tolls: f32 = get!(o, has_use_tolls, HasUseTolls::UseTolls);
        let toll_factor = if use_tolls < 0.5 {
            4.0 - 8.0 * use_tolls
        } else {
            (0.5 - use_tolls) * 0.03
        };

        let mut cost = Self {
            base,
            highway_factor,
            alley_factor: get!(o, has_alley_factor, HasAlleyFactor::AlleyFactor),
            toll_factor,
            surface_factor: 0.5,
            height: get!(o, has_height, HasHeight::Height),
            width: get!(o, has_width, HasWidth::Width),
            length: get!(o, has_length, HasLength::Length),
            weight: get!(o, has_weight, HasWeight::Weight),
        };
        cost.base.include_hot = o.include_hot;
        cost.base.include_hov2 = o.include_hov2;
        cost.base.include_hov3 = o.include_hov3;
        cost
    }

    /// Whether HOV rules let the costing onto the edge (mainline's `IsHOVAllowed`).
    #[inline]
    fn is_hov_allowed(&self, edge: &DirectedEdge) -> bool {
        match edge.hov_type() {
            None => true,
            Some(hov) => {
                self.base.include_hov3
                    || (self.base.include_hov2 && hov == HovType::Hov2)
                    || (self.base.include_hot && edge.toll())
            }
        }
    }

    /// Mainline's `ModeSpecificAllowed`: the vehicle's dimensions against the edge's limits
    /// (stored in centimeters or centitonnes).
    #[inline]
    fn mode_allowed(&self, restriction: &AccessRestriction) -> bool {
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "mainline computes value * 0.01 as a double and converts it to float"
        )]
        let limit = || (restriction.value() as f64 * 0.01) as f32;
        match restriction.restriction_type() {
            Ok(AccessRestrictionType::MaxHeight) => self.height <= limit(),
            Ok(AccessRestrictionType::MaxWidth) => self.width <= limit(),
            Ok(AccessRestrictionType::MaxLength) => self.length <= limit(),
            Ok(AccessRestrictionType::MaxWeight) => self.weight <= limit(),
            _ => true,
        }
    }

    /// The access checks before restrictions, shared by both directions.
    #[inline]
    fn rejects<L: Label>(
        &self,
        forward: bool,
        edge: &DirectedEdge,
        edge_id: GraphId,
        pred: &L,
        restricted_turn: bool,
        uturn: bool,
    ) -> bool {
        let b = &self.base;
        !b.is_accessible(edge)
            || (!pred.deadend() && uturn)
            || (restricted_turn && !b.ignore_turn_restrictions)
            || edge.surface() == Surface::Impassable
            || b.is_user_avoid_edge(edge_id)
            || (!b.allow_destination_only && !pred.destonly() && edge.dest_only())
            || (pred.closure_pruning() && b.is_closed(edge))
            || (b.exclude_unpaved && !pred.unpaved() && edge.is_unpaved())
            || !self.is_hov_allowed(edge)
            || b.check_exclusions(forward, edge, pred)
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
        let (edge_speed, flow_sources) = if b.fixed_speed == DISABLE_FIXED_SPEED {
            edge_speed(tile, edge, edge_id, b.flow_mask, seconds_of_week, false)?
        } else {
            (b.fixed_speed, EnumSet::empty())
        };
        let final_speed = edge_speed.min(b.top_speed);
        #[expect(clippy::cast_precision_loss, reason = "edge lengths fit in 24 bits")]
        let length = edge.length() as f32;
        let sec = length * speed_factor(final_speed);

        if b.shortest {
            return Ok(EdgeCostResult {
                cost: Cost::new(length, sec),
                flow_sources,
            });
        }

        let edge_use = road_use(edge);
        let mut factor = match edge_use {
            RoadUse::Ferry => b.ferry_factor,
            RoadUse::RailFerry => b.rail_ferry_factor,
            _ => DENSITY_FACTOR[usize::from(edge.density() & 0xf)],
        };
        factor += self.highway_factor
            * HIGHWAY_FACTOR[edge.classification().discriminant() as usize]
            + self.surface_factor * SURFACE_FACTOR[edge.surface() as usize]
            + b.speed_penalty(edge_speed)
            + f32::from(u8::from(edge.toll())) * self.toll_factor;

        match edge_use {
            RoadUse::Alley => factor *= self.alley_factor,
            RoadUse::Track => factor *= b.track_factor,
            RoadUse::LivingStreet => factor *= b.living_street_factor,
            RoadUse::ServiceRoad => factor *= b.service_factor,
            // Boost only measured speeds.
            RoadUse::TurnChannel
                if !(flow_sources & EnumSet::from_repr_truncated(DEFAULT_FLOW_MASK)).is_empty() =>
            {
                factor *= TURN_CHANNEL_FACTOR;
            }
            _ => {}
        }

        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline's float *= double"
        )]
        {
            factor = (f64::from(factor) * b.edge_factor(factor_edge)) as f32;
        }
        if b.is_closed(edge) {
            factor *= b.closure_factor;
        }

        Ok(EdgeCostResult {
            cost: Cost::new(
                (sec * b.inv_distance_factor + length * b.distance_factor) * factor,
                sec,
            ),
            flow_sources,
        })
    }

    /// The turn part of a transition (shared by both directions).
    #[expect(clippy::too_many_arguments, reason = "mainline's inputs")]
    #[inline]
    fn turn_cost<P: TransitionPred + ?Sized>(
        &self,
        mut c: Cost,
        idx: u8,
        node: &NodeInfo,
        edge: &DirectedEdge,
        pred: &P,
        pred_opp_local_idx: u8,
        has_measured_speed: bool,
        internal_turn: InternalTurn,
    ) -> Cost {
        let b = &self.base;
        c.secs += osrm_car_turn_duration(edge, node, pred_opp_local_idx);

        let stopimpact = stop_impact(edge, idx);
        let turntype = turn_type(edge, idx);
        if stopimpact > 0 && !b.shortest {
            let mut turn_cost =
                if edge.edge_to_right().contains(idx) && edge.edge_to_left().contains(idx) {
                    TC_CROSSING
                } else if node.drive_on_right() {
                    RIGHT_SIDE_TURN_COSTS[turntype as usize]
                } else {
                    LEFT_SIDE_TURN_COSTS[turntype as usize]
                };

            let edge_ramp = road_use(edge) == RoadUse::Ramp;
            let pred_ramp = pred.pred_use() == RoadUse::Ramp;
            if edge_ramp != pred_ramp {
                turn_cost += TC_RAMP;
                if edge.roundabout() {
                    turn_cost += TC_ROUNDABOUT;
                }
            }

            let mut seconds = turn_cost;
            let has_left = matches!(turntype, TurnType::Left | TurnType::SharpLeft);
            let has_right = matches!(turntype, TurnType::Right | TurnType::SharpRight);
            let has_reverse = turntype == TurnType::Reverse;
            let is_turn = has_left || has_right || has_reverse;
            let stopimpact = f32::from(stopimpact);
            if is_turn {
                seconds *= stopimpact;
            }
            DynamicCost::add_uturn_penalty(
                idx,
                node,
                edge,
                has_reverse,
                has_left,
                has_right,
                true,
                internal_turn,
                &mut seconds,
            );
            // Without measured speeds, add the stop impact and density factor.
            if !has_measured_speed {
                if !is_turn {
                    seconds *= stopimpact;
                }
                seconds *= TRANS_DENSITY_FACTOR[usize::from(node.density() & 0xf)];
            }
            c.cost += seconds;
        }

        // Account for the user preferring distance.
        c.cost *= b.inv_distance_factor;
        c
    }
}

impl Costing for AutoCost {
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
        let local = edge.local_edge_index();
        if self.rejects(
            true,
            edge,
            edge_id,
            pred,
            has_bit(pred.restrictions(), local),
            pred.opp_local_idx() == local,
        ) {
            return false;
        }
        access.edge_destonly = edge.dest_only();
        self.base.evaluate_restrictions(
            self.base.access_mask,
            edge,
            tile,
            edge_id,
            time,
            access,
            pred.destonly(),
            |r| self.mode_allowed(r),
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
        if self.rejects(
            false,
            opp_edge,
            opp_edge_id,
            pred,
            opp_edge.restrictions().contains(pred.opp_local_idx()),
            pred.opp_local_idx() == edge.local_edge_index(),
        ) {
            return false;
        }
        access.edge_destonly = opp_edge.dest_only();
        self.base.evaluate_restrictions(
            self.base.access_mask,
            opp_edge,
            tile,
            opp_edge_id,
            time,
            access,
            pred.destonly(),
            |r| self.mode_allowed(r),
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
        factor_edge: Option<GraphId>,
        tile: &T,
        seconds_of_week: Option<u32>,
        start: f32,
        end: f32,
    ) -> Result<EdgeCostResult, CostingError> {
        let full = self.edge_cost_inner(edge, edge_id, None, tile, seconds_of_week)?;
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
        let pred_view = LabelPred(pred);
        let c = self
            .base
            .base_transition_cost(node, edge, &pred_view, idx, edge_destonly, false);
        Ok(self.turn_cost(
            c,
            idx,
            node,
            edge,
            &pred_view,
            idx,
            pred.has_measured_speed(),
            pred.internal_turn(),
        ))
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
        has_measured_speed: bool,
        internal_turn: InternalTurn,
        opp_edge_destonly: bool,
    ) -> Result<Cost, CostingError> {
        // Mainline names `opp_edge` "pred" and `opp_pred_edge` "edge" here.
        let c = self.base.base_transition_cost(
            node,
            opp_pred_edge,
            opp_edge,
            idx,
            pred.destonly(),
            opp_edge_destonly,
        );
        Ok(self.turn_cost(
            c,
            idx,
            node,
            opp_pred_edge,
            opp_edge,
            opp_edge.opposing_local_index(),
            has_measured_speed,
            internal_turn,
        ))
    }

    #[inline]
    fn edge_filter<T: GraphTile>(
        &self,
        edge: &DirectedEdge,
        _tile: &T,
        disallow: DisallowMask,
    ) -> bool {
        // Closures come from live traffic, which Valinor doesn't have, so none are filtered.
        self.base.base_edge_filter(edge, disallow)
            && !edge.has_bss_connection()
            && self.is_hov_allowed(edge)
    }

    #[inline]
    fn astar_cost_factor(&self) -> f32 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline's float * float(double)"
        )]
        let factor = speed_factor(self.base.top_speed) * self.base.min_linear_cost_factor as f32;
        factor
    }

    #[inline]
    fn allow_multi_pass(&self) -> bool {
        true
    }

    #[inline]
    fn travel_type(&self) -> u8 {
        // VehicleType::kCar
        0
    }
}

// OSRM's car turn durations (mainline's `sif/osrm_car_duration.h`).
const TURN_PENALTY: f64 = 7.5;
const UTURN_PENALTY: f64 = 20.0;
const TURN_BIAS: f64 = 1.075;
const TURN_BIAS_INV: f64 = 1.0 / TURN_BIAS;
const TRAFFIC_LIGHT_PENALTY: f64 = 2.0;

/// Turn durations by turn degree (mainline's `lookup_table`).
fn turn_duration_table(right: bool) -> [f64; 360] {
    let mut durations = [0.0; 360];
    for (angle, duration) in durations.iter_mut().enumerate() {
        let angle = i32::try_from(angle).unwrap_or(0);
        let symmetric = f64::from(if angle > 180 { angle - 360 } else { angle });
        *duration = if symmetric >= 0.0 {
            TURN_PENALTY
                / (1.0
                    + (-((13.0 * if right { TURN_BIAS_INV } else { TURN_BIAS }) * symmetric
                        / 180.0
                        - 6.5 * if right { TURN_BIAS } else { TURN_BIAS_INV }))
                    .exp())
        } else {
            TURN_PENALTY
                / (1.0
                    + (-((13.0 * if right { TURN_BIAS } else { TURN_BIAS_INV }) * -symmetric
                        / 180.0
                        - 6.5 * if right { TURN_BIAS_INV } else { TURN_BIAS }))
                    .exp())
        };
    }
    durations
}

static LEFT_HAND_TURNS: LazyLock<[f64; 360]> = LazyLock::new(|| turn_duration_table(false));
static RIGHT_HAND_TURNS: LazyLock<[f64; 360]> = LazyLock::new(|| turn_duration_table(true));

/// Mainline's `Turn::GetType` for a turn degree.
fn turn_type_of_degree(degree: u32) -> TurnType {
    match degree % 360 {
        0..=10 | 350..=359 => TurnType::Straight,
        11..=44 => TurnType::SlightRight,
        45..=135 => TurnType::Right,
        136..=159 => TurnType::SharpRight,
        160..=200 => TurnType::Reverse,
        201..=224 => TurnType::SharpLeft,
        225..=315 => TurnType::Left,
        _ => TurnType::SlightLeft,
    }
}

/// Mainline's `OSRMCarTurnDuration`: a port of OSRM's car profile turn function,
/// with a traffic light penalty.
#[inline]
pub(crate) fn osrm_car_turn_duration(
    edge: &DirectedEdge,
    node: &NodeInfo,
    idx_pred_opp: u8,
) -> f32 {
    let mut turn_duration = if node.is_traffic_signal() {
        TRAFFIC_LIGHT_PENALTY
    } else {
        0.0
    };
    let in_heading = (u32::from(node.heading(idx_pred_opp).unwrap_or(0)) + 180) % 360;
    let out_heading = u32::from(node.heading(edge.local_edge_index()).unwrap_or(0));
    // Mainline's `GetTurnDegree`, in unsigned arithmetic.
    let turn_degree = (out_heading.wrapping_sub(in_heading).wrapping_add(360)) % 360;
    let is_u_turn = turn_type_of_degree(turn_degree) == TurnType::Reverse;
    if node.local_edge_count() > 2 || is_u_turn {
        let table = if node.drive_on_right() {
            &*RIGHT_HAND_TURNS
        } else {
            &*LEFT_HAND_TURNS
        };
        turn_duration += table[turn_degree as usize];
        if is_u_turn {
            turn_duration += UTURN_PENALTY;
        }
    }
    #[expect(clippy::cast_possible_truncation, reason = "mainline returns a float")]
    let duration = turn_duration as f32;
    duration
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_degrees_map_like_mainline() {
        assert_eq!(turn_type_of_degree(0), TurnType::Straight);
        assert_eq!(turn_type_of_degree(10), TurnType::Straight);
        assert_eq!(turn_type_of_degree(11), TurnType::SlightRight);
        assert_eq!(turn_type_of_degree(135), TurnType::Right);
        assert_eq!(turn_type_of_degree(136), TurnType::SharpRight);
        assert_eq!(turn_type_of_degree(160), TurnType::Reverse);
        assert_eq!(turn_type_of_degree(200), TurnType::Reverse);
        assert_eq!(turn_type_of_degree(201), TurnType::SharpLeft);
        assert_eq!(turn_type_of_degree(315), TurnType::Left);
        assert_eq!(turn_type_of_degree(349), TurnType::SlightLeft);
        assert_eq!(turn_type_of_degree(350), TurnType::Straight);
    }
}
