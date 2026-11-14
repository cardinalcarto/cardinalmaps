//! The paths of a map match (mainline's `thor::MapMatcher::FormPath`, `map_matcher.cc`):
//! Meili's matched edge segments become path edges, with the elapsed cost along them,
//! split into a path at every discontinuity and every break point.
//!
//! The costs follow mainline's arithmetic: each segment's partial edge cost at the time it is
//! reached, the transition onto it from the previous segment (not on the first segment of a
//! path), and, with `use_timestamps` on a trace without discontinuities, the trace's own times
//! (interpolated along the path, `interpolate_matches`) in place of the elapsed times.
//! Each path restarts the elapsed cost, and each path after the first gets its first point's
//! time as a `date_time`.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::Options;
use valinor_meili::{EdgeSegment, MatchResult};
use valinor_sif::{AnyCosting, Cost, Costing, EdgeLabel, InternalTurn, LabelState, TimeInfo};

use crate::error::ThorError;
use crate::graph;
use crate::path::PathEdge;
use crate::route_matcher::start_time_info;
use crate::time::time_info_date_time;

/// One path of a match: its edges, and the index of the segment each came from.
pub(crate) struct MatchedPath {
    pub(crate) edges: Vec<PathEdge>,
    pub(crate) segments: Vec<usize>,
}

/// A point of a matched path, for timestamps (mainline's `interpolation_t`).
#[derive(Clone, Copy, Debug)]
struct Interpolation {
    edge: GraphId,
    total_distance: f64,
    /// The index of the measurement.
    original_index: usize,
    epoch_time: f64,
}

/// A node's time zone index, 0 for none.
#[inline]
fn tz_of(node: &NodeInfo) -> u32 {
    node.time_zone_index().map_or(0, |i| u32::from(i.get()))
}

