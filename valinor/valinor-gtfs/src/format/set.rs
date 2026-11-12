//! Several compiled timetables loaded at once, numbered as one.
//!
//! Each feed keeps its own compiled file, memory-mapped as it is.
//! A [`TimetableSet`] gives those files one numbering: a stop's number is its number in its file
//! plus that file's base, and likewise for patterns and trips.
//! Only the materialized window that a search reads is concatenated
//! (see `valinor_transit::materialize_parts`).
//!
//! Rider-facing questions (stop names, trips, routes, agencies, and so on) are answered by finding
//! the file a number belongs to and asking that file, on the cold path, in the file's own numbers.
//! This is why adding or removing a feed never rewrites a file:
//! each one is unchanged, and the base numbers are recomputed when a set is built.

use std::collections::BTreeMap;

use thiserror::Error;
use valinor_transit::{RouteId, StopId, TripId};

use crate::format::reader::{CompiledTimetable, LoadedTimetable};

/// Why some timetables can't be loaded together.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SetError {
    /// A set needs at least one file: there is nothing to route on otherwise.
    #[error("load a timetable set with no files")]
    Empty,
    /// Two files (or the same one twice) hold the same feed, so its identifiers would be ambiguous.
    #[error("load feed {0} from two files")]
    DuplicateFeed(String),
    /// A table's numbering would pass what a `u32` can number.
    #[error("number more {0} across files than a u32 can hold")]
    TooLarge(&'static str),
}

/// One file's place in a [`TimetableSet`].
struct SetPart {
    timetable: LoadedTimetable,
    /// How many earlier files' stops are numbered before this file's.
    first_stop: u32,
    first_pattern: u32,
    first_trip: u32,
}

/// One file of a [`TimetableSet`], with the bases that map its numbers to the set's numbers.
///
/// Cheap to copy; it borrows the set.
#[derive(Debug, Clone, Copy)]
pub struct TimetablePart<'a> {
    /// The file.
    compiled: &'a CompiledTimetable<'a>,
    /// The file's position in the set.
    index: usize,
    first_stop: u32,
    first_pattern: u32,
    first_trip: u32,
}

impl<'a> TimetablePart<'a> {
    /// The file this part is.
    #[inline]
    pub fn compiled(&self) -> &'a CompiledTimetable<'a> {
        self.compiled
    }

    /// This file's position in the set.
    #[inline]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Added to a file's stop number to get the set's.
    #[inline]
    pub fn first_stop(&self) -> u32 {
        self.first_stop
    }

    /// Added to a file's pattern number to get the set's.
    #[inline]
    pub fn first_pattern(&self) -> u32 {
        self.first_pattern
    }

    /// Added to a file's trip number to get the set's.
    #[inline]
    pub fn first_trip(&self) -> u32 {
        self.first_trip
    }

    /// The set's number for one of this file's stops.
    #[inline]
    pub fn global_stop(&self, local: StopId) -> StopId {
        StopId::new(local.into_inner() + self.first_stop)
    }

    /// This file's number for a stop of the set's, if the stop is in this file.
    #[inline]
    pub fn local_stop(&self, global: StopId) -> Option<StopId> {
        let raw = global.into_inner().checked_sub(self.first_stop)?;
        (raw as usize)
            .lt(&self.compiled.stop_count())
            .then(|| StopId::new(raw))
    }

    /// The set's number for one of this file's patterns.
    #[inline]
    pub fn global_route(&self, local: RouteId) -> RouteId {
        RouteId::new(local.into_inner() + self.first_pattern)
    }

    /// The set's number for one of this file's trips.
    #[inline]
    pub fn global_trip(&self, local: TripId) -> TripId {
        TripId::new(local.into_inner() + self.first_trip)
    }
}

/// Timetables, each from its own file, numbered together; see the module documentation.
pub struct TimetableSet {
    parts: Vec<SetPart>,
    /// Which file holds each feed, by feed ID.
    feeds: BTreeMap<String, usize>,
    stop_count: u32,
    pattern_count: u32,
    trip_count: u32,
}

impl core::fmt::Debug for TimetableSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TimetableSet")
            .field("files", &self.parts.len())
            .field("feeds", &self.feeds.keys().collect::<Vec<_>>())
            .field("stop_count", &self.stop_count)
            .field("pattern_count", &self.pattern_count)
            .field("trip_count", &self.trip_count)
            .finish()
    }
}

