//! Precomputed walks between nearby stops, and the transfer search that reads them.
//!
//! A rider walks at most once between two trips, so a table needs no closure: each entry is one
//! whole walk. An entry is the time of the route Sif's costing likes best, found by the same
//! edge-based search as access and egress tables, so a transfer honors what a rider would
//! actually be told to do: for a bicycle, the bike path instead of the busy road beside it.
//!
//! Forward and backward searches read the same entry for a directed pair of stops,
//! so they agree by construction.
//! The table holds only times; a transfer that ends up in a result is routed again to draw it,
//! at the table's own bound: a shorter limit is a different search, whose cost-minimal route
//! can be a different one, or be cut off where the table's wasn't.

use std::num::NonZeroUsize;
use std::sync::Arc;

use valhalla_graphtile::tile_provider::GraphTileProvider;
use valinor_sif::Costing;
use valinor_transit::{
    Clock, Direction, StopId, TransferArrival, TransferOutcome, TransferRequest, TransferSearch,
    TransferSink,
};

use crate::error::WalkError;
use crate::snap::StopSnaps;
use crate::table::{AccessSide, StopTables, TableRequest};

/// The default bound on a transfer, in seconds.
pub const DEFAULT_TRANSFER_SECONDS: u32 = 900;

/// How fast a walk can possibly go, in meters per second: Sif's largest edge speed, 252 km/h.
///
/// Costing has no faster edge, including ferries, which are charged at the edge's own speed.
/// Far above what a walk reaches in practice, so a straight-line radius from it is conservative.
const MAX_SPEED_METERS_PER_SECOND: f64 = 70.0;

/// The spherical distance between two `(longitude, latitude)` points, a little under the ground's.
fn great_circle_meters(a: (f64, f64), b: (f64, f64)) -> f64 {
    /// A little under the mean radius, so the distance is never overestimated into an exclusion.
    const EARTH_RADIUS_METERS: f64 = 6_335_000.0;
    let (lat_a, lat_b) = (a.1.to_radians(), b.1.to_radians());
    let half_lat = (lat_b - lat_a) / 2.0;
    let half_lon = (b.0 - a.0).to_radians() / 2.0;
    let h = half_lat.sin().powi(2) + lat_a.cos() * lat_b.cos() * half_lon.sin().powi(2);
    2.0 * EARTH_RADIUS_METERS * h.sqrt().min(1.0).asin()
}

/// A walk to or from another stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The stop at the other end, in the numbering of the search's snaps.
    pub stop: u32,
    /// The walk's whole seconds.
    pub seconds: u32,
}

/// Compressed rows of hops by stop.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Rows {
    offsets: Vec<u32>,
    hops: Vec<Hop>,
}

impl Rows {
    fn from_sorted(stop_count: usize, walks: &[(u32, Hop)]) -> Result<Self, WalkError> {
        let mut rows = Self {
            offsets: Vec::with_capacity(stop_count + 1),
            hops: Vec::with_capacity(walks.len()),
        };
        rows.offsets.push(0);
        let mut at = 0;
        for stop in 0..stop_count {
            while let Some(&(from, hop)) = walks.get(at)
                && from as usize == stop
            {
                rows.hops.push(hop);
                at += 1;
            }
            rows.offsets
                .push(u32::try_from(rows.hops.len()).map_err(|_| WalkError::TooLarge)?);
        }
        Ok(rows)
    }

    fn from_lists(lists: &[Vec<Hop>]) -> Result<Self, WalkError> {
        let mut rows = Self {
            offsets: Vec::with_capacity(lists.len() + 1),
            hops: Vec::with_capacity(lists.iter().map(Vec::len).sum()),
        };
        rows.offsets.push(0);
        for list in lists {
            rows.hops.extend_from_slice(list);
            rows.offsets
                .push(u32::try_from(rows.hops.len()).map_err(|_| WalkError::TooLarge)?);
        }
        Ok(rows)
    }

