//! Finding the edges near a measurement (mainline's `CandidateGridQuery` and
//! `CandidateCollector`).
//!
//! The local level's bins are indexed into [`Grid`]s on first use and kept between requests
//! ([`CandidateGrids`], mainline's grid cache).
//! A query takes the edges in the grid cells around the measurement, projects the measurement
//! onto each, and keeps the edge and its opposing edge if the costing allows them and the
//! projection is within the search radius.
//! Candidates that snap to a node the query has already found are dropped,
//! since Meili routes to the node, not to the edge.
//!
//! Mainline iterates the edges in hash order, so which edge stands for a node, and the order of
//! the candidates (their state IDs), depend on libstdc++'s hashing.
//! Here they are in graph ID order. Either way a node is the same candidate for routing,
//! and only ties between equal costs can come out differently.

#![allow(
    clippy::float_cmp,
    reason = "a port of mainline's arithmetic, which compares floats exactly"
)]

use std::collections::{HashMap, HashSet};
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_reader::GraphReadError;
use valhalla_graphtile::graph_reader::{GraphReader, edge, shape};
use valhalla_graphtile::graph_tile::GraphTile;
use valhalla_graphtile::midgard::{Aabb, PointLL, Projector};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valinor_sif::{AnyCosting, Costing, DisallowMask};

use crate::geometry::{
    LOCAL_TILE_SIZE, bin_list, bin_tile, expand_meters, local_tile_bounds, project,
};
use crate::grid::Grid;

/// Meili snaps projections within a meter of an edge's end to its node
/// (`kSnapToNodeDistance`).
const SNAP_TO_NODE_DISTANCE: f64 = 1.0;

/// The local hierarchy level, whose bins are searched.
const BIN_LEVEL: u8 = 2;

/// One edge a measurement may be on (mainline's correlated `PathEdge`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateEdge {
    /// The edge.
    pub edge_id: GraphId,
    /// How far along the edge (in its direction) the projected point is, from 0 to 1.
    pub percent_along: f64,
    /// The projected point.
    pub point: (f64, f64),
    /// The approximate squared distance from the measurement to the projected point,
    /// in square meters (mainline stores it in the `PathEdge`'s `distance`).
    pub sq_distance: f64,
    /// Whether the point is at the edge's start node.
    pub begin_node: bool,
    /// Whether the point is at the edge's end node.
    pub end_node: bool,
}

/// A place a measurement may be (mainline's correlated `Location`):
/// a point on an edge, with that edge and its opposing edge where the costing allows each
/// (so one or two edges).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    /// The edges, the edge found by the query first, then its opposing edge.
    pub edges: Vec<CandidateEdge>,
}

/// The candidate grids of the local level's bins, kept between requests
/// (mainline's `CandidateGridQuery` and its grid cache).
///
/// The grids are built from one tile set: use one cache per tile set.
#[derive(Debug, Default)]
pub struct CandidateGrids {
    grids: HashMap<i32, Grid>,
    cell_size: f32,
    bins: Vec<i32>,
    edges: Vec<GraphId>,
    visited_nodes: HashSet<GraphId>,
    /// For the oracle tests: keep candidates at nodes already found.
    #[cfg(test)]
    pub(crate) keep_repeated_nodes: bool,
}

impl CandidateGrids {
    /// The number of bins indexed.
    #[inline]
    pub fn len(&self) -> usize {
        self.grids.len()
    }