impl TimetableSet {
    /// Numbers `files` as one, in the order given.
    ///
    /// # Errors
    ///
    /// If there are no files, two files hold the same feed, or a numbering overflows.
    pub fn new(files: Vec<LoadedTimetable>) -> Result<Self, SetError> {
        if files.is_empty() {
            return Err(SetError::Empty);
        }
        let mut feeds = BTreeMap::new();
        let mut parts = Vec::with_capacity(files.len());
        let (mut first_stop, mut first_pattern, mut first_trip) = (0_u32, 0_u32, 0_u32);
        for (index, timetable) in files.into_iter().enumerate() {
            let compiled = timetable.timetable();
            for info in compiled.feeds() {
                if feeds.insert(info.id.to_owned(), index).is_some() {
                    return Err(SetError::DuplicateFeed(info.id.to_owned()));
                }
            }
            let stop_count = count(compiled.stop_count(), "stops")?;
            let pattern_count = count(compiled.pattern_count(), "patterns")?;
            let trip_count = count(compiled.trip_count(), "trips")?;
            parts.push(SetPart {
                timetable,
                first_stop,
                first_pattern,
                first_trip,
            });
            first_stop = first_stop
                .checked_add(stop_count)
                .ok_or(SetError::TooLarge("stops"))?;
            first_pattern = first_pattern
                .checked_add(pattern_count)
                .ok_or(SetError::TooLarge("patterns"))?;
            first_trip = first_trip
                .checked_add(trip_count)
                .ok_or(SetError::TooLarge("trips"))?;
        }
        Ok(Self {
            parts,
            feeds,
            stop_count: first_stop,
            pattern_count: first_pattern,
            trip_count: first_trip,
        })
    }

    /// A set of one file, for a timetable that isn't part of a multi-feed build.
    ///
    /// # Errors
    ///
    /// Only if the file holds two feeds of the same ID, which a compiled file never has.
    pub fn single(file: LoadedTimetable) -> Result<Self, SetError> {
        Self::new(vec![file])
    }

    /// The file at `part`'s position.
    fn part_at(&self, index: usize) -> Option<TimetablePart<'_>> {
        let part = self.parts.get(index)?;
        Some(TimetablePart {
            compiled: part.timetable.timetable(),
            index,
            first_stop: part.first_stop,
            first_pattern: part.first_pattern,
            first_trip: part.first_trip,
        })
    }

    /// Every file, in numbering order.
    pub fn parts(&self) -> impl ExactSizeIterator<Item = TimetablePart<'_>> + '_ {
        self.parts
            .iter()
            .enumerate()
            .map(|(index, part)| TimetablePart {
                compiled: part.timetable.timetable(),
                index,
                first_stop: part.first_stop,
                first_pattern: part.first_pattern,
                first_trip: part.first_trip,
            })
    }

    /// How many files there are.
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    /// How many stops the set numbers across all files.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.stop_count as usize
    }

    /// How many patterns the set numbers across all files.
    #[inline]
    pub fn pattern_count(&self) -> usize {
        self.pattern_count as usize
    }

    /// How many trips the set numbers across all files.
    #[inline]
    pub fn trip_count(&self) -> usize {
        self.trip_count as usize
    }

    /// Every feed ID in the set, ascending, with its file's position.
    pub fn feeds(&self) -> impl Iterator<Item = (&str, usize)> + '_ {
        self.feeds.iter().map(|(id, part)| (id.as_str(), *part))
    }

    /// The file that holds `feed`.
    pub fn part_of_feed(&self, feed: &str) -> Option<TimetablePart<'_>> {
        self.part_at(*self.feeds.get(feed)?)
    }

    /// The file holding the stop numbered `stop`, and that stop's number in the file.
    ///
    /// `None` means it is past every file: an overlay's new stop, or simply unknown.
    #[inline]
    pub fn stop_part(&self, stop: StopId) -> Option<(TimetablePart<'_>, StopId)> {
        let raw = stop.into_inner();
        if raw >= self.stop_count {
            return None;
        }
        let index = self
            .parts
            .partition_point(|part| part.first_stop <= raw)
            .checked_sub(1)?;
        let part = self.part_at(index)?;
        Some((part, StopId::new(raw - part.first_stop)))
    }

    /// The file holding the pattern numbered `route`, and that pattern's number in the file.
    ///
    /// `None` means it is past every file, as an overlay's added route is.
    #[inline]
    pub fn route_part(&self, route: RouteId) -> Option<(TimetablePart<'_>, RouteId)> {
        let raw = route.into_inner();
        if raw >= self.pattern_count {
            return None;
        }
        let index = self
            .parts
            .partition_point(|part| part.first_pattern <= raw)
            .checked_sub(1)?;
        let part = self.part_at(index)?;
        Some((part, RouteId::new(raw - part.first_pattern)))
    }

    /// The file holding the trip numbered `trip`, and that trip's number in the file.
    ///
    /// `None` means it is past every file, as an overlay's added trip is.
    #[inline]
    pub fn trip_part(&self, trip: TripId) -> Option<(TimetablePart<'_>, TripId)> {
        let raw = trip.into_inner();
        if raw >= self.trip_count {
            return None;
        }
        let index = self
            .parts
            .partition_point(|part| part.first_trip <= raw)
            .checked_sub(1)?;
        let part = self.part_at(index)?;
        Some((part, TripId::new(raw - part.first_trip)))
    }
}

