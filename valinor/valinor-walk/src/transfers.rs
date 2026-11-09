//! Precomputed walks between nearby stops, and the transfer search that reads them.
//!
//! A rider walks at most once between two trips, so a table needs no closure: each entry is one
//! whole walk. An entry is the time of the route Sif's costing likes best, found by the same
//! edge-based search as access and egress tables, so a transfer honors what a rider would
//! actually be told to do: for a bicycle, the bike path instead of the busy road beside it.
//!
//! Forward and backward searches read the same entry for a directed pair of stops,
//! so they agree by construction.
//! The table holds only times; a transfer that ends up in a result is routed again to draw it.
//! A search applies each stop's transfer time and any rule for the pair of stops when it reads
//! a walk (see [`valinor_transit::WalkRules`]); the table itself stays real seconds.
//! A draw searches with the table's own limit, which sets the search's cost bound,
//! so that it finds the same route as the table's own search; it stops once that route is final.
//! A walk from a GTFS-RT new stop's backward search is drawn with that same backward search;
//! [`TableWalk::search`] records which search found a walk.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;

use valhalla_graphtile::tile_provider::GraphTileProvider;
use valinor_sif::Costing;
use valinor_transit::{
    Clock, Direction, SeedRules, StopId, TransferArrival, TransferOutcome, TransferRequest,
    TransferSearch, TransferSink,
};
use zerocopy::little_endian::{U16, U32};

use crate::error::WalkError;
use crate::flat_table::{Columns, FlatTableError, MappedTable, TableKey, map_flat, write_flat};
use crate::snap::StopSnaps;
use crate::table::{AccessSide, StopTables, TableRequest};

/// The default bound on a transfer, in seconds.
pub const DEFAULT_TRANSFER_SECONDS: u32 = 900;

/// A walk to or from another stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hop {
    /// The stop at the other end, in the numbering of the search's snaps.
    pub stop: u32,
    /// The walk's whole seconds.
    pub seconds: u32,
}

/// Rows of walks by stop, in the same columns as a mapped table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Rows {
    offsets: Vec<U32>,
    stops: Vec<U32>,
    seconds: Vec<U16>,
}

impl Rows {
    fn from_sorted(stop_count: usize, walks: &[(u32, Hop)]) -> Result<Self, WalkError> {
        let mut rows = Self {
            offsets: Vec::with_capacity(stop_count + 1),
            stops: Vec::with_capacity(walks.len()),
            seconds: Vec::with_capacity(walks.len()),
        };
        rows.offsets.push(U32::new(0));
        let mut at = 0;
        for stop in 0..stop_count {
            while let Some(&(from, hop)) = walks.get(at)
                && from as usize == stop
            {
                rows.push(hop)?;
                at += 1;
            }
            rows.finish_row()?;
        }
        Ok(rows)
    }

    fn from_lists(lists: &[Vec<Hop>]) -> Result<Self, WalkError> {
        let mut rows = Self {
            offsets: Vec::with_capacity(lists.len() + 1),
            stops: Vec::with_capacity(lists.iter().map(Vec::len).sum()),
            seconds: Vec::with_capacity(lists.iter().map(Vec::len).sum()),
        };
        rows.offsets.push(U32::new(0));
        for list in lists {
            for &hop in list {
                rows.push(hop)?;
            }
            rows.finish_row()?;
        }
        Ok(rows)
    }

    fn push(&mut self, hop: Hop) -> Result<(), WalkError> {
        let seconds = u16::try_from(hop.seconds).map_err(|_| WalkError::TooLarge)?;
        self.stops.push(U32::new(hop.stop));
        self.seconds.push(U16::new(seconds));
        Ok(())
    }

    fn finish_row(&mut self) -> Result<(), WalkError> {
        let count = u32::try_from(self.stops.len()).map_err(|_| WalkError::TooLarge)?;
        self.offsets.push(U32::new(count));
        Ok(())
    }

    fn columns(&self) -> Columns<'_> {
        Columns {
            offsets: &self.offsets,
            stops: &self.stops,
            seconds: &self.seconds,
        }
    }

    fn bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<U32>()
            + self.stops.capacity() * size_of::<U32>()
            + self.seconds.capacity() * size_of::<U16>()
    }
}

/// What backs a table's base walks.
#[derive(Clone, Debug)]
enum Storage {
    /// Held on the heap, as loaded or built here.
    Heap { out: Arc<Rows>, into: Arc<Rows> },
    /// A flat file, mapped into memory.
    Mapped(Arc<MappedTable>),
}

