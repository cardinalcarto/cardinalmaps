//! # Tagged values
//!
//! An edge info's name list can point at *tagged* strings in the text list:
//! the first byte is a tag (see [`TaggedValueKind`]) saying how to read the rest.
//! Valhalla uses these for attributes that only some edges have,
//! like the layer, indoor levels, tunnel names, landmarks, and conditional speed limits.
//!
//! Some tagged values are binary and may contain NUL bytes,
//! so each kind has its own size rule (mainline's `EdgeInfo::TaggedValueSize`).
//! A tag that Valhalla 3.9.1 does not define has no known size;
//! we treat its value as a NUL-terminated string and otherwise ignore it,
//! where mainline would throw.

use super::linguistic::LinguisticRecords;
use super::sections::{SectionDecodingError, text_at, text_tail};
use super::time_domain::ConditionalSpeedLimit;
use num_enum::TryFromPrimitive;
use std::borrow::Cow;

/// The kinds of tagged values in Valhalla 3.9.1 (mainline's `TaggedValue`).
///
/// The discriminants are the tag bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, TryFromPrimitive)]
#[repr(u8)]
pub enum TaggedValueKind {
    /// The OSM `layer` (a single signed byte).
    Layer = 1,
    /// Linguistic records (pronunciations and languages) for the edge's names.
    Linguistic = 2,
    /// Bike share station information (a string).
    BssInfo = 3,
    /// The OSM `level` as a string (deprecated in favor of [`TaggedValueKind::Levels`]).
    Level = 4,
    /// One OSM `level:ref` value (a string).
    LevelRef = 5,
    /// A landmark near the edge.
    Landmark = 6,
    /// A conditional speed limit.
    ConditionalSpeedLimits = 7,
    /// The OSM `level` as varint-encoded ranges.
    Levels = 8,
    /// The OSM node IDs along the edge, varint and delta encoded.
    OsmNodeIds = 9,
    /// A tunnel name (a string).
    ///
    /// Mainline once wrote the tunnel and bridge tags as the ASCII digits `'1'` and `'2'`
    /// instead of the values 1 and 2
    /// ([valhalla#3262](https://github.com/valhalla/valhalla/issues/3262)),
    /// and kept those values for compatibility.
    Tunnel = b'1',
    /// A bridge name (a string; see [`TaggedValueKind::Tunnel`]).
    Bridge = b'2',
}

/// Mainline's sentinel between level ranges (`kLevelRangeSeparator`).
const LEVEL_RANGE_SEPARATOR: i32 = 1_048_575;

/// The size of a landmark's fixed fields: the packed coordinate and the type.
const LANDMARK_HEADER_SIZE: usize = 9;

/// The size of a conditional speed limit.
const CONDITIONAL_SPEED_LIMIT_SIZE: usize = 8;

/// Reads one of mainline's "7-bit encoded varints" (`parse_varint` in `edgeinfo.cc`):
/// a little-endian base-128 varint, zigzag decoded to a 32-bit signed integer.
///
/// This reproduces mainline's arithmetic exactly,
/// which is only a correct zigzag decoding for magnitudes below 2^30
/// (it shifts the signed value arithmetically).
/// That covers any real level or size.
///
/// Returns `None` if the varint is truncated or longer than 5 bytes.
fn read_zigzag_i32(bytes: &[u8], position: &mut usize) -> Option<i32> {
    let mut result: u32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *bytes.get(*position)?;
        *position += 1;
        result |= u32::from(byte & 0x7f).wrapping_shl(shift);
        if byte & 0x80 == 0 {
            // Reinterpreting the bits is the point here (mainline accumulates in an int32_t).
            #[allow(clippy::cast_possible_wrap)]
            let signed = result as i32;
            return Some(if signed & 1 == 1 { !signed } else { signed } >> 1);
        }
    }
    None
}

/// Reads an unsigned base-128 varint of up to 64 bits (mainline's `Int7Decoder::read_varint`).
///
/// Returns `None` if the varint is truncated or longer than 10 bytes.
fn read_varint_u64(bytes: &[u8], position: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*position)?;
        *position += 1;
        result |= u64::from(byte & 0x7f).wrapping_shl(shift);
        if byte & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