/// An edge's length, as a float.
fn edge_length<P: GraphTileProvider>(provider: &P, edge_id: GraphId) -> Result<f32, ThorError> {
    let tile = graph::tile(provider, edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
    #[expect(clippy::cast_precision_loss, reason = "mainline's float")]
    let length = graph::edge(&tile, edge_id)?.length() as f32;
    Ok(length)
}

/// Whether `edge2` leaves the end node of `edge1`, on `edge2`'s level
/// (mainline's `GraphReader::AreEdgesConnectedForward`).
///
/// As in mainline, only `edge2`'s index is compared with the node's edges, not its tile.
fn are_edges_connected_forward<P: GraphTileProvider>(
    provider: &P,
    edge1: GraphId,
    edge2: GraphId,
) -> Result<bool, ThorError> {
    let tile = graph::tile(provider, edge1)?.ok_or(ThorError::TileGone(edge1))?;
    let mut end_node = graph::edge(&tile, edge1)?.end_node_id();
    let Some(mut tile) = graph::tile_for(provider, &tile, end_node)? else {
        return Ok(false);
    };
    if edge2.level() != end_node.level() {
        let node = graph::node(&tile, end_node)?;
        if let Some(transition) = tile
            .get_transitions(node)
            .iter()
            .find(|t| t.corresponding_end_node_id().level() == edge2.level())
        {
            end_node = transition.corresponding_end_node_id();
            let Some(next) = graph::tile(provider, end_node)? else {
                return Ok(false);
            };
            tile = next;
        }
    }
    let node = graph::node(&tile, end_node)?;
    let first = u64::from(node.edge_index());
    Ok(first <= edge2.feature_index()
        && edge2.feature_index() < first + u64::from(node.edge_count()))
}

/// The distance and time of every match along each stretch of connected segments, with the
/// times of points without one interpolated by distance (mainline's `interpolate_matches`).
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
fn interpolate_matches<P: GraphTileProvider>(
    provider: &P,
    matches: &[MatchResult],
    edges: &[EdgeSegment],
) -> Result<Vec<Vec<Interpolation>>, ThorError> {
    let mut interpolations = Vec::with_capacity(edges.len());
    let first_source = edges.first().map_or(0.0, |e| e.source);
    let mut idx = 0;
    let mut begin = 0;
    while begin < edges.len() {
        // The end of this stretch of connected edges.
        let mut end = begin + 1;
        while end < edges.len()
            && are_edges_connected_forward(provider, edges[end - 1].edge_id, edges[end].edge_id)?
        {
            end += 1;
        }

        let mut interpolated: Vec<Interpolation> = Vec::with_capacity(matches.len());
        let mut last_idx = idx;
        for (at, segment) in edges.iter().enumerate().take(end).skip(begin) {
            let edge_length = edge_length(provider, segment.edge_id)?;
            // Mainline starts every stretch from the first segment's source.
            #[expect(clippy::cast_possible_truncation, reason = "mainline's float")]
            let total_length: f32 = if at == begin {
                (-first_source * f64::from(edge_length)) as f32
            } else {
                interpolated.last().map_or(0.0, |i| i.total_distance) as f32
            };
            interpolated.push(Interpolation {
                edge: segment.edge_id,
                total_distance: f64::from(total_length),
                original_index: last_idx,
                epoch_time: -1.0,
            });
            // Every match on this edge.
            while idx < matches.len() {
                let m = &matches[idx];
                match m.edge_id {
                    // Unmatched points: we don't know what edge they were on.
                    None => {
                        idx += 1;
                        continue;
                    }
                    Some(e) if e != segment.edge_id => break,
                    Some(_) => {}
                }
                interpolated.push(Interpolation {
                    edge: segment.edge_id,
                    total_distance: m.distance_along * f64::from(edge_length)
                        + f64::from(total_length),
                    original_index: idx,
                    epoch_time: m.epoch_time,
                });
                last_idx = idx;
                idx += 1;
            }
            interpolated.push(Interpolation {
                edge: segment.edge_id,
                total_distance: f64::from(edge_length + total_length),
                original_index: last_idx,
                epoch_time: -1.0,
            });
        }

        // Backfill the times of the points that have none.
        let len = interpolated.len();
        let mut backfill = 0;
        while backfill < len {
            if interpolated[backfill].epoch_time != -1.0 {
                backfill += 1;
                continue;
            }
            let left = backfill.saturating_sub(1);
            let mut right = backfill + 1;
            while right < len && interpolated[right].epoch_time == -1.0 {
                right += 1;
            }
            while backfill != right {
                // `left != begin()` and `right != end()`.
                let has_left = left != 0;
                let has_right = right != len;
                if has_left && has_right {
                    let (l, r) = (interpolated[left], interpolated[right]);
                    let time_diff = r.epoch_time - l.epoch_time;
                    #[expect(clippy::cast_possible_truncation, reason = "mainline's floats")]
                    let distance_diff = (r.total_distance - l.total_distance) as f32;
                    #[expect(clippy::cast_possible_truncation, reason = "mainline's floats")]
                    let distance_ratio: f32 = if distance_diff > 0.0 {
                        ((interpolated[backfill].total_distance - l.total_distance)
                            / f64::from(distance_diff)) as f32
                    } else {
                        0.0
                    };
                    interpolated[backfill].epoch_time =
                        l.epoch_time + f64::from(distance_ratio) * time_diff;
                } else if has_left
                    && interpolated[backfill].total_distance == interpolated[left].total_distance
                {
                    interpolated[backfill].epoch_time = interpolated[left].epoch_time;
                    interpolated[backfill].original_index = interpolated[left].original_index;
                } else if has_right
                    && interpolated[backfill].total_distance == interpolated[right].total_distance
                {
                    interpolated[backfill].epoch_time = interpolated[right].epoch_time;
                    interpolated[backfill].original_index = interpolated[right].original_index;
                }
                backfill += 1;
            }
        }
        interpolations.push(interpolated);
        begin = end;
    }
    Ok(interpolations)
}

/// The time at the start of the match (`init_time_info`): the end node of the first segment
/// with a time zone gives it, and a shape that has point times but no `date_time` gets its
/// first point's time as a date and time in that zone.
fn init_time_info<P: GraphTileProvider>(
    provider: &P,
    segments: &[EdgeSegment],
    options: &mut Options,
    now: jiff::Timestamp,
) -> Result<TimeInfo, ThorError> {
    for segment in segments {
        let Some(tile) = graph::tile(provider, segment.edge_id)? else {
            continue;
        };
        let end = graph::edge(&tile, segment.edge_id)?.end_node_id();
        let Some(end_tile) = graph::tile_for(provider, &tile, end)? else {
            continue;
        };
        let tz = tz_of(graph::node(&end_tile, end)?);
        if valinor_sif::datetime::time_zone(tz).is_none() {
            continue;
        }
        return start_time_info(provider, options, tz, now);
    }
    Ok(TimeInfo::INVALID)
}

/// Turns a match into paths (mainline's `MapMatcher::FormPath`): a new path at the first
/// segment, after each discontinuity, and at each break point.
///
/// Each path after the first sets its first shape point's `date_time`, when the trace has a
/// time; the first shape point may get one from its `time`.
///
/// # Errors
///
/// Fails if a segment's tile, or the tile of its end node, is missing
/// (where mainline would dereference a null tile), or can't be read.
#[expect(clippy::too_many_lines, reason = "mainline's function, step by step")]
pub(crate) fn form_path<P: GraphTileProvider>(
    provider: &P,
    costing: &AnyCosting,
    results: &[MatchResult],
    segments: &[EdgeSegment],
    options: &mut Options,
    now: jiff::Timestamp,
) -> Result<Vec<MatchedPath>, ThorError> {
    let time_info = init_time_info(provider, segments, options, now)?;

    let mut interpolations = Vec::new();
    let mut last_interp_index = 0;
    let mut use_timestamps = options.use_timestamps;
    if use_timestamps {
        interpolations = interpolate_matches(provider, results, segments)?;
        // A discontinuity: use the costs instead.
        if let [only] = interpolations.as_slice() {
            last_interp_index = only.last().map_or(0, |i| i.original_index);
        } else {
            use_timestamps = false;
        }
    }

    let base = costing.base();
    let mode = base.travel_mode();
    let mut elapsed = Cost::ZERO;
    let mut accumulated_elapsed = Cost::ZERO;
    let mut paths: Vec<MatchedPath> = Vec::new();
    // The node at the end of the previous segment, and that segment's label.
    let mut prior: Option<(NodeInfo, EdgeLabel)> = None;
    let mut prev_segment: Option<&EdgeSegment> = None;
    let mut interpolated_index = 0;
    let epoch = |i: usize| results.get(i).map_or(0.0, |r| r.epoch_time);

    for (segment_index, segment) in segments.iter().enumerate() {
        let edge_id = segment.edge_id;
        let tile = graph::tile(provider, edge_id)?.ok_or(ThorError::TileGone(edge_id))?;
        let de = graph::edge(&tile, edge_id)?.clone();

        let disconnected = prev_segment.is_some_and(|p| p.discontinuity);
        let break_point = segment
            .first_match_idx
            .and_then(|i| results.get(i))
            .is_some_and(|r| r.is_break_point);
        let new_leg = disconnected || prev_segment.is_none() || break_point;

        // The time now (the first segment's is the start).
        let offset_time_info = match &prior {
            Some((node, _)) => {
                time_info.forward(accumulated_elapsed.secs + elapsed.secs, tz_of(node))
            }
            None => time_info,
        };

        if new_leg {
            if offset_time_info.valid
                && prev_segment.is_some()
                && let Some(location) = segment
                    .first_match_idx
                    .and_then(|i| options.shape.get_mut(i))
            {
                location.date_time = time_info_date_time(&offset_time_info);
            }
            paths.push(MatchedPath {
                edges: Vec::with_capacity(segments.len() - segment_index),
                segments: Vec::with_capacity(segments.len() - segment_index),
            });
            accumulated_elapsed += elapsed;
            elapsed = Cost::ZERO;
        }

        // The transition onto the edge (none at the start of a path).
        let mut transition_cost = Cost::ZERO;
        if elapsed.secs > 0.0
            && let Some((node, pred)) = &prior
        {
            transition_cost = costing.transition_cost(&de, &tile, node, pred, provider, false)?;
            elapsed += transition_cost;
        }

        // The time along the edge, which may be partial at the ends.
        #[expect(clippy::cast_possible_truncation, reason = "mainline passes floats")]
        let partial = costing.partial_edge_cost(
            &de,
            edge_id,
            Some(edge_id),
            &tile,
            offset_time_info.seconds_of_week(),
            segment.source as f32,
            segment.target as f32,
        )?;
        elapsed += partial.cost;

        // With timestamps: the time at the first interpolation past this edge
        // (or at the last one).
        if use_timestamps && let Some(points) = interpolations.first() {
            let mut idx = last_interp_index;
            for (i, point) in points.iter().enumerate().skip(interpolated_index) {
                if point.edge != edge_id {
                    idx = point.original_index;
                    interpolated_index = i;
                    break;
                }
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline stores the double difference in a float"
            )]
            let secs = (epoch(idx) - epoch(0)) as f32;
            elapsed.secs = secs;
        }

        let turn = match &prior {
            Some((node, pred)) => {
                base.turn_type(valinor_sif::Label::opp_local_idx(pred), node, &de, None)
            }
            None => InternalTurn::NoTurn,
        };
        let label = EdgeLabel::new(
            edge_id,
            &de,
            LabelState {
                predecessor: None,
                mode,
                path_distance: 0,
                closure_pruning: true,
                has_measured_speed: !partial.flow_sources.is_empty(),
                internal_turn: turn,
                destonly: base.is_hgv() && de.dest_only_hgv(),
            },
        );
        let path = paths.last_mut().ok_or(ThorError::EmptyLeg)?;
        path.edges.push(PathEdge {
            edge_id,
            mode,
            elapsed_cost: elapsed,
            transition_cost,
            path_distance: 0.0,
            restriction_index: segment.restriction_idx,
            start_node_is_recovered: false,
            is_disconnected: false,
        });
        path.segments.push(segment_index);

        prev_segment = Some(segment);
        let end = de.end_node_id();
        let end_tile = graph::tile_for(provider, &tile, end)?.ok_or(ThorError::TileGone(end))?;
        prior = Some((graph::node(&end_tile, end)?.clone(), label));
    }
    Ok(paths)
}
