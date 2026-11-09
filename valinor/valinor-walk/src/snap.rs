//! Snapping stops onto the street network.
//!
//! A stop lands on an edge with a fraction along it, via Loki's search and reach filter,
//! so that it doesn't land on a disconnected footpath.
//! The result is bound to the build of the tiles it was snapped on, like
//! [`TileSets`](https://docs.rs/valinor-router) refuses to mix tile builds.
//! Snapping one stop is cheap and needs no global state,
//! as new stops can arrive with a GTFS-RT overlay at any time.

use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::{BuildId, GraphTile};
use valhalla_graphtile::tile_provider::GraphTileProvider;
use valhalla_proto::lat_lng::{HasLat, HasLng};
use valhalla_proto::location::{HasMinimumReachability, HasSearchCutoff};
use valhalla_proto::{LatLng, Location, PathEdge};
use valinor_loki::{LokiConfig, parse_location, search};
use valinor_sif::Costing;
use valinor_transit::StopId;

use crate::error::WalkError;

/// How far Loki is allowed to look, in meters, by default.
///
/// Stops sit at the curb: if nothing walkable is within this distance, the snap is wrong.
pub const DEFAULT_MAX_SNAP_DISTANCE_M: u32 = 400;

/// How many nodes a snap must be able to reach, in and out, to count as connected.
pub const DEFAULT_MIN_REACHABILITY: u32 = 50;

/// Options for snapping stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapConfig {
    /// The farthest a stop may be from its edge, in meters.
    pub max_distance_m: u32,
    /// The reachability, in nodes, a snap must have to count as connected.
    pub min_reachability: u32,
}

impl Default for SnapConfig {
    fn default() -> Self {
        Self {
            max_distance_m: DEFAULT_MAX_SNAP_DISTANCE_M,
            min_reachability: DEFAULT_MIN_REACHABILITY,
        }
    }
}

/// Where on the street network a stop is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapPoint {
    /// The directed edge the stop is on.
    pub edge: GraphId,
    /// The fraction along that edge, in the edge's direction.
    pub percent_along: f32,
    /// The distance from the stop to the edge, in meters.
    pub distance_m: f32,
    /// The point on the edge, as longitude and latitude.
    pub point: (f64, f64),
}

/// How good a snap is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapQuality {
    /// The snap's edge reaches at least the configured number of nodes in both directions.
    Connected,
    /// Loki found only edges that don't reach far enough;
    /// the stop keeps the closest, and may be cut off from the rest of the network.
    Disconnected,
}

/// Why a stop wasn't snapped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapFailure {
    /// No edge the costing could use is within the configured distance.
    NothingNearby,
}

/// A stop's snap, or why there isn't one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnappedStop {
    /// The snap, or the reason there is none.
    pub result: Result<SnapPoint, SnapFailure>,
    /// How good the snap is (`Disconnected` if there is no snap).
    pub quality: SnapQuality,
    /// The build ID of the tile the snap's edge is in, or `None` if the stop isn't snapped.
    pub build_id: Option<BuildId>,
}

/// A problem found while snapping a stop, for reporting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapReport {
    /// The stop could not be snapped; it can't be reached by a street search.
    Unsnapped(StopId),
    /// The stop was snapped, but to an edge that isn't connected to much.
    Disconnected(StopId, SnapPoint),
}

/// The snap of every stop of a [`Snapshot`](valinor_transit::Snapshot), by [`StopId`].
///
/// Bound to the build of the tiles the stops were snapped on:
/// searches over other tiles are refused, as [`TileSets`](https://docs.rs/valinor-router)
/// refuses to mix builds.
#[derive(Clone, Debug, PartialEq)]
pub struct StopSnaps {
    build_id: BuildId,
    points: Vec<Option<SnapPoint>>,
    reports: Vec<SnapReport>,
}

