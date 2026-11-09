//! The bytes of a transfer tile.
//!
//! A tile is a zstd frame around a fixed little-endian header and six columns of
//! LEB128 variable-length integers:
//!
//! | Field | Width | Meaning |
//! |---|---|---|
//! | magic | 4 | [`TILE_MAGIC`] |
//! | version | 2 | [`TILE_FORMAT_VERSION`] |
//! | mode | 1 | [`TransferMode`] |
//! | zoom | 1 | [`TILE_ZOOM`] |
//! | x, y | 4 each | The tile |
//! | manifest ID | 8 | Fingerprint of the build manifest's feed list |
//! | max seconds | 4 | The bound the tile was built with |
//! | origins | 4 | Walk origins in the tile |
//! | walks | 4 | Walks in the tile |
//! | checksum | 4 | CRC-32 of everything else, with this field as zero |
//!
//! The columns are, in order: the origins' feeds, the origins' stops, the walk counts of each
//! origin, the destinations' feeds, the destinations' stops, and the walks' seconds.
//! Sorted columns hold deltas, which is what makes them compress well:
//! a feed column holds the difference from the previous value, and a stop column holds the
//! difference from the previous stop of the same feed or the absolute value when the feed changed.
//! Each column restarts its deltas at a new origin's row, and walk rows are sorted by
//! `(feed, stop)`. Nothing is implied by what's left out: the file records exactly what is read.
//!
//! Any change to the layout gets a new version.

use std::io::Read;

use crc32fast::Hasher;
use ruzstd::decoding::StreamingDecoder;
use thiserror::Error;

/// The first bytes of a decompressed tile.
pub const TILE_MAGIC: [u8; 4] = *b"VTFR";
/// The only format version this build reads and writes.
pub const TILE_FORMAT_VERSION: u16 = 1;
/// The zoom every tile is at (the standard slippy-map scheme).
pub const TILE_ZOOM: u8 = 12;

/// The most tiles there are on one axis at the tile zoom.
const TILES_PER_AXIS: u32 = 1 << TILE_ZOOM;

/// The fixed part of a decompressed tile.
const HEADER_LEN: usize = 40;

/// How much of a tile to take from the zstd decoder at a time.
const READ_CHUNK: usize = 64 * 1024;

/// How large a decompressed tile may be, by default.
///
/// The largest real tiles measured are far below this (see the build stage's report);
/// a device with tighter memory should pass a smaller limit in [`DecodeLimits`].
pub const DEFAULT_MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;

/// The largest decompressed tile [`decode_tile`] will accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Decompression past this many bytes is an error, which keeps a zstd bomb from growing.
    pub max_decompressed_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
        }
    }
}

/// The travel mode of a transfer tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TransferMode {
    /// On foot.
    Walking,
    /// On a bicycle.
    Cycling,
}

impl TransferMode {
    /// Every mode, in the order they are built.
    pub const ALL: [Self; 2] = [Self::Walking, Self::Cycling];

    /// The name used in paths and manifests.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Walking => "walking",
            Self::Cycling => "cycling",
        }
    }

    const fn byte(self) -> u8 {
        match self {
            Self::Walking => 0,
            Self::Cycling => 1,
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Walking),
            1 => Some(Self::Cycling),
            _ => None,
        }
    }
}

/// One walk of a tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileWalk {
    /// The feed the walk ends in.
    pub feed: u32,
    /// The stop it ends at, in that feed's compiled numbering.
    pub stop: u32,
    /// Its whole seconds.
    pub seconds: u32,
}

/// The walks from one origin stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferTileRow<'a> {
    /// The origin's feed.
    pub feed: u32,
    /// The origin's stop, in that feed's compiled numbering.
    pub stop: u32,
    /// The origin's walks, sorted by `(feed, stop)`.
    pub walks: &'a [TileWalk],
}

/// Why a tile could not be encoded: the caller gave it rows a decoder would refuse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TileEncodeError {
    /// The rows break an invariant of the format.
    #[error("encode a tile whose rows break the format: {0}")]
    Invalid(#[from] TileDecodeError),
    /// The tile doesn't fit the format's 32-bit counts.
    #[error("encode a tile with more than 2^32 origins or walks")]
    TooLarge,
}