/// A single tagged value from the text list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaggedValue<'a> {
    tag: u8,
    /// The value after the tag byte, sized as mainline sizes it (without the terminator).
    value: &'a [u8],
    /// The text list offset of the tag byte.
    offset: usize,
}

impl<'a> TaggedValue<'a> {
    /// Reads the tagged value whose tag byte is at `offset` in the text list.
    ///
    /// # Errors
    ///
    /// Returns an error if the value runs past the end of the text list.
    pub(crate) fn parse(text_list: &'a [u8], offset: usize) -> Result<Self, SectionDecodingError> {
        let tail = text_tail(text_list, offset)?;
        let tag = tail[0];
        let body = &tail[1..];
        let malformed = |reason| SectionDecodingError::MalformedText {
            what: "tagged value",
            offset,
            reason,
        };

        let value = match TaggedValueKind::try_from(tag) {
            Ok(TaggedValueKind::Landmark) => {
                // The packed coordinate may contain NUL bytes, so only the name is a C string.
                if body.len() <= LANDMARK_HEADER_SIZE {
                    return Err(malformed("the landmark runs past the end of the text list"));
                }
                let name = text_at(text_list, offset + 1 + LANDMARK_HEADER_SIZE)?;
                &body[..LANDMARK_HEADER_SIZE + name.len()]
            }
            Ok(TaggedValueKind::Levels | TaggedValueKind::OsmNodeIds) => {
                // A varint byte count, then that many bytes.
                let mut position = 0;
                let size = read_zigzag_i32(body, &mut position)
                    .ok_or(malformed("the size varint is malformed"))?;
                let size = usize::try_from(size).map_err(|_| malformed("the size is negative"))?;
                body.get(..position + size)
                    .ok_or(malformed("the value runs past the end of the text list"))?
            }
            Ok(TaggedValueKind::ConditionalSpeedLimits) => body
                .get(..CONDITIONAL_SPEED_LIMIT_SIZE)
                .ok_or(malformed("the value runs past the end of the text list"))?,
            Ok(TaggedValueKind::Linguistic) => {
                let mut records = LinguisticRecords::new(body, offset + 1);
                for record in &mut records {
                    record?;
                }
                &body[..records.consumed()]
            }
            // Strings, and (as a best effort) tags we do not know.
            Ok(
                TaggedValueKind::Layer
                | TaggedValueKind::BssInfo
                | TaggedValueKind::Level
                | TaggedValueKind::LevelRef
                | TaggedValueKind::Tunnel
                | TaggedValueKind::Bridge,
            )
            | Err(_) => {
                if body.is_empty() {
                    return Err(SectionDecodingError::UnterminatedText { offset });
                }
                text_at(text_list, offset + 1)?
            }
        };
        Ok(Self { tag, value, offset })
    }

    /// The raw tag byte.
    #[inline]
    pub const fn tag(&self) -> u8 {
        self.tag
    }

    /// The kind of value, or `None` if the tag is not one Valhalla 3.9.1 defines.
    #[inline]
    pub fn kind(&self) -> Option<TaggedValueKind> {
        TaggedValueKind::try_from(self.tag).ok()
    }