    #[inline]
    fn of(&self, stop: usize) -> &[Hop] {
        match (self.offsets.get(stop), self.offsets.get(stop + 1)) {
            (Some(&start), Some(&end)) => {
                self.hops.get(start as usize..end as usize).unwrap_or(&[])
            }
            _ => &[],
        }
    }

    fn bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.hops.capacity() * size_of::<Hop>()
    }
}

/// Every stop's walks to the stops within a bound, in both directions of lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferTable {
    max_seconds: u32,
    stop_count: usize,
    /// By stop: where a rider can walk to from it.
    out: Rows,
    /// By stop: where a rider can walk to it from.
    into: Rows,
}

impl TransferTable {
    /// Builds the table for every snapped stop, on `threads` threads.
    ///
    /// A stop with no snap has no walks.
    /// Returns `Ok(None)` if `interrupt` asks to stop.
    ///
    /// # Errors
    ///
    /// Fails on unreadable tiles, tiles of another build, or a costing error.
    pub fn build<P, C>(
        provider: &P,
        costing: &C,
        snaps: &Arc<StopSnaps>,
        max_seconds: u32,
        threads: NonZeroUsize,
        interrupt: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Self>, WalkError>
    where
        P: GraphTileProvider + Clone + Send + Sync,
        C: Costing + Clone + Send + Sync,
    {
        let stop_count = snaps.stop_count();
        let chunk = stop_count.div_ceil(threads.get()).max(1);
        let ranges: Vec<(usize, usize)> = (0..threads.get())
            .map(|i| (i * chunk, ((i + 1) * chunk).min(stop_count)))
            .filter(|(start, end)| start < end)
            .collect();
        let results: Vec<Result<Option<Vec<Vec<Hop>>>, WalkError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = ranges
                .iter()
                .map(|&(start, end)| {
                    scope.spawn(move || {
                        build_range(provider, costing, snaps, max_seconds, start, end, interrupt)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or(Err(WalkError::WorkerPanicked)))
                .collect()
        });
        let mut lists = Vec::with_capacity(stop_count);
        for result in results {
            let Some(part) = result? else {
                return Ok(None);
            };
            lists.extend(part);
        }
        Self::from_lists(&lists, max_seconds).map(Some)
    }

    /// This table with an overlay's new stops added, for `snaps` that cover them.
    ///
    /// Base-to-base walks are kept as they are. A walk to or from a new stop is found by the
    /// same forward search as [`Self::build`], so a base stop's walk to a new stop is drawn
    /// exactly as it was tabled. Those base stops are every one within the straight-line
    /// distance a rider could possibly cover in the bound.
    /// Returns `Ok(None)` if `interrupt` asks to stop.
    ///
    /// # Performance
    ///
    /// Runs one search for each new stop and each base stop in range,
    /// which in a metro area can be most of them. New stops are rare.
    ///
    /// # Errors
    ///
    /// Fails on unreadable tiles, tiles of another build, or a costing error.
    pub fn with_new_stops<P, C>(
        &self,
        provider: &P,
        costing: &C,
        snaps: &Arc<StopSnaps>,
        new_stops: &[StopId],
        interrupt: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Self>, WalkError>
    where
        P: GraphTileProvider + Clone,
        C: Costing + Clone,
    {
        let stop_count = snaps.stop_count().max(self.stop_count);
        let mut lists: Vec<Vec<Hop>> = (0..stop_count).map(|s| self.out.of(s).to_vec()).collect();
        let is_new: std::collections::HashSet<u32> =
            new_stops.iter().map(|s| s.into_inner()).collect();
        let mut tables = StopTables::new(provider.clone(), costing.clone());
        tables.bind(Arc::clone(snaps))?;
        for &stop in new_stops {
            let Some(hops) = self.walks_of(&mut tables, stop, interrupt)? else {
                return Ok(None);
            };
            lists[stop.into_inner() as usize].extend(hops);
        }
        for from in self.base_stops_in_reach(snaps, new_stops, &is_new, stop_count)? {
            let Some(hops) = self.walks_of(&mut tables, from, interrupt)? else {
                return Ok(None);
            };
            let list = &mut lists[from.into_inner() as usize];
            let before = list.len();
            list.extend(hops.into_iter().filter(|hop| is_new.contains(&hop.stop)));
            if list.len() != before {
                // New stops aren't always numbered past the base stops.
                list.sort_unstable_by_key(|hop| hop.stop);
            }
        }
        Self::from_lists(&lists, self.max_seconds).map(Some)
    }

    /// The walks a forward search finds from `from`, the way [`Self::build`] finds them.
    fn walks_of<P, C>(
        &self,
        tables: &mut StopTables<P, C>,
        from: StopId,
        interrupt: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Vec<Hop>>, WalkError>
    where
        P: GraphTileProvider,
        C: Costing,
    {
        let seeds = tables.stop_seeds(from)?;
        if seeds.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if !tables.search(&TableRequest {
            side: AccessSide::Access,
            seeds: &seeds,
            max_seconds: self.max_seconds,
            interrupt: &|| interrupt(),
        })? {
            return Ok(None);
        }
        Ok(Some(sorted_walks_from(tables, from)))
    }

    /// Base stops close enough to a new stop that a search from them might reach it.
    fn base_stops_in_reach(
        &self,
        snaps: &StopSnaps,
        new_stops: &[StopId],
        is_new: &std::collections::HashSet<u32>,
        stop_count: usize,
    ) -> Result<Vec<StopId>, WalkError> {
        let targets: Vec<(f64, f64)> = new_stops
            .iter()
            .filter_map(|&stop| snaps.point(stop).map(|point| point.point))
            .collect();
        let radius = f64::from(self.max_seconds) * MAX_SPEED_METERS_PER_SECOND;
        let mut in_reach = Vec::new();
        for stop in 0..stop_count {
            let id = u32::try_from(stop).map_err(|_| WalkError::TooLarge)?;
            if is_new.contains(&id) {
                continue;
            }
            let Some(point) = snaps.point(StopId::new(id)) else {
                continue;
            };
            if targets
                .iter()
                .any(|&target| great_circle_meters(point.point, target) <= radius)
            {
                in_reach.push(StopId::new(id));
            }
        }
        Ok(in_reach)
    }

    /// Builds a table from walks already sorted by `(from, to)`, for loading precomputed tiles.
    ///
    /// Equal to the [`Self::build`] that found those walks, field for field.
    ///
    /// # Errors
    ///
    /// Refuses walks out of order, walking from a stop to itself, past `stop_count`, or above
    /// `max_seconds`: those are never produced by a search, so a violation means bad input.
    pub fn from_sorted_walks(
        stop_count: usize,
        max_seconds: u32,
        walks: &[(u32, Hop)],
    ) -> Result<Self, WalkError> {
        for (index, &(from, hop)) in walks.iter().enumerate() {
            let in_range = (from as usize) < stop_count && (hop.stop as usize) < stop_count;
            let increasing = index
                .checked_sub(1)
                .and_then(|prior| walks.get(prior))
                .is_none_or(|&(prior_from, prior_hop)| {
                    (prior_from, prior_hop.stop) < (from, hop.stop)
                });
            if !in_range || !increasing || from == hop.stop || hop.seconds > max_seconds {
                return Err(WalkError::InvalidWalk {
                    from,
                    to: hop.stop,
                    seconds: hop.seconds,
                });
            }
        }
        let mut inverse: Vec<(u32, Hop)> = walks
            .iter()
            .map(|&(from, hop)| {
                (
                    hop.stop,
                    Hop {
                        stop: from,
                        seconds: hop.seconds,
                    },
                )
            })
            .collect();
        // Stable sort of walks already by `(from, to)`: each `into` row reads by increasing `from`.
        inverse.sort_by_key(|&(stop, _)| stop);
        Ok(Self {
            max_seconds,
            stop_count,
            out: Rows::from_sorted(stop_count, walks)?,
            into: Rows::from_sorted(stop_count, &inverse)?,
        })
    }

    fn from_lists(lists: &[Vec<Hop>], max_seconds: u32) -> Result<Self, WalkError> {
        let mut inverse: Vec<Vec<Hop>> = vec![Vec::new(); lists.len()];
        for (from, list) in lists.iter().enumerate() {
            let from = u32::try_from(from).map_err(|_| WalkError::TooLarge)?;
            for hop in list {
                inverse[hop.stop as usize].push(Hop {
                    stop: from,
                    seconds: hop.seconds,
                });
            }
        }
        Ok(Self {
            max_seconds,
            stop_count: lists.len(),
            out: Rows::from_lists(lists)?,
            into: Rows::from_lists(&inverse)?,
        })
    }

    /// How many stops the table covers.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.stop_count
    }

    /// The bound the table was built with, in seconds.
    #[inline]
    pub fn max_seconds(&self) -> u32 {
        self.max_seconds
    }

    /// How many walks the table holds.
    pub fn walk_count(&self) -> usize {
        self.out.hops.len()
    }

    /// Bytes held by the table.
    pub fn bytes(&self) -> usize {
        self.out.bytes() + self.into.bytes()
    }

    /// The walks starting at a stop, as `(stop, seconds)`, by increasing stop number.
    pub fn walks_from(&self, from: StopId) -> impl Iterator<Item = (StopId, u32)> + '_ {
        self.out
            .of(from.into_inner() as usize)
            .iter()
            .map(|hop| (StopId::new(hop.stop), hop.seconds))
    }

    /// The seconds of the walk from one stop to another, if it is in the table.
    pub fn seconds(&self, from: StopId, to: StopId) -> Option<u32> {
        let target = to.into_inner();
        self.out
            .of(from.into_inner() as usize)
            .iter()
            .find(|hop| hop.stop == target)
            .map(|hop| hop.seconds)
    }
}

fn build_range<P, C>(
    provider: &P,
    costing: &C,
    snaps: &Arc<StopSnaps>,
    max_seconds: u32,
    start: usize,
    end: usize,
    interrupt: &(dyn Fn() -> bool + Sync),
) -> Result<Option<Vec<Vec<Hop>>>, WalkError>
where
    P: GraphTileProvider + Clone,
    C: Costing + Clone,
{
    let mut tables = StopTables::new(provider.clone(), costing.clone());
    tables.bind(Arc::clone(snaps))?;
    let mut lists = Vec::with_capacity(end - start);
    for from in start..end {
        if interrupt() {
            return Ok(None);
        }
        let from_id = StopId::new(u32::try_from(from).map_err(|_| WalkError::TooLarge)?);
        let seeds = tables.stop_seeds(from_id)?;
        if seeds.is_empty() {
            lists.push(Vec::new());
            continue;
        }
        if !tables.search(&TableRequest {
            side: AccessSide::Access,
            seeds: &seeds,
            max_seconds,
            interrupt: &|| interrupt(),
        })? {
            return Ok(None);
        }
        lists.push(sorted_walks_from(&tables, from_id));
    }
    Ok(Some(lists))
}

/// The walks found by the last search from `from`, by increasing stop number.
///
/// Anything using one stop's walks calls this, so the table and the tiles agree by construction.
///
/// # Performance
///
/// Allocates only for the walks found, which is what the caller is about to keep.
pub fn sorted_walks_from<P: GraphTileProvider, C: Costing>(
    tables: &StopTables<P, C>,
    from: StopId,
) -> Vec<Hop> {
    let mut hops: Vec<Hop> = tables
        .reached_stops()
        .filter(|&(to, _)| to != from)
        .map(|(to, cost)| Hop {
            stop: to.into_inner(),
            seconds: cost.into_inner(),
        })
        .collect();
    hops.sort_unstable_by_key(|hop| hop.stop);
    hops
}

/// One transfer, resolved: the stops in real travel order, and how long it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableWalk {
    /// The stop the walk begins at.
    pub from: StopId,
    /// The stop the walk ends at.
    pub to: StopId,
    /// Its seconds in the table.
    pub seconds: u32,
}

/// A [`TransferSearch`] that reads a [`TransferTable`].
///
/// It keeps one integer and one stamp per stop between searches.
#[derive(Clone, Debug)]
pub struct TableTransfers {
    table: Arc<TransferTable>,
    best: Vec<i32>,
    stamps: Vec<u32>,
    epoch: u32,
    limit: u32,
}

impl TableTransfers {
    /// A search with no walks, until [`Self::bind`] gives it a table.
    pub fn empty() -> Self {
        Self::new(Arc::new(TransferTable {
            max_seconds: 0,
            stop_count: 0,
            out: Rows::default(),
            into: Rows::default(),
        }))
    }