/// Why a tile could not be decoded. Decoding never panics; every failure is one of these.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TileDecodeError {
    /// Decompressing the zstd frame failed.
    #[error("decompress a transfer tile: {0}")]
    Decompress(String),
    /// The frame decompresses to more than the limit allows.
    #[error("decompress a tile over the limit of {limit} bytes")]
    TooLarge {
        /// The limit in force.
        limit: usize,
    },
    /// Bytes follow the zstd frame.
    #[error("read a tile with trailing bytes after its frame")]
    TrailingFrameData,
    /// The bytes are shorter than a header.
    #[error("read a tile of {found} bytes (at least {expected} expected)")]
    Truncated {
        /// How many bytes were present.
        found: usize,
        /// How many the structure needs.
        expected: usize,
    },
    /// The file doesn't start with the magic bytes.
    #[error("recognize this file as a transfer tile")]
    BadMagic,
    /// The file's format version isn't one this build reads.
    #[error("read tile format version {found} (this build reads version {supported})")]
    UnsupportedVersion {
        /// The version in the file.
        found: u16,
        /// The version this build reads.
        supported: u16,
    },
    /// The file's contents don't match its checksum.
    #[error("verify a tile checksum: stored {stored:#010x}, computed {computed:#010x}")]
    ChecksumMismatch {
        /// The checksum in the header.
        stored: u32,
        /// The checksum of the bytes.
        computed: u32,
    },
    /// A varint ran past the end of the tile.
    #[error("read a varint past the end of the tile")]
    TruncatedVarint,
    /// A varint doesn't fit 32 bits.
    #[error("read a varint too large for 32 bits")]
    VarintOverflow,
    /// A value in a column is outside its valid range.
    #[error("read a value in {column}: {reason}")]
    InvalidValue {
        /// The column.
        column: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// Bytes remain after the last column.
    #[error("read a tile with {found} trailing bytes after its columns")]
    TrailingData {
        /// How many bytes were left.
        found: usize,
    },
}

/// A validated tile: its decompressed bytes and where its columns start.
///
/// Holding the bytes rather than decoded columns keeps a tile to what was decompressed,
/// so [`DecodeLimits::max_decompressed_bytes`] bounds its memory, and validation allocates nothing.
/// [`Self::rows`] decodes as it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferTile {
    /// The tile's mode.
    pub mode: TransferMode,
    /// The tile's column.
    pub x: u32,
    /// The tile's row.
    pub y: u32,
    /// The fingerprint of the manifest this tile was built with.
    pub manifest_id: u64,
    /// The bound the tile was built with, in seconds.
    pub max_seconds: u32,
    origins: u32,
    walks: u32,
    raw: Vec<u8>,
    counts_start: usize,
    walks_start: usize,
    seconds_start: usize,
}