/// Walks added to a table for an overlay's new stops, which a mapped file can't take.
///
/// Small: a new stop is rare, and walks touch only stops near it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SideTable {
    /// Walks to add to a stop's row, by the stop whose row they extend.
    out: BTreeMap<u32, Vec<Hop>>,
    /// As `out`, for walks read backward.
    into: BTreeMap<u32, Vec<Hop>>,
    /// The new stops added so far, to tell which walks came from a backward search.
    new_stops: BTreeSet<u32>,
}

impl SideTable {
    /// Which search found a side walk: one from a base stop to a new stop came from the new
    /// stop's backward search, and any other from the new stop's forward search.
    #[inline]
    fn found_by(&self, from: u32, to: u32) -> AccessSide {
        if self.new_stops.contains(&to) && !self.new_stops.contains(&from) {
            AccessSide::Egress
        } else {
            AccessSide::Access
        }
    }
}

/// Every stop's walks to the stops within a bound, in both directions of lookup.
///
/// Base walks are read from `storage`, on the heap or mapped from a flat file;
/// [`Self::with_new_stops`] adds to a small side table, since a mapped file can't change.
#[derive(Clone, Debug)]
pub struct TransferTable {
    max_seconds: u32,
    stop_count: usize,
    storage: Storage,
    side: Option<Arc<SideTable>>,
}

impl PartialEq for TransferTable {
    /// Whether two tables hold the same walks, however each one stores them.
    fn eq(&self, other: &Self) -> bool {
        self.max_seconds == other.max_seconds
            && self.stop_count == other.stop_count
            && (0..self.stop_count).all(|stop| {
                let stop = StopId::new(u32::try_from(stop).unwrap_or(u32::MAX));
                self.walks_from(stop).eq(other.walks_from(stop))
                    && self.walks_into(stop).eq(other.walks_into(stop))
            })
    }
}

