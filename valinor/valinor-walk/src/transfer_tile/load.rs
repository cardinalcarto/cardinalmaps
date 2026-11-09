//! Assembling a [`TransferTable`] from the transfer tiles a client downloaded.
//!
//! A client downloads the tiles the build manifest lists for each of its feeds.
//! Walks to or from feeds it doesn't have are ignored: they exist in the tile
//! only for the riders of other feed sets.

use thiserror::Error;

use super::format::{TransferMode, TransferTile};
use crate::error::WalkError;
use crate::transfers::{Hop, TransferTable};

/// Where one client feed's compiled stops land in the client's stop numbering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopPlacement {
    /// Compiled stop `n` is local stop `first + n`, for `count` stops.
    Contiguous {
        /// The local number of the feed's first compiled stop.
        first: u32,
        /// How many stops the feed has.
        count: u32,
    },
    /// Compiled stop `n` is local stop `locals[n]`.
    Explicit(Vec<u32>),
}

impl StopPlacement {
    fn count(&self) -> u32 {
        match self {
            Self::Contiguous { count, .. } => *count,
            Self::Explicit(locals) => u32::try_from(locals.len()).unwrap_or(u32::MAX),
        }
    }

    fn local(&self, stop: u32) -> Option<u32> {
        match self {
            Self::Contiguous { first, count } if stop < *count => first.checked_add(stop),
            Self::Contiguous { .. } => None,
            Self::Explicit(locals) => locals.get(stop as usize).copied(),
        }
    }
}

/// One feed the client has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedPlacement {
    /// The feed's index in the build manifest.
    pub feed_index: u32,
    /// Where its stops are locally.
    pub stops: StopPlacement,
}

/// What to assemble.
#[derive(Clone, Copy, Debug)]
pub struct LoadRequest<'a> {
    /// The tiles' mode.
    pub mode: TransferMode,
    /// The manifest fingerprint the tiles must come from.
    pub manifest_id: u64,
    /// The bound every tile must have been built with.
    pub max_seconds: u32,
    /// How many local stops the assembled table covers.
    pub local_stop_count: u32,
    /// The feeds the client has.
    pub feeds: &'a [FeedPlacement],
}

/// What [`assemble_transfer_table`] kept and left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Tiles read.
    pub tiles: usize,
    /// Walks now in the table.
    pub walks: usize,
    /// Walks left out because the origin's feed isn't on this client.
    pub walks_from_absent_feeds: usize,
    /// Walks left out because the destination's feed isn't on this client.
    pub walks_to_absent_feeds: usize,
}

/// Why a set of tiles could not be assembled.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TileLoadError {
    /// A feed placement falls outside the local numbering.
    #[error("place feed {feed} at local stop {stop}, past the {stops} local stops")]
    PlacementOutOfRange {
        /// The feed's index.
        feed: u32,
        /// The local stop.
        stop: u32,
        /// How many local stops there are.
        stops: u32,
    },
    /// Two feed placements claim one local stop, or one feed appears twice.
    #[error("place feed {feed} on a local stop or index already in use")]
    PlacementOverlap {
        /// The feed's index.
        feed: u32,
    },
    /// A tile's header doesn't match what was asked for.
    #[error("load a tile from another build (mode, manifest, or bound differs)")]
    HeaderMismatch,
    /// A tile names a stop past what its feed has on this client.
    #[error("resolve stop {stop} of feed {feed}, which has {count} stops here")]
    StopOutOfRange {
        /// The feed's index.
        feed: u32,
        /// The stop.
        stop: u32,
        /// How many stops the feed has.
        count: u32,
    },
    /// The same walk was given twice, as from a tile loaded twice.
    #[error("load a duplicate walk: {0}")]
    Duplicate(String),
}

/// Assembles the walks of `tiles` into a table in the client's numbering.
///
/// Walks whose origin or destination isn't in `request.feeds` are left out and counted.
///
/// # Errors
///
/// Fails if a placement is inconsistent, a tile's header doesn't match, a tile names a stop past a
/// feed's stop count, or a walk appears twice.
///
/// # Performance
///
/// One sort of the kept walks, plus a binary search of `request.feeds` for each end of each walk.
pub fn assemble_transfer_table<'a>(
    request: &LoadRequest<'_>,
    tiles: impl IntoIterator<Item = &'a TransferTile>,
) -> Result<(TransferTable, LoadReport), TileLoadError> {
    let feeds = sorted_placements(request)?;
    let mut report = LoadReport::default();
    let mut walks: Vec<(u32, Hop)> = Vec::new();
    for tile in tiles {
        if tile.mode != request.mode
            || tile.manifest_id != request.manifest_id
            || tile.max_seconds != request.max_seconds
        {
            return Err(TileLoadError::HeaderMismatch);
        }
        report.tiles += 1;
        for (origin_feed, origin_stop, row) in tile.rows() {
            let Some(origin_placement) = placement_of(&feeds, origin_feed) else {
                report.walks_from_absent_feeds += row.len();
                continue;
            };
            let origin = resolve(origin_placement, origin_feed, origin_stop)?;
            for walk in row.iter() {
                let Some(destination_placement) = placement_of(&feeds, walk.feed) else {
                    report.walks_to_absent_feeds += 1;
                    continue;
                };
                let destination = resolve(destination_placement, walk.feed, walk.stop)?;
                walks.push((
                    origin,
                    Hop {
                        stop: destination,
                        seconds: walk.seconds,
                    },
                ));
            }
        }
    }
    report.walks = walks.len();
    walks.sort_unstable_by_key(|&(from, hop)| (from, hop.stop));
    let table = TransferTable::from_sorted_walks(
        request.local_stop_count as usize,
        request.max_seconds,
        &walks,
    )
    .map_err(|e: WalkError| TileLoadError::Duplicate(e.to_string()))?;
    Ok((table, report))
}

