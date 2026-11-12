//! Exact dominance pruning of transfer walks: leaving out a walk `A → T` that no plan can use,
//! because for every pattern a rider could board at `T`, walking from `A` to a stop `T′`
//! upstream of `T` on that pattern catches every trip `T` would, at least as early.
//!
//! # The rule
//!
//! A walk from origin `A` to target `T` is dropped if, for **every** pattern `P` with a trip
//! picking up at `T` (at any position of `T` on `P`), some earlier position of `P` has a stop
//! `T′` where
//!
//! - every trip of `P` picks up (`pickup_type` isn't 1, as the planner reads it);
//! - `A → T′` is in `A`'s walks and isn't a forbidden pair;
//! - `cost(A → T) > cost(A → T′) + R`, where `cost` is what the planner charges for a walk
//!   (the longest of its seconds, both stops' transfer times, and any `transfers.txt` floor
//!   for the pair) and `R` bounds, over every trip of `P` on every day, the time from its
//!   departure at `T′` to its departure at `T`, by the lesser of two bounds: the latest
//!   departure offset at `T` minus the earliest at `T′` (offsets from each trip's first
//!   departure), and the sum of the longest step between departures at each stop between them.
//!
//! Then any trip catchable at `T` is catchable at `T′`: a rider at `T′` by
//! `t + cost(A → T′)` is there before the trip leaves `T′`, since it leaves `T` no more than `R`
//! later and the rider at `T` would have needed `t + cost(A → T)`. The same trip is ridden,
//! so arrivals, alighting stops, and the number of boardings are the same; the walk is
//! shorter. A target with no pattern picking up at it serves no boarding, so it goes too.
//!
//! # What the planner checks, and how this keeps to it
//!
//! - **Where walks lead.** A walk between rounds only serves the next round's boardings
//!   (`valinor-transit`'s RAPTOR: the target is reached from trips, and a walk never follows a
//!   walk), so a target matters only as a boarding stop.
//! - **Arrival and boardings.** The same trip from `T′`: identical (above). In a backward
//!   (latest departure) search, the trip's departure from `T′` minus `cost(A → T′)` is later
//!   than from `T` minus `cost(A → T)`, by the same inequality.
//! - **Ties.** The inequality is strict, so a walk to `T′` is strictly better for every trip
//!   in both directions, and no search picks `T` over `T′` on a tie; plans stay identical.
//! - **Transfer times, `transfers.txt` minimums, forbidden pairs.** Compared as costs, with
//!   both stops' transfer times and the pair's floor; a forbidden `A → T′` never dominates.
//!   A forbidden `A → T` is left as it is.
//! - **Patterns are the planner's routes** (a compiled pattern is one route), across every
//!   feed: a target's patterns are read from its own feed's timetable, whichever feed `A` is in.
//! - **Stay-seated merges** are patterns of their own in the compiled timetable (a merged
//!   trip's stops in order), so `T′` upstream on a merged pattern is ridden through on the same
//!   vehicle; block continuations onto other patterns are never used as dominators.
//! - **Frequencies** are compiled to fixed runs, which `R` covers like any trip.
//! - **Wheelchair.** For wheelchair walks, patterns not marked accessible are skipped (a
//!   wheelchair query can't ride them), and `T′` must be a stop a wheelchair can board at;
//!   trips are the same trips, so their accessibility is the same.
//! - **Route filters** (bicycles allowed, modes) act on the pattern, which `T′` shares.
//! - **The bootstrap walk limit** of a profile caps walks' seconds, but it only tightens the
//!   profile's bounds, never its result (`valinor-transit`'s `ProfileOptions`).
//!
//! # GTFS-RT
//!
//! Real time is assumed not to lengthen a trip's run from `T′` to `T` past `R` (its delay
//! doesn't grow between them), and not to cancel its stop at `T′` while keeping `T`.
//! Trips added in real time on patterns not in the timetable aren't considered.
//! None of this is checked at run time.

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use valinor_gtfs::CompiledTimetable;
use valinor_transit::{PairRule, RouteId, StopId, TimetableSource, TripId};
use valinor_walk::Hop;