impl TransferTile {
    /// A tile from rows sorted by origin, each with its walks sorted by `(feed, stop)`.
    ///
    /// # Errors
    ///
    /// Refuses rows that are out of order, walk from a stop to itself, exceed `max_seconds`,
    /// have a tile past the zoom's range, or don't fit the format.
    pub fn from_rows(
        mode: TransferMode,
        (x, y): (u32, u32),
        manifest_id: u64,
        max_seconds: u32,
        rows: &[TransferTileRow<'_>],
    ) -> Result<Self, TileEncodeError> {
        let origins = u32::try_from(rows.len()).map_err(|_| TileEncodeError::TooLarge)?;
        let walks = rows.iter().map(|row| row.walks.len()).sum::<usize>();
        let walks = u32::try_from(walks).map_err(|_| TileEncodeError::TooLarge)?;
        let mut out = Vec::with_capacity(HEADER_LEN + walks as usize * 4);
        out.extend_from_slice(&TILE_MAGIC);
        out.extend_from_slice(&TILE_FORMAT_VERSION.to_le_bytes());
        out.push(mode.byte());
        out.push(TILE_ZOOM);
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
        out.extend_from_slice(&manifest_id.to_le_bytes());
        out.extend_from_slice(&max_seconds.to_le_bytes());
        out.extend_from_slice(&origins.to_le_bytes());
        out.extend_from_slice(&walks.to_le_bytes());
        // The checksum goes last, so it lands in its place once the body is hashed.
        out.extend_from_slice(&[0; 4]);
        let mut previous = None;
        for row in rows {
            write_keyed(&mut previous, row.feed, row.stop, &mut out);
        }
        for row in rows {
            write_varint(
                u32::try_from(row.walks.len()).map_err(|_| TileEncodeError::TooLarge)?,
                &mut out,
            );
        }
        for row in rows {
            let mut previous = None;
            for walk in row.walks {
                write_keyed(&mut previous, walk.feed, walk.stop, &mut out);
            }
        }
        for row in rows {
            for walk in row.walks {
                write_varint(walk.seconds, &mut out);
            }
        }
        let mut hasher = Hasher::new();
        hasher.update(&out);
        let crc = hasher.finalize();
        out[HEADER_LEN - 4..HEADER_LEN].copy_from_slice(&crc.to_le_bytes());
        // The same validation a decoder runs, so a tile that encodes is one that decodes.
        Ok(Self::from_vec(out)?)
    }

    /// How many origin stops this tile has.
    #[inline]
    pub fn origin_count(&self) -> usize {
        self.origins as usize
    }

    /// How many walks this tile has.
    #[inline]
    pub fn walk_count(&self) -> usize {
        self.walks as usize
    }

    /// The walks grouped by origin, sorted by `(feed, stop)`.
    pub fn rows(&self) -> TileRows<'_> {
        let column = |start, end| self.raw.get(start..end).unwrap_or(&[]);
        TileRows {
            remaining: self.origins as usize,
            origins: KeyedReader::new(column(HEADER_LEN, self.counts_start)),
            counts: Cursor {
                bytes: column(self.counts_start, self.walks_start),
            },
            walks: Cursor {
                bytes: column(self.walks_start, self.seconds_start),
            },
            seconds: Cursor {
                bytes: self.raw.get(self.seconds_start..).unwrap_or(&[]),
            },
        }
    }

    /// The uncompressed tile, ready for a zstd frame.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.raw
    }

    /// Reads an uncompressed tile, validating every invariant the format promises.
    ///
    /// # Errors
    ///
    /// Everything [`TileDecodeError`] names, never a panic.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TileDecodeError> {
        Self::from_vec(bytes.to_vec())
    }

    fn from_vec(raw: Vec<u8>) -> Result<Self, TileDecodeError> {
        if raw.len() < HEADER_LEN {
            return Err(TileDecodeError::Truncated {
                found: raw.len(),
                expected: HEADER_LEN,
            });
        }
        if raw[..4] != TILE_MAGIC {
            return Err(TileDecodeError::BadMagic);
        }
        let version = le_u16(&raw[4..6]);
        if version != TILE_FORMAT_VERSION {
            return Err(TileDecodeError::UnsupportedVersion {
                found: version,
                supported: TILE_FORMAT_VERSION,
            });
        }
        let stored_crc = le_u32(&raw[36..40]);
        let mut hasher = Hasher::new();
        hasher.update(&raw[..36]);
        hasher.update(&[0; 4]);
        hasher.update(&raw[40..]);
        let computed = hasher.finalize();
        if stored_crc != computed {
            return Err(TileDecodeError::ChecksumMismatch {
                stored: stored_crc,
                computed,
            });
        }
        let invalid = |column, reason| TileDecodeError::InvalidValue { column, reason };
        let mode = TransferMode::from_byte(raw[6]).ok_or(invalid("header", "unknown mode"))?;
        if raw[7] != TILE_ZOOM {
            return Err(invalid("header", "a zoom other than the format's"));
        }
        let (x, y) = (le_u32(&raw[8..12]), le_u32(&raw[12..16]));
        if x >= TILES_PER_AXIS || y >= TILES_PER_AXIS {
            return Err(invalid("tile", "beyond the zoom's range"));
        }
        let max_seconds = le_u32(&raw[24..28]);
        let origins = le_u32(&raw[28..32]);
        let walks = le_u32(&raw[32..36]);
        let columns = Columns::validate(&raw[HEADER_LEN..], origins, walks, max_seconds)?;
        Ok(Self {
            mode,
            x,
            y,
            manifest_id: u64::from_le_bytes(raw[16..24].try_into().unwrap_or([0; 8])),
            max_seconds,
            origins,
            walks,
            counts_start: HEADER_LEN + columns.counts,
            walks_start: HEADER_LEN + columns.walks,
            seconds_start: HEADER_LEN + columns.seconds,
            raw,
        })
    }
}

