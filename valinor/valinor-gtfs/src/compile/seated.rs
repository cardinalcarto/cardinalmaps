//! Stay-seated transfers: consecutive trips of one vehicle compiled into one combined trip.
//!
//! This follows nigiri's `block::rule_services` and its handling of `transfer_type` 4.
//! A `block_id` links consecutive trips of the block, when the first's last stop is the same stop
//! as the next's first and the vehicle waits there at most [`CompileOptions::seated_wait_seconds`].
//! A `transfer_type=4` row links its two trips whatever the wait (up to a sanity limit of two
//! hours, as nigiri has) and whatever their stops, and takes precedence over `block_id`:
//! a block with any trip in a type 4 row isn't merged by its `block_id`.
//! A `transfer_type=5` row forbids its pair from being merged.
//! Trips linked this way run as one trip only on the days they all run;
//! on other days each still runs as itself.
//!
//! A combined trip keeps every position of its original trips, so the last stop of one original
//! trip and the first stop of the next are two consecutive positions:
//! a rider staying on rides straight through them, and a rider getting off or on uses only the
//! one that's theirs (no boarding at the end of the first, no alighting at the start of the
//! next). GTFS-RT addresses each original trip by its own stop sequences, on its own positions.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::compile::assemble::StringPool;
use crate::compile::read::{SegmentModel, ServiceModel, TripModel};
use crate::compile::report::FeedReport;
use crate::error::CompileError;
use crate::format::layout::{RULE_DROP_OFF_SHIFT, RULE_PICKUP_MASK, RUN_SCHEDULED};

/// The longest wait a `transfer_type=4` row may bridge, in seconds, as nigiri has it.
const MAX_SEATED_DWELL_SECONDS: i32 = 2 * 3600;

/// The default for [`CompileOptions::seated_wait_seconds`], in seconds.
pub const DEFAULT_SEATED_WAIT_SECONDS: i32 = 600;

/// The default for [`CompileOptions::max_seated_chain`].
pub const DEFAULT_MAX_SEATED_CHAIN: usize = 8;

/// What a compile may do beyond reading the feed as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileOptions {
    /// The longest wait a `block_id` may bridge between two trips, in seconds.
    pub seated_wait_seconds: i32,
    /// The most trips one combined trip may hold; longer chains are cut, and counted.
    pub max_seated_chain: usize,
    /// Whether to merge at all.
    pub merge_seated_trips: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            seated_wait_seconds: DEFAULT_SEATED_WAIT_SECONDS,
            max_seated_chain: DEFAULT_MAX_SEATED_CHAIN,
            merge_seated_trips: true,
        }
    }
}

/// One row of `transfers.txt` about staying seated.
#[derive(Debug, Clone)]
pub(super) struct SeatedRow {
    pub(super) from_trip: String,
    pub(super) to_trip: String,
    /// `true` for `transfer_type=4`, `false` for `transfer_type=5`.
    pub(super) stay_seated: bool,
}

/// A successor link between two trips, valid on the days both run.
#[derive(Clone, Copy)]
struct Link {
    from: usize,
    to: usize,
}

