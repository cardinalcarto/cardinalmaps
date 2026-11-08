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
//! and writes them in its iteration order, which [`LibstdcxxSet`] reproduces.

use super::locate_verbose::{edge_verbose, node_verbose};
use super::writer::JsonWriter;
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

/// The order in which libstdc++'s `std::unordered_set<uint64_t>` iterates its elements.
///
/// The hash of an integer is the integer itself, so an element's bucket is its value modulo
/// the bucket count. The elements form one list: a new element goes first in its bucket,
/// or, when its bucket is empty, at the front of the list.
/// The first insertion makes 13 buckets; the table grows (to the next of libstdc++'s primes
/// at least twice as large) when it would hold more elements than buckets, re-linking the
/// elements in list order.
#[derive(Debug, Default)]
pub(super) struct LibstdcxxSet {
    /// The elements in iteration order.
    list: Vec<u64>,
    buckets: u64,
    next_resize: u64,
}

impl LibstdcxxSet {
    /// libstdc++'s `_Prime_rehash_policy::_M_next_bkt`: the bucket count for at least `n`.
    ///
    /// The small sizes come from its `__fast_bkt` table; larger ones are the entries of its
    /// prime list that a growing set reaches (measured with g++ 13), and beyond those
    /// (more than 20,000 nodes, which a location never has) the next prime.
    fn next_bucket_count(&mut self, n: u64) -> u64 {
        const FAST: [u64; 14] = [2, 2, 2, 3, 5, 5, 7, 7, 11, 11, 11, 11, 13, 13];
        const PRIMES: [u64; 10] = [29, 59, 127, 257, 541, 1109, 2357, 5087, 10273, 20753];
        let count =
            match usize::try_from(n).ok().and_then(|i| FAST.get(i)) {
                Some(&count) => count,
                None => PRIMES.iter().copied().find(|&p| p >= n).unwrap_or_else(|| {
                    (n..n.saturating_mul(2)).find(|&c| is_prime(c)).unwrap_or(n)
                }),
            };
        self.next_resize = count;
        count
    }

    /// Inserts a value; returns whether it was new.
    pub(super) fn insert(&mut self, value: u64) -> bool {
        if self.list.contains(&value) {
            return false;
        }
        let n_elt = self.list.len() as u64;
        if n_elt + 1 > self.next_resize {
            let min_buckets = (n_elt + 1).max(if self.next_resize == 0 { 11 } else { 0 });
            if min_buckets >= self.buckets {
                let wanted = (min_buckets + 1).max(self.buckets * 2);
                let count = self.next_bucket_count(wanted);
                self.rehash(count);
            } else {
                self.next_resize = self.buckets;
            }
        }
        self.insert_node(value);
        true
    }

    fn bucket(&self, value: u64) -> u64 {
        value % self.buckets.max(1)
    }

    /// Links a new node first in its bucket (`_M_insert_bucket_begin`).
    fn insert_node(&mut self, value: u64) {
        let bucket = self.bucket(value);
        match self.list.iter().position(|&v| self.bucket(v) == bucket) {
            // After the bucket's "before" node, which is just before its first element.
            Some(first) => self.list.insert(first, value),
            None => self.list.insert(0, value),
        }
    }

    /// Re-links every node into a new bucket array, in list order (`_M_rehash_aux`).
    fn rehash(&mut self, buckets: u64) {
        let old = core::mem::take(&mut self.list);
        self.buckets = buckets;
        // Each node goes first in its bucket, or at the front of the list, as on insertion.
        for value in old {
            self.insert_node(value);
        }
    }

    /// The elements in iteration order.
    pub(super) fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        self.list.iter().copied()
    }
}

fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    let mut d = 2;
    while d * d <= n {
        if n.is_multiple_of(d) {
            return false;
        }
        d += 1;
    }
    true
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
    let mut nodes = LibstdcxxSet::default();
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
    for node in nodes.iter() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_sets_iterate_like_libstdcxx() {
        // 13 buckets: values in empty buckets go to the front.
        let mut set = LibstdcxxSet::default();
        for v in [1, 2, 3] {
            set.insert(v);
        }
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![3, 2, 1]);
        // 14 is in bucket 1, so it goes before 1.
        set.insert(14);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![3, 2, 14, 1]);
        assert!(!set.insert(2));
    }

    #[test]
    fn growing_sets_iterate_like_libstdcxx() {
        // From g++ 13, which grows the table from 13 to 29 buckets at the 14th element.
        let mut set = LibstdcxxSet::default();
        let values = [
            8_456_098_473,
            5161,
            99999,
            12,
            25,
            38,
            7_281_335_864,
            1,
            14,
            27,
            40,
            53,
            66,
            79,
            92,
            105,
            118,
        ];
        for v in values {
            set.insert(v);
        }
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![
                118,
                105,
                92,
                79,
                8_456_098_473,
                5161,
                99999,
                12,
                25,
                38,
                7_281_335_864,
                1,
                14,
                27,
                40,
                53,
                66
            ]
        );
    }

    #[test]
    fn growing_keeps_bucket_neighbors_together() {
        let mut set = LibstdcxxSet::default();
        for v in 0..20 {
            set.insert(v);
        }
        let values: Vec<u64> = set.iter().collect();
        assert_eq!(values.len(), 20);
        let mut sorted = values.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    }
}