/// Where a body's columns start, relative to the body.
struct Columns {
    counts: usize,
    walks: usize,
    seconds: usize,
}

impl Columns {
    /// Checks every column of a tile body and finds its columns, allocating nothing.
    ///
    /// Each origin needs at least three bytes and each walk three more,
    /// so a header claiming more than the body can hold fails before any column is read.
    fn validate(
        body: &[u8],
        origins: u32,
        walks: u32,
        max_seconds: u32,
    ) -> Result<Self, TileDecodeError> {
        fn invalid<T>(column: &'static str, reason: &'static str) -> Result<T, TileDecodeError> {
            Err(TileDecodeError::InvalidValue { column, reason })
        }
        let (origin_total, walk_total) = (u64::from(origins), u64::from(walks));
        if (origin_total + walk_total).saturating_mul(3) > body.len() as u64 {
            return invalid("header", "counts the body can't hold");
        }
        let consumed = |cursor: &Cursor<'_>| body.len() - cursor.bytes.len();

        let mut origin_keys = KeyedReader::new(body);
        let mut previous = None;
        for _ in 0..origins {
            let origin = origin_keys.next()?;
            if previous.is_some_and(|prior| prior >= origin) {
                return invalid("origins", "not strictly increasing");
            }
            previous = Some(origin);
        }
        let counts_start = consumed(&origin_keys.cursor);
        let mut counts = Cursor {
            bytes: origin_keys.cursor.bytes,
        };
        let mut total = 0_u64;
        for _ in 0..origins {
            let count = counts.varint()?;
            if count == 0 {
                return invalid("walk counts", "an origin with no walks");
            }
            total += u64::from(count);
            if total > walk_total {
                return invalid("walk counts", "don't sum to the walk count");
            }
        }
        if total != walk_total {
            return invalid("walk counts", "don't sum to the walk count");
        }
        let walks_start = consumed(&counts);

        // Rows again, in step with their origins, now that the column ends are known.
        let mut origin_keys = KeyedReader::new(body);
        let mut row_counts = Cursor {
            bytes: body.get(counts_start..).unwrap_or(&[]),
        };
        let mut walk_cursor = Cursor {
            bytes: body.get(walks_start..).unwrap_or(&[]),
        };
        for _ in 0..origins {
            let origin = origin_keys.next()?;
            let mut row = KeyedReader::new(walk_cursor.bytes);
            let mut previous_walk = None;
            for _ in 0..row_counts.varint()? {
                let walk = row.next()?;
                if previous_walk.is_some_and(|prior| prior >= walk) {
                    return invalid("walks", "not strictly increasing within an origin");
                }
                if walk == origin {
                    return invalid("walks", "a stop walking to itself");
                }
                previous_walk = Some(walk);
            }
            walk_cursor = row.cursor;
        }
        let seconds_start = consumed(&walk_cursor);
        for _ in 0..walks {
            if walk_cursor.varint()? > max_seconds {
                return invalid("walks", "past the bound");
            }
        }
        if !walk_cursor.bytes.is_empty() {
            return Err(TileDecodeError::TrailingData {
                found: walk_cursor.bytes.len(),
            });
        }
        Ok(Self {
            counts: counts_start,
            walks: walks_start,
            seconds: seconds_start,
        })
    }
}

/// The rows of a [`TransferTile`], decoding as it goes.
///
/// A tile is validated when it is made, so the decodes here can't fail;
/// a row that somehow did would simply end the iteration.
#[derive(Clone, Debug)]
pub struct TileRows<'a> {
    remaining: usize,
    origins: KeyedReader<'a>,
    counts: Cursor<'a>,
    walks: Cursor<'a>,
    seconds: Cursor<'a>,
}