/// Merges `feed`'s trips along their blocks and type 4 rows, per `options`.
#[allow(
    clippy::too_many_lines,
    reason = "one pass from rows to combined trips"
)]
pub(super) fn merge_seated_trips(
    trips_in: &mut Vec<TripModel>,
    services_in: &mut Vec<ServiceModel>,
    seated_rows: &[SeatedRow],
    options: CompileOptions,
    pool: &mut StringPool,
    report: &mut FeedReport,
) -> Result<(), CompileError> {
    if !options.merge_seated_trips || trips_in.is_empty() {
        return Ok(());
    }
    let trip_by_id: HashMap<String, usize> = trips_in
        .iter()
        .enumerate()
        .filter(|(_, trip)| trip.frequency_run == RUN_SCHEDULED)
        .map(|(index, trip)| (pool.text(trip.gtfs_id).to_owned(), index))
        .collect();

    let mut seated_links: Vec<Link> = Vec::new();
    let mut forbidden: HashSet<(usize, usize)> = HashSet::new();
    let mut seated_trips: HashSet<usize> = HashSet::new();
    for row in seated_rows {
        let (Some(&from), Some(&to)) = (
            trip_by_id.get(row.from_trip.as_str()),
            trip_by_id.get(row.to_trip.as_str()),
        ) else {
            report.seated_rows_unmatched += 1;
            continue;
        };
        if from == to {
            report.seated_rows_unmatched += 1;
            continue;
        }
        if row.stay_seated {
            seated_links.push(Link { from, to });
            seated_trips.insert(from);
            seated_trips.insert(to);
        } else {
            forbidden.insert((from, to));
        }
    }

    // Blocks with any trip in a type 4 row don't merge by `block_id`, as nigiri has it.
    let mut block_trips: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (index, trip) in trips_in.iter().enumerate() {
        if trip.block != 0 && trip.frequency_run == RUN_SCHEDULED {
            block_trips.entry(trip.block).or_default().push(index);
        }
    }
    block_trips.retain(|_, trips| !trips.iter().any(|t| seated_trips.contains(t)));

    // Every trip that might join a chain: for each, which other trips its chains can involve.
    let mut groups = UnionFind::new(trips_in.len());
    for trips in block_trips.values() {
        for pair in trips.windows(2) {
            groups.join(pair[0], pair[1]);
        }
    }
    for link in &seated_links {
        groups.join(link.from, link.to);
    }

    // Group involved trips by component and work out each component's dates.
    let mut components: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let block_member: HashSet<usize> = block_trips.values().flatten().copied().collect();
    for &trip in block_member.iter().chain(seated_trips.iter()) {
        components.entry(groups.find(trip)).or_default().push(trip);
    }
    let services: &[ServiceModel] = services_in;
    let trips: &[TripModel] = trips_in;

    let mut seated_chains: BTreeMap<Vec<usize>, Vec<u32>> = BTreeMap::new();
    let mut cover: HashMap<usize, HashSet<u32>> = HashMap::new();
    let mut links_tried = 0_usize;
    let mut links_ok = 0_usize;
    let mut cut_chains: HashSet<Vec<usize>> = HashSet::new();
    for (_, mut members) in components {
        members.sort_unstable();
        members.dedup();
        let mut dates: Vec<u32> = members
            .iter()
            .flat_map(|&t| service_days(&services[trips[t].service as usize]))
            .collect();
        dates.sort_unstable();
        dates.dedup();
        for date in dates {
            let mut running: Vec<usize> = members
                .iter()
                .copied()
                .filter(|&t| service_runs(&services[trips[t].service as usize], date))
                .collect();
            if running.len() < 2 {
                continue;
            }
            running.sort_by_key(|&t| (trips[t].times[0].1, t));
            let running_set: HashSet<usize> = running.iter().copied().collect();

            let mut next: HashMap<usize, usize> = HashMap::new();
            let mut has_prev: HashSet<usize> = HashSet::new();
            let mut link = |from: usize, to: usize| {
                if !next.contains_key(&from) && !has_prev.contains(&to) {
                    next.insert(from, to);
                    has_prev.insert(to);
                }
            };
            // Type 4 rows first, then each block's trips in departure order.
            for link_row in &seated_links {
                if running_set.contains(&link_row.from)
                    && running_set.contains(&link_row.to)
                    && !forbidden.contains(&(link_row.from, link_row.to))
                {
                    links_tried += 1;
                    if seated_link_valid(&trips[link_row.from], &trips[link_row.to]) {
                        links_ok += 1;
                        link(link_row.from, link_row.to);
                    }
                }
            }
            let mut in_block: Vec<usize> = running
                .iter()
                .copied()
                .filter(|t| block_member.contains(t))
                .collect();
            in_block.sort_by_key(|&t| (trips[t].block, trips[t].times[0].1, t));
            for pair in in_block.windows(2) {
                let (a, b) = (pair[0], pair[1]);
                if trips[a].block == trips[b].block && !forbidden.contains(&(a, b)) {
                    links_tried += 1;
                    if block_link_valid(&trips[a], &trips[b], options.seated_wait_seconds) {
                        links_ok += 1;
                        link(a, b);
                    }
                }
            }

            // Follow chains from their heads.
            for &head in running.iter().filter(|t| !has_prev.contains(t)) {
                let mut chain = vec![head];
                let mut cursor = head;
                while let Some(&successor) = next.get(&cursor) {
                    chain.push(successor);
                    cursor = successor;
                }
                if chain.len() > options.max_seated_chain.max(1) {
                    cut_chains.insert(chain.clone());
                }
                for piece in chain.chunks(options.max_seated_chain.max(1)) {
                    if piece.len() >= 2 {
                        seated_chains.entry(piece.to_vec()).or_default().push(date);
                        for &trip in piece {
                            cover.entry(trip).or_default().insert(date);
                        }
                    }
                }
            }
        }
    }
    report.seated_links_tried = links_tried;
    report.seated_links_valid = links_ok;
    report.seated_chains_cut = cut_chains.len();
    if seated_chains.is_empty() {
        return Ok(());
    }

    // Build the combined trips and the services they, and their leftovers, run on.
    let mut services = core::mem::take(services_in);
    let trips = core::mem::take(trips_in);
    let mut service_of_dates: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut merged: Vec<TripModel> = Vec::with_capacity(seated_chains.len());
    for (chain, mut dates) in seated_chains {
        dates.sort_unstable();
        dates.dedup();
        let service = add_service(&mut services, &mut service_of_dates, dates, pool)?;
        merged.push(combine(&chain, &trips, service));
    }
    report.seated_trips_merged = merged.len();
    let mut kept = Vec::with_capacity(trips.len() + merged.len());
    for (index, mut trip) in trips.into_iter().enumerate() {
        let Some(covered) = cover.get(&index) else {
            kept.push(trip);
            continue;
        };
        let remaining: Vec<u32> = service_days(&services[trip.service as usize])
            .filter(|date| !covered.contains(date))
            .collect();
        if remaining.is_empty() {
            report.seated_trips_absorbed += 1;
            continue;
        }
        report.seated_trips_partial += 1;
        trip.service = add_service(&mut services, &mut service_of_dates, remaining, pool)?;
        kept.push(trip);
    }
    kept.extend(merged);
    *trips_in = kept;
    *services_in = services;
    Ok(())
}

