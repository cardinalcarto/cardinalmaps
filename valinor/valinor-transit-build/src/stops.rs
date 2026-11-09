//! The stop lists of all compiled feeds, in one numbering.
//!
//! Stops are numbered consecutively, feed by feed, in the manifest's sorted feed order.
//! This is an in-memory convenience: on disk a stop is always addressed as
//! `(feed index, stop number)`, so nothing depends on the global numbering.

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};
use valinor_gtfs::CompiledTimetable;
use valinor_transit::{StopId, TimetableSource};

use crate::compile::{FeedOutcome, FeedRecord};
use crate::error::BuildError;
use crate::feeds::hex_digest;

/// Where a stop is, if its feed gives a coordinate at all.
pub type Coordinate = Option<(f64, f64)>;

/// One feed's place in the global numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedStops {
    /// The first global stop of this feed.
    pub first: u32,
    /// How many stops the feed has.
    pub count: u32,
}

/// Every stop of every feed: only what the transfer stage needs.
#[derive(Clone, Debug)]
pub struct StopList {
    /// By feed index.
    pub feeds: Vec<FeedStops>,
    /// By global stop: `(longitude, latitude)`.
    pub coordinates: Vec<Coordinate>,
    /// For a global stop with `transfers.txt` rules, the global stops they lead to.
    ///
    /// A rule needs a walk on the street network to apply to; the transfer stage counts those
    /// with none.
    pub pair_rules: HashMap<u32, Vec<u32>>,
}

impl StopList {
    /// Loads every compiled feed's stops from `compiled`, on `threads` threads.
    ///
    /// Each timetable is opened, validated, read, and dropped, so memory stays at the stop list.
    ///
    /// # Errors
    ///
    /// If a timetable marked as compiled can't be read or parsed.
    pub fn load(
        compiled: &Path,
        feeds: &[FeedRecord],
        threads: NonZeroUsize,
    ) -> Result<Self, BuildError> {
        let next = AtomicUsize::new(0);
        let mut lists: Vec<Option<FeedStopData>> = vec![None; feeds.len()];
        let lists_lock = Mutex::new(&mut lists);
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
                                if let Ok(mut lists) = lists_lock.lock() {
                                    lists[index] = Some(data);
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
        let mut list = Self {
            feeds: Vec::with_capacity(feeds.len()),
            coordinates: Vec::new(),
            pair_rules: HashMap::new(),
        };
        for data in lists {
            let (coordinates, pairs) = data.ok_or(BuildError::WorkerPanicked)?;
            let first =
                u32::try_from(list.coordinates.len()).map_err(|_| BuildError::TooManyEntries)?;
            let count = u32::try_from(coordinates.len()).map_err(|_| BuildError::TooManyEntries)?;
            list.coordinates.extend(coordinates);
            for (from, to) in pairs {
                list.pair_rules
                    .entry(first + from)
                    .or_default()
                    .push(first + to);
            }
            list.feeds.push(FeedStops { first, count });
        }
        Ok(list)
    }

    /// How many stops there are.
    #[inline]
    pub fn len(&self) -> usize {
        self.coordinates.len()
    }

    /// Whether there are no stops.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.coordinates.is_empty()
    }

    /// The `(feed index, stop number)` of a global stop.
    ///
    /// # Panics
    ///
    /// Never, in practice: an out-of-range `stop` returns the last feed's address, which a
    /// caller would have to construct by hand.
    #[inline]
    pub fn address(&self, stop: u32) -> (u32, u32) {
        let index = self.feeds.partition_point(|feed| feed.first <= stop);
        let index = index.saturating_sub(1);
        let feed = self
            .feeds
            .get(index)
            .copied()
            .unwrap_or(FeedStops { first: 0, count: 0 });
        (u32::try_from(index).unwrap_or(u32::MAX), stop - feed.first)
    }

    /// The global number of a feed's stop.
    #[inline]
    pub fn global(&self, feed: u32, stop: u32) -> Option<u32> {
        let feed = self.feeds.get(feed as usize)?;
        (stop < feed.count).then(|| feed.first + stop)
    }
}

/// One feed's stops: coordinates, and the local `(from, to)` stop pairs that have rules.
type FeedStopData = (Vec<Coordinate>, Vec<(u32, u32)>);

fn load_feed(compiled: &Path, feed: &FeedRecord) -> Result<FeedStopData, BuildError> {
    if feed.outcome != FeedOutcome::Compiled {
        return Ok((Vec::new(), Vec::new()));
    }
    let Some(relative) = &feed.timetable else {
        return Ok((Vec::new(), Vec::new()));
    };
    let path = compiled.join(relative);
    let bytes = fs::read(&path).map_err(|source| BuildError::Read { path, source })?;
    if feed.timetable_sha256.as_deref() != Some(hex_digest(&Sha256::digest(&bytes)).as_str()) {
        return Err(BuildError::TimetableChanged {
            feed: feed.id.clone(),
        });
    }
    let timetable = CompiledTimetable::parse(&bytes).map_err(|source| BuildError::Timetable {
        feed: feed.id.clone(),
        source: source.into(),
    })?;
    let count = timetable.stop_count();
    let mut coordinates = Vec::with_capacity(count);
    for stop in 0..count {
        let stop = u32::try_from(stop).map_err(|_| BuildError::TooManyEntries)?;
        coordinates.push(
            timetable
                .stop(StopId::new(stop))
                .and_then(|info| info.coordinate)
                .map(|coordinate| (coordinate.lon, coordinate.lat)),
        );
    }
    let pairs = TimetableSource::transfer_pairs(&timetable)
        .into_iter()
        .map(|pair| (pair.from.into_inner(), pair.to.into_inner()))
        .collect();
    Ok((coordinates, pairs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_skip_empty_feeds() {
        let list = StopList {
            feeds: vec![
                FeedStops { first: 0, count: 2 },
                FeedStops { first: 2, count: 0 },
                FeedStops { first: 2, count: 3 },
            ],
            coordinates: vec![None; 5],
            pair_rules: HashMap::new(),
        };
        assert_eq!(list.address(0), (0, 0));
        assert_eq!(list.address(1), (0, 1));
        assert_eq!(list.address(2), (2, 0));
        assert_eq!(list.address(4), (2, 2));
        assert_eq!(list.global(2, 1), Some(3));
        assert_eq!(list.global(1, 0), None);
        assert_eq!(list.global(0, 2), None);
    }
}