use crate::compile::{FeedOutcome, FeedRecord};
use crate::error::BuildError;
use crate::stops::StopList;

/// A step no reach bound can cross: a trip with no time at a position.
const UNBOUNDED_STEP: i32 = 1 << 28;

/// `pickup_type` 1 is the only one that keeps riders off (as `valinor-gtfs` reads it).
const NO_PICKUP: u8 = 1;

/// A pattern position: `(stop, every trip picks up, some trip picks up, earliest and latest
/// departure offset)`.
type Position = (u32, bool, bool, i32, i32);

/// One feed's patterns, in its own stop numbering.
#[derive(Default)]
struct FeedPatterns {
    /// Per stop, its transfer time, and whether a wheelchair may board there.
    stops: Vec<(i32, bool)>,
    /// `(from, to, floor)`, `None` for forbidden.
    pairs: Vec<(u32, u32, Option<i32>)>,
    /// Per pattern: whether it's wheelchair accessible, and its positions as
    /// `(stop, every trip picks up, some trip picks up, earliest and latest departure offset)`.
    patterns: Vec<(bool, Vec<Position>)>,
    /// Per pattern and position, the longest a trip takes from its departure at the position
    /// before to its departure here (0 at the first).
    steps: Vec<Vec<i32>>,
}

/// What pruning needs of every feed's timetable, in the global stop numbering.
pub struct Dominance {
    transfer_time: Vec<i32>,
    wheelchair_stop: Vec<bool>,
    pairs: HashMap<(u32, u32), Option<i32>>,
    /// Per global stop, `(pattern, position)` where some trip picks up (a CSR).
    boarding_start: Vec<u32>,
    boardings: Vec<(u32, u32)>,
    /// Per pattern, where its positions start (a CSR), and whether it's wheelchair accessible.
    pattern_start: Vec<u32>,
    pattern_wheelchair: Vec<bool>,
    position_stop: Vec<u32>,
    position_all_pickup: Vec<bool>,
    earliest: Vec<i32>,
    latest: Vec<i32>,
    /// Per position, the sum of the longest steps from the pattern's first position.
    reach: Vec<i64>,
}

/// A worker's scratch space for [`Dominance::prune`].
#[derive(Default)]
pub struct Scratch {
    /// Bumped by each [`Dominance::prune`], which marks the patterns it visits with it.
    epoch: u32,
    /// Per pattern, the epoch it was last visited in.
    pattern_seen: Vec<u32>,
    patterns: Vec<u32>,
    /// Per hop: its cost, and how many of its boardings are not yet dominated.
    cost: Vec<Option<i32>>,
    open: Vec<u32>,
}

