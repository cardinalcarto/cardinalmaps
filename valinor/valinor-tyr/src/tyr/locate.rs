//! The `/locate` response (mainline's `tyr::serializeLocate`, `src/tyr/locate_serializer.cc`).
//!
//! Loki answers `/locate` itself: for each location, the edges (and filtered edges) it was
//! correlated to, and the nodes it snapped to, read back from the tiles.
//! A plain response gives each edge's way ID, correlated point, side of street,
//! and fraction along; a verbose one gives everything the tiles say about it
//! (see [`super::locate_verbose`]).
//!
//! # Node order
//!
//! Mainline collects the snapped nodes in a `std::unordered_set<uint64_t>`
//! and writes them in its (unspecified) iteration order; we write them sorted by ID.

use super::locate_verbose::{edge_verbose, node_verbose};
use super::writer::JsonWriter;
use std::collections::BTreeSet;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{DirectedEdge, GraphTile};
use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError};
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::SideOfStreet;
use valhalla_proto::options::HasVerbose;
use valhalla_proto::{Api, LatLng, Location, PathEdge};

/// `tyr::kCoordinatePrecision`.
pub(super) const COORDINATE_PRECISION: u32 = 6;
/// `tyr::kDefaultPrecision`.
pub(super) const DEFAULT_PRECISION: u32 = 3;

/// A failure to read what a location was correlated to.
///
/// Mainline catches these and writes the location without edges or nodes.
#[derive(Debug, thiserror::Error)]
pub(super) enum LocateError {
    #[error(transparent)]
    Tile(#[from] GraphTileProviderError),
    #[error("read the graph: {0}")]
    Data(String),
}

pub(super) fn data(e: impl core::fmt::Display) -> LocateError {
    LocateError::Data(e.to_string())
}

/// The latitude and longitude of a `LatLng`, with unset coordinates read as 0.
pub(super) fn lat_lng(ll: Option<&LatLng>) -> (f64, f64) {
    let ll = ll.copied().unwrap_or_default();
    let lat = match ll.has_lat {
        Some(HasLat::Lat(lat)) => lat,
        None => 0.0,
    };
    let lng = match ll.has_lng {
        Some(HasLng::Lng(lng)) => lng,
        None => 0.0,
    };
    (lat, lng)
}

/// Serializes the `/locate` response for a correlated request (`serializeLocate`).
pub fn serialize_locate<P: GraphTileProvider>(api: &Api, provider: &P) -> String {
    let options = api.options.clone().unwrap_or_default();
    let verbose = matches!(options.has_verbose, Some(HasVerbose::Verbose(true)));
    let mut writer = JsonWriter::with_capacity(4096);
    writer.start_array();
    for location in &options.locations {
        let mut one = JsonWriter::new();
        if serialize_location(&mut one, location, provider, verbose).is_ok() {
            writer.raw(&one.finish());
        } else {
            let (lat, lng) = lat_lng(location.ll.as_ref());
            writer.start_object();
            writer.set_precision(COORDINATE_PRECISION);
            writer.key("input_lat").double(lat);
            writer.key("input_lon").double(lng);
            writer.set_precision(DEFAULT_PRECISION);
            writer.key("edges").null();
            writer.key("nodes").null();
            if verbose {
                writer.key("reason").string("No data found for location");
            }
            writer.end_object();
        }
    }
    writer.end_array();
    writer.finish()
}

fn serialize_location<P: GraphTileProvider>(
    writer: &mut JsonWriter,
    location: &Location,
    provider: &P,
    verbose: bool,
) -> Result<(), LocateError> {
    let (lat, lng) = lat_lng(location.ll.as_ref());
    writer.start_object();
    writer.set_precision(COORDINATE_PRECISION);
    writer.key("input_lat").double(lat);
    writer.key("input_lon").double(lng);
    writer.set_precision(DEFAULT_PRECISION);

    let correlation = location.correlation.clone().unwrap_or_default();
    writer.key("edges").start_array();
    for edge in &correlation.edges {
        serialize_edge(writer, location, edge, provider, verbose);
    }
    writer.end_array();
    if !correlation.filtered_edges.is_empty() {
        writer.key("filtered_edges").start_array();
        for edge in &correlation.filtered_edges {
            serialize_edge(writer, location, edge, provider, verbose);
        }
        writer.end_array();
    }

    // The nodes the location snapped to: the end nodes of its edges that end at the point.
    let mut nodes = BTreeSet::new();
    for edge in correlation.edges.iter().filter(|e| e.end_node) {
        let id = GraphId::try_from_id(edge.graph_id).map_err(data)?;
        let end = provider
            .with_tile_containing(id, |tile| {
                tile.get_directed_edge(id).map(DirectedEdge::end_node_id)
            })?
            .map_err(data)?;
        nodes.insert(end.value());
    }
    writer.key("nodes").start_array();
    for &node in &nodes {
        let id = GraphId::try_from_id(node).map_err(data)?;
        let tile = provider.get_handle_for_tile_containing(id)?;
        let info = tile.get_node(id).map_err(data)?;
        writer.start_object();
        if verbose {
            node_verbose(writer, &tile, id, info)?;
        } else {
            let ll = info.coordinate_f64(tile.header().sw_corner());
            writer.set_precision(COORDINATE_PRECISION);
            writer.key("lon").double(ll.x);
            writer.key("lat").double(ll.y);
            writer.set_precision(DEFAULT_PRECISION);
        }
        writer.end_object();
    }
    writer.end_array();
    writer.end_object();
    Ok(())
}

/// The name of a side of the street in the response.
pub(super) fn side_name(side: i32) -> &'static str {
    if side == i32::from(SideOfStreet::KLeft) {
        "left"
    } else if side == i32::from(SideOfStreet::KRight) {
        "right"
    } else {
        "neither"
    }
}

/// Writes one edge. Mainline writes whatever it got through before a failure,
/// so a failure leaves the object as it is.
fn serialize_edge<P: GraphTileProvider>(
    writer: &mut JsonWriter,
    location: &Location,
    edge: &PathEdge,
    provider: &P,
    verbose: bool,
) {
    writer.start_object();
    let _ = (|| -> Result<(), LocateError> {
        let id = GraphId::try_from_id(edge.graph_id).map_err(data)?;
        let tile = provider.get_handle_for_tile_containing(id)?;
        let directed = tile.get_directed_edge(id).map_err(data)?;
        let info = tile.get_edge_info(directed).map_err(data)?;
        if verbose {
            return edge_verbose(writer, location, edge, &tile, id, directed, &info);
        }
        let (lat, lng) = lat_lng(edge.ll.as_ref());
        writer.key("way_id").u64(info.way_id());
        writer.set_precision(COORDINATE_PRECISION);
        writer.key("correlated_lat").double(lat);
        writer.key("correlated_lon").double(lng);
        writer
            .key("side_of_street")
            .string(side_name(edge.side_of_street));
        writer.set_precision(5);
        writer.key("percent_along").double(edge.percent_along);
        writer.set_precision(DEFAULT_PRECISION);
        Ok(())
    })();
    writer.end_object();
}