impl<'a> Iterator for TileRows<'a> {
    type Item = (u32, u32, TileWalks<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.remaining = self.remaining.checked_sub(1)?;
        let (feed, stop) = self.origins.next().ok()?;
        let count = self.counts.varint().ok()? as usize;
        let row = TileWalks {
            count,
            walks: self.walks.clone(),
            seconds: self.seconds.clone(),
        };
        for _ in 0..count {
            self.walks.varint().ok()?;
            self.walks.varint().ok()?;
            self.seconds.varint().ok()?;
        }
        Some((feed, stop, row))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for TileRows<'_> {}

/// The walks of one origin in a [`TransferTile`].
#[derive(Clone, Debug)]
pub struct TileWalks<'a> {
    count: usize,
    walks: Cursor<'a>,
    seconds: Cursor<'a>,
}

impl TileWalks<'_> {
    /// How many walks this origin has.
    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether this origin has no walks; never so in a valid tile.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The walks, by increasing `(feed, stop)`.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = TileWalk> + '_ {
        let mut keys = KeyedReader::new(self.walks.bytes);
        let mut seconds = self.seconds.clone();
        // Validation guarantees these decodes succeed, so the fallbacks are never used.
        (0..self.count).map(move |_| {
            let (feed, stop) = keys.next().unwrap_or((0, 0));
            TileWalk {
                feed,
                stop,
                seconds: seconds.varint().unwrap_or(0),
            }
        })
    }
}

/// Reads a zstd frame holding a tile, bounded by `limits`, and validates it.
///
/// # Errors
///
/// Everything [`TileDecodeError`] names, never a panic: a corrupt, oversized, or truncated input,
/// a frame that decompresses past [`DecodeLimits::max_decompressed_bytes`], or a tile whose
/// contents break the format.
///
/// # Performance
///
/// Memory stays within a small multiple of the limit for any bytes: decompression is capped,
/// and validation reads the tile without building columns.
pub fn decode_tile(
    compressed: &[u8],
    limits: DecodeLimits,
) -> Result<TransferTile, TileDecodeError> {
    let raw = decompress_bounded(compressed, limits.max_decompressed_bytes)?;
    TransferTile::from_vec(raw)
}

fn decompress_bounded(compressed: &[u8], limit: usize) -> Result<Vec<u8>, TileDecodeError> {
    let mut source = compressed;
    // The window is capped too: a header claiming a huge one must not allocate it.
    let window = u64::try_from(limit)
        .unwrap_or(u64::MAX)
        .clamp(1 << 16, 128 * 1024 * 1024);
    let mut decoder = StreamingDecoder::new_with_max_window_size(&mut source, window)
        .map_err(|e| TileDecodeError::Decompress(e.to_string()))?;
    // Fail on the frame's own claim, before decompressing a byte, when it makes one.
    if decoder.decoder.content_size() > u64::try_from(limit).unwrap_or(u64::MAX) {
        return Err(TileDecodeError::TooLarge { limit });
    }
    let mut buffer = vec![0; READ_CHUNK];
    let mut out: Vec<u8> = Vec::new();
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|e| TileDecodeError::Decompress(e.to_string()))?;
        if read == 0 {
            break;
        }
        let total = out.len() + read;
        if total > limit {
            return Err(TileDecodeError::TooLarge { limit });
        }
        if total > out.capacity() {
            // Not `Vec`'s doubling, which could reach twice the limit.
            let target = (out.capacity() * 2).max(total).max(READ_CHUNK).min(limit);
            out.reserve_exact(target - out.len());
        }
        out.extend_from_slice(&buffer[..read]);
    }
    if !decoder.decoder.is_finished() {
        return Err(TileDecodeError::Decompress(
            "the frame ended before its last block".to_owned(),
        ));
    }
    if !source.is_empty() {
        return Err(TileDecodeError::TrailingFrameData);
    }
    Ok(out)
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().unwrap_or([0; 2]))
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap_or([0; 4]))
}