impl Dominance {
    /// Reads every compiled feed's patterns and stop rules, on `threads` threads.
    ///
    /// # Errors
    ///
    /// If a timetable can't be read.
    #[expect(
        clippy::too_many_lines,
        reason = "one pass over the feeds into flat tables"
    )]
    pub fn load(
        compiled: &Path,
        feeds: &[FeedRecord],
        stops: &StopList,
        threads: NonZeroUsize,
    ) -> Result<Self, BuildError> {
        let next = AtomicUsize::new(0);
        let mut loaded: Vec<Option<FeedPatterns>> = Vec::new();
        loaded.resize_with(feeds.len(), || None);
        let loaded_lock = Mutex::new(&mut loaded);
        let first_error: Mutex<Option<BuildError>> = Mutex::new(None);
        std::thread::scope(|scope| {
            for _ in 0..threads.get().min(feeds.len().max(1)) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(feed) = feeds.get(index) else {
                            break;
                        };
                        match load_feed(compiled, feed) {
                            Ok(data) => {
                                if let Ok(mut loaded) = loaded_lock.lock() {
                                    loaded[index] = Some(data);
                                }
                            }
                            Err(error) => {
                                if let Ok(mut first) = first_error.lock() {
                                    first.get_or_insert(error);
                                }
                            }
                        }
                    }
                });
            }
        });
        if let Some(error) = first_error.into_inner().ok().flatten() {
            return Err(error);
        }
        let mut dominance = Self {
            transfer_time: vec![0; stops.len()],
            wheelchair_stop: vec![true; stops.len()],
            pairs: HashMap::new(),
            boarding_start: Vec::new(),
            boardings: Vec::new(),
            pattern_start: vec![0],
            pattern_wheelchair: Vec::new(),
            position_stop: Vec::new(),
            position_all_pickup: Vec::new(),
            earliest: Vec::new(),
            latest: Vec::new(),
            reach: Vec::new(),
        };
        let mut boardings: Vec<(u32, u32, u32)> = Vec::new();
        for (feed, data) in loaded.into_iter().enumerate() {
            let data = data.ok_or(BuildError::WorkerPanicked)?;
            let feed = u32::try_from(feed).map_err(|_| BuildError::TooManyEntries)?;
            let global = |stop: u32| stops.global(feed, stop);
            for (stop, &(time, wheelchair)) in data.stops.iter().enumerate() {
                let stop = u32::try_from(stop).map_err(|_| BuildError::TooManyEntries)?;
                if let Some(g) = global(stop) {
                    dominance.transfer_time[g as usize] = time;
                    dominance.wheelchair_stop[g as usize] = wheelchair;
                }
            }
            for (from, to, floor) in data.pairs {
                if let (Some(from), Some(to)) = (global(from), global(to)) {
                    dominance.pairs.insert((from, to), floor);
                }
            }
            for ((wheelchair, positions), steps) in data.patterns.into_iter().zip(data.steps) {
                let mut reach = 0_i64;
                for step in steps {
                    reach += i64::from(step);
                    dominance.reach.push(reach);
                }
                let pattern = u32::try_from(dominance.pattern_wheelchair.len())
                    .map_err(|_| BuildError::TooManyEntries)?;
                dominance.pattern_wheelchair.push(wheelchair);
                for (position, (stop, all, some, earliest, latest)) in
                    positions.into_iter().enumerate()
                {
                    let stop = global(stop).unwrap_or(u32::MAX);
                    if some && stop != u32::MAX {
                        let position =
                            u32::try_from(position).map_err(|_| BuildError::TooManyEntries)?;
                        boardings.push((stop, pattern, position));
                    }
                    dominance.position_stop.push(stop);
                    dominance.position_all_pickup.push(all);
                    dominance.earliest.push(earliest);
                    dominance.latest.push(latest);
                }
                dominance.pattern_start.push(
                    u32::try_from(dominance.position_stop.len())
                        .map_err(|_| BuildError::TooManyEntries)?,
                );
            }
        }
        boardings.sort_unstable();
        dominance.boarding_start = vec![0; stops.len() + 1];
        for &(stop, ..) in &boardings {
            dominance.boarding_start[stop as usize + 1] += 1;
        }
        for i in 0..stops.len() {
            dominance.boarding_start[i + 1] += dominance.boarding_start[i];
        }
        dominance.boardings = boardings.into_iter().map(|(_, p, i)| (p, i)).collect();
        Ok(dominance)
    }

    /// How many patterns there are.
    pub fn pattern_count(&self) -> usize {
        self.pattern_wheelchair.len()
    }

    /// What the planner charges for a walk of `seconds` from `from` to `to`, or `None` if the
    /// pair is forbidden (`valinor-transit`'s `SeedRules::cost`).
    fn cost(&self, from: u32, to: u32, seconds: u32) -> Option<i32> {
        let seconds = i32::try_from(seconds).unwrap_or(i32::MAX);
        let time = |stop: u32| self.transfer_time.get(stop as usize).copied().unwrap_or(0);
        let mut cost = seconds.max(time(from)).max(time(to));
        if let Some(rule) = self.pairs.get(&(from, to)) {
            cost = cost.max((*rule)?);
        }
        Some(cost)
    }

    fn boardings_of(&self, stop: u32) -> &[(u32, u32)] {
        let start = self.boarding_start.get(stop as usize).copied().unwrap_or(0) as usize;
        let end = self
            .boarding_start
            .get(stop as usize + 1)
            .copied()
            .unwrap_or(0) as usize;
        self.boardings.get(start..end).unwrap_or(&[])
    }

    /// Leaves out of `hops` (sorted by stop, walks from global stop `origin`) the walks that
    /// no plan can use (see the [module docs](self)); returns how many.
    /// With `wheelchair`, as wheelchair plans ride.
    pub fn prune(
        &self,
        origin: u32,
        hops: &mut Vec<Hop>,
        wheelchair: bool,
        scratch: &mut Scratch,
    ) -> usize {
        let pattern_count = self.pattern_count();
        if scratch.pattern_seen.len() < pattern_count {
            scratch.pattern_seen.resize(pattern_count, 0);
        }
        scratch.epoch = scratch.epoch.wrapping_add(1);
        if scratch.epoch == 0 {
            scratch.pattern_seen.fill(0);
            scratch.epoch = 1;
        }
        let stamp = scratch.epoch;
        scratch.patterns.clear();
        scratch.cost.clear();
        scratch.open.clear();
        for hop in hops.iter() {
            scratch.cost.push(self.cost(origin, hop.stop, hop.seconds));
            let mut open = 0;
            for &(pattern, _) in self.boardings_of(hop.stop) {
                if wheelchair && !self.pattern_wheelchair[pattern as usize] {
                    continue;
                }
                open += 1;
                let seen = &mut scratch.pattern_seen[pattern as usize];
                if *seen != stamp {
                    *seen = stamp;
                    scratch.patterns.push(pattern);
                }
            }
            scratch.open.push(open);
        }
        let find = |stop: u32| hops.binary_search_by_key(&stop, |hop| hop.stop).ok();
        for &pattern in &scratch.patterns {
            let start = self.pattern_start[pattern as usize] as usize;
            let end = self.pattern_start[pattern as usize + 1] as usize;
            // The least `cost(A → T′) - earliest(T′)`, and `cost(A → T′) - reach(T′)`, over the
            // dominators seen so far: each bound alone is enough.
            let mut best: Option<i64> = None;
            let mut best_reach: Option<i64> = None;
            for position in start..end {
                let stop = self.position_stop[position];
                let Some(hop) = find(stop) else {
                    continue;
                };
                let cost = scratch.cost[hop];
                // Is this a boarding of the target here, and is it dominated?
                if let (Some(cost), Some(best), Some(best_reach)) = (cost, best, best_reach)
                    && self
                        .boardings_of(stop)
                        .binary_search(&(
                            pattern,
                            u32::try_from(position - start).unwrap_or(u32::MAX),
                        ))
                        .is_ok()
                    && (i64::from(cost) - i64::from(self.latest[position]) > best
                        || i64::from(cost) - self.reach[position] > best_reach)
                {
                    scratch.open[hop] -= 1;
                }
                if let Some(cost) = cost
                    && self.position_all_pickup[position]
                    && (!wheelchair || self.wheelchair_stop[stop as usize])
                {
                    let candidate = i64::from(cost) - i64::from(self.earliest[position]);
                    best = Some(best.map_or(candidate, |b| b.min(candidate)));
                    let candidate = i64::from(cost) - self.reach[position];
                    best_reach = Some(best_reach.map_or(candidate, |b| b.min(candidate)));
                }
            }
        }
        let before = hops.len();
        let mut index = 0;
        hops.retain(|_| {
            let keep = scratch.cost[index].is_none() || scratch.open[index] > 0;
            index += 1;
            keep
        });
        before - hops.len()
    }
}

