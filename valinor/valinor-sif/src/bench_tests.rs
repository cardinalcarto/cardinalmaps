//! A timing test for the hot paths: run it in release mode with
//! `cargo test --release -p valinor-sif costing_hot_paths -- --ignored --nocapture`.
//!
//! It times `allowed`, `edge_cost`, and `transition_cost` over every pair of edges
//! at every node of the Andorra fixture tiles (as path search calls them),
//! for each costing with default options, through [`AnyCosting`]'s dispatch.
//! It's a guide, not a benchmark suite: no warm-up statistics, one machine.

use core::hint::black_box;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile, NodeInfo};
use valhalla_graphtile::tile_provider::{DirectoryGraphTileProvider, GraphTileProvider};
use valhalla_proto::Costing as CostingProto;
use valhalla_proto::costing::Type as CostingType;

use crate::costing_options::parse_costing;
use crate::json::JsonValue;
use crate::{AnyCosting, Costing, EdgeAccess, EdgeLabel, LabelState, TimeContext};

/// One transition: the predecessor's label, the edge, and the node between them.
struct Pair {
    pred: EdgeLabel,
    edge: DirectedEdge,
    edge_id: GraphId,
    node: NodeInfo,
}

#[test]
#[ignore = "a timing test; run it in release mode"]
fn costing_hot_paths() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles");
    let provider = DirectoryGraphTileProvider::new(dir, NonZeroUsize::new(16).expect("nonzero"));
    // The level 2 tile, which has most of the edges.
    let tile_id = GraphId::try_from_components(2, 763_926, 0).expect("tile id");
    let tile = provider
        .get_handle_for_tile_containing(tile_id)
        .expect("tile");

    for (costing_type, name) in [
        (CostingType::Auto, "auto"),
        (CostingType::Pedestrian, "pedestrian"),
        (CostingType::Bicycle, "bicycle"),
    ] {
        let doc = JsonValue::empty_object();
        let mut proto = CostingProto::default();
        parse_costing(
            &doc,
            "/costing_options/x",
            &mut proto,
            &mut Vec::new(),
            Some(costing_type),
        )
        .expect("defaults");
        let costing = AnyCosting::new(&proto).expect("costing");

        let mut pairs = Vec::new();
        for (n, node) in tile.nodes().iter().enumerate() {
            let first = usize::try_from(node.edge_index()).expect("index");
            let count = usize::from(node.edge_count());
            for j in first..first + count {
                let Ok(in_id) = provider
                    .get_opposing_edge_id(tile_id.with_feature_index(j as u64).expect("id"), &tile)
                else {
                    continue;
                };
                let Ok(in_tile) = provider.get_handle_for_tile_containing(in_id) else {
                    continue;
                };
                let in_edge = in_tile.get_directed_edge(in_id).expect("edge");
                let state = LabelState {
                    mode: costing.base().travel_mode(),
                    ..LabelState::default()
                };
                let pred = EdgeLabel::new(in_id, in_edge, state);
                for i in first..first + count {
                    pairs.push(Pair {
                        pred,
                        edge: tile.directed_edges()[i].clone(),
                        edge_id: tile_id.with_feature_index(i as u64).expect("id"),
                        node: tile.nodes()[n].clone(),
                    });
                }
            }
        }

        let rounds = 20;
        let start = Instant::now();
        let mut allowed = 0usize;
        for _ in 0..rounds {
            for p in &pairs {
                let mut access = EdgeAccess::new(0);
                if costing.allowed(
                    &p.edge,
                    p.edge_id,
                    &tile,
                    &p.pred,
                    TimeContext::NONE,
                    &mut access,
                ) {
                    allowed += 1;
                }
            }
        }
        let allowed_ns = start.elapsed().as_nanos() / (rounds * pairs.len()) as u128;

        let start = Instant::now();
        for _ in 0..rounds {
            for p in &pairs {
                black_box(
                    costing
                        .edge_cost(&p.edge, p.edge_id, &tile, Some(30_000))
                        .expect("cost"),
                );
            }
        }
        let edge_ns = start.elapsed().as_nanos() / (rounds * pairs.len()) as u128;

        let start = Instant::now();
        for _ in 0..rounds {
            for p in &pairs {
                black_box(
                    costing
                        .transition_cost(&p.edge, &tile, &p.node, &p.pred, &provider, false)
                        .expect("cost"),
                );
            }
        }
        let transition_ns = start.elapsed().as_nanos() / (rounds * pairs.len()) as u128;
        black_box(allowed);
        println!(
            "{name}: {} pairs; allowed {allowed_ns} ns, edge_cost {edge_ns} ns, transition_cost {transition_ns} ns",
            pairs.len()
        );
    }
}
