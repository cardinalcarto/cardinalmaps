//! The matched path as edge segments (mainline's `meili/match_route.cc`),
//! and the edge sequences mainline compares paths by (`path_t` and `MatchResults::operator==`).

use valhalla_graphtile::GraphId;

use crate::error::MeiliError;
use crate::match_result::{EdgeSegment, MatchResult, StateId};
use crate::state::Container;

/// No access restriction on a segment (`baldr::kInvalidRestriction`).
pub(crate) const NO_RESTRICTION: u8 = u8::MAX;

/// Mainline's `EdgeSegment` constructor check.
fn segment(
    edge_id: GraphId,
    source: f64,
    target: f64,
    restriction_idx: u8,
) -> Result<EdgeSegment, MeiliError> {
    if !(0.0 <= source && source <= target && target <= 1.0) {
        return Err(MeiliError::Logic("expect 0 <= source <= target <= 1"));
    }
    Ok(EdgeSegment {
        edge_id,
        source,
        target,
        first_match_idx: None,
        last_match_idx: None,
        restriction_idx,
        discontinuity: false,
    })
}

/// Appends the segments of the route from `source` to `target` (`MergeRoute`);
/// false if there is no route.
///
/// A route that covers no edge (its origin label is its only one) becomes a segment of no
/// length at `target_result`.
///
/// # Errors
///
/// Fails where mainline's `EdgeSegment` constructor throws (a trivial route to a result
/// without an edge, or one outside its edge).
pub(crate) fn merge_route(
    container: &Container,
    source: StateId,
    target: StateId,
    route: &mut Vec<EdgeSegment>,
    target_result: &MatchResult,
) -> Result<bool, MeiliError> {
    let mut labels = container.route(source, target).peekable();
    if labels.peek().is_none() {
        return Ok(false);
    }
    let start = route.len();
    // Every label but the route's origin, which is a placeholder (at the first state) or the
    // last label of the route before.
    while let Some(label) = labels.next() {
        if labels.peek().is_none() {
            break;
        }
        let edge_id = label.edge_id.ok_or(MeiliError::Logic("Invalid edgeid"))?;
        route.push(segment(
            edge_id,
            f64::from(label.source),
            f64::from(label.target),
            label.restriction_idx,
        )?);
    }
    if route.len() == start {
        let edge_id = target_result
            .edge_id
            .ok_or(MeiliError::Logic("Invalid edgeid"))?;
        let along = target_result.distance_along;
        route.push(segment(edge_id, along, along, NO_RESTRICTION)?);
    }
    route[start..].reverse();
    Ok(true)
}

/// Splits the segments between two matched results at the break points between them,
/// and marks which results are on which segments (`cut_segments`).
///
/// # Errors
///
/// Fails, as mainline does ("unexpectedly unable to locate target edge"), if a result's edge
/// isn't among the segments after the previous result's (for example, an unmatched result
/// between two matched ones).
pub(crate) fn cut_segments(
    results: &[MatchResult],
    first_idx: usize,
    last_idx: usize,
    segments: &mut [EdgeSegment],
    new_segments: &mut Vec<EdgeSegment>,
) -> Result<(), MeiliError> {
    let mut first_segment = 0;
    let mut search = 0;
    let mut prev_idx = first_idx;
    for curr_idx in first_idx + 1..=last_idx {
        let curr = results
            .get(curr_idx)
            .ok_or(MeiliError::Logic("no such match result"))?;
        let prev = results
            .get(prev_idx)
            .ok_or(MeiliError::Logic("no such match result"))?;
        let prev_gt_curr = prev.distance_along > curr.distance_along + 1e-3;
        // Back on the same edge, behind: a loop, so look past the current segment.
        let looped = prev.edge_id == curr.edge_id && prev_gt_curr;
        search += usize::from(looped);
        while search < segments.len() && Some(segments[search].edge_id) != curr.edge_id {
            search += 1;
        }
        if search >= segments.len() {
            return Err(MeiliError::Logic(
                "In meili::cutsegments(), unexpectedly unable to locate target edge.",
            ));
        }
        if !curr.is_break_point && curr_idx != last_idx {
            let s = &mut segments[search];
            if s.first_match_idx.is_none() {
                s.first_match_idx = Some(curr_idx);
            }
            s.last_match_idx = Some(curr_idx);
            continue;
        }
        let old_size = new_segments.len();
        new_segments.extend_from_slice(&segments[first_segment..=search]);
        let first = &mut new_segments[old_size];
        first.first_match_idx = Some(prev_idx);
        // An interpolated point's own position; a matched one may be at a node,
        // which the segment's source or target places on the right side.
        first.source = if prev.has_state() {
            segments[first_segment].source
        } else {
            prev.distance_along
        };
        if let Some(last) = new_segments.last_mut() {
            last.last_match_idx = Some(curr_idx);
            last.target = if curr.has_state() {
                segments[search].target
            } else {
                curr.distance_along
            };
        }
        first_segment = search;
        prev_idx = curr_idx;
    }
    Ok(())
}

