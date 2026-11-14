//! What a match produces (mainline's `meili/match_result.h` and `meili/stateid.h`):
//! a [`MatchResult`] per measurement, the [`EdgeSegment`]s of the matched path, and the score.
//!
//! These are the interface between Meili, Thor, and Tyr, so their fields follow mainline's.
//! Mainline's sentinels become `Option`s:
//! an invalid edge ID or state ID is `None`, and so is an unset edge index.

use valhalla_graphtile::GraphId;

/// A state of the Viterbi search: a candidate edge for a measurement
/// (mainline's `StateId`).
///
/// `time` is the index of the measurement's column in the search
/// (several measurements can share one when they are close together),
/// and `id` is the index of the candidate within that column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateId {
    pub time: u32,
    pub id: u32,
}

/// What kind of match a measurement got (mainline's `MatchResult::Type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchResultType {
    /// No edge: the measurement matched nothing.
    Unmatched,
    /// An edge but no state: the measurement was too close to a neighbor to get a column
    /// of its own, and was interpolated onto the path.
    Interpolated,
    /// An edge from the measurement's own state.
    Matched,
}

/// The match of one measurement (mainline's `meili::MatchResult`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchResult {
    /// The matched point: longitude and latitude.
    pub lnglat: (f64, f64),
    /// Meters from the measurement to the matched point.
    pub distance_from: f64,
    /// The edge the point is on, if any.
    pub edge_id: Option<GraphId>,
    /// How far along the edge the point is, from 0 to 1.
    pub distance_along: f64,
    /// The measurement's epoch time (seconds), copied from it; -1 when it has none.
    pub epoch_time: f64,
    /// The state the match came from, if it came from one.
    pub state_id: Option<StateId>,
    /// Whether the measurement was a `break` or `break_through` location.
    pub is_break_point: bool,
    /// Whether a discontinuity starts at this point.
    pub begins_discontinuity: bool,
    /// Whether a discontinuity ends at this point.
    pub ends_discontinuity: bool,
    /// The index of the point's edge in the full list of edges of the path,
    /// across discontinuities (set by Thor for `trace_attributes`).
    pub edge_index: Option<usize>,
}

impl MatchResult {
    /// Whether the match came from a state (mainline's `HasState`).
    #[inline]
    #[must_use]
    pub fn has_state(&self) -> bool {
        self.state_id.is_some()
    }

    /// What kind of match this is (mainline's `GetType`).
    #[inline]
    #[must_use]
    pub fn match_type(&self) -> MatchResultType {
        match (self.edge_id, self.state_id) {
            (Some(_), Some(_)) => MatchResultType::Matched,
            (Some(_), None) => MatchResultType::Interpolated,
            (None, _) => MatchResultType::Unmatched,
        }
    }
}

/// A part of an edge on the matched path (mainline's `meili::EdgeSegment`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeSegment {
    /// The edge.
    pub edge_id: GraphId,
    /// Where the segment starts along the edge, from 0 to 1.
    pub source: f64,
    /// Where it ends, from 0 to 1.
    pub target: f64,
    /// The index of the first match result on the segment, if any.
    pub first_match_idx: Option<usize>,
    /// The index of the last match result on the segment, if any.
    pub last_match_idx: Option<usize>,
    /// The access restriction seen on the edge (an index among the edge's restrictions),
    /// or `u8::MAX` for none (mainline's `restriction_idx`, as in Thor's `PathEdge`).
    pub restriction_idx: u8,
    /// Whether the path is broken after this segment (no route to the next one).
    pub discontinuity: bool,
}

/// One match of a whole trace (mainline's `meili::MatchResults`):
/// the result of each measurement, the segments of the path, and its score.
///
/// Mainline also keeps the path's edge sequence (`edges`, `e1`, and `e2`)
/// to tell top-k results apart; that belongs to the search, not to this interface.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchResults {
    /// A result per measurement, in the trace's order.
    pub results: Vec<MatchResult>,
    /// The matched path.
    pub segments: Vec<EdgeSegment>,
    /// The path's score (lower is better).
    pub score: f32,
}