fn count(value: usize, what: &'static str) -> Result<u32, SetError> {
    u32::try_from(value).map_err(|_| SetError::TooLarge(what))
}

#[cfg(all(test, feature = "compile"))]
mod tests {
    use core::fmt::Write as _;

    use super::*;
    use crate::{FeedId, FeedInput, InMemorySource, compile};
    use proptest::prelude::*;

    /// A feed with `stops` stops, `patterns` two-stop patterns, and `trips` trips per pattern.
    fn feed(id: &str, stops: u32, patterns: u32, trips: u32) -> Vec<u8> {
        let mut stops_csv = String::from("stop_id,stop_name,stop_lat,stop_lon\n");
        for s in 0..stops {
            writeln!(stops_csv, "S{s},{id}-stop-{s},47.{s:03},-122.{s:03}").expect("to a string");
        }
        let mut routes_csv = String::from("route_id,route_short_name,route_type\n");
        let mut trips_csv = String::from("route_id,service_id,trip_id\n");
        let mut times_csv =
            String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
        for p in 0..patterns {
            writeln!(routes_csv, "R{p},{p},3").expect("to a string");
            for t in 0..trips {
                writeln!(trips_csv, "R{p},WK,T{p}x{t}").expect("to a string");
                let (a, b) = (p % stops, (p + 1) % stops);
                let minute = t * 10;
                writeln!(
                    times_csv,
                    "T{p}x{t},08:{minute:02}:00,08:{minute:02}:00,S{a},1\n\
                     T{p}x{t},09:{minute:02}:00,09:{minute:02}:00,S{b},2"
                )
                .expect("to a string");
            }
        }
        let source = InMemorySource::new()
            .with(
                "agency.txt",
                "agency_name,agency_timezone,agency_url\nX,America/Los_Angeles,https://x.test\n",
            )
            .with("routes.txt", &routes_csv)
            .with("stops.txt", &stops_csv)
            .with(
                "calendar.txt",
                "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,\
                 start_date,end_date\nWK,1,1,1,1,1,1,1,20260601,20260630\n",
            )
            .with("trips.txt", &trips_csv)
            .with("stop_times.txt", &times_csv);
        compile(vec![FeedInput {
            id: FeedId::try_new(id.to_owned()).expect("valid feed ID"),
            source: Box::new(source),
        }])
        .expect("the feed compiles")
        .bytes
    }

    fn loaded(id: &str, stops: u32, patterns: u32, trips: u32) -> LoadedTimetable {
        LoadedTimetable::from_bytes(feed(id, stops, patterns, trips)).expect("loads")
    }

    /// A window read straight from a file matches one from its full in-memory copy.
    #[test]
    fn a_file_materializes_like_its_in_memory_copy() {
        use crate::parse_service_date;
        use valinor_transit::{ServiceDay, Time, TimetableSource};

        let bytes = feed("feed", 8, 5, 3);
        let compiled = crate::CompiledTimetable::parse(&bytes).expect("parses");
        let timetable = compiled.to_timetable().expect("copies");
        assert_eq!(
            compiled.stop_count(),
            TimetableSource::stop_count(&timetable)
        );
        assert_eq!(
            TimetableSource::route_count(&compiled),
            TimetableSource::route_count(&timetable)
        );
        let day = ServiceDay {
            date: parse_service_date("20260615").expect("date"),
            offset: Time::try_new(3_600).expect("in range"),
        };
        for raw in 0..u32::try_from(compiled.pattern_count()).expect("fits") {
            let route = RouteId::new(raw);
            assert_eq!(
                compiled.resolved_trips_on(route, day.date, day.offset),
                timetable.resolved_trips_on(route, day.date, day.offset)
            );
            assert_eq!(
                compiled.route_stops_of(route),
                TimetableSource::route_stops_of(&timetable, route)
            );
            assert_eq!(
                compiled.route_attributes_of(route),
                TimetableSource::route_attributes_of(&timetable, route)
            );
        }
        for stop in 0..TimetableSource::stop_count(&compiled) {
            assert_eq!(
                TimetableSource::min_change(&compiled, stop),
                TimetableSource::min_change(&timetable, stop)
            );
        }
    }