/// The segments of the matched path (`ConstructRoute`): the routes between consecutive
/// matched results, cut at break points, with a discontinuity where there is no route.
///
/// # Errors
///
/// As for [`merge_route`] and [`cut_segments`].
pub(crate) fn construct_route(
    container: &Container,
    results: &[MatchResult],
) -> Result<Vec<EdgeSegment>, MeiliError> {
    let mut route: Vec<EdgeSegment> = Vec::new();
    let mut segments = Vec::new();
    let mut new_segments = Vec::new();
    let mut prev: Option<usize> = None;
    for (curr_idx, m) in results.iter().enumerate() {
        let Some(state) = m.state_id.filter(|_| m.edge_id.is_some()) else {
            continue;
        };
        if let Some(prev_idx) = prev
            && let Some(prev_state) = results[prev_idx].state_id
        {
            segments.clear();
            if !merge_route(container, prev_state, state, &mut segments, m)? {
                // Only a discontinuity after the start of the route.
                if let Some(last) = route.last_mut() {
                    last.discontinuity = true;
                }
                prev = Some(curr_idx);
                continue;
            }
            new_segments.clear();
            cut_segments(
                results,
                prev_idx,
                curr_idx,
                &mut segments,
                &mut new_segments,
            )?;
            let prev_break = results[prev_idx].is_break_point;
            if let (Some(last), Some(front)) = (route.last().copied(), new_segments.first_mut())
                && !prev_break
                && !last.discontinuity
                && last.edge_id == front.edge_id
            {
                // Merge the route's last segment into the first new one.
                front.source = last.source;
                if last.first_match_idx.is_some() {
                    front.first_match_idx = last.first_match_idx;
                }
                if front.last_match_idx.is_none() {
                    front.last_match_idx = last.last_match_idx;
                }
                route.pop();
            }
            route.extend_from_slice(&new_segments);
        }
        prev = Some(curr_idx);
    }
    Ok(route)
}

/// A path's edges, without consecutive repeats, and the range of them that counts:
/// an edge the path only touches at its end (a first segment starting at 1, or a last one
/// ending at 0) is left out (mainline's `path_t` and `MatchResults::edges`, `e1`, and `e2`).
#[derive(Clone, Debug, Default)]
pub(crate) struct EdgeSequence {
    pub(crate) edges: Vec<GraphId>,
    e1: usize,
    e2: usize,
}

/// `std::search(hay, needle)`: the index of the first match in `hay[h1..h2]`, `h2` if none,
/// and `h1` for an empty needle.
fn search(hay: &[GraphId], h1: usize, h2: usize, needle: &[GraphId]) -> usize {
    if needle.is_empty() {
        return h1;
    }
    // A reversed range (which only an edge sequence of one touched edge can have)
    // is an empty one.
    let range = hay.get(h1..h2.max(h1)).unwrap_or(&[]);
    range
        .windows(needle.len())
        .position(|w| w == needle)
        .map_or(h2, |i| h1 + i)
}

impl EdgeSequence {
    pub(crate) fn new(segments: &[EdgeSegment]) -> Self {
        let mut edges: Vec<GraphId> = Vec::with_capacity(segments.len());
        for s in segments {
            if edges.last() != Some(&s.edge_id) {
                edges.push(s.edge_id);
            }
        }
        let e1 = match segments.first() {
            Some(s) if s.source >= 1.0 => 1,
            _ => 0,
        };
        let e2 = match segments.last() {
            Some(s) if s.target <= 0.0 => edges.len().saturating_sub(1),
            _ => edges.len(),
        };
        Self { edges, e1, e2 }
    }

    fn range(&self) -> &[GraphId] {
        self.edges.get(self.e1..self.e2.max(self.e1)).unwrap_or(&[])
    }

    /// Whether neither path's counted edges contain the other's (`path_t::operator!=`).
    pub(crate) fn differs(&self, p: &Self) -> bool {
        search(&self.edges, self.e1, self.e2, p.range()) == self.e2
            && search(&p.edges, p.e1, p.e2, self.range()) == p.e2
    }

