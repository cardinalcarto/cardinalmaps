//! Viability tests for alternate routes (mainline's `thor/alternates.cc`),
//! after M. Kobitzsch's "Alternative Route Techniques" (2015):
//! an alternate must share little with the routes already chosen,
//! must not be much longer than the best one (overall and in the part where it differs),
//! and should be locally optimal (which mainline doesn't check).

use std::collections::HashSet;

use valhalla_graphtile::GraphId;
use valhalla_proto::Location;
use valinor_sif::Cost;

use crate::bidirectional::CandidateConnection;
use crate::error::ThorError;
use crate::path::PathEdge;
use crate::search::first_candidate_point;

/// The overall stretch for routes over five hours (`kAtMostLonger`).
const AT_MOST_LONGER: f64 = 1.25;
/// A detour may cost at most this much more than the part of the best route it replaces
/// (`kAtMostLongerDetour`).
const AT_MOST_LONGER_DETOUR: f32 = 2.0;
/// The sharing limit for routes over 100 km (`kAtMostShared`).
const AT_MOST_SHARED: f32 = 0.75;

/// How much of an alternate may be shared with the routes already chosen,
/// by the distance between the locations (`get_max_sharing`).
pub(crate) fn get_max_sharing(origin: &Location, destination: &Location) -> Result<f32, ThorError> {
    let from = first_candidate_point(origin, 0)?;
    let to = first_candidate_point(destination, 1)?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "mainline stores it in a float"
    )]
    let distance = from.distance(to) as f32;
    Ok(if distance < 10_000.0 {
        0.6
    } else if distance < 100_000.0 {
        // From 0.6 at 10 km to 0.75 at 100 km.
        0.6 + (AT_MOST_SHARED - 0.6) * (distance - 10_000.0) / (100_000.0 - 10_000.0)
    } else {
        AT_MOST_SHARED
    })
}

/// The stretch allowed over the best route's cost (`get_at_most_longer`).
fn get_at_most_longer(optimal_cost: f64) -> f64 {
    if optimal_cost < 10.0 * 60.0 {
        return 2.0;
    }
    if optimal_cost < 5.0 * 3600.0 {
        // A quadratic hyperbola through 2.0 at 10 minutes, 1.75 at 20, 1.5 at 30,
        // 1.4 at an hour, 1.3 at two, and 1.25 at five.
        const A: f64 = 1.210_679_94e+00;
        const B: f64 = 7.229_415_76e+02;
        const C: f64 = -1.457_262_21e+05;
        return A + B / optimal_cost + C / (optimal_cost * optimal_cost);
    }
    AT_MOST_LONGER
}

/// Sorts the connections by cost and drops those much costlier than the best
/// (`filter_alternates_by_stretch`).
///
/// Mainline's `std::sort` isn't stable; connections with equal costs keep their order here,
/// which only matters between alternates that cost the same.
pub(crate) fn filter_alternates_by_stretch(connections: &mut Vec<CandidateConnection>) {
    connections.sort_by(|a, b| a.cost.total_cmp(&b.cost));
    let Some(best) = connections.first() else {
        return;
    };
    let at_most_longer = get_at_most_longer(f64::from(best.cost));
    // A double, which `std::lower_bound` passes to `operator<(float)`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the comparison narrows the double limit to a float"
    )]
    let max_cost = (f64::from(best.cost) * at_most_longer) as f32;
    // The first connection that isn't cheaper than the limit.
    let end = connections.partition_point(|c| c.cost < max_cost);
    connections.truncate(end);
}

/// The cost of a path between two of its edges (`get_segment_cost`).
fn get_segment_cost(path: &[PathEdge], first: usize, last: usize) -> Cost {
    let (Some(start), Some(end)) = (path.get(first), path.get(last)) else {
        return Cost::ZERO;
    };
    let mut cost = end.elapsed_cost - start.transition_cost;
    if let Some(previous) = first.checked_sub(1).and_then(|i| path.get(i)) {
        cost -= previous.elapsed_cost;
    }
    cost
}

/// The part where two paths differ, as index ranges into each (`find_diff_segment`).
/// Bidirectional search makes alternates that differ in one stretch.
fn find_diff_segment(path_1: &[PathEdge], path_2: &[PathEdge]) -> ((usize, usize), (usize, usize)) {
    let mut first_1 = 0;
    let mut first_2 = 0;
    while first_1 < path_1.len()
        && first_2 < path_2.len()
        && path_1[first_1].edge_id == path_2[first_2].edge_id
    {
        first_1 += 1;
        first_2 += 1;
    }
    if first_1 == path_1.len() {
        return (
            (first_1, first_1),
            (first_2, first_2.max(path_2.len().saturating_sub(1))),
        );
    } else if first_2 == path_2.len() {
        return (
            (first_1, first_1.max(path_1.len().saturating_sub(1))),
            (first_2, first_2),
        );
    }
    let mut last_1 = path_1.len() - 1;
    let mut last_2 = path_2.len() - 1;
    while last_1 > first_1 && last_2 > first_2 && path_1[last_1].edge_id == path_2[last_2].edge_id {
        last_1 -= 1;
        last_2 -= 1;
    }
    ((first_1, last_1), (first_2, last_2))
}