/// Writes a `(key, value)` pair after the previous one: the key as a delta, and the value as a
/// delta against the previous value of the same key, absolute when the key changes.
///
/// A decreasing pair wraps, so it can't be mistaken for an increasing one: the decoder's
/// checked addition overflows and the tile is refused.
fn write_keyed(previous: &mut Option<(u32, u32)>, key: u32, value: u32, out: &mut Vec<u8>) {
    match *previous {
        None => {
            write_varint(key, out);
            write_varint(value, out);
        }
        Some((prior_key, prior_value)) => {
            write_varint(key.wrapping_sub(prior_key), out);
            write_varint(
                if key == prior_key {
                    value.wrapping_sub(prior_value)
                } else {
                    value
                },
                out,
            );
        }
    }
    *previous = Some((key, value));
}

/// A little-endian base-128 varint.
fn write_varint(mut value: u32, out: &mut Vec<u8>) {
    while value >= 0x80 {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "low seven bits, set the continuation bit"
        )]
        let byte = (value as u8 & 0x7f) | 0x80;
        out.push(byte);
        value >>= 7;
    }
    #[expect(clippy::cast_possible_truncation, reason = "less than 0x80")]
    out.push(value as u8);
}

/// A bounded reader over a tile body.
#[derive(Clone, Debug)]
struct Cursor<'a> {
    bytes: &'a [u8],
}

impl Cursor<'_> {
    fn varint(&mut self) -> Result<u32, TileDecodeError> {
        let mut value = 0_u32;
        let mut shift = 0_u32;
        loop {
            let (&byte, rest) = self
                .bytes
                .split_first()
                .ok_or(TileDecodeError::TruncatedVarint)?;
            self.bytes = rest;
            let low = u32::from(byte & 0x7f);
            if shift >= 32 || (shift == 28 && low > 0x0f) {
                return Err(TileDecodeError::VarintOverflow);
            }
            value |= low << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }
}

/// Reads what [`write_keyed`] wrote, one pair at a time.
#[derive(Clone, Debug)]
struct KeyedReader<'a> {
    cursor: Cursor<'a>,
    previous: Option<(u32, u32)>,
}