fn load_feed(compiled: &Path, feed: &FeedRecord) -> Result<FeedPatterns, BuildError> {
    if feed.outcome != FeedOutcome::Compiled {
        return Ok(FeedPatterns::default());
    }
    let Some(relative) = &feed.timetable else {
        return Ok(FeedPatterns::default());
    };
    let path = compiled.join(relative);
    let bytes = fs::read(&path).map_err(|source| BuildError::Read { path, source })?;
    let timetable = CompiledTimetable::parse(&bytes).map_err(|source| BuildError::Timetable {
        feed: feed.id.clone(),
        source: source.into(),
    })?;
    let mut data = FeedPatterns::default();
    for stop in 0..timetable.stop_count() {
        data.stops.push((
            TimetableSource::min_change(&timetable, stop),
            TimetableSource::wheelchair_stop_accessible(&timetable, stop),
        ));
    }
    for pair in TimetableSource::transfer_pairs(&timetable) {
        data.pairs.push((
            pair.from.into_inner(),
            pair.to.into_inner(),
            match pair.rule {
                PairRule::Forbidden => None,
                PairRule::Floor(seconds) => {
                    Some(i32::try_from(seconds.into_inner()).unwrap_or(i32::MAX))
                }
            },
        ));
    }
    for pattern in 0..timetable.pattern_count() {
        let id = RouteId::new(u32::try_from(pattern).map_err(|_| BuildError::TooManyEntries)?);
        let Some(info) = timetable.pattern(id) else {
            data.patterns.push((false, Vec::new()));
            data.steps.push(Vec::new());
            continue;
        };
        let positions: Vec<Position> = info
            .stops()
            .map(|stop: StopId| (stop.into_inner(), true, false, i32::MAX, i32::MIN))
            .collect();
        data.steps.push(vec![0; positions.len()]);
        data.patterns.push((info.wheelchair_accessible, positions));
    }
    for trip in 0..timetable.trip_count() {
        let id = TripId::new(u32::try_from(trip).map_err(|_| BuildError::TooManyEntries)?);
        let Some(info) = timetable.trip(id) else {
            continue;
        };
        let pattern = info.pattern.into_inner() as usize;
        let (Some((_, positions)), Some(steps)) =
            (data.patterns.get_mut(pattern), data.steps.get_mut(pattern))
        else {
            continue;
        };
        let mut previous: Option<i32> = None;
        let Some((_, first)) = info.time_at(0) else {
            continue;
        };
        for (position, slot) in positions.iter_mut().enumerate() {
            let Some((_, departure)) = info.time_at(position) else {
                // A trip without a time here can't be bounded: no dominance through it.
                slot.1 = false;
                slot.3 = i32::MIN;
                slot.4 = i32::MAX;
                steps[position] = UNBOUNDED_STEP;
                previous = None;
                continue;
            };
            if position > 0 {
                let step = previous.map_or(UNBOUNDED_STEP, |p| departure - p);
                steps[position] = steps[position].max(step);
            }
            previous = Some(departure);
            let picks_up = info.pickup_type_at(position) != NO_PICKUP;
            slot.1 &= picks_up;
            slot.2 |= picks_up;
            let offset = departure - first;
            slot.3 = slot.3.min(offset);
            slot.4 = slot.4.max(offset);
        }
    }
    // Positions no trip runs through can't dominate, and their latest offset bounds nothing.
    for (_, positions) in &mut data.patterns {
        for slot in positions.iter_mut() {
            if slot.3 == i32::MAX {
                slot.1 = false;
                slot.3 = i32::MIN;
                slot.4 = i32::MAX;
            }
        }
    }
    Ok(data)
}