/// Whether a type 4 row's pair can be one trip: times must run forward across the join.
fn seated_link_valid(a: &TripModel, b: &TripModel) -> bool {
    let (Some(arrival), Some(departure)) =
        (a.times.last().map(|t| t.0), b.times.first().map(|t| t.1))
    else {
        return false;
    };
    (0..=MAX_SEATED_DWELL_SECONDS).contains(&(departure - arrival)) && attributes_match(a, b)
}

/// Whether two consecutive trips of a block can be one trip.
fn block_link_valid(a: &TripModel, b: &TripModel, wait_seconds: i32) -> bool {
    let (Some(arrival), Some(departure)) =
        (a.times.last().map(|t| t.0), b.times.first().map(|t| t.1))
    else {
        return false;
    };
    a.stops.last() == b.stops.first()
        && (0..=wait_seconds).contains(&(departure - arrival))
        && attributes_match(a, b)
}

/// One route's pattern carries one bike and wheelchair rule, so differing trips stay apart.
fn attributes_match(a: &TripModel, b: &TripModel) -> bool {
    (a.bikes == 1) == (b.bikes == 1) && (a.wheelchair != 2) == (b.wheelchair != 2)
}

/// Combines a chain of trips into one, on the dates in `service`.
fn combine(chain: &[usize], trips: &[TripModel], service: u32) -> TripModel {
    let first = &trips[chain[0]];
    let mut stops = Vec::new();
    let mut times = Vec::new();
    let mut rules = Vec::new();
    let mut sequences = Vec::new();
    let mut stop_headsigns = Vec::new();
    let mut stop_dists = Vec::new();
    let mut segments = Vec::with_capacity(chain.len());
    let has_dists = chain.iter().any(|&t| trips[t].stop_dists.is_some());
    let mut wheelchair = 1;
    for (position_in_chain, &trip_index) in chain.iter().enumerate() {
        let trip = &trips[trip_index];
        let first_position = u32::try_from(stops.len()).unwrap_or(u32::MAX);
        let is_first = position_in_chain == 0;
        let is_last = position_in_chain + 1 == chain.len();
        for position in 0..trip.stops.len() {
            let (arrival, mut departure) = trip.times[position];
            let mut rule = trip.rules[position];
            let mut pair_arrival = arrival;
            if position == 0 && !is_first {
                // The start of a later piece: no alighting; the vehicle has been waiting here.
                rule = (rule & !(RULE_PICKUP_MASK << RULE_DROP_OFF_SHIFT))
                    | (1 << RULE_DROP_OFF_SHIFT);
                pair_arrival = departure;
            }
            if position + 1 == trip.stops.len() && !is_last {
                // The end of an earlier piece: no boarding, because staying on is the point.
                rule = (rule & !RULE_PICKUP_MASK) | 1;
                departure = arrival;
            }
            stops.push(trip.stops[position]);
            times.push((pair_arrival, departure));
            rules.push(rule);
            sequences.push(trip.sequences[position]);
            let headsign = trip
                .stop_headsigns
                .get(position)
                .copied()
                .filter(|&id| id != 0)
                .unwrap_or(trip.headsign);
            stop_headsigns.push(headsign);
            if has_dists {
                stop_dists.push(
                    trip.stop_dists
                        .as_ref()
                        .and_then(|dists| dists.get(position))
                        .copied()
                        .unwrap_or(0.0),
                );
            }
        }
        wheelchair = wheelchair.min(u8::from(trip.wheelchair == 1));
        segments.push(SegmentModel {
            gtfs_id: trip.gtfs_id,
            route: trip.route,
            headsign: trip.headsign,
            shape: trip.shape,
            direction: trip.direction,
            wheelchair: trip.wheelchair,
            bikes: trip.bikes,
            first_position,
            last_position: u32::try_from(stops.len() - 1).unwrap_or(u32::MAX),
        });
    }
    TripModel {
        gtfs_id: first.gtfs_id,
        route: first.route,
        service,
        shape: first.shape,
        headsign: first.headsign,
        block: first.block,
        bikes: first.bikes,
        wheelchair,
        direction: first.direction,
        frequency_run: RUN_SCHEDULED,
        stops,
        times,
        rules,
        stop_headsigns,
        sequences,
        stop_dists: has_dists.then_some(stop_dists),
        segments: Some(segments),
    }
}