    /// Whether the paths count as the same (`MatchResults::operator==`).
    ///
    /// Mainline compares the search's result with the end of all edges rather than with the
    /// end of the counted ones, so a path whose last segment ends at 0 equals any path
    /// that fits in it; that is reproduced, since it decides which alternates are returned.
    pub(crate) fn same(&self, p: &Self) -> bool {
        (p.edges.len() <= self.edges.len()
            && search(&self.edges, self.e1, self.e2, p.range()) != self.edges.len())
            || (self.edges.len() <= p.edges.len()
                && search(&p.edges, p.e1, p.e2, self.range()) != p.edges.len())
    }

    /// Whether any of the candidate's edges is on the path (`path_t::crosses`).
    pub(crate) fn crosses(&self, candidate: &crate::candidate_search::Candidate) -> bool {
        candidate
            .edges
            .iter()
            .any(|e| self.edges.contains(&e.edge_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u64) -> GraphId {
        GraphId::try_from_components(2, 1, i).unwrap()
    }

    fn seg(i: u64, source: f64, target: f64) -> EdgeSegment {
        segment(id(i), source, target, NO_RESTRICTION).unwrap()
    }

    fn result(edge: Option<u64>, along: f64, state: bool, brk: bool) -> MatchResult {
        MatchResult {
            lnglat: (0.0, 0.0),
            distance_from: 0.0,
            edge_id: edge.map(id),
            distance_along: along,
            epoch_time: -1.0,
            state_id: state.then_some(StateId { time: 0, id: 0 }),
            is_break_point: brk,
            begins_discontinuity: false,
            ends_discontinuity: false,
            edge_index: None,
        }
    }

    #[test]
    fn cuts_at_break_points_and_marks_interpolated_results() {
        let results = [
            result(Some(1), 0.5, true, false),
            result(Some(2), 0.3, false, false),
            result(Some(2), 0.6, false, true),
            result(Some(3), 0.2, true, false),
        ];
        let mut segments = vec![seg(1, 0.5, 1.0), seg(2, 0.0, 1.0), seg(3, 0.0, 0.2)];
        let mut out = Vec::new();
        cut_segments(&results, 0, 3, &mut segments, &mut out).unwrap();
        // The break point splits edge 2 in two at its own position.
        let edges: Vec<_> = out.iter().map(|s| s.edge_id).collect();
        assert_eq!(edges, [id(1), id(2), id(2), id(3)]);
        assert_eq!(
            (out[0].source, out[0].first_match_idx, out[0].last_match_idx),
            (0.5, Some(0), None)
        );
        assert_eq!(
            (out[1].first_match_idx, out[1].last_match_idx),
            (Some(1), Some(2))
        );
        assert_eq!((out[1].source, out[1].target), (0.0, 0.6));
        // The second half is a copy of the marked segment, so it keeps its last index (1),
        // as in mainline.
        assert_eq!((out[2].source, out[2].target), (0.6, 1.0));
        assert_eq!(
            (out[2].first_match_idx, out[2].last_match_idx),
            (Some(2), Some(1))
        );
        assert_eq!(
            (out[3].target, out[3].first_match_idx, out[3].last_match_idx),
            (0.2, None, Some(3))
        );
        // An unmatched result in between fails, as in mainline.
        let results = [
            result(Some(1), 0.5, true, false),
            result(None, -1.0, false, false),
            result(Some(1), 0.7, true, false),
        ];
        let mut segments = vec![seg(1, 0.5, 0.7)];
        assert!(cut_segments(&results, 0, 2, &mut segments, &mut Vec::new()).is_err());
    }

    #[test]
    fn compares_edge_sequences_as_mainline() {
        let a = EdgeSequence::new(&[seg(1, 0.2, 1.0), seg(2, 0.0, 1.0), seg(3, 0.0, 0.5)]);
        let b = EdgeSequence::new(&[seg(2, 0.1, 0.9)]);
        let c = EdgeSequence::new(&[seg(4, 0.1, 0.9)]);
        assert!(a.same(&b) && b.same(&a));
        assert!(!a.same(&c));
        assert!(!a.differs(&b));
        assert!(a.differs(&c));
        // Touching the next edge at its start doesn't count, but makes the sequence equal
        // to anything that fits in it (mainline compares with the end of all edges).
        let d = EdgeSequence::new(&[seg(1, 0.2, 1.0), seg(5, 0.0, 0.0)]);
        assert_eq!(d.range(), [id(1)]);
        assert!(d.same(&c));
        assert!(EdgeSequence::new(&[]).same(&a));
    }
}