    /// The raw value after the tag byte, without the terminator.
    ///
    /// This is the value mainline's `EdgeInfo::GetTags` maps the tag to.
    #[inline]
    pub const fn value(&self) -> &'a [u8] {
        self.value
    }

    /// The value as a string (lossy if the tile holds invalid UTF-8).
    ///
    /// This is meaningful for the string kinds:
    /// bike share info, level, level ref, tunnel, and bridge.
    #[inline]
    pub fn text_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.value)
    }

    /// The text list offset of the tag byte.
    #[inline]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    fn malformed(&self, what: &'static str, reason: &'static str) -> SectionDecodingError {
        SectionDecodingError::MalformedText {
            what,
            offset: self.offset,
            reason,
        }
    }

    fn expect_kind(&self, kind: TaggedValueKind) -> Result<(), SectionDecodingError> {
        if self.tag == kind as u8 {
            Ok(())
        } else {
            Err(SectionDecodingError::UnknownValue {
                field: "tagged value kind",
                value: u64::from(self.tag),
            })
        }
    }

    /// Decodes a layer value.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a layer, or the value is not exactly one byte
    /// (mainline throws in both cases).
    #[inline]
    pub fn layer(&self) -> Result<i8, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::Layer)?;
        match self.value {
            &[layer] => Ok(i8::from_le_bytes([layer])),
            _ => Err(self.malformed("layer", "the value is not exactly one byte")),
        }
    }

    /// Decodes a levels value.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a levels value, or its header is malformed.
    #[inline]
    pub fn levels(&self) -> Result<Levels<'a>, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::Levels)?;
        Levels::parse(self.value, self.offset)
    }

    /// Decodes an OSM node ID list.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not an OSM node ID list.
    /// The iterator yields an error if the list itself is malformed.
    #[inline]
    pub fn osm_node_ids(&self) -> Result<OsmNodeIds<'a>, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::OsmNodeIds)?;
        // Skip the byte count; the value was already sized by it.
        let mut position = 0;
        read_zigzag_i32(self.value, &mut position)
            .ok_or(self.malformed("OSM node IDs", "the size varint is malformed"))?;
        Ok(OsmNodeIds {
            bytes: &self.value[position..],
            position: 0,
            value: 0,
            offset: self.offset,
            done: false,
        })
    }

    /// Decodes a conditional speed limit.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a conditional speed limit.
    #[inline]
    pub fn conditional_speed_limit(&self) -> Result<ConditionalSpeedLimit, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::ConditionalSpeedLimits)?;
        let bytes: [u8; CONDITIONAL_SPEED_LIMIT_SIZE] = self
            .value
            .try_into()
            .map_err(|_| self.malformed("conditional speed limit", "the value is not 8 bytes"))?;
        Ok(ConditionalSpeedLimit::from_raw(u64::from_le_bytes(bytes)))
    }

    /// Decodes a landmark (mainline's `Landmark(const std::string&)`).
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a landmark,
    /// the landmark has no name, or its type is not one Valhalla 3.9.1 defines.
    pub fn landmark(&self) -> Result<Landmark<'a>, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::Landmark)?;
        let Some((header, name)) = self.value.split_at_checked(LANDMARK_HEADER_SIZE) else {
            return Err(self.malformed("landmark", "the value is too short"));
        };
        if name.is_empty() {
            return Err(self.malformed("landmark", "the landmark has no name"));
        }
        let mut packed = [0; 8];
        packed.copy_from_slice(&header[..8]);
        let packed = u64::from_le_bytes(packed);
        let landmark_type =
            LandmarkType::try_from(header[8]).map_err(|_| SectionDecodingError::UnknownValue {
                field: "landmark type",
                value: u64::from(header[8]),
            })?;
        // 31 bits of latitude above 32 bits of longitude, both at 7 digits of precision.
        // The masks make the conversions to u32 exact.
        #[allow(clippy::cast_possible_truncation)]
        let (lat_fixed, lng_fixed) = (
            ((packed >> 32) & ((1 << 31) - 1)) as u32,
            (packed & 0xffff_ffff) as u32,
        );
        Ok(Landmark {
            name,
            kind: landmark_type,
            lat: f64::from(lat_fixed) / 1e7 - 90.0,
            lng: f64::from(lng_fixed) / 1e7 - 180.0,
        })
    }

    /// Iterates over linguistic records.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a linguistic value.
    #[inline]
    pub fn linguistic_records(&self) -> Result<LinguisticRecords<'a>, SectionDecodingError> {
        self.expect_kind(TaggedValueKind::Linguistic)?;
        // The value excludes the terminator, so give the records one to stop at.
        // (We validated the run when parsing; the terminator follows the value in the tile.)
        Ok(LinguisticRecords::new_terminated_at_end(
            self.value,
            self.offset + 1,
        ))
    }
}

/// The types of landmark (mainline's `LandmarkType`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
#[allow(missing_docs)]
pub enum LandmarkType {
    Fuel = 1,
    PostOffice = 2,
    Police = 3,
    FireStation = 4,
    CarWash = 5,
    Restaurant = 6,
    FastFood = 7,
    Cafe = 8,
    Bank = 9,
    Pharmacy = 10,
    Kindergarten = 11,
    Bar = 12,
    Hospital = 13,
    Pub = 14,
    Clinic = 15,
    Theatre = 16,
    Cinema = 17,
    Casino = 18,
}

/// A landmark near an edge, used for landmark-based guidance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landmark<'a> {
    name: &'a [u8],
    kind: LandmarkType,
    lng: f64,
    lat: f64,
}