    /// A search over `table`.
    pub fn new(table: Arc<TransferTable>) -> Self {
        let stops = table.stop_count();
        Self {
            table,
            best: vec![0; stops],
            stamps: vec![0; stops],
            epoch: 0,
            limit: u32::MAX,
        }
    }

    /// Points this search at another table, as when a snapshot adds stops.
    pub fn bind(&mut self, table: Arc<TransferTable>) {
        if Arc::ptr_eq(&self.table, &table) {
            return;
        }
        let stops = table.stop_count();
        self.table = table;
        self.best.clear();
        self.best.resize(stops, 0);
        self.stamps.clear();
        self.stamps.resize(stops, 0);
        self.epoch = 0;
    }

    /// The table this search reads.
    pub fn table(&self) -> &TransferTable {
        &self.table
    }

    /// Bytes this search retains, not counting the table it shares.
    pub fn working_memory_bytes(&self) -> usize {
        self.best.capacity() * size_of::<i32>() + self.stamps.capacity() * size_of::<u32>()
    }
}

impl TransferSearch for TableTransfers {
    type Parent = TableWalk;
    type Walk = TableWalk;
    type Error = WalkError;

    fn begin_run(&mut self, _direction: Direction) {}

    fn set_walk_limit(&mut self, limit: Option<u32>) {
        self.limit = limit.unwrap_or(u32::MAX);
    }