    #[test]
    fn duplicate_feed_is_refused() {
        let error = TimetableSet::new(vec![loaded("a", 3, 2, 1), loaded("a", 4, 1, 1)])
            .expect_err("two copies of feed a");
        assert_eq!(error, SetError::DuplicateFeed("a".to_owned()));
    }

    #[test]
    fn empty_is_refused() {
        assert_eq!(TimetableSet::new(Vec::new()).err(), Some(SetError::Empty));
    }

    #[test]
    fn numbers_past_every_file_belong_to_no_file() {
        let set = TimetableSet::new(vec![loaded("a", 2, 1, 1), loaded("b", 3, 1, 1)]).expect("set");
        assert!(set.stop_part(StopId::new(4)).is_some());
        assert!(set.stop_part(StopId::new(5)).is_none());
        assert!(set.route_part(RouteId::new(2)).is_none());
        assert!(set.trip_part(TripId::new(2)).is_none());
        assert!(set.part_of_feed("nope").is_none());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// Whatever files are loaded together, a number finds exactly the file it came from,
        /// and that file answers for it just as it would alone.
        #[test]
        fn numbering_round_trips_across_files(
            sizes in proptest::collection::vec((2_u32..12, 1_u32..6, 1_u32..4), 1..6),
        ) {
            let ids: Vec<String> = (0..sizes.len()).map(|i| format!("feed{i}")).collect();
            let files: Vec<LoadedTimetable> = ids
                .iter()
                .zip(&sizes)
                .map(|(id, &(stops, patterns, trips))| loaded(id, stops, patterns, trips))
                .collect();
            let alone: Vec<LoadedTimetable> = ids
                .iter()
                .zip(&sizes)
                .map(|(id, &(stops, patterns, trips))| loaded(id, stops, patterns, trips))
                .collect();
            let set = TimetableSet::new(files).expect("set");
            prop_assert_eq!(set.part_count(), sizes.len());
            let (mut stops_seen, mut patterns_seen, mut trips_seen) = (0_u32, 0_u32, 0_u32);
            for (part, alone) in set.parts().zip(&alone) {
                let compiled = part.compiled();
                let alone = alone.timetable();
                prop_assert_eq!(part.first_stop(), stops_seen);
                prop_assert_eq!(part.first_pattern(), patterns_seen);
                prop_assert_eq!(part.first_trip(), trips_seen);
                for raw in 0..u32::try_from(compiled.stop_count()).expect("fits") {
                    let local = StopId::new(raw);
                    let global = part.global_stop(local);
                    prop_assert_eq!(global.into_inner(), stops_seen + raw);
                    let (found, back) = set.stop_part(global).expect("found");
                    prop_assert_eq!(found.index(), part.index());
                    prop_assert_eq!(back, local);
                    // Dispatch reaches the same answer as the file alone.
                    prop_assert_eq!(
                        found.compiled().stop(back).map(|info| info.name),
                        alone.stop(local).map(|info| info.name)
                    );
                    prop_assert_eq!(found.compiled().stop(back).map(|info| info.feed), Some(ids[part.index()].as_str()));
                }
                for raw in 0..u32::try_from(compiled.pattern_count()).expect("fits") {
                    let local = RouteId::new(raw);
                    let global = part.global_route(local);
                    prop_assert_eq!(global.into_inner(), patterns_seen + raw);
                    let (found, back) = set.route_part(global).expect("found");
                    prop_assert_eq!(found.index(), part.index());
                    prop_assert_eq!(back, local);
                }
                for raw in 0..u32::try_from(compiled.trip_count()).expect("fits") {
                    let local = TripId::new(raw);
                    let global = part.global_trip(local);
                    prop_assert_eq!(global.into_inner(), trips_seen + raw);
                    let (found, back) = set.trip_part(global).expect("found");
                    prop_assert_eq!(found.index(), part.index());
                    prop_assert_eq!(back, local);
                    prop_assert_eq!(
                        found.compiled().trip(back).map(|info| info.gtfs_id),
                        alone.trip(local).map(|info| info.gtfs_id)
                    );
                }
                prop_assert_eq!(set.part_of_feed(&ids[part.index()]).map(|p| p.index()), Some(part.index()));
                stops_seen += u32::try_from(compiled.stop_count()).expect("fits");
                patterns_seen += u32::try_from(compiled.pattern_count()).expect("fits");
                trips_seen += u32::try_from(compiled.trip_count()).expect("fits");
            }
            prop_assert_eq!(set.stop_count(), stops_seen as usize);
            prop_assert!(set.stop_part(StopId::new(stops_seen)).is_none());
        }
    }
}