impl<'a> Landmark<'a> {
    /// The raw name bytes.
    #[inline]
    pub const fn name(&self) -> &'a [u8] {
        self.name
    }

    /// The name as a string (lossy if the tile holds invalid UTF-8).
    #[inline]
    pub fn name_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.name)
    }

    /// The landmark type.
    #[inline]
    pub const fn landmark_type(&self) -> LandmarkType {
        self.kind
    }

    /// The longitude, at 7 digits of precision.
    #[inline]
    pub const fn lng(&self) -> f64 {
        self.lng
    }

    /// The latitude, at 7 digits of precision.
    #[inline]
    pub const fn lat(&self) -> f64 {
        self.lat
    }
}

/// An edge's indoor levels, as contiguous ranges (from OSM `level`).
///
/// For example, `0-2;4` is stored as the ranges `(0, 2)` and `(4, 4)`.
///
/// # Precision
///
/// Levels are stored as integers scaled by `10^p`,
/// where `p` is the largest number of decimal places among the tag's values.
/// Mainline 3.9.1's `decode_levels` has a bug:
/// it parses `p` as `int32_t prec_power = parse_varint(ptr) > 0`,
/// so it divides by 10 whenever `p` is positive, even when `p` is 2 or more.
/// A level of `0.25` thus decodes as `2.5`.
/// Since routing (level filtering) and JSON output both use the decoded values,
/// [`Levels::ranges`] and [`Levels::precision`] reproduce this;
/// [`Levels::precision_exponent`] gives the stored `p`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Levels<'a> {
    /// The varint values (levels and separators).
    values: &'a [u8],
    precision_exponent: i32,
    offset: usize,
}

impl<'a> Levels<'a> {
    fn parse(value: &'a [u8], offset: usize) -> Result<Self, SectionDecodingError> {
        let malformed = |reason| SectionDecodingError::MalformedText {
            what: "levels",
            offset,
            reason,
        };
        let mut position = 0;
        // The byte count, which the value was already sized by.
        read_zigzag_i32(value, &mut position).ok_or(malformed("the size varint is malformed"))?;
        let precision_exponent = read_zigzag_i32(value, &mut position)
            .ok_or(malformed("the precision varint is malformed"))?;
        Ok(Self {
            values: &value[position..],
            precision_exponent,
            offset,
        })
    }

    /// The number of decimal places the values were stored with.
    #[inline]
    pub const fn precision_exponent(&self) -> i32 {
        self.precision_exponent
    }

    /// The divisor mainline applies to the stored values:
    /// 0 (meaning none) for integer levels, and 10 otherwise (see the [type docs](Levels)).
    #[inline]
    pub const fn precision(&self) -> u32 {
        if self.precision_exponent > 0 { 10 } else { 0 }
    }

    /// Iterates over the level ranges, with both ends inclusive,
    /// exactly as mainline's `decode_levels` produces them.
    ///
    /// A single level is a range with equal ends.
    #[inline]
    pub fn ranges(&self) -> LevelRanges<'a> {
        LevelRanges {
            values: self.values,
            position: 0,
            precision: self.precision(),
            offset: self.offset,
            done: false,
        }
    }

    /// Whether the edge connects the given level,
    /// as mainline's `EdgeInfo::includes_level` decides it.
    ///
    /// # Errors
    ///
    /// Returns an error if the values are malformed.
    pub fn includes_level(&self, level: f32) -> Result<bool, SectionDecodingError> {
        let ranges = self.ranges().collect::<Result<Vec<_>, _>>()?;
        // A port of std::lower_bound (as libstdc++ implements it),
        // comparing each range's upper end with the level.
        // The ranges are sorted by their lower ends, which makes this a correct search
        // whenever the ranges do not overlap.
        let mut first = 0;
        let mut len = ranges.len();
        while len > 0 {
            let half = len / 2;
            let middle = first + half;
            if ranges[middle].1 < level {
                first = middle + 1;
                len = len - half - 1;
            } else {
                len = half;
            }
        }
        Ok(ranges
            .get(first)
            .is_some_and(|&(lo, hi)| lo <= level && level <= hi))
    }
}

/// An iterator over level ranges (see [`Levels::ranges`]).
#[derive(Clone, Debug)]
pub struct LevelRanges<'a> {
    values: &'a [u8],
    position: usize,
    precision: u32,
    offset: usize,
    done: bool,
}