    fn walk(&self, parent: TableWalk) -> TableWalk {
        parent
    }

    fn search<K: TransferSink<TableWalk>>(
        &mut self,
        request: &TransferRequest<'_>,
        sink: &mut K,
    ) -> Result<TransferOutcome, WalkError> {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamps.fill(0);
            self.epoch = 1;
        }
        let forward = request.direction == Direction::Forward;
        let rows = if forward {
            &self.table.out
        } else {
            &self.table.into
        };
        let bound = request.upper_bound.seconds();
        for seed in request.seeds {
            let source = seed.stop.into_inner();
            for hop in rows.of(source as usize) {
                if hop.seconds > self.limit {
                    continue;
                }
                let Some(cost) = i32::try_from(hop.seconds).ok() else {
                    continue;
                };
                let clock = seed.clock.seconds().saturating_add(cost);
                if clock >= bound {
                    continue;
                }
                let at = hop.stop as usize;
                let (Some(stamp), Some(best)) = (self.stamps.get_mut(at), self.best.get_mut(at))
                else {
                    continue;
                };
                if *stamp == self.epoch && *best <= clock {
                    continue;
                }
                *stamp = self.epoch;
                *best = clock;
                let stop = StopId::new(hop.stop);
                let (from, to) = if forward {
                    (seed.stop, stop)
                } else {
                    (stop, seed.stop)
                };
                sink.reach(TransferArrival {
                    stop,
                    source: seed.stop,
                    clock: Clock::new(clock),
                    parent: TableWalk {
                        from,
                        to,
                        seconds: hop.seconds,
                    },
                });
            }
        }
        Ok(TransferOutcome::Completed)
    }
}
