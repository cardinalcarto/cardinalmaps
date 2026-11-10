//! A [`TransferTable`](crate::TransferTable) as one flat file, memory-mapped when loaded.
//!
//! Tiles of precomputed walks are for the network. A client unpacks the tiles it has into one
//! file per travel mode, holding offsets and walks in both directions in the client's own stop
//! numbering, and maps that file: its bytes are ordinary file-backed pages,
//! so a table costs almost no anonymous memory, and a mode nobody asks about costs nothing.
//!
//! # Layout
//!
//! A header, then for the outgoing walks and then the incoming walks:
//! `offsets` (`stop_count + 1` little-endian `u32`s), `stops` (`u32`s), and `seconds` (`u16`s).
//! Records have alignment one, so the sections are cast directly over the mapped bytes.
//!
//! Opening validates the header, the file's exact length, and that each offset array is
//! increasing and ends at its walk count. Individual walks are not read, since that would page
//! in the whole file; every read goes through bounds-checked slices, so a stop past the table
//! is simply a miss. A corrupt walk can make a transfer wrong, but it can't cause a panic.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use memmap2::Mmap;
use thiserror::Error;
use zerocopy::little_endian::{U16, U32};
use zerocopy::{FromBytes as _, IntoBytes as _};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The first bytes of a flat table.
pub const FLAT_TABLE_MAGIC: [u8; 8] = *b"VTFLAT01";

/// A key that says what a flat table was assembled from, so a stale one is noticed.
pub type TableKey = [u8; 32];

#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct Header {
    magic: [u8; 8],
    key: TableKey,
    max_seconds: U32,
    stop_count: U32,
    out_walks: U32,
    into_walks: U32,
}

const HEADER_LEN: usize = size_of::<Header>();

/// Why a flat table couldn't be opened.
#[derive(Debug, Error)]
pub enum FlatTableError {
    /// The file couldn't be read.
    #[error("read a flat transfer table: {0}")]
    Io(#[from] io::Error),
    /// The file isn't a flat table of this version, or its contents don't fit its header.
    #[error("read a flat transfer table: {0}")]
    Invalid(&'static str),
    /// The file is for other tiles, feeds, or a bound.
    #[error("use a flat transfer table with a different key")]
    KeyMismatch,
}

/// One direction of a table, as borrowed columns.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Columns<'a> {
    pub(crate) offsets: &'a [U32],
    pub(crate) stops: &'a [U32],
    pub(crate) seconds: &'a [U16],
}

impl<'a> Columns<'a> {
    /// The walks of a stop, as parallel stop and second columns.
    ///
    /// A stop past the table has no walks.
    #[inline]
    pub(crate) fn of(self, stop: usize) -> (&'a [U32], &'a [U16]) {
        let (Some(start), Some(end)) = (self.offsets.get(stop), self.offsets.get(stop + 1)) else {
            return (&[], &[]);
        };
        let range = start.get() as usize..end.get() as usize;
        (
            self.stops.get(range.clone()).unwrap_or(&[]),
            self.seconds.get(range).unwrap_or(&[]),
        )
    }
}

/// The byte ranges of one direction's columns in the file.
#[derive(Clone, Copy, Debug)]
struct ColumnRanges {
    offsets: (usize, usize),
    stops: (usize, usize),
    seconds: (usize, usize),
}

/// A flat table mapped into memory.
#[derive(Debug)]
pub(crate) struct MappedTable {
    map: Mmap,
    max_seconds: u32,
    stop_count: usize,
    out: ColumnRanges,
    into: ColumnRanges,
}

impl MappedTable {
    pub(crate) fn max_seconds(&self) -> u32 {
        self.max_seconds
    }

    pub(crate) fn stop_count(&self) -> usize {
        self.stop_count
    }

    pub(crate) fn outgoing(&self) -> Columns<'_> {
        self.columns(self.out)
    }

    pub(crate) fn incoming(&self) -> Columns<'_> {
        self.columns(self.into)
    }

    fn columns(&self, ranges: ColumnRanges) -> Columns<'_> {
        let bytes: &[u8] = &self.map;
        let section = |(start, end): (usize, usize)| bytes.get(start..end).unwrap_or(&[]);
        // The ranges come from a validated header, so the casts succeed; a failure reads as empty.
        Columns {
            offsets: <[U32]>::ref_from_bytes(section(ranges.offsets)).unwrap_or(&[]),
            stops: <[U32]>::ref_from_bytes(section(ranges.stops)).unwrap_or(&[]),
            seconds: <[U16]>::ref_from_bytes(section(ranges.seconds)).unwrap_or(&[]),
        }
    }
}