fn add_service(
    services: &mut Vec<ServiceModel>,
    by_dates: &mut HashMap<Vec<u32>, u32>,
    dates: Vec<u32>,
    pool: &mut StringPool,
) -> Result<u32, CompileError> {
    if let Some(&index) = by_dates.get(&dates) {
        return Ok(index);
    }
    let (Some(&first), Some(&last)) = (dates.first(), dates.last()) else {
        return Err(CompileError::OutOfRange("an empty calendar".to_owned()));
    };
    let day_count = last - first + 1;
    let mut bits = vec![0_u8; day_count.div_ceil(8) as usize];
    for date in &dates {
        let bit = (date - first) as usize;
        bits[bit / 8] |= 1 << (bit % 8);
    }
    let index =
        u32::try_from(services.len()).map_err(|_| CompileError::TableTooLarge("services"))?;
    let name = pool.intern(&format!("__seated_{index}"))?;
    services.push(ServiceModel {
        gtfs_id: name,
        first_date: first,
        day_count,
        bits,
    });
    by_dates.insert(dates, index);
    Ok(index)
}

fn service_days(service: &ServiceModel) -> impl Iterator<Item = u32> + '_ {
    (0..service.day_count).filter_map(move |offset| {
        (service.bits[(offset / 8) as usize] & (1 << (offset % 8)) != 0)
            .then_some(service.first_date + offset)
    })
}

fn service_runs(service: &ServiceModel, date: u32) -> bool {
    let Some(offset) = date.checked_sub(service.first_date) else {
        return false;
    };
    offset < service.day_count && service.bits[(offset / 8) as usize] & (1 << (offset % 8)) != 0
}

/// Union-find, to split the feed into groups of trips that could chain together.
struct UnionFind {
    parents: Vec<usize>,
}

impl UnionFind {
    fn new(size: usize) -> Self {
        Self {
            parents: (0..size).collect(),
        }
    }

    fn find(&mut self, mut value: usize) -> usize {
        while self.parents[value] != value {
            self.parents[value] = self.parents[self.parents[value]];
            value = self.parents[value];
        }
        value
    }

    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.parents[a] = b;
    }
}