/// Whether the candidate's detour is reasonable next to the best path
/// (`validate_alternate_by_stretch`).
pub(crate) fn validate_alternate_by_stretch(optimal: &[PathEdge], candidate: &[PathEdge]) -> bool {
    let (optimal_segment, candidate_segment) = find_diff_segment(optimal, candidate);
    if optimal_segment.0 == optimal.len() {
        // Equal paths pass; a candidate that extends the best path doesn't.
        return candidate_segment.0 >= candidate.len();
    }
    let optimal_cost = get_segment_cost(optimal, optimal_segment.0, optimal_segment.1);
    let candidate_cost = get_segment_cost(candidate, candidate_segment.0, candidate_segment.1);
    // Mainline rejects only a longer detour (not one whose cost is NaN).
    !matches!(
        (AT_MOST_LONGER_DETOUR * optimal_cost.cost).partial_cmp(&candidate_cost.cost),
        Some(core::cmp::Ordering::Less)
    )
}

/// Whether the candidate shares at most `at_most_shared` of each chosen path's length
/// with it (`validate_alternate_by_sharing`).
///
/// `shared_edge_ids` caches the edges of each chosen path between calls.
pub(crate) fn validate_alternate_by_sharing(
    shared_edge_ids: &mut Vec<HashSet<GraphId>>,
    paths: &[Vec<PathEdge>],
    candidate: &[PathEdge],
    at_most_shared: f32,
) -> bool {
    if paths.len() > shared_edge_ids.len() {
        shared_edge_ids.resize_with(paths.len(), HashSet::new);
    }
    for (path, shared) in paths.iter().zip(shared_edge_ids.iter_mut()) {
        if shared.is_empty() {
            shared.extend(path.iter().map(|p| p.edge_id));
        }
        let mut shared_length = 0.0f32;
        let mut previous: Option<f32> = None;
        for edge in candidate {
            let length = match previous {
                None => edge.path_distance,
                Some(p) => edge.path_distance - p,
            };
            previous = Some(edge.path_distance);
            if shared.contains(&edge.edge_id) {
                shared_length += length;
            }
        }
        let total = path.last().map_or(0.0, |p| p.path_distance);
        if shared_length > at_most_shared * total {
            return false;
        }
    }
    true
}

/// Local optimality (`validate_alternate_by_local_optimality`), which mainline doesn't
/// implement: every candidate passes.
pub(crate) fn validate_alternate_by_local_optimality(_candidate: &[PathEdge]) -> bool {
    true
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "exact thresholds")]
mod tests {
    use super::*;
    use valinor_sif::TravelMode;

    fn edge(id: u64, cost: f32, transition: f32, distance: f32) -> PathEdge {
        PathEdge {
            edge_id: GraphId::try_from_id(id).unwrap(),
            mode: TravelMode::Drive,
            elapsed_cost: Cost::new(cost, cost),
            transition_cost: Cost::new(transition, transition),
            path_distance: distance,
            restriction_index: u8::MAX,
            start_node_is_recovered: false,
        }
    }

    #[test]
    fn stretch_thresholds_match_mainline() {
        assert_eq!(get_at_most_longer(100.0), 2.0);
        // The fit is approximate: 1.71 at 20 minutes.
        assert!((get_at_most_longer(1200.0) - 1.75).abs() < 0.05);
        assert!((get_at_most_longer(3600.0) - 1.4).abs() < 0.03);
        assert_eq!(get_at_most_longer(20_000.0), 1.25);

        let id = GraphId::try_from_id(1).unwrap();
        let c = |cost| CandidateConnection {
            edge_id: id,
            opp_edge_id: id,
            cost,
        };
        let mut connections = vec![c(300.0), c(100.0), c(250.0), c(200.0), c(199.0)];
        filter_alternates_by_stretch(&mut connections);
        let costs: Vec<f32> = connections.iter().map(|c| c.cost).collect();
        assert_eq!(costs, [100.0, 199.0]);
    }

    #[test]
    fn detours_and_sharing() {
        let best = [
            edge(1, 10.0, 0.0, 100.0),
            edge(2, 20.0, 1.0, 200.0),
            edge(3, 30.0, 0.0, 300.0),
        ];
        let detour = [
            edge(1, 10.0, 0.0, 100.0),
            edge(4, 25.0, 1.0, 180.0),
            edge(5, 40.0, 1.0, 260.0),
            edge(3, 50.0, 0.0, 360.0),
        ];
        // The best path's middle costs 10 (20 - 10, less its transition of 1: 9);
        // the detour's costs 30 - 1 = 29 > 2 * 9.
        assert!(!validate_alternate_by_stretch(&best, &detour));
        assert!(validate_alternate_by_stretch(&best, &best));
        // An alternate that only extends the best path fails.
        assert!(!validate_alternate_by_stretch(&best[..2], &best));
        let mut cache = Vec::new();
        let paths = vec![best.to_vec()];
        // The detour shares edges 1 and 3: 100 + 100 = 200 of the best path's 300.
        assert!(!validate_alternate_by_sharing(
            &mut cache, &paths, &detour, 0.6
        ));
        assert!(validate_alternate_by_sharing(
            &mut cache, &paths, &detour, 0.7
        ));
    }
}