impl StopSnaps {
    /// An empty table of snaps for `stop_count` stops, for tiles of the given build.
    pub fn new(build_id: BuildId, stop_count: usize) -> Self {
        Self {
            build_id,
            points: vec![None; stop_count],
            reports: Vec::new(),
        }
    }

    /// The build these snaps were made on.
    #[inline]
    pub fn build_id(&self) -> BuildId {
        self.build_id
    }

    /// How many stops this table covers.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.points.len()
    }

    /// The snap of a stop, if it has one.
    pub fn point(&self, stop: StopId) -> Option<SnapPoint> {
        self.points.get(index(stop)).copied().flatten()
    }

    /// The stops that were not snapped, or that landed on a disconnected piece of the network.
    pub fn reports(&self) -> &[SnapReport] {
        &self.reports
    }

    /// Adds one more stop's snap, keyed by its [`StopId`], growing the table if needed.
    ///
    /// New stops arrive with GTFS-RT overlays; call this for each of them when the overlay
    /// is built.
    ///
    /// # Errors
    ///
    /// Refuses a snap from a different tile build.
    pub fn insert(&mut self, stop: StopId, snapped: SnappedStop) -> Result<(), WalkError> {
        if let Some(found) = snapped.build_id
            && found != self.build_id
        {
            return Err(WalkError::BuildMismatch {
                snaps: self.build_id,
                tiles: found,
            });
        }
        let at = index(stop);
        if at >= self.points.len() {
            self.points.resize(at + 1, None);
        }
        match snapped.result {
            Ok(point) => {
                self.points[at] = Some(point);
                if snapped.quality == SnapQuality::Disconnected {
                    self.reports.push(SnapReport::Disconnected(stop, point));
                }
            }
            Err(SnapFailure::NothingNearby) => {
                self.points[at] = None;
                self.reports.push(SnapReport::Unsnapped(stop));
            }
        }
        Ok(())
    }

    /// Moves every stop of `part` into this table, `part`'s stop `n` becoming `ids[n]`.
    ///
    /// A build snaps stops in batches of neighbours, then gathers them into one table.
    ///
    /// # Errors
    ///
    /// Refuses a part from another tile build, or `ids` that don't match `part`.
    pub fn absorb(&mut self, part: &Self, ids: &[u32]) -> Result<(), WalkError> {
        if part.build_id != self.build_id {
            return Err(WalkError::BuildMismatch {
                snaps: self.build_id,
                tiles: part.build_id,
            });
        }
        if ids.len() != part.stop_count() || ids.iter().any(|&id| id as usize >= self.stop_count())
        {
            return Err(WalkError::TooLarge);
        }
        for (&id, point) in ids.iter().zip(&part.points) {
            self.points[id as usize] = *point;
        }
        for report in &part.reports {
            let remap = |stop: StopId| StopId::new(ids[index(stop)]);
            self.reports.push(match *report {
                SnapReport::Unsnapped(stop) => SnapReport::Unsnapped(remap(stop)),
                SnapReport::Disconnected(stop, point) => {
                    SnapReport::Disconnected(remap(stop), point)
                }
            });
        }
        Ok(())
    }

    /// Whether every stop in the table has a snap.
    pub fn all_snapped(&self) -> bool {
        self.points.iter().all(Option::is_some)
    }
}

#[inline]
fn index(stop: StopId) -> usize {
    stop.into_inner() as usize
}

/// A location for Loki with the reachability and search cutoff `config` asks for.
fn location_for(ll: (f64, f64), config: SnapConfig, loki: &LokiConfig) -> Location {
    let mut location = Location {
        ll: Some(LatLng {
            has_lat: Some(HasLat::Lat(ll.1)),
            has_lng: Some(HasLng::Lng(ll.0)),
        }),
        has_minimum_reachability: Some(HasMinimumReachability::MinimumReachability(
            config.min_reachability,
        )),
        has_search_cutoff: Some(HasSearchCutoff::SearchCutoff(config.max_distance_m)),
        ..Location::default()
    };
    parse_location(&mut location, loki);
    location
}

