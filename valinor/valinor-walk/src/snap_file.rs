//! Stop snaps saved by the offline build, one file per feed and travel mode.
//!
//! Snapping every stop takes seconds and pages in much of the street network, so the build does
//! it once and a client reads the results: a small header, then one fixed-size record per stop
//! in the feed's own stop numbering. Reading validates the header, each record, and that the
//! length is exact, so any bytes give an error rather than a panic. A snap is bound to the
//! street build it was made on, like a [`crate::StopSnaps`] table.

use thiserror::Error;
use valhalla_graphtile::GraphId;
use valhalla_graphtile::graph_tile::BuildId;
use zerocopy::little_endian::{F32, F64, U16, U32, U64};
use zerocopy::{FromBytes as _, IntoBytes as _};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

use crate::snap::{SnapFailure, SnapPoint, SnapQuality, SnappedStop};

/// The first bytes of a stop snap file.
const SNAP_FILE_MAGIC: [u8; 8] = *b"VTSNAP01";

const FLAG_SNAPPED: u8 = 1;
const FLAG_DISCONNECTED: u8 = 2;

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct SnapHeader {
    magic: [u8; 8],
    build_id: U16,
    stop_count: U32,
}

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct SnapRecord {
    edge: U64,
    percent_along: F32,
    distance_m: F32,
    longitude: F64,
    latitude: F64,
    flags: u8,
}

const HEADER_LEN: usize = size_of::<SnapHeader>();
const RECORD_LEN: usize = size_of::<SnapRecord>();

/// Why a stop snap file couldn't be decoded.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SnapFileError {
    /// The file doesn't have the right magic bytes or length.
    #[error("read a stop snap file: {0}")]
    Invalid(&'static str),
    /// The file was made on a different street build.
    #[error("use stop snaps of street build {found} with street build {expected}")]
    WrongBuild {
        /// The build in the file.
        found: u16,
        /// The build wanted.
        expected: u16,
    },
    /// The file covers a different number of stops than the feed has.
    #[error("use stop snaps of {found} stops for a feed of {expected}")]
    WrongStopCount {
        /// The count in the file.
        found: u32,
        /// The count wanted.
        expected: u32,
    },
    /// A record is out of range.
    #[error("read a stop snap: {0}")]
    InvalidRecord(&'static str),
}

/// One stop's saved snap: `None` if it had no usable street, else where and how good.
pub type SavedSnap = Option<(SnapPoint, SnapQuality)>;

/// Encodes one feed's stop snaps for one travel mode.
#[must_use]
pub fn encode_snap_file(build_id: BuildId, stops: &[SavedSnap]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + stops.len() * RECORD_LEN);
    let header = SnapHeader {
        magic: SNAP_FILE_MAGIC,
        build_id: U16::new(build_id.value()),
        stop_count: U32::new(u32::try_from(stops.len()).unwrap_or(u32::MAX)),
    };
    out.extend_from_slice(header.as_bytes());
    for stop in stops {
        let record = match stop {
            Some((point, quality)) => SnapRecord {
                edge: U64::new(point.edge.value()),
                percent_along: F32::new(point.percent_along),
                distance_m: F32::new(point.distance_m),
                longitude: F64::new(point.point.0),
                latitude: F64::new(point.point.1),
                flags: FLAG_SNAPPED
                    | match quality {
                        SnapQuality::Connected => 0,
                        SnapQuality::Disconnected => FLAG_DISCONNECTED,
                    },
            },
            None => SnapRecord {
                edge: U64::new(0),
                percent_along: F32::new(0.0),
                distance_m: F32::new(0.0),
                longitude: F64::new(0.0),
                latitude: F64::new(0.0),
                flags: 0,
            },
        };
        out.extend_from_slice(record.as_bytes());
    }
    out
}

/// Decodes a stop snap file into one [`SnappedStop`] per stop, in the feed's stop numbering.
///
/// # Errors
///
/// If the bytes aren't a stop snap file for `expected_build` and `expected_stops` stops,
/// or a record isn't a valid snap.
pub fn decode_snap_file(
    bytes: &[u8],
    expected_build: BuildId,
    expected_stops: u32,
) -> Result<Vec<SnappedStop>, SnapFileError> {
    let (header, rest) = SnapHeader::ref_from_prefix(bytes)
        .map_err(|_| SnapFileError::Invalid("the file is shorter than its header"))?;
    if header.magic != SNAP_FILE_MAGIC {
        return Err(SnapFileError::Invalid("the magic bytes are wrong"));
    }
    if header.build_id.get() != expected_build.value() {
        return Err(SnapFileError::WrongBuild {
            found: header.build_id.get(),
            expected: expected_build.value(),
        });
    }
    if header.stop_count.get() != expected_stops {
        return Err(SnapFileError::WrongStopCount {
            found: header.stop_count.get(),
            expected: expected_stops,
        });
    }
    let records = <[SnapRecord]>::ref_from_bytes(rest)
        .map_err(|_| SnapFileError::Invalid("the file isn't a whole number of records"))?;
    if records.len() != expected_stops as usize {
        return Err(SnapFileError::Invalid(
            "the file's length doesn't match its header",
        ));
    }
    records
        .iter()
        .map(|record| snapped_of(record, expected_build))
        .collect()
}

