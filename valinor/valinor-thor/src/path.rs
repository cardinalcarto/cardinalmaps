//! What path search produces: the edges of each leg of each route,
//! with what trip building (mainline's `TripLegBuilder`) needs to turn them into `TripLeg`s.

use valhalla_graphtile::GraphId;
use valinor_sif::{Cost, TravelMode};

/// One directed edge of a path (mainline's `thor::PathInfo`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathEdge {
    /// The directed edge.
    pub edge_id: GraphId,
    /// The travel mode along the edge.
    pub mode: TravelMode,
    /// The cost and time from the start of the leg to the end of this edge
    /// (or to where the leg ends on it), including the transition onto it.
    ///
    /// Through locations don't restart it: the legs between them are joined
    /// as mainline does, adding the cost so far.
    pub elapsed_cost: Cost,
    /// The cost and time of the transition onto this edge (zero on the first edge).
    pub transition_cost: Cost,
    /// Meters from the start of the leg to the end of this edge, as mainline counts them.
    ///
    /// Mainline warns that this "is not correct across the board" once legs are joined;
    /// trip building doesn't read it.
    pub path_distance: f32,
    /// The access restriction seen on the edge (an index among the edge's restrictions),
    /// or `u8::MAX` for none (mainline's `restriction_index`).
    pub restriction_index: u8,
    /// Whether the edge came from recovering a shortcut, and isn't its first edge
    /// (mainline's `start_node_is_recovered`).
    pub start_node_is_recovered: bool,
    /// Whether the edge doesn't continue from the previous one: map matching starts it after
    /// a discontinuity, so trip building starts it at its own start node and includes its
    /// first shape point (mainline's `is_disconnected`). Path search never sets it.
    pub is_disconnected: bool,
}

/// How much of an edge a leg uses where it meets an intermediate location
/// (mainline's `EdgeTrimmingInfo`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeTrimming {
    /// Whether to trim the edge here at all.
    pub trim: bool,
    /// The point to trim at (the location's correlated point): longitude and latitude.
    pub point: (f64, f64),
    /// How far along the edge to trim, from 0 to 1.
    pub percent_along: f64,
}

/// How to trim one edge of a leg at intermediate locations: at its start and at its end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeTrim {
    /// The index of the edge in [`Leg::edges`].
    pub edge_index: usize,
    /// Where the edge starts being used.
    pub begin: EdgeTrimming,
    /// Where the edge stops being used.
    pub end: EdgeTrimming,
}

/// A leg: the path between two break locations, through any intermediate ones.
#[derive(Clone, Debug, PartialEq)]
pub struct Leg {
    /// The index of the leg's first location in the request's `options.locations`
    /// (for map matching, in its `options.shape`).
    pub origin: usize,
    /// The index of the leg's last location.
    pub destination: usize,
    /// The indexes of the through and via locations in between, in order.
    ///
    /// Thor sets each one's `correlation.leg_shape_index` to the index of the edge
    /// (in [`Leg::edges`]) where the leg reaches it, as mainline does before trip building.
    pub intermediates: Vec<usize>,
    /// The leg's locations as they were when the leg was made (the origin,
    /// the intermediates, and the destination), which is what mainline gives trip building:
    /// later legs can still change them in `options.locations`
    /// (a break-through location's candidates, for one).
    pub locations: Vec<valhalla_proto::Location>,
    /// The percent along the first edge where the leg starts
    /// (from the origin's candidate for that edge), if it is one of them.
    pub begin_percent: Option<f64>,
    /// The percent along the last edge where the leg ends.
    pub end_percent: Option<f64>,
    /// The edges, from the origin to the destination.
    pub edges: Vec<PathEdge>,
    /// The path algorithms used for each segment of the leg (mainline's `algorithms`),
    /// for example `bidirectional_a*`.
    pub algorithms: Vec<&'static str>,
    /// How to trim edges where the leg meets intermediate locations mid-edge
    /// (mainline's `edge_trimming`), sorted by edge index.
    pub edge_trimming: Vec<EdgeTrim>,
}

/// One route: its legs, in order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    pub legs: Vec<Leg>,
}

/// What path search found for a request: the best route,
/// followed by any alternates (each with one leg; mainline only finds alternates
/// for two locations).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paths {
    pub routes: Vec<Route>,
}