/// The snap in a location Loki has correlated.
fn snap_of<P: GraphTileProvider>(
    location: &Location,
    config: SnapConfig,
    provider: &P,
) -> Result<SnappedStop, WalkError> {
    let candidates = location
        .correlation
        .as_ref()
        .map_or(&[][..], |c| c.edges.as_slice());
    let is_connected = |e: &PathEdge| {
        u32::try_from(e.outbound_reach.max(0)).unwrap_or(0) >= config.min_reachability
            && u32::try_from(e.inbound_reach.max(0)).unwrap_or(0) >= config.min_reachability
    };
    let chosen = candidates.iter().find(|e| is_connected(e));
    let (edge, quality) = match chosen.or_else(|| candidates.first()) {
        Some(edge) if chosen.is_some() => (edge, SnapQuality::Connected),
        Some(edge) => (edge, SnapQuality::Disconnected),
        None => {
            return Ok(SnappedStop {
                result: Err(SnapFailure::NothingNearby),
                quality: SnapQuality::Disconnected,
                build_id: None,
            });
        }
    };
    let edge_id = GraphId::try_from_id(edge.graph_id).map_err(|_| {
        WalkError::SnappedEdgeMissing(
            GraphId::try_from_id(0).unwrap_or_else(|_| unreachable!("zero is a valid ID")),
        )
    })?;
    let build_id = provider
        .with_tile_containing(edge_id, |tile| tile.header().build_id())
        .map_err(WalkError::from)?;
    let ll = edge.ll.as_ref();
    let lat = match ll.and_then(|l| l.has_lat.as_ref()) {
        Some(HasLat::Lat(v)) => *v,
        None => 0.0,
    };
    let lng = match ll.and_then(|l| l.has_lng.as_ref()) {
        Some(HasLng::Lng(v)) => *v,
        None => 0.0,
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "percent and distance as floats"
    )]
    Ok(SnappedStop {
        result: Ok(SnapPoint {
            edge: edge_id,
            percent_along: edge.percent_along as f32,
            distance_m: edge.distance as f32,
            point: (lng, lat),
        }),
        quality,
        build_id: Some(build_id),
    })
}

/// Snaps a single stop, for a GTFS-RT stop arriving with an overlay.
///
/// # Performance
///
/// Runs one Loki search; nothing here depends on other stops.
///
/// # Errors
///
/// Fails if a tile can't be read.
pub fn snap_stop<P: GraphTileProvider, C: Costing>(
    provider: &P,
    costing: &C,
    config: &SnapConfig,
    ll: (f64, f64),
) -> Result<SnappedStop, WalkError> {
    let loki = LokiConfig::default();
    let mut locations = [location_for(ll, *config, &loki)];
    search(&mut locations, provider, costing)?;
    snap_of(&locations[0], *config, provider)
}

/// Snaps every stop, for building a [`StopSnaps`] table at startup.
///
/// `build_id` is the build of the tiles (for example [`TileSets::build_id`]); a tile
/// from any other build refuses the whole table.
///
/// # Errors
///
/// Fails if a tile can't be read, or a stop lands in a tile of a different build.
pub fn snap_stops<P: GraphTileProvider, C: Costing>(
    provider: &P,
    costing: &C,
    config: &SnapConfig,
    build_id: BuildId,
    stops: &[(f64, f64)],
) -> Result<StopSnaps, WalkError> {
    let loki = LokiConfig::default();
    let mut locations: Vec<Location> = stops
        .iter()
        .map(|&ll| location_for(ll, *config, &loki))
        .collect();
    search(&mut locations, provider, costing)?;
    let mut table = StopSnaps::new(build_id, stops.len());
    for (i, location) in locations.iter().enumerate() {
        let stop = StopId::new(u32::try_from(i).map_err(|_| WalkError::TooLarge)?);
        table.insert(stop, snap_of(location, *config, provider)?)?;
    }
    Ok(table)
}