impl<'a> KeyedReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            cursor: Cursor { bytes },
            previous: None,
        }
    }

    fn next(&mut self) -> Result<(u32, u32), TileDecodeError> {
        let key_delta = self.cursor.varint()?;
        let value_part = self.cursor.varint()?;
        let pair = match self.previous {
            None => (key_delta, value_part),
            Some((prior_key, prior_value)) => {
                let key =
                    prior_key
                        .checked_add(key_delta)
                        .ok_or(TileDecodeError::InvalidValue {
                            column: "feeds",
                            reason: "delta past 32 bits",
                        })?;
                let value = if key == prior_key {
                    prior_value
                        .checked_add(value_part)
                        .ok_or(TileDecodeError::InvalidValue {
                            column: "stops",
                            reason: "delta past 32 bits",
                        })?
                } else {
                    value_part
                };
                (key, value)
            }
        };
        self.previous = Some(pair);
        Ok(pair)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn compress(bytes: &[u8]) -> Vec<u8> {
        zstd::bulk::compress(bytes, 3).expect("compress")
    }

    fn sample_rows() -> Vec<(u32, u32, Vec<TileWalk>)> {
        vec![
            (
                0,
                3,
                vec![
                    TileWalk {
                        feed: 0,
                        stop: 7,
                        seconds: 90,
                    },
                    TileWalk {
                        feed: 2,
                        stop: 0,
                        seconds: 0,
                    },
                ],
            ),
            (
                0,
                9,
                vec![TileWalk {
                    feed: 0,
                    stop: 3,
                    seconds: 900,
                }],
            ),
            (
                2,
                1,
                vec![TileWalk {
                    feed: 2,
                    stop: 900_000,
                    seconds: 5,
                }],
            ),
        ]
    }

    fn sample() -> TransferTile {
        build_tile(&sample_rows())
    }

    fn build_tile(rows: &[(u32, u32, Vec<TileWalk>)]) -> TransferTile {
        let rows: Vec<TransferTileRow<'_>> = rows
            .iter()
            .map(|(feed, stop, walks)| TransferTileRow {
                feed: *feed,
                stop: *stop,
                walks,
            })
            .collect();
        TransferTile::from_rows(
            TransferMode::Cycling,
            (656, 1422),
            0xfeed_cafe_0123_4567,
            900,
            &rows,
        )
        .expect("valid rows")
    }

    #[test]
    fn a_tile_round_trips() {
        let tile = sample();
        let bytes = tile.as_bytes().to_vec();
        assert_eq!(TransferTile::from_bytes(&bytes).expect("decode"), tile);
        let decoded = decode_tile(&compress(&bytes), DecodeLimits::default()).expect("decode");
        assert_eq!(decoded, tile);
        assert_eq!(decoded.origin_count(), 3);
        assert_eq!(decoded.walk_count(), 4);
        assert_eq!(decoded.mode, TransferMode::Cycling);
    }

    /// The exact bytes of version 1, so a format change has to be deliberate.
    #[test]
    fn the_format_is_stable() {
        let bytes = sample().as_bytes().to_vec();
        let hex: Vec<String> = bytes
            .chunks(16)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        insta::assert_snapshot!(hex.join("\n"));
    }

    #[test]
    fn an_empty_tile_round_trips() {
        let tile = build_tile(&[]);
        let bytes = tile.as_bytes().to_vec();
        assert_eq!(TransferTile::from_bytes(&bytes).expect("decode"), tile);
    }

    fn sample_with(
        mutate: impl FnOnce(&mut Vec<(u32, u32, Vec<TileWalk>)>),
    ) -> Result<TransferTile, TileEncodeError> {
        let mut rows = sample_rows();
        mutate(&mut rows);
        let views: Vec<TransferTileRow<'_>> = rows
            .iter()
            .map(|(feed, stop, walks)| TransferTileRow {
                feed: *feed,
                stop: *stop,
                walks,
            })
            .collect();
        TransferTile::from_rows(
            TransferMode::Cycling,
            (656, 1422),
            0xfeed_cafe_0123_4567,
            900,
            &views,
        )
    }

    #[test]
    fn encoding_refuses_what_decoding_would_refuse() {
        // Origins out of order.
        assert!(sample_with(|rows| rows[1].1 = 3).is_err());
        // Walks out of order within a row.
        assert!(
            sample_with(|rows| {
                rows[0].2[1].feed = 0;
                rows[0].2[1].stop = 7;
            })
            .is_err()
        );
        // A stop walking to itself.
        assert!(
            sample_with(|rows| {
                rows[0].2[0].feed = 0;
                rows[0].2[0].stop = 3;
            })
            .is_err()
        );
        // Seconds over the bound.
        assert!(sample_with(|rows| rows[0].2[0].seconds = 901).is_err());
        // An origin with no walks.
        assert!(sample_with(|rows| rows[0].2.clear()).is_err());
        // A tile beyond the zoom.
        let tile = sample();
        assert!(
            TransferTile::from_rows(tile.mode, (4096, tile.y), tile.manifest_id, 900, &[]).is_err()
        );
    }

    #[test]
    fn a_bomb_is_an_error_not_an_allocation() {
        let plain = vec![0_u8; 300 * 1024 * 1024];
        let bomb = zstd::bulk::compress(&plain, 3).expect("compress");
        assert!(bomb.len() < plain.len() / 100);
        let limited = |max_decompressed_bytes| {
            decode_tile(
                &bomb,
                DecodeLimits {
                    max_decompressed_bytes,
                },
            )
            .expect_err("too big")
        };
        // The window it asks for is over a small limit, so it's refused before decoding.
        assert!(matches!(limited(1 << 20), TileDecodeError::Decompress(_)));
        // Past the window check, the output limit stops it.
        assert!(matches!(
            limited(16 << 20),
            TileDecodeError::TooLarge { .. }
        ));
    }

    #[test]
    fn trailing_bytes_after_a_frame_are_refused() {
        let mut bytes = compress(sample().as_bytes());
        bytes.extend_from_slice(b"\0\0\0\0");
        let error = decode_tile(&bytes, DecodeLimits::default()).expect_err("trailing");
        assert_eq!(error, TileDecodeError::TrailingFrameData);
    }

    #[test]
    fn a_frame_with_trailing_data_after_it_is_refused_even_if_valid_tile() {
        let compressed = compress(sample().as_bytes());
        let mut twice = compressed.clone();
        twice.extend_from_slice(&compressed);
        assert!(decode_tile(&twice, DecodeLimits::default()).is_err());
    }

    fn rows_strategy() -> impl Strategy<Value = Vec<(u32, u32, Vec<TileWalk>)>> {
        // Feeds and stops chosen so deltas stress large gaps and repeats alike.
        let walk = (
            0_u32..4,
            prop_oneof![0_u32..20, 1_000_000_u32..1_000_020],
            0_u32..=900,
        )
            .prop_map(|(feed, stop, seconds)| TileWalk {
                feed,
                stop,
                seconds,
            });
        let origin = (
            0_u32..4,
            prop_oneof![0_u32..20, 1_000_000_u32..1_000_020],
            proptest::collection::btree_map(
                (0_u32..4, prop_oneof![0_u32..20, 1_000_000_u32..1_000_020]),
                0_u32..=900,
                1..8,
            ),
        );
        let _ = walk;
        proptest::collection::btree_map(
            (0_u32..4, prop_oneof![0_u32..20, 1_000_000_u32..1_000_020]),
            origin.prop_map(|(_, _, walks)| walks),
            0..12,
        )
        .prop_map(|rows| {
            rows.into_iter()
                .filter_map(|((feed, stop), walks)| {
                    let walks: Vec<TileWalk> = walks
                        .into_iter()
                        .filter(|&((walk_feed, walk_stop), _)| {
                            (walk_feed, walk_stop) != (feed, stop)
                        })
                        .map(|((feed, stop), seconds)| TileWalk {
                            feed,
                            stop,
                            seconds,
                        })
                        .collect();
                    (!walks.is_empty()).then_some((feed, stop, walks))
                })
                .collect()
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn any_valid_tile_round_trips(rows in rows_strategy()) {
            let tile = build_tile(&rows);
            let bytes = tile.as_bytes().to_vec();
            prop_assert_eq!(&TransferTile::from_bytes(&bytes).expect("decode"), &tile);
            let frame = compress(&bytes);
            prop_assert_eq!(decode_tile(&frame, DecodeLimits::default()).expect("frame"), tile);
        }

        /// Any truncation of a tile is an error, never a panic.
        #[test]
        fn truncated_bytes_are_errors(rows in rows_strategy(), cut in 0.0_f64..1.0) {
            let tile = build_tile(&rows);
            let bytes = tile.as_bytes().to_vec();
            #[expect(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss, reason = "test")]
            let at = ((bytes.len() as f64 * cut) as usize).min(bytes.len().saturating_sub(1));
            prop_assert!(TransferTile::from_bytes(&bytes[..at]).is_err());
            let frame = compress(&bytes);
            #[expect(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss, reason = "test")]
            let frame_at = ((frame.len() as f64 * cut) as usize).min(frame.len().saturating_sub(1));
            prop_assert!(decode_tile(&frame[..frame_at], DecodeLimits::default()).is_err());
        }

        /// Any single corrupted byte is detected or decoded to a valid tile, never a panic.
        #[test]
        fn corrupted_bytes_never_panic(
            rows in rows_strategy(),
            position in 0.0_f64..1.0,
            mask in 1_u8..=255,
        ) {
            let tile = build_tile(&rows);
            let mut bytes = tile.as_bytes().to_vec();
            #[expect(clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::cast_sign_loss, reason = "test")]
            let at = (bytes.len() as f64 * position) as usize % bytes.len().max(1);
            bytes[at] ^= mask;
            if let Ok(decoded) = TransferTile::from_bytes(&bytes) {
                // The checksum caught everything; only a byte it doesn't cover could slip through.
                prop_assert_ne!(decoded, tile);
            }
        }

        /// Arbitrary input to the whole decoder never panics.
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = decode_tile(&bytes, DecodeLimits::default());
            let _ = TransferTile::from_bytes(&bytes);
        }
    }
}