fn placement_of<'a>(feeds: &[&'a FeedPlacement], feed: u32) -> Option<&'a StopPlacement> {
    feeds
        .binary_search_by_key(&feed, |placement| placement.feed_index)
        .ok()
        .map(|at| &feeds[at].stops)
}

fn resolve(placement: &StopPlacement, feed: u32, stop: u32) -> Result<u32, TileLoadError> {
    placement
        .local(stop)
        .ok_or_else(|| TileLoadError::StopOutOfRange {
            feed,
            stop,
            count: placement.count(),
        })
}

/// Validates placements and sorts them by feed index.
fn sorted_placements<'a>(
    request: &LoadRequest<'a>,
) -> Result<Vec<&'a FeedPlacement>, TileLoadError> {
    let mut feeds: Vec<&FeedPlacement> = request.feeds.iter().collect();
    feeds.sort_unstable_by_key(|feed| feed.feed_index);
    let local_count = request.local_stop_count as usize;
    let mut used = vec![false; local_count];
    for (index, feed) in feeds.iter().enumerate() {
        if index > 0 && feeds[index - 1].feed_index == feed.feed_index {
            return Err(TileLoadError::PlacementOverlap {
                feed: feed.feed_index,
            });
        }
        let stops = local_stops(&feed.stops);
        for local in stops {
            let Some(flag) = used.get_mut(local as usize) else {
                return Err(TileLoadError::PlacementOutOfRange {
                    feed: feed.feed_index,
                    stop: local,
                    stops: request.local_stop_count,
                });
            };
            if core::mem::replace(flag, true) {
                return Err(TileLoadError::PlacementOverlap {
                    feed: feed.feed_index,
                });
            }
        }
    }
    Ok(feeds)
}

