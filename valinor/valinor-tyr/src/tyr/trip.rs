//! The parts of Odin's `EnhancedTripLeg` that the serializers use to walk a trip leg.
//!
//! Mainline's accessors return a node's `mutable_edge()`,
//! which reads as an empty edge when the node has none (the leg's last node);
//! [`edge_of`] keeps that behavior without modifying the trip.

use std::sync::LazyLock;
use valhalla_proto::TripLeg;
use valhalla_proto::trip_leg::{Edge, Node};

static EMPTY_EDGE: LazyLock<Edge> = LazyLock::new(Edge::default);

/// A node's edge, or an empty one if it has none (as protobuf's `mutable_edge()` reads).
pub(crate) fn edge_of(node: &Node) -> &Edge {
    node.edge.as_ref().unwrap_or(&EMPTY_EDGE)
}

/// `EnhancedTripLeg::GetEnhancedNode`, or `None` for an index outside the leg.
pub(crate) fn node(leg: &TripLeg, node_index: usize) -> Option<&Node> {
    leg.node.get(node_index)
}

/// `EnhancedTripLeg::GetPrevEdge`: the edge leading into the node at `node_index`.
///
/// `None` for the first node (and for indexes outside the leg).
pub(crate) fn prev_edge(leg: &TripLeg, node_index: usize) -> Option<&Edge> {
    node_index
        .checked_sub(1)
        .and_then(|index| leg.node.get(index))
        .map(edge_of)
}

/// `EnhancedTripLeg::GetCurrEdge`: the edge leaving the node at `node_index`.
///
/// `None` for the last node (and for indexes outside the leg).
pub(crate) fn curr_edge(leg: &TripLeg, node_index: usize) -> Option<&Edge> {
    if node_index + 1 < leg.node.len() {
        leg.node.get(node_index).map(edge_of)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg() -> TripLeg {
        let edge = |heading| Edge {
            begin_heading: heading,
            ..Edge::default()
        };
        TripLeg {
            node: vec![
                Node {
                    edge: Some(edge(10)),
                    ..Node::default()
                },
                Node {
                    edge: Some(edge(20)),
                    ..Node::default()
                },
                Node::default(),
            ],
            ..TripLeg::default()
        }
    }

    #[test]
    fn edges_around_nodes() {
        let leg = leg();
        assert!(prev_edge(&leg, 0).is_none());
        assert_eq!(prev_edge(&leg, 1).map(|e| e.begin_heading), Some(10));
        assert_eq!(prev_edge(&leg, 2).map(|e| e.begin_heading), Some(20));
        assert!(prev_edge(&leg, 4).is_none());
        assert_eq!(curr_edge(&leg, 0).map(|e| e.begin_heading), Some(10));
        assert_eq!(curr_edge(&leg, 1).map(|e| e.begin_heading), Some(20));
        assert!(curr_edge(&leg, 2).is_none());
        assert_eq!(edge_of(&leg.node[2]), &Edge::default());
        assert!(node(&leg, 3).is_none());
    }
}