impl LevelRanges<'_> {
    fn next_value(&mut self) -> Option<Result<i32, SectionDecodingError>> {
        if self.position >= self.values.len() {
            return None;
        }
        if let Some(value) = read_zigzag_i32(self.values, &mut self.position) {
            Some(Ok(value))
        } else {
            self.done = true;
            Some(Err(SectionDecodingError::MalformedText {
                what: "levels",
                offset: self.offset,
                reason: "a level varint is truncated",
            }))
        }
    }

    fn scale(&self, value: i32) -> f32 {
        // Levels are small integers, so f32 is what mainline uses and is exact enough.
        #[allow(clippy::cast_precision_loss)]
        let value = value as f32;
        if self.precision == 0 {
            value
        } else {
            #[allow(clippy::cast_precision_loss)]
            let precision = self.precision as f32;
            value / precision
        }
    }
}

impl Iterator for LevelRanges<'_> {
    type Item = Result<(f32, f32), SectionDecodingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // Skip separators until the first value of a range.
        let first = loop {
            match self.next_value()? {
                Ok(LEVEL_RANGE_SEPARATOR) => {}
                Ok(value) => break self.scale(value),
                Err(e) => return Some(Err(e)),
            }
        };
        // Every further value before a separator replaces the upper end.
        let mut last = first;
        loop {
            match self.next_value() {
                None | Some(Ok(LEVEL_RANGE_SEPARATOR)) => break,
                Some(Ok(value)) => last = self.scale(value),
                Some(Err(e)) => return Some(Err(e)),
            }
        }
        Some(Ok((first, last)))
    }
}

/// An iterator over an edge's OSM node IDs (see [`TaggedValue::osm_node_ids`]).
///
/// # Errors
///
/// Yields an error (and then stops) if a varint is truncated.
#[derive(Clone, Debug)]
pub struct OsmNodeIds<'a> {
    bytes: &'a [u8],
    position: usize,
    value: u64,
    offset: usize,
    done: bool,
}