fn local_stops(placement: &StopPlacement) -> Box<dyn Iterator<Item = u32> + '_> {
    match placement {
        StopPlacement::Contiguous { first, count } => {
            Box::new((0..*count).map_while(move |offset| first.checked_add(offset)))
        }
        StopPlacement::Explicit(locals) => Box::new(locals.iter().copied()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::{TileWalk, TransferTileRow};
    use super::*;
    use valinor_transit::StopId;

    const MANIFEST: u64 = 0x1234;

    fn tile(rows: &[(u32, u32, Vec<TileWalk>)]) -> TransferTile {
        let rows: Vec<TransferTileRow<'_>> = rows
            .iter()
            .map(|(feed, stop, walks)| TransferTileRow {
                feed: *feed,
                stop: *stop,
                walks,
            })
            .collect();
        TransferTile::from_rows(TransferMode::Walking, (0, 0), MANIFEST, 900, &rows).expect("valid")
    }

    fn walk(feed: u32, stop: u32, seconds: u32) -> TileWalk {
        TileWalk {
            feed,
            stop,
            seconds,
        }
    }

    fn request(feeds: &[FeedPlacement], local_stop_count: u32) -> LoadRequest<'_> {
        LoadRequest {
            mode: TransferMode::Walking,
            manifest_id: MANIFEST,
            max_seconds: 900,
            local_stop_count,
            feeds,
        }
    }

    fn contiguous(feed_index: u32, first: u32, count: u32) -> FeedPlacement {
        FeedPlacement {
            feed_index,
            stops: StopPlacement::Contiguous { first, count },
        }
    }

    #[test]
    fn walks_across_feeds_land_in_local_numbering() {
        let feeds = [contiguous(0, 2, 3), contiguous(5, 0, 2)];
        let tiles = [
            tile(&[(0, 1, vec![walk(0, 2, 30), walk(5, 1, 90)])]),
            tile(&[(5, 0, vec![walk(0, 0, 60)])]),
        ];
        let (table, report) =
            assemble_transfer_table(&request(&feeds, 5), &tiles).expect("assemble");
        assert_eq!(report.tiles, 2);
        assert_eq!(report.walks, 3);
        assert_eq!(table.stop_count(), 5);
        assert_eq!(table.max_seconds(), 900);
        // Feed 0's stops are 2.. locally, feed 5's are 0..2.
        assert_eq!(table.seconds(StopId::new(3), StopId::new(4)), Some(30));
        assert_eq!(table.seconds(StopId::new(3), StopId::new(1)), Some(90));
        assert_eq!(table.seconds(StopId::new(0), StopId::new(2)), Some(60));
        let forward: Vec<_> = table
            .walks_from(StopId::new(3))
            .map(|(stop, secs)| (stop.into_inner(), secs))
            .collect();
        assert_eq!(forward, vec![(1, 90), (4, 30)]);
    }

    #[test]
    fn walks_to_a_feed_the_client_dropped_are_ignored() {
        let feeds = [contiguous(2, 0, 2)];
        let tiles = [tile(&[
            (2, 0, vec![walk(2, 1, 40), walk(7, 0, 200)]),
            (7, 3, vec![walk(2, 1, 60), walk(7, 4, 5)]),
        ])];
        let (table, report) =
            assemble_transfer_table(&request(&feeds, 2), &tiles).expect("assemble");
        assert_eq!(report.walks, 1);
        assert_eq!(report.walks_to_absent_feeds, 1);
        assert_eq!(report.walks_from_absent_feeds, 2);
        assert_eq!(table.walk_count(), 1);
        assert_eq!(table.seconds(StopId::new(0), StopId::new(1)), Some(40));
    }

    #[test]
    fn explicit_placement_reorders_stops() {
        let feeds = [FeedPlacement {
            feed_index: 0,
            stops: StopPlacement::Explicit(vec![2, 0, 1]),
        }];
        let tiles = [tile(&[(0, 0, vec![walk(0, 1, 10), walk(0, 2, 20)])])];
        let (table, _) = assemble_transfer_table(&request(&feeds, 3), &tiles).expect("assemble");
        // Compiled 0 is local 2; compiled 1 is local 0; compiled 2 is local 1.
        assert_eq!(table.seconds(StopId::new(2), StopId::new(0)), Some(10));
        assert_eq!(table.seconds(StopId::new(2), StopId::new(1)), Some(20));
    }

    #[test]
    fn a_stop_past_its_feed_is_an_error() {
        let feeds = [contiguous(0, 0, 3)];
        let tiles = [tile(&[(0, 0, vec![walk(0, 5, 10)])])];
        let error = assemble_transfer_table(&request(&feeds, 3), &tiles).expect_err("range");
        assert!(matches!(
            error,
            TileLoadError::StopOutOfRange { stop: 5, .. }
        ));
    }

    #[test]
    fn inconsistent_placements_are_refused() {
        let overlap = [contiguous(0, 0, 3), contiguous(1, 2, 2)];
        assert_eq!(
            assemble_transfer_table(&request(&overlap, 10), &[]).expect_err("overlap"),
            TileLoadError::PlacementOverlap { feed: 1 }
        );
        let beyond = [contiguous(0, 8, 3)];
        assert!(matches!(
            assemble_transfer_table(&request(&beyond, 10), &[]).expect_err("beyond"),
            TileLoadError::PlacementOutOfRange { stop: 10, .. }
        ));
        let duplicate = [contiguous(0, 0, 1), contiguous(0, 1, 1)];
        assert_eq!(
            assemble_transfer_table(&request(&duplicate, 10), &[]).expect_err("duplicate"),
            TileLoadError::PlacementOverlap { feed: 0 }
        );
    }

    #[test]
    fn tiles_from_another_build_are_refused() {
        let feeds = [contiguous(0, 0, 2)];
        let mut other = tile(&[(0, 0, vec![walk(0, 1, 10)])]);
        other.manifest_id = MANIFEST + 1;
        assert_eq!(
            assemble_transfer_table(&request(&feeds, 2), core::slice::from_ref(&other))
                .expect_err("manifest"),
            TileLoadError::HeaderMismatch
        );
    }

    #[test]
    fn the_same_tile_twice_is_refused() {
        let feeds = [contiguous(0, 0, 2)];
        let once = tile(&[(0, 0, vec![walk(0, 1, 10)])]);
        let tiles = [once.clone(), once];
        assert!(matches!(
            assemble_transfer_table(&request(&feeds, 2), &tiles).expect_err("duplicate"),
            TileLoadError::Duplicate(_)
        ));
    }

    #[test]
    fn walks_land_sorted_with_no_tiles_given() {
        let feeds = [contiguous(0, 0, 2)];
        let (table, report) =
            assemble_transfer_table(&request(&feeds, 2), core::iter::empty()).expect("empty");
        assert_eq!(table.walk_count(), 0);
        assert_eq!(report.tiles, 0);
    }
}