/// Maps a flat table and validates what can be validated without reading its walks.
///
/// `key` must match what the file was written with.
///
/// # Safety
///
/// The file must not be modified while it is mapped; replace it by renaming over it.
///
/// # Errors
///
/// If the file can't be read, is the wrong length, has the wrong key, or its offsets don't fit.
pub(crate) unsafe fn map_flat(path: &Path, key: &TableKey) -> Result<MappedTable, FlatTableError> {
    let file = File::open(path)?;
    // SAFETY: passed on to the caller's promise.
    unsafe { map_flat_file(&file, key) }
}

/// As [`map_flat`], for a file already open for reading.
///
/// The mapping is of the open file, not of whatever its path names later,
/// so it is unaffected by the path being renamed over or unlinked.
///
/// # Safety
///
/// As [`map_flat`].
///
/// # Errors
///
/// As [`map_flat`].
pub(crate) unsafe fn map_flat_file(
    file: &File,
    key: &TableKey,
) -> Result<MappedTable, FlatTableError> {
    // SAFETY: the caller promises the file doesn't change underneath this mapping.
    let map = unsafe { Mmap::map(file) }?;
    let (header, _) = Header::ref_from_prefix(&map)
        .map_err(|_| FlatTableError::Invalid("the file is shorter than its header"))?;
    if header.magic != FLAT_TABLE_MAGIC {
        return Err(FlatTableError::Invalid("the magic bytes are wrong"));
    }
    if &header.key != key {
        return Err(FlatTableError::KeyMismatch);
    }
    let stop_count = header.stop_count.get() as usize;
    let (mut at, out) = lay_out(HEADER_LEN, stop_count, header.out_walks.get() as usize)?;
    let (end, into) = lay_out(at, stop_count, header.into_walks.get() as usize)?;
    at = end;
    if at != map.len() {
        return Err(FlatTableError::Invalid(
            "the file's length doesn't match its header",
        ));
    }
    let table = MappedTable {
        max_seconds: header.max_seconds.get(),
        stop_count,
        out,
        into,
        map,
    };
    check_offsets(table.outgoing(), stop_count, "outgoing")?;
    check_offsets(table.incoming(), stop_count, "incoming")?;
    Ok(table)
}

fn lay_out(
    start: usize,
    stop_count: usize,
    walks: usize,
) -> Result<(usize, ColumnRanges), FlatTableError> {
    let bad = || FlatTableError::Invalid("a section's size overflows");
    let offsets_end = stop_count
        .checked_add(1)
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| n.checked_add(start))
        .ok_or_else(bad)?;
    let stops_end = walks
        .checked_mul(4)
        .and_then(|n| n.checked_add(offsets_end))
        .ok_or_else(bad)?;
    let seconds_end = walks
        .checked_mul(2)
        .and_then(|n| n.checked_add(stops_end))
        .ok_or_else(bad)?;
    Ok((
        seconds_end,
        ColumnRanges {
            offsets: (start, offsets_end),
            stops: (offsets_end, stops_end),
            seconds: (stops_end, seconds_end),
        },
    ))
}

fn check_offsets(
    columns: Columns<'_>,
    stop_count: usize,
    which: &'static str,
) -> Result<(), FlatTableError> {
    if columns.offsets.len() != stop_count + 1 {
        return Err(FlatTableError::Invalid(which));
    }
    let walks = columns.stops.len() as u64;
    let mut previous = 0_u64;
    for (index, offset) in columns.offsets.iter().enumerate() {
        let value = u64::from(offset.get());
        let first = index == 0 && value != 0;
        let last = index == stop_count && value != walks;
        if first || last || value < previous || value > walks {
            return Err(FlatTableError::Invalid("an offset array isn't monotonic"));
        }
        previous = value;
    }
    Ok(())
}

/// Writes the header and columns of a table.
///
/// # Errors
///
/// If the writer fails.
pub(crate) fn write_flat(
    writer: &mut impl Write,
    key: &TableKey,
    max_seconds: u32,
    stop_count: u32,
    out: Columns<'_>,
    into: Columns<'_>,
) -> io::Result<()> {
    let count = |slice: &[U32]| u32::try_from(slice.len()).map_err(|_| io::ErrorKind::InvalidInput);
    let header = Header {
        magic: FLAT_TABLE_MAGIC,
        key: *key,
        max_seconds: U32::new(max_seconds),
        stop_count: U32::new(stop_count),
        out_walks: U32::new(count(out.stops)?),
        into_walks: U32::new(count(into.stops)?),
    };
    writer.write_all(header.as_bytes())?;
    for columns in [out, into] {
        writer.write_all(columns.offsets.as_bytes())?;
        writer.write_all(columns.stops.as_bytes())?;
        writer.write_all(columns.seconds.as_bytes())?;
    }
    Ok(())
}