fn snapped_of(record: &SnapRecord, build: BuildId) -> Result<SnappedStop, SnapFileError> {
    if record.flags & FLAG_SNAPPED == 0 {
        return Ok(SnappedStop {
            result: Err(SnapFailure::NothingNearby),
            quality: SnapQuality::Disconnected,
            build_id: None,
        });
    }
    let edge = GraphId::try_from_id(record.edge.get())
        .map_err(|_| SnapFileError::InvalidRecord("an edge ID is invalid"))?;
    let percent_along = record.percent_along.get();
    let distance_m = record.distance_m.get();
    let longitude = record.longitude.get();
    let latitude = record.latitude.get();
    let in_range = (0.0..=1.0).contains(&percent_along)
        && distance_m.is_finite()
        && distance_m >= 0.0
        && longitude.is_finite()
        && latitude.is_finite();
    if !in_range {
        return Err(SnapFileError::InvalidRecord("a value is out of range"));
    }
    let quality = if record.flags & FLAG_DISCONNECTED == 0 {
        SnapQuality::Connected
    } else {
        SnapQuality::Disconnected
    };
    Ok(SnappedStop {
        result: Ok(SnapPoint {
            edge,
            percent_along,
            distance_m,
            point: (longitude, latitude),
        }),
        quality,
        build_id: Some(build),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn edge() -> GraphId {
        GraphId::try_from_id(0x2a | (1 << 25)).expect("valid")
    }

    fn sample() -> Vec<SavedSnap> {
        vec![
            Some((
                SnapPoint {
                    edge: edge(),
                    percent_along: 0.25,
                    distance_m: 3.5,
                    point: (-122.3, 47.6),
                },
                SnapQuality::Connected,
            )),
            None,
            Some((
                SnapPoint {
                    edge: edge(),
                    percent_along: 1.0,
                    distance_m: 0.0,
                    point: (1.5, -2.5),
                },
                SnapQuality::Disconnected,
            )),
        ]
    }

    #[test]
    fn round_trips_every_field() {
        let stops = sample();
        let bytes = encode_snap_file(BuildId::new(4060), &stops);
        let decoded = decode_snap_file(&bytes, BuildId::new(4060), 3).expect("decodes");
        for (saved, snapped) in stops.iter().zip(&decoded) {
            if let Some((point, quality)) = saved {
                assert_eq!(snapped.result, Ok(*point));
                assert_eq!(snapped.quality, *quality);
                assert_eq!(snapped.build_id, Some(BuildId::new(4060)));
            } else {
                assert_eq!(snapped.result, Err(SnapFailure::NothingNearby));
                assert_eq!(snapped.build_id, None);
            }
        }
    }

    #[test]
    fn a_file_of_another_build_or_feed_is_refused() {
        let bytes = encode_snap_file(BuildId::new(1), &sample());
        assert_eq!(
            decode_snap_file(&bytes, BuildId::new(2), 3),
            Err(SnapFileError::WrongBuild {
                found: 1,
                expected: 2
            })
        );
        assert_eq!(
            decode_snap_file(&bytes, BuildId::new(1), 4),
            Err(SnapFileError::WrongStopCount {
                found: 3,
                expected: 4
            })
        );
    }

    proptest! {
        /// No bytes make decoding panic, and damage to a valid file is either refused or valid.
        #[test]
        fn any_bytes_never_panic(mut bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            drop(decode_snap_file(&bytes, BuildId::new(4060), 3));
            let mut valid = encode_snap_file(BuildId::new(4060), &sample());
            valid.truncate(bytes.len().min(valid.len()));
            bytes.truncate(valid.len());
            valid.iter_mut().zip(&bytes).for_each(|(v, b)| *v ^= *b);
            drop(decode_snap_file(&valid, BuildId::new(4060), 3));
        }
    }
}
