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

/// Bytes of one decoded tile, in columns so a phone doesn't pay for a pointer per row.
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
    origin_feed: Vec<u32>,
    origin_stop: Vec<u32>,
    /// For each origin, the walk index just past its row.
    row_end: Vec<u32>,
    walk_feed: Vec<u32>,
    walk_stop: Vec<u32>,
    walk_seconds: Vec<u32>,
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
        let walk_count: usize = rows.iter().map(|row| row.walks.len()).sum();
        let mut tile = Self {
            mode,
            x,
            y,
            manifest_id,
            max_seconds,
            origin_feed: Vec::with_capacity(rows.len()),
            origin_stop: Vec::with_capacity(rows.len()),
            row_end: Vec::with_capacity(rows.len()),
            walk_feed: Vec::with_capacity(walk_count),
            walk_stop: Vec::with_capacity(walk_count),
            walk_seconds: Vec::with_capacity(walk_count),
        };
        for row in rows {
            tile.origin_feed.push(row.feed);
            tile.origin_stop.push(row.stop);
            for walk in row.walks {
                tile.walk_feed.push(walk.feed);
                tile.walk_stop.push(walk.stop);
                tile.walk_seconds.push(walk.seconds);
            }
            tile.row_end
                .push(u32::try_from(tile.walk_feed.len()).map_err(|_| TileEncodeError::TooLarge)?);
        }
        tile.validate()?;
        Ok(tile)
    }

    fn validate(&self) -> Result<(), TileDecodeError> {
        fn invalid<T>(column: &'static str, reason: &'static str) -> Result<T, TileDecodeError> {
            Err(TileDecodeError::InvalidValue { column, reason })
        }
        if self.x >= TILES_PER_AXIS || self.y >= TILES_PER_AXIS {
            return invalid("tile", "beyond the zoom's range");
        }
        let lengths = [
            self.origin_feed.len(),
            self.origin_stop.len(),
            self.row_end.len(),
        ];
        if lengths.iter().any(|&len| len != lengths[0]) {
            return invalid("origins", "columns of different lengths");
        }
        if self.walk_stop.len() != self.walk_feed.len()
            || self.walk_seconds.len() != self.walk_feed.len()
        {
            return invalid("walks", "columns of different lengths");
        }
        let mut row_start = 0_u32;
        let mut previous_origin = None;
        for (index, &row_end) in self.row_end.iter().enumerate() {
            let (Some(&origin_feed), Some(&origin_stop)) =
                (self.origin_feed.get(index), self.origin_stop.get(index))
            else {
                return invalid("origins", "columns of different lengths");
            };
            let origin = (origin_feed, origin_stop);
            if previous_origin.is_some_and(|prior| prior >= origin) {
                return invalid("origins", "not strictly increasing");
            }
            previous_origin = Some(origin);
            if row_end <= row_start {
                return invalid("walk counts", "an origin with no walks");
            }
            let Some(row) = self.row_range(row_start, row_end) else {
                return invalid("walk counts", "a row past the end of the walks");
            };
            let mut previous_walk = None;
            for walk in row {
                let (Some(&feed), Some(&stop), Some(&seconds)) = (
                    self.walk_feed.get(walk),
                    self.walk_stop.get(walk),
                    self.walk_seconds.get(walk),
                ) else {
                    return invalid("walks", "columns of different lengths");
                };
                if previous_walk.is_some_and(|prior| prior >= (feed, stop)) {
                    return invalid("walks", "not strictly increasing within an origin");
                }
                previous_walk = Some((feed, stop));
                if (feed, stop) == origin {
                    return invalid("walks", "a stop walking to itself");
                }
                if seconds > self.max_seconds {
                    return invalid("walks", "past the bound");
                }
            }
            row_start = row_end;
        }
        if row_start as usize != self.walk_feed.len() {
            return invalid("walk counts", "don't sum to the walk count");
        }
        Ok(())
    }

    fn row_range(&self, start: u32, end: u32) -> Option<core::ops::Range<usize>> {
        let (start, end) = (start as usize, end as usize);
        (start <= end && end <= self.walk_feed.len()).then_some(start..end)
    }

    /// How many origin stops this tile has.
    #[inline]
    pub fn origin_count(&self) -> usize {
        self.origin_feed.len()
    }

    /// How many walks this tile has.
    #[inline]
    pub fn walk_count(&self) -> usize {
        self.walk_feed.len()
    }

    /// The walks grouped by origin, sorted by `(feed, stop)`.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = (u32, u32, TileWalks<'_>)> + '_ {
        // Validation guarantees the columns line up, so these lookups never fall back.
        (0..self.origin_feed.len()).map(|index| {
            let start = index
                .checked_sub(1)
                .and_then(|prior| self.row_end.get(prior))
                .copied()
                .unwrap_or(0) as usize;
            let end = self.row_end.get(index).copied().unwrap_or(0) as usize;
            (
                self.origin_feed.get(index).copied().unwrap_or(0),
                self.origin_stop.get(index).copied().unwrap_or(0),
                TileWalks {
                    tile: self,
                    range: start..end,
                },
            )
        })
    }

    /// Writes the uncompressed tile, ready for a zstd frame.
    ///
    /// # Errors
    ///
    /// Fails only if a count doesn't fit its field.
    pub fn to_bytes(&self) -> Result<Vec<u8>, TileEncodeError> {
        self.validate()?;
        let origins =
            u32::try_from(self.origin_feed.len()).map_err(|_| TileEncodeError::TooLarge)?;
        let walks = u32::try_from(self.walk_feed.len()).map_err(|_| TileEncodeError::TooLarge)?;
        let mut out = Vec::with_capacity(HEADER_LEN + self.walk_feed.len() * 4);
        out.extend_from_slice(&TILE_MAGIC);
        out.extend_from_slice(&TILE_FORMAT_VERSION.to_le_bytes());
        out.push(self.mode.byte());
        out.push(TILE_ZOOM);
        out.extend_from_slice(&self.x.to_le_bytes());
        out.extend_from_slice(&self.y.to_le_bytes());
        out.extend_from_slice(&self.manifest_id.to_le_bytes());
        out.extend_from_slice(&self.max_seconds.to_le_bytes());
        out.extend_from_slice(&origins.to_le_bytes());
        out.extend_from_slice(&walks.to_le_bytes());
        // The checksum goes last, so it lands in its place once the body is hashed.
        out.extend_from_slice(&[0; 4]);
        write_keyed_deltas(&self.origin_feed, &self.origin_stop, &mut out);
        let mut previous_end = 0;
        for &end in &self.row_end {
            write_varint(end - previous_end, &mut out);
            previous_end = end;
        }
        for (index, _) in self.row_end.iter().enumerate() {
            let start = index
                .checked_sub(1)
                .and_then(|prior| self.row_end.get(prior))
                .copied()
                .unwrap_or(0) as usize;
            let end = self.row_end.get(index).copied().unwrap_or(0) as usize;
            write_keyed_deltas(
                self.walk_feed.get(start..end).unwrap_or(&[]),
                self.walk_stop.get(start..end).unwrap_or(&[]),
                &mut out,
            );
        }
        for &seconds in &self.walk_seconds {
            write_varint(seconds, &mut out);
        }
        let mut hasher = Hasher::new();
        hasher.update(&out);
        let crc = hasher.finalize();
        out[HEADER_LEN - 4..HEADER_LEN].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }

    /// Reads an uncompressed tile, validating every invariant the format promises.
    ///
    /// # Errors
    ///
    /// Everything [`TileDecodeError`] names, never a panic.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TileDecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(TileDecodeError::Truncated {
                found: bytes.len(),
                expected: HEADER_LEN,
            });
        }
        if bytes[..4] != TILE_MAGIC {
            return Err(TileDecodeError::BadMagic);
        }
        let version = le_u16(&bytes[4..6]);
        if version != TILE_FORMAT_VERSION {
            return Err(TileDecodeError::UnsupportedVersion {
                found: version,
                supported: TILE_FORMAT_VERSION,
            });
        }
        let stored_crc = le_u32(&bytes[36..40]);
        let mut hasher = Hasher::new();
        hasher.update(&bytes[..36]);
        hasher.update(&[0; 4]);
        hasher.update(&bytes[40..]);
        let computed = hasher.finalize();
        if stored_crc != computed {
            return Err(TileDecodeError::ChecksumMismatch {
                stored: stored_crc,
                computed,
            });
        }
        let mode = TransferMode::from_byte(bytes[6]).ok_or(TileDecodeError::InvalidValue {
            column: "header",
            reason: "unknown mode",
        })?;
        if bytes[7] != TILE_ZOOM {
            return Err(TileDecodeError::InvalidValue {
                column: "header",
                reason: "a zoom other than the format's",
            });
        }
        let origin_count = le_u32(&bytes[28..32]) as usize;
        let walk_count = le_u32(&bytes[32..36]) as usize;
        let mut cursor = Cursor {
            bytes: &bytes[HEADER_LEN..],
        };
        // Each value is at least one byte, so a lying count can't reserve past the data.
        let (origin_feed, origin_stop) = read_keyed_deltas(&mut cursor, origin_count, None)?;
        let remaining = cursor.bytes.len();
        let mut row_end = Vec::with_capacity(origin_count.min(remaining));
        let mut total = 0_u64;
        for _ in 0..origin_count {
            total += u64::from(cursor.varint()?);
            row_end.push(u32::try_from(total).map_err(|_| TileDecodeError::VarintOverflow)?);
        }
        if total != walk_count as u64 {
            return Err(TileDecodeError::InvalidValue {
                column: "walk counts",
                reason: "don't sum to the walk count",
            });
        }
        let reserve = walk_count.min(cursor.bytes.len());
        let mut walk_feed = Vec::with_capacity(reserve);
        let mut walk_stop = Vec::with_capacity(reserve);
        let mut row_start = 0_usize;
        for &row_end_value in &row_end {
            let end = row_end_value as usize;
            let count = end
                .checked_sub(row_start)
                .ok_or(TileDecodeError::InvalidValue {
                    column: "walk counts",
                    reason: "not increasing",
                })?;
            let (feeds, stops) = read_keyed_deltas(&mut cursor, count, Some(reserve))?;
            walk_feed.extend(feeds);
            walk_stop.extend(stops);
            row_start = end;
        }
        let mut walk_seconds = Vec::with_capacity(reserve);
        for _ in 0..walk_count {
            walk_seconds.push(cursor.varint()?);
        }
        if !cursor.bytes.is_empty() {
            return Err(TileDecodeError::TrailingData {
                found: cursor.bytes.len(),
            });
        }
        let tile = Self {
            mode,
            x: le_u32(&bytes[8..12]),
            y: le_u32(&bytes[12..16]),
            manifest_id: u64::from_le_bytes(bytes[16..24].try_into().unwrap_or([0; 8])),
            max_seconds: le_u32(&bytes[24..28]),
            origin_feed,
            origin_stop,
            row_end,
            walk_feed,
            walk_stop,
            walk_seconds,
        };
        tile.validate()?;
        Ok(tile)
    }
}