    /// Whether no bins are indexed.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.grids.is_empty()
    }

    /// Drops the grids if there are more than `cache_size` (mainline's `ClearFullCache`),
    /// and all of them if the grid size changed.
    pub(crate) fn prepare(&mut self, grid_size: usize, cache_size: usize) {
        // `local_tile_size() / grid_size`: a float divided by a size_t converted to float.
        #[expect(
            clippy::cast_precision_loss,
            reason = "mainline converts the size_t to float"
        )]
        let cell_size = LOCAL_TILE_SIZE / grid_size as f32;
        if cell_size != self.cell_size || self.grids.len() > cache_size {
            self.grids.clear();
            self.cell_size = cell_size;
        }
    }

    /// The edge IDs in the grid cells `range` touches, in graph ID order without duplicates
    /// (`RangeQuery`).
    fn range_query<P: GraphTileProvider>(
        &mut self,
        graph: GraphReader<'_, P>,
        range: &Aabb,
    ) -> Result<(), GraphReadError> {
        bin_list(range, &mut self.bins);
        self.edges.clear();
        for &bin_id in &self.bins {
            if !self.grids.contains_key(&bin_id) {
                let Some(grid) = index_bin(graph, bin_id, self.cell_size)? else {
                    // Mainline caches nothing for a tile it doesn't have.
                    continue;
                };
                self.grids.insert(bin_id, grid);
            }
            if let Some(grid) = self.grids.get(&bin_id) {
                grid.query(range, &mut self.edges);
            }
        }
        self.edges.sort_unstable_by_key(GraphId::value);
        self.edges.dedup();
        Ok(())
    }

    /// The candidates within `sq_search_radius` (square meters) of `location`
    /// (`CandidateGridQuery::Query` with a `CandidateCollector`), appended to `out`.
    ///
    /// # Errors
    ///
    /// Fails if a tile can't be read or is corrupt.
    pub(crate) fn query<P: GraphTileProvider>(
        &mut self,
        graph: GraphReader<'_, P>,
        location: PointLL,
        sq_search_radius: f32,
        costing: &AnyCosting,
        out: &mut Vec<Candidate>,
    ) -> Result<(), GraphReadError> {
        let range = expand_meters(location, sq_search_radius.sqrt());
        self.range_query(graph, &range)?;
        self.visited_nodes.clear();
        let projector = Projector::new(location);
        for &edge_id in &self.edges {
            let Some(tile) = graph.tile(edge_id)? else {
                continue;
            };
            let de = edge(&tile, edge_id)?;
            let Some((opp_id, opp_tile)) = graph.opposing(edge_id, &tile)? else {
                continue;
            };
            let opp = edge(&opp_tile, opp_id)?;
            let points = shape(&tile.get_edge_info(de)?)?;
            let Some(projection) = project(&projector, &points, SNAP_TO_NODE_DISTANCE) else {
                continue;
            };
            let mut candidate = Candidate::default();
            let mut snapped_node = None;
            let in_range = projection.sq_distance <= f64::from(sq_search_radius);
            let mut add = |id: GraphId, forward: bool, end: GraphId, other_end: GraphId| {
                let dist = if forward {
                    projection.percent_along
                } else {
                    1.0 - projection.percent_along
                };
                if dist == 1.0 {
                    snapped_node = Some(end);
                } else if dist == 0.0 {
                    snapped_node = Some(other_end);
                }
                candidate.edges.push(CandidateEdge {
                    edge_id: id,
                    percent_along: dist,
                    point: (projection.point.lng, projection.point.lat),
                    sq_distance: projection.sq_distance,
                    begin_node: dist == 0.0,
                    end_node: dist == 1.0,
                });
            };
            if in_range && costing.edge_filter(de, &tile, DisallowMask::SHORTCUT) {
                add(
                    edge_id,
                    de.edge_info_is_forward(),
                    de.end_node_id(),
                    opp.end_node_id(),
                );
            }
            if in_range && costing.edge_filter(opp, &opp_tile, DisallowMask::SHORTCUT) {
                add(
                    opp_id,
                    opp.edge_info_is_forward(),
                    opp.end_node_id(),
                    de.end_node_id(),
                );
            }
            #[cfg(test)]
            let snapped_node = snapped_node.filter(|_| !self.keep_repeated_nodes);
            if !candidate.edges.is_empty()
                && snapped_node.is_none_or(|node| self.visited_nodes.insert(node))
            {
                out.push(candidate);
            }
        }
        Ok(())
    }
}

/// Indexes a bin's edges into a grid over its tile (`GetGrid` and `IndexBin`),
/// or `None` if the tile set doesn't have the tile.
fn index_bin<P: GraphTileProvider>(
    graph: GraphReader<'_, P>,
    bin_id: i32,
    cell_size: f32,
) -> Result<Option<Grid>, GraphReadError> {
    let (tile_id, bin_index) = bin_tile(bin_id);
    // Bins from `bin_list` are within the world, so their tiles' IDs are non-negative.
    let id = GraphId::try_from_components(BIN_LEVEL, u64::from(tile_id.unsigned_abs()), 0)
        .map_err(|e| GraphReadError::Tile(GraphTileProviderError::from(e)))?;
    let Some(tile) = graph.tile(id)? else {
        return Ok(None);
    };
    let mut grid = Grid::new(&local_tile_bounds(tile_id), cell_size, cell_size);
    for &edge_id in tile.edges_in_bin(bin_index) {
        // An edge in a bin may be in another tile (one it passes through).
        let Some(edge_tile) = graph.tile_from(edge_id, &tile)? else {
            continue;
        };
        let de = edge(&edge_tile, edge_id)?;
        let points = shape(&edge_tile.get_edge_info(de)?)?;
        for pair in points.windows(2) {
            grid.add_segment(edge_id, pair[0], pair[1]);
        }
    }
    Ok(Some(grid))
}