#[cfg(test)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a handful of test stops and patterns"
)]
mod tests {
    use super::*;

    /// Stops 0 (the origin) to 5; patterns as `(wheelchair, [(stop, every trip picks up,
    /// some trip does, earliest offset, latest offset)])`.
    fn dominance(patterns: &[(bool, &[Position])]) -> Dominance {
        let mut d = Dominance {
            transfer_time: vec![0; 6],
            wheelchair_stop: vec![true; 6],
            pairs: HashMap::new(),
            boarding_start: Vec::new(),
            boardings: Vec::new(),
            pattern_start: vec![0],
            pattern_wheelchair: Vec::new(),
            position_stop: Vec::new(),
            position_all_pickup: Vec::new(),
            earliest: Vec::new(),
            latest: Vec::new(),
            reach: Vec::new(),
        };
        let mut boardings = Vec::new();
        for (p, (wheelchair, positions)) in patterns.iter().enumerate() {
            d.pattern_wheelchair.push(*wheelchair);
            for (i, &(stop, all, some, earliest, latest)) in positions.iter().enumerate() {
                if some {
                    boardings.push((stop, p as u32, i as u32));
                }
                d.position_stop.push(stop);
                d.position_all_pickup.push(all);
                d.earliest.push(earliest);
                d.latest.push(latest);
                // Steps that add up to the latest offsets: a looser bound than they give.
                d.reach
                    .push(i64::from(latest) + i64::from(latest - earliest));
            }
            d.pattern_start.push(d.position_stop.len() as u32);
        }
        boardings.sort_unstable();
        d.boarding_start = vec![0; 7];
        for &(stop, ..) in &boardings {
            d.boarding_start[stop as usize + 1] += 1;
        }
        for i in 0..6 {
            d.boarding_start[i + 1] += d.boarding_start[i];
        }
        d.boardings = boardings.into_iter().map(|(_, p, i)| (p, i)).collect();
        d
    }

