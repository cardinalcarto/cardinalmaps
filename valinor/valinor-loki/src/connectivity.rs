//! Which local tiles are connected to which (mainline's `baldr::connectivity_map_t`).
//!
//! With `loki.use_connectivity` (on by default), Loki refuses routes between locations in
//! regions that can't be connected (error 170) before Thor searches for a path.
//! Mainline decides this from tiles alone: two local level tiles are connected if they are in
//! the tile set and neighbors (north, south, east, or west, wrapping around the antimeridian),
//! or connected through such neighbors.
//! A location's colors are those of the tiles that its correlated edges' points are in.
//!
//! Only the local level matters here: Loki colors locations on it unless the route uses transit,
//! which Valinor doesn't route.

use crate::geo::Aabb;
use crate::search::point_of;
use crate::tiles::{bottom_neighbor, left_neighbor, right_neighbor, tile_list, top_neighbor};
use std::collections::{HashMap, HashSet, VecDeque};
use valhalla_graphtile::GraphId;
use valhalla_proto::Location;

/// Local tiles that mainline treats as unconnected although they are neighbors:
/// the Darién Gap between North and South America (`not_neighbors`).
const NOT_NEIGHBORS: [(u32, u32); 5] = [
    (563_449, 563_450),
    (563_450, 563_451),
    (563_451, 564_891),
    (564_891, 564_892),
    (566_331, 566_332),
];

/// The connected regions of the local level's tiles.
#[derive(Clone, Debug, Default)]
pub struct ConnectivityMap {
    /// The region of each local tile in the tile set.
    colors: HashMap<u32, usize>,
}

impl ConnectivityMap {
    /// Colors the tile set given by its tile IDs (on any level; only local tiles are used).
    pub fn new(tiles: impl IntoIterator<Item = GraphId>) -> Self {
        let mut colors: HashMap<u32, usize> = tiles
            .into_iter()
            .filter(|t| t.level() == 2)
            .filter_map(|t| u32::try_from(t.tile_id()).ok())
            .map(|t| (t, 0))
            .collect();
        let feuding = |a: u32, b: u32| {
            NOT_NEIGHBORS
                .iter()
                .any(|&(x, y)| (x == a && y == b) || (x == b && y == a))
        };
        // Sorted, so that the colors don't depend on hash order (their values don't matter).
        let mut tiles: Vec<u32> = colors.keys().copied().collect();
        tiles.sort_unstable();
        let mut color = 1;
        for tile in tiles {
            if colors.get(&tile).copied().unwrap_or(0) > 0 {
                continue;
            }
            colors.insert(tile, color);
            let mut queue = VecDeque::from([tile]);
            while let Some(next) = queue.pop_front() {
                let Ok(signed) = i32::try_from(next) else {
                    continue;
                };
                for neighbor in [
                    left_neighbor(signed),
                    right_neighbor(signed),
                    top_neighbor(signed),
                    bottom_neighbor(signed),
                ] {
                    let Ok(neighbor) = u32::try_from(neighbor) else {
                        continue;
                    };
                    if let Some(c) = colors.get_mut(&neighbor)
                        && *c == 0
                        && !feuding(next, neighbor)
                    {
                        *c = color;
                        queue.push_back(neighbor);
                    }
                }
            }
            color += 1;
        }
        Self { colors }
    }

    /// The regions of a location's correlated edges (`get_colors` with radius 0).
    pub fn location_colors(&self, location: &Location) -> HashSet<usize> {
        let mut result = HashSet::new();
        let Some(correlation) = &location.correlation else {
            return result;
        };
        for edge in correlation.edges.iter().chain(&correlation.filtered_edges) {
            let ll = point_of(edge.ll.as_ref());
            let bbox = Aabb {
                minx: ll.lng,
                miny: ll.lat,
                maxx: ll.lng,
                maxy: ll.lat,
            };
            for tile in tile_list(&bbox) {
                if let Ok(tile) = u32::try_from(tile)
                    && let Some(&color) = self.colors.get(&tile)
                {
                    result.insert(color);
                }
            }
        }
        result
    }

    /// Whether every location shares a region (mainline's check for error 170).
    pub fn connected<'a>(&self, locations: impl IntoIterator<Item = &'a Location>) -> bool {
        let mut counts: HashMap<usize, usize> = HashMap::new();
        let mut total = 0;
        for location in locations {
            total += 1;
            for color in self.location_colors(location) {
                *counts.entry(color).or_default() += 1;
            }
        }
        counts.values().any(|&count| count == total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::PointLL;
    use crate::search::lat_lng;
    use valhalla_proto::{Correlation, PathEdge};

    fn located(lng: f64, lat: f64) -> Location {
        Location {
            correlation: Some(Correlation {
                edges: vec![PathEdge {
                    ll: Some(lat_lng(PointLL::new(lng, lat))),
                    ..PathEdge::default()
                }],
                ..Correlation::default()
            }),
            ..Location::default()
        }
    }

    fn tile(id: u64) -> GraphId {
        GraphId::try_from_components(2, id, 0).unwrap()
    }

    #[test]
    fn neighbors_are_connected_and_islands_are_not() {
        // Andorra's tiles are neighbors; a tile far away is its own island.
        let map = ConnectivityMap::new(
            [763_925, 763_926, 763_927, 762_485, 762_486, 100]
                .into_iter()
                .map(tile)
                .chain([GraphId::try_from_components(0, 3015, 0).unwrap()]),
        );
        let la_vella = located(1.5211, 42.5078);
        let encamp = located(1.583, 42.5355);
        let island = located(-154.9, -89.9);
        assert!(map.connected([&la_vella, &encamp]));
        assert!(!map.connected([&la_vella, &island]));
        assert_eq!(map.location_colors(&island).len(), 1);
        // Nowhere in the tile set has no color, so it connects to nothing.
        assert!(!map.connected([&la_vella, &located(10.0, 10.0)]));
        assert!(!map.connected([&la_vella, &Location::default()]));
    }

    #[test]
    fn the_darien_gap_is_not_a_connection() {
        let map = ConnectivityMap::new([563_449, 563_450].into_iter().map(tile));
        let colors: HashSet<usize> = map.colors.values().copied().collect();
        assert_eq!(colors.len(), 2);
    }
}