/// The walks of one origin in a [`TransferTile`].
#[derive(Clone, Debug)]
pub struct TileWalks<'a> {
    tile: &'a TransferTile,
    range: core::ops::Range<usize>,
}

impl TileWalks<'_> {
    /// How many walks this origin has.
    #[inline]
    pub fn len(&self) -> usize {
        self.range.len()
    }

    /// Whether this origin has no walks; never so in a valid tile.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    /// The walks, by increasing `(feed, stop)`.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = TileWalk> + '_ {
        self.range.clone().map(|i| TileWalk {
            feed: self.tile.walk_feed.get(i).copied().unwrap_or(0),
            stop: self.tile.walk_stop.get(i).copied().unwrap_or(0),
            seconds: self.tile.walk_seconds.get(i).copied().unwrap_or(0),
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
/// Decompresses into one buffer and allocates columns proportional to the tile.
pub fn decode_tile(
    compressed: &[u8],
    limits: DecodeLimits,
) -> Result<TransferTile, TileDecodeError> {
    let raw = decompress_bounded(compressed, limits.max_decompressed_bytes)?;
    TransferTile::from_bytes(&raw)
}

fn decompress_bounded(compressed: &[u8], limit: usize) -> Result<Vec<u8>, TileDecodeError> {
    let mut source = compressed;
    // The window is capped too: a header claiming a huge one must not allocate it.
    let window = u64::try_from(limit)
        .unwrap_or(u64::MAX)
        .clamp(1 << 16, 128 * 1024 * 1024);
    let mut decoder = StreamingDecoder::new_with_max_window_size(&mut source, window)
        .map_err(|e| TileDecodeError::Decompress(e.to_string()))?;
    let mut out = Vec::new();
    let take_limit = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    Read::take(&mut decoder, take_limit)
        .read_to_end(&mut out)
        .map_err(|e| TileDecodeError::Decompress(e.to_string()))?;
    if out.len() > limit {
        return Err(TileDecodeError::TooLarge { limit });
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

/// Writes `(key, value)` pairs sorted by key then value: keys as deltas, and values as a delta
/// against the previous value of the same key, absolute when the key changes.
///
/// A column resets at each row of walks; the origin columns are one long row.
fn write_keyed_deltas(keys: &[u32], values: &[u32], out: &mut Vec<u8>) {
    let mut previous = None;
    for (&key, &value) in keys.iter().zip(values) {
        match previous {
            None => {
                write_varint(key, out);
                write_varint(value, out);
            }
            Some((prior_key, prior_value)) => {
                write_varint(key - prior_key, out);
                write_varint(
                    if key == prior_key {
                        value - prior_value
                    } else {
                        value
                    },
                    out,
                );
            }
        }
        previous = Some((key, value));
    }
}

/// Reads what [`write_keyed_deltas`] wrote, without trusting `count`.
fn read_keyed_deltas(
    cursor: &mut Cursor<'_>,
    count: usize,
    reserve_hint: Option<usize>,
) -> Result<(Vec<u32>, Vec<u32>), TileDecodeError> {
    let reserve = count
        .min(reserve_hint.unwrap_or(usize::MAX))
        .min(cursor.bytes.len());
    let mut keys = Vec::with_capacity(reserve);
    let mut values = Vec::with_capacity(reserve);
    let mut previous: Option<(u32, u32)> = None;
    for _ in 0..count {
        let key_delta = cursor.varint()?;
        let value_part = cursor.varint()?;
        let (key, value) = match previous {
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
        keys.push(key);
        values.push(value);
        previous = Some((key, value));
    }
    Ok((keys, values))
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
        let bytes = tile.to_bytes().expect("encode");
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
        let bytes = sample().to_bytes().expect("encode");
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
        let bytes = tile.to_bytes().expect("encode");
        assert_eq!(TransferTile::from_bytes(&bytes).expect("decode"), tile);
    }

    fn tile_with(mutate: impl FnOnce(&mut TransferTile)) -> Result<Vec<u8>, TileEncodeError> {
        let mut tile = sample();
        mutate(&mut tile);
        // `to_bytes` validates what `from_rows` would, so build by hand.
        tile.to_bytes()
    }

    #[test]
    fn encoding_refuses_what_decoding_would_refuse() {
        // Origins out of order.
        assert!(tile_with(|t| t.origin_stop[1] = 3).is_err());
        // Walks out of order within a row.
        assert!(
            tile_with(|t| {
                t.walk_feed[1] = 0;
                t.walk_stop[1] = 7;
            })
            .is_err()
        );
        // A stop walking to itself.
        assert!(
            tile_with(|t| {
                t.walk_feed[0] = 0;
                t.walk_stop[0] = 3;
            })
            .is_err()
        );
        // Seconds over the bound.
        assert!(tile_with(|t| t.walk_seconds[0] = 901).is_err());
        // A tile beyond the zoom.
        assert!(tile_with(|t| t.x = 4096).is_err());
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
        let mut bytes = compress(&sample().to_bytes().expect("encode"));
        bytes.extend_from_slice(b"\0\0\0\0");
        let error = decode_tile(&bytes, DecodeLimits::default()).expect_err("trailing");
        assert_eq!(error, TileDecodeError::TrailingFrameData);
    }

    #[test]
    fn a_frame_with_trailing_data_after_it_is_refused_even_if_valid_tile() {
        let compressed = compress(&sample().to_bytes().expect("encode"));
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
            let bytes = tile.to_bytes().expect("encode");
            prop_assert_eq!(&TransferTile::from_bytes(&bytes).expect("decode"), &tile);
            let frame = compress(&bytes);
            prop_assert_eq!(decode_tile(&frame, DecodeLimits::default()).expect("frame"), tile);
        }

        /// Any truncation of a tile is an error, never a panic.
        #[test]
        fn truncated_bytes_are_errors(rows in rows_strategy(), cut in 0.0_f64..1.0) {
            let tile = build_tile(&rows);
            let bytes = tile.to_bytes().expect("encode");
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
            let mut bytes = tile.to_bytes().expect("encode");
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