impl Eq for TransferTable {}

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
    /// Base-to-base walks are kept as they are, in storage that is shared, not copied.
    /// Each new stop runs one forward search from it, for walks from the new stop,
    /// and one backward search from it, for walks from every other stop into the new one.
    /// New walks live in a small side table.
    /// Forward and backward Sif searches can settle on different routes (a tie, or a difference
    /// in costing), so a walk found backward is drawn backward, by [`TableWalk::search`],
    /// and a base-to-new walk won't always be the same as a full build's forward search.
    /// Returns `Ok(None)` if `interrupt` asks to stop.
    ///
    /// # Performance
    ///
    /// Two searches per new stop, however many stops the table has.
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
        let mut added: BTreeMap<u32, Vec<Hop>> = BTreeMap::new();
        let is_new: BTreeSet<u32> = new_stops.iter().map(|s| s.into_inner()).collect();
        let mut tables = StopTables::new(provider.clone(), costing.clone());
        tables.bind(Arc::clone(snaps))?;
        for &stop in new_stops {
            let new_stop = stop.into_inner();
            let Some(out) = self.walks_around(&mut tables, stop, AccessSide::Access, interrupt)?
            else {
                return Ok(None);
            };
            added.entry(new_stop).or_default().extend(out);
            let Some(into) = self.walks_around(&mut tables, stop, AccessSide::Egress, interrupt)?
            else {
                return Ok(None);
            };
            // A walk between two new stops is the one the forward search of its origin found.
            for hop in into.into_iter().filter(|hop| !is_new.contains(&hop.stop)) {
                added.entry(hop.stop).or_default().push(Hop {
                    stop: new_stop,
                    seconds: hop.seconds,
                });
            }
        }
        for hops in added.values_mut() {
            // New stops aren't always numbered past the base stops.
            hops.sort_unstable_by_key(|hop| hop.stop);
        }
        let mut side = self.side.as_deref().cloned().unwrap_or_default();
        side.new_stops.extend(is_new);
        for (from, hops) in added {
            for &hop in &hops {
                side.into.entry(hop.stop).or_default().push(Hop {
                    stop: from,
                    seconds: hop.seconds,
                });
            }
            side.out.entry(from).or_default().extend(hops);
        }
        Ok(Some(Self {
            max_seconds: self.max_seconds,
            stop_count,
            storage: self.storage.clone(),
            side: Some(Arc::new(side)),
        }))
    }

    /// The walks a search from `stop` finds: forward (into other stops) or backward (from them).
    fn walks_around<P, C>(
        &self,
        tables: &mut StopTables<P, C>,
        stop: StopId,
        side: AccessSide,
        interrupt: &(dyn Fn() -> bool + Sync),
    ) -> Result<Option<Vec<Hop>>, WalkError>
    where
        P: GraphTileProvider,
        C: Costing,
    {
        let seeds = tables.stop_seeds(stop)?;
        if seeds.is_empty() {
            return Ok(Some(Vec::new()));
        }
        if !tables.search(&TableRequest {
            side,
            seeds: &seeds,
            max_seconds: self.max_seconds,
            interrupt: &|| interrupt(),
        })? {
            return Ok(None);
        }
        Ok(Some(sorted_walks_from(tables, stop)))
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
        Ok(Self::on_heap(
            max_seconds,
            stop_count,
            Rows::from_sorted(stop_count, walks)?,
            Rows::from_sorted(stop_count, &inverse)?,
        ))
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
        Ok(Self::on_heap(
            max_seconds,
            lists.len(),
            Rows::from_lists(lists)?,
            Rows::from_lists(&inverse)?,
        ))
    }

    fn on_heap(max_seconds: u32, stop_count: usize, out: Rows, into: Rows) -> Self {
        Self {
            max_seconds,
            stop_count,
            storage: Storage::Heap {
                out: Arc::new(out),
                into: Arc::new(into),
            },
            side: None,
        }
    }

    /// A table with no walks and no stops, for a search that hasn't been bound.
    fn empty_table() -> Self {
        Self::on_heap(0, 0, Rows::default(), Rows::default())
    }

    #[inline]
    fn outgoing_columns(&self) -> Columns<'_> {
        match &self.storage {
            Storage::Heap { out, .. } => out.columns(),
            Storage::Mapped(mapped) => mapped.outgoing(),
        }
    }

    #[inline]
    fn incoming_columns(&self) -> Columns<'_> {
        match &self.storage {
            Storage::Heap { into, .. } => into.columns(),
            Storage::Mapped(mapped) => mapped.incoming(),
        }
    }

    /// Writes this table's base walks as a flat file; see [`crate::flat_table`].
    ///
    /// # Errors
    ///
    /// If this table has an overlay's side table (only a base table is written), the walks don't
    /// fit, or the writer fails.
    pub fn write_flat(&self, writer: &mut impl Write, key: &TableKey) -> io::Result<()> {
        if self.side.is_some() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let stop_count = u32::try_from(self.stop_count)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        write_flat(
            writer,
            key,
            self.max_seconds,
            stop_count,
            self.outgoing_columns(),
            self.incoming_columns(),
        )
    }

    /// Maps a flat table written by [`Self::write_flat`].
    ///
    /// # Safety
    ///
    /// The file must not be modified while it is mapped; replace it by renaming over it.
    /// A mapping whose file is truncated can fault at any later read.
    ///
    /// # Errors
    ///
    /// If the file can't be read, its key is wrong, or its header and offsets don't fit.
    pub unsafe fn map_flat(path: &Path, key: &TableKey) -> Result<Self, FlatTableError> {
        // SAFETY: passed on to the caller's promise.
        let mapped = unsafe { map_flat(path, key) }?;
        Ok(Self {
            max_seconds: mapped.max_seconds(),
            stop_count: mapped.stop_count(),
            storage: Storage::Mapped(Arc::new(mapped)),
            side: None,
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

    /// How many walks the table holds, base and side.
    pub fn walk_count(&self) -> usize {
        self.outgoing_columns().stops.len()
            + self
                .side
                .as_ref()
                .map_or(0, |side| side.out.values().map(Vec::len).sum())
    }

    /// Bytes of heap held by the table; a mapped table's walks aren't heap and aren't counted.
    pub fn bytes(&self) -> usize {
        let base = match &self.storage {
            Storage::Heap { out, into } => out.bytes() + into.bytes(),
            Storage::Mapped(_) => 0,
        };
        base + self.side.as_ref().map_or(0, |side| {
            side.out.values().map(Vec::capacity).sum::<usize>() * size_of::<Hop>()
        })
    }

    /// The walks starting at a stop, as `(stop, seconds)`, by increasing stop number.
    ///
    /// # Performance
    ///
    /// Allocates; for inspection, not the transfer search.
    pub fn walks_from(&self, from: StopId) -> impl Iterator<Item = (StopId, u32)> + '_ {
        self.merged_row(self.outgoing_columns(), from, |side| &side.out)
            .into_iter()
    }

    /// The walks arriving at a stop, as `(stop, seconds)`, by increasing stop number.
    ///
    /// # Performance
    ///
    /// Allocates; for inspection, not the transfer search.
    pub fn walks_into(&self, to: StopId) -> impl Iterator<Item = (StopId, u32)> + '_ {
        self.merged_row(self.incoming_columns(), to, |side| &side.into)
            .into_iter()
    }

    /// One stop's base walks and side walks, as one sorted row.
    fn merged_row(
        &self,
        columns: Columns<'_>,
        stop: StopId,
        side_of: impl Fn(&SideTable) -> &BTreeMap<u32, Vec<Hop>>,
    ) -> Vec<(StopId, u32)> {
        let (stops, seconds) = columns.of(stop.into_inner() as usize);
        let mut row: Vec<(StopId, u32)> = stops
            .iter()
            .zip(seconds)
            .map(|(hop, seconds)| (StopId::new(hop.get()), u32::from(seconds.get())))
            .collect();
        if let Some(extra) = self
            .side
            .as_ref()
            .and_then(|side| side_of(side).get(&stop.into_inner()))
        {
            row.extend(extra.iter().map(|hop| (StopId::new(hop.stop), hop.seconds)));
            row.sort_unstable_by_key(|&(to, _)| to);
        }
        row
    }

    /// The seconds of the walk from one stop to another, if it is in the table.
    pub fn seconds(&self, from: StopId, to: StopId) -> Option<u32> {
        self.walks_from(from)
            .find(|&(stop, _)| stop == to)
            .map(|(_, seconds)| seconds)
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
    /// Which search found this walk, and so how to draw it:
    /// [`AccessSide::Access`] forward from `from`, or [`AccessSide::Egress`] backward from `to`.
    ///
    /// Forward and backward Sif searches can settle on different routes between the same stops,
    /// so a walk must be drawn the way its time was computed. Base table walks are all forward;
    /// a walk from a base stop to a GTFS-RT new stop comes from that new stop's backward search.
    pub search: AccessSide,
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
        Self::new(Arc::new(TransferTable::empty_table()))
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
        let table = Arc::clone(&self.table);
        let rows = if forward {
            table.outgoing_columns()
        } else {
            table.incoming_columns()
        };
        let side_table = table.side.as_deref();
        let side = side_table.map(|side| if forward { &side.out } else { &side.into });
        let bound = request.upper_bound.seconds();
        let limit = self.limit;
        let epoch = self.epoch;
        let (stamps, best) = (&mut self.stamps, &mut self.best);
        let direction = request.direction;
        let rules = request.rules;
        let mut visit = |source: StopId,
                         clock: Clock,
                         hop_stop: u32,
                         hop_seconds: u32,
                         from_side,
                         seed: SeedRules<'_>| {
            if hop_seconds > limit {
                return;
            }
            let Ok(seconds) = i32::try_from(hop_seconds) else {
                return;
            };
            let Some(cost) = seed.cost(StopId::new(hop_stop), seconds) else {
                return;
            };
            let reached = clock.seconds().saturating_add(cost);
            if reached >= bound {
                return;
            }
            let at = hop_stop as usize;
            let (Some(stamp), Some(best)) = (stamps.get_mut(at), best.get_mut(at)) else {
                return;
            };
            if *stamp == epoch && *best <= reached {
                return;
            }
            *stamp = epoch;
            *best = reached;
            let stop = StopId::new(hop_stop);
            let (from, to) = if forward {
                (source, stop)
            } else {
                (stop, source)
            };
            let search = match side_table {
                Some(side_table) if from_side => {
                    side_table.found_by(from.into_inner(), to.into_inner())
                }
                _ => AccessSide::Access,
            };
            sink.reach(TransferArrival {
                stop,
                source,
                clock: Clock::new(reached),
                parent: TableWalk {
                    from,
                    to,
                    seconds: hop_seconds,
                    search,
                },
            });
        };
        for seed in request.seeds {
            let source = seed.stop.into_inner();
            let seed_rules = rules.of_seed(seed.stop, direction);
            let (stops, seconds) = rows.of(source as usize);
            for (stop, second) in stops.iter().zip(seconds) {
                visit(
                    seed.stop,
                    seed.clock,
                    stop.get(),
                    u32::from(second.get()),
                    false,
                    seed_rules,
                );
            }
            if let Some(extra) = side.and_then(|side| side.get(&source)) {
                for hop in extra {
                    visit(
                        seed.stop,
                        seed.clock,
                        hop.stop,
                        hop.seconds,
                        true,
                        seed_rules,
                    );
                }
            }
        }
        Ok(TransferOutcome::Completed)
    }
}