    fn hops(walks: &[(u32, u32)]) -> Vec<Hop> {
        walks
            .iter()
            .map(|&(stop, seconds)| Hop { stop, seconds })
            .collect()
    }

    fn kept(d: &Dominance, walks: &[(u32, u32)], wheelchair: bool) -> Vec<u32> {
        let mut hops = hops(walks);
        d.prune(0, &mut hops, wheelchair, &mut Scratch::default());
        hops.iter().map(|hop| hop.stop).collect()
    }

    /// A line 1 → 2 → 3, leaving 2 between 100 and 150 s after 1 on every trip.
    const LINE: &[Position] = &[
        (1, true, true, 0, 0),
        (2, true, true, 100, 150),
        (3, true, true, 300, 400),
    ];

    #[test]
    fn a_walk_dominated_upstream_goes() {
        let d = dominance(&[(true, LINE)]);
        // 2 at 400 s: 1 at 200 s plus a ride of at most 150 s is sooner.
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1]);
        // At exactly 350 s it's a tie, which stays.
        assert_eq!(kept(&d, &[(1, 200), (2, 350)], false), [1, 2]);
        // 3 is dominated by 1 (200 + 400 < 700) or 2.
        assert_eq!(kept(&d, &[(1, 200), (2, 349), (3, 700)], false), [1, 2]);
    }

    #[test]
    fn every_pattern_needs_a_dominator() {
        // Stop 2 is also served by a pattern starting there.
        let d = dominance(&[
            (true, LINE),
            (true, &[(2, true, true, 0, 0), (4, true, true, 60, 60)]),
        ]);
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1, 2]);
        // A target served by nothing serves no boarding.
        assert_eq!(kept(&d, &[(1, 200), (5, 900)], false), [1]);
    }

    #[test]
    fn dominators_must_be_boardable() {
        let no_pickup: &[(u32, bool, bool, i32, i32)] =
            &[(1, false, true, 0, 0), (2, true, true, 100, 150)];
        let d = dominance(&[(true, no_pickup)]);
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1, 2]);

        // Transfer times count: stop 1's 300 s makes its walk cost 300, which no longer wins.
        let mut d = dominance(&[(true, LINE)]);
        d.transfer_time[1] = 300;
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1, 2]);
        // A forbidden walk to 1 dominates nothing.
        let mut d = dominance(&[(true, LINE)]);
        d.pairs.insert((0, 1), None);
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1, 2]);
        // A floor on the walk to 1 counts too.
        d.pairs.insert((0, 1), Some(260));
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1, 2]);
        d.pairs.insert((0, 1), Some(240));
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1]);
    }

    #[test]
    fn wheelchairs_board_only_where_they_can() {
        let mut d = dominance(&[(true, LINE)]);
        d.wheelchair_stop[1] = false;
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], false), [1]);
        assert_eq!(kept(&d, &[(1, 200), (2, 400)], true), [1, 2]);
        // A pattern a wheelchair can't ride needs no dominator for wheelchair walks.
        let d = dominance(&[(false, &[(2, true, true, 0, 0), (4, true, true, 60, 60)])]);
        assert_eq!(kept(&d, &[(2, 400)], true), Vec::<u32>::new());
        assert_eq!(kept(&d, &[(2, 400)], false), [2]);
    }
}