impl Iterator for OsmNodeIds<'_> {
    type Item = Result<u64, SectionDecodingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.position >= self.bytes.len() {
            return None;
        }
        let Some(diff) = read_varint_u64(self.bytes, &mut self.position) else {
            self.done = true;
            return Some(Err(SectionDecodingError::MalformedText {
                what: "OSM node IDs",
                offset: self.offset,
                reason: "an ID varint is truncated",
            }));
        };
        // Zigzag decoded deltas, accumulated with wrapping like mainline's unsigned arithmetic.
        #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
        let delta = ((diff >> 1) as i64 ^ -((diff & 1) as i64)) as u64;
        self.value = self.value.wrapping_add(delta);
        Some(Ok(self.value))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Mainline's `encode_level` (from `osmway.cc`), which zigzag encodes an int.
    // The truncations take 7 bits at a time, as mainline does.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn encode_level(value: i32) -> Vec<u8> {
        let mut out = Vec::new();
        #[allow(clippy::cast_sign_loss)]
        let mut v = ((value << 1) ^ (value >> 31)) as u32;
        while v > 0x7f {
            out.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        out.push(v as u8);
        out
    }

    /// Builds a levels tagged value the way `OSMWay::GetTaggedValues` does.
    pub(crate) fn encode_levels(ranges: &[(i32, i32)], precision: i32) -> Vec<u8> {
        let mut levels = Vec::new();
        for (i, &(lo, hi)) in ranges.iter().enumerate() {
            levels.extend(encode_level(lo));
            if lo != hi {
                levels.extend(encode_level(hi));
            }
            if i + 1 != ranges.len() {
                levels.extend(encode_level(LEVEL_RANGE_SEPARATOR));
            }
        }
        let precision = encode_level(precision);
        let mut out = vec![TaggedValueKind::Levels as u8];
        out.extend(encode_level(
            i32::try_from(levels.len() + precision.len()).unwrap(),
        ));
        out.extend(precision);
        out.extend(levels);
        out.push(0);
        out
    }

    fn parse(bytes: &[u8]) -> Result<TaggedValue<'_>, SectionDecodingError> {
        TaggedValue::parse(bytes, 0)
    }

    #[test]
    fn test_zigzag_varint() {
        // Mainline's encoder and decoder only agree for magnitudes below 2^30.
        for value in [0, 1, -1, 63, -64, 64, 1_048_575, (1 << 30) - 1, -(1 << 30)] {
            let bytes = encode_level(value);
            let mut position = 0;
            assert_eq!(read_zigzag_i32(&bytes, &mut position), Some(value));
            assert_eq!(position, bytes.len());
        }
        // Truncated
        assert_eq!(read_zigzag_i32(&[0x80], &mut 0), None);
        // Too long
        assert_eq!(read_zigzag_i32(&[0x80; 6], &mut 0), None);
    }

    #[test]
    fn test_levels() {
        let bytes = encode_levels(&[(-1, -1), (0, 12), (14, 14)], 0);
        let value = parse(&bytes).unwrap();
        assert_eq!(value.kind(), Some(TaggedValueKind::Levels));
        assert_eq!(value.value().len(), bytes.len() - 2);
        let levels = value.levels().unwrap();
        assert_eq!(levels.precision(), 0);
        let ranges = levels.ranges().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(ranges, vec![(-1.0, -1.0), (0.0, 12.0), (14.0, 14.0)]);
        assert!(levels.includes_level(5.0).unwrap());
        assert!(levels.includes_level(14.0).unwrap());
        assert!(levels.includes_level(-1.0).unwrap());
        assert!(!levels.includes_level(13.0).unwrap());
        assert!(!levels.includes_level(15.0).unwrap());
    }

    #[test]
    fn test_levels_precision_bug() {
        // "0.5;1.25" has 2 decimal places, so it is stored as 50 and 125.
        let bytes = encode_levels(&[(50, 50), (125, 125)], 2);
        let levels = parse(&bytes).unwrap().levels().unwrap();
        assert_eq!(levels.precision_exponent(), 2);
        // Mainline divides by 10, not 100.
        assert_eq!(levels.precision(), 10);
        let ranges = levels.ranges().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(ranges, vec![(5.0, 5.0), (12.5, 12.5)]);
    }

    #[test]
    fn test_level_ranges_extra_values_extend_the_range() {
        // Mainline keeps replacing the upper end until a separator.
        let mut levels = Vec::new();
        for v in [1, 2, 3] {
            levels.extend(encode_level(v));
        }
        let mut bytes = vec![TaggedValueKind::Levels as u8];
        bytes.extend(encode_level(i32::try_from(levels.len()).unwrap() + 1));
        bytes.extend(encode_level(0));
        bytes.extend(levels);
        bytes.push(0);
        let ranges = parse(&bytes)
            .unwrap()
            .levels()
            .unwrap()
            .ranges()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(ranges, vec![(1.0, 3.0)]);
    }

    #[test]
    fn test_truncated_sized_values() {
        // The size says 10 bytes, but there are only 2.
        let bytes = [TaggedValueKind::Levels as u8, 20, 0, 2, 0];
        assert!(matches!(
            parse(&bytes),
            Err(SectionDecodingError::MalformedText {
                what: "tagged value",
                ..
            })
        ));
        // A negative size
        let bytes = [TaggedValueKind::OsmNodeIds as u8, 1, 0];
        assert!(parse(&bytes).is_err());
        // A level varint that runs past the value's size
        let bytes = [TaggedValueKind::Levels as u8, 4, 0, 0x80, 0];
        let levels = parse(&bytes).unwrap().levels().unwrap();
        let mut ranges = levels.ranges();
        assert!(matches!(ranges.next(), Some(Err(_))));
        assert!(ranges.next().is_none());
    }

    #[test]
    fn test_layer_and_strings() {
        let bytes = [TaggedValueKind::Layer as u8, 0xff, 0];
        assert_eq!(parse(&bytes).unwrap().layer().unwrap(), -1);
        let bytes = [TaggedValueKind::Layer as u8, 1, 2, 0];
        assert!(parse(&bytes).unwrap().layer().is_err());

        let mut bytes = vec![b'1'];
        bytes.extend(b"T\xc3\xbanel d'Envalira\0");
        let value = parse(&bytes).unwrap();
        assert_eq!(value.kind(), Some(TaggedValueKind::Tunnel));
        assert_eq!(value.text_lossy(), "Túnel d'Envalira");
        assert!(value.layer().is_err());

        // Unterminated
        let bytes = [TaggedValueKind::LevelRef as u8, b'A'];
        assert_eq!(
            parse(&bytes),
            Err(SectionDecodingError::UnterminatedText { offset: 1 })
        );
        let bytes = [TaggedValueKind::LevelRef as u8];
        assert_eq!(
            parse(&bytes),
            Err(SectionDecodingError::UnterminatedText { offset: 0 })
        );
    }

    #[test]
    fn test_unknown_tag() {
        let bytes = [42, b'x', b'y', 0, b'z'];
        let value = parse(&bytes).unwrap();
        assert_eq!(value.kind(), None);
        assert_eq!(value.tag(), 42);
        assert_eq!(value.value(), b"xy");
        assert!(value.layer().is_err());
    }

    #[test]
    // The truncations take 7 bits at a time, as mainline's encode7int does.
    #[allow(clippy::cast_possible_truncation)]
    fn test_osm_node_ids() {
        // Mainline's encode7int: zigzag deltas, then the byte count prepended (also encode7int).
        let ids: [u64; 4] = [12_345_678_901, 12_345_678_905, 12_345_678_000, 1];
        let mut data = Vec::new();
        let mut last = 0u64;
        for id in ids {
            #[allow(clippy::cast_possible_wrap)]
            let diff = id.wrapping_sub(last) as i64;
            #[allow(clippy::cast_sign_loss)]
            let mut v = ((diff << 1) ^ (diff >> 63)) as u64;
            while v > 0x7f {
                data.push(0x80 | (v & 0x7f) as u8);
                v >>= 7;
            }
            data.push(v as u8);
            last = id;
        }
        let mut bytes = vec![TaggedValueKind::OsmNodeIds as u8];
        bytes.extend(encode_level(i32::try_from(data.len()).unwrap()));
        bytes.extend(&data);
        bytes.push(0);
        let decoded = parse(&bytes)
            .unwrap()
            .osm_node_ids()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(decoded, ids);
    }

    #[test]
    fn test_conditional_speed_limit() {
        // Contains NUL bytes, which must not end the value early.
        let raw: u64 = (30 << 54) | (0b011_1110 << 1);
        let mut bytes = vec![TaggedValueKind::ConditionalSpeedLimits as u8];
        bytes.extend(raw.to_le_bytes());
        bytes.push(0);
        let limit = parse(&bytes).unwrap().conditional_speed_limit().unwrap();
        assert_eq!(limit.speed(), 30);
        assert_eq!(limit.time_domain().to_string(), "Mo-Fr");
        // Truncated
        assert!(parse(&bytes[..5]).is_err());
    }

    /// Mainline's `Landmark::to_str` plus the tag that `GraphTileBuilder::AddLandmark` prepends.
    pub(crate) fn encode_landmark(lng: f64, lat: f64, landmark_type: u8, name: &[u8]) -> Vec<u8> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let packed = ((lng + 180.0) * 1e7 + 0.5) as u64 | (((lat + 90.0) * 1e7 + 0.5) as u64) << 32;
        let mut out = vec![TaggedValueKind::Landmark as u8];
        out.extend(packed.to_le_bytes());
        out.push(landmark_type);
        out.extend(name);
        out.push(0);
        out
    }

    #[test]
    fn test_landmark() {
        // A longitude of exactly -180 packs to zero, so the value starts with NUL bytes.
        let bytes = encode_landmark(-180.0, 42.5063, LandmarkType::Cafe as u8, b"Caf\xc3\xa8");
        let value = parse(&bytes).unwrap();
        assert_eq!(value.value().len(), bytes.len() - 2);
        let landmark = value.landmark().unwrap();
        assert_eq!(landmark.name_lossy(), "Cafè");
        assert_eq!(landmark.landmark_type(), LandmarkType::Cafe);
        assert!((landmark.lng() - -180.0).abs() < 1e-9);
        assert!((landmark.lat() - 42.5063).abs() < 1e-7);

        // Unknown type
        let bytes = encode_landmark(1.5, 42.5, 19, b"x");
        assert!(parse(&bytes).unwrap().landmark().is_err());
        // No name
        let bytes = encode_landmark(1.5, 42.5, 1, b"");
        assert!(parse(&bytes).unwrap().landmark().is_err());
        // Truncated header
        let bytes = encode_landmark(1.5, 42.5, 1, b"x");
        assert!(parse(&bytes[..6]).is_err());
    }
}
