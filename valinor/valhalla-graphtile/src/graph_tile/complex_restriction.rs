//! # Complex restrictions
//!
//! A complex restriction is a turn restriction that a simple edge-to-edge bit cannot express:
//! one with via ways, one that only applies to some travel modes,
//! or one that only applies at certain times.
//! Each is a fixed 24-byte record followed by its via edges (mainline's `ComplexRestriction`).
//!
//! Tiles keep two lists of them:
//! forward restrictions (in the tile of the *to* edge) for forward searches,
//! and reverse restrictions (in the tile of the *from* edge) for reverse searches.

use super::GraphId;
use super::sections::SectionDecodingError;
use crate::Access;
use bitfield_struct::bitfield;
use enumset::EnumSet;
use num_enum::TryFromPrimitive;
use zerocopy::{FromBytes as _, LE, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// Turn restriction types (mainline's `RestrictionType`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum RestrictionType {
    NoLeftTurn = 0,
    NoRightTurn = 1,
    NoStraightOn = 2,
    NoUTurn = 3,
    OnlyRightTurn = 4,
    OnlyLeftTurn = 5,
    OnlyStraightOn = 6,
    NoEntry = 7,
    NoExit = 8,
    NoTurn = 9,
    /// An "only" restriction that holds with some probability.
    OnlyProbable = 10,
    /// A "no" restriction that holds with some probability.
    NoProbable = 11,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout)]
struct FromBitfield {
    #[bits(46)]
    from_graph_id: u64,
    #[bits(1)]
    has_dt: u8,
    #[bits(5)]
    begin_day_dow: u8,
    #[bits(4)]
    begin_month: u8,
    #[bits(3)]
    begin_week: u8,
    #[bits(5)]
    begin_hrs: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout)]
struct ToBitfield {
    #[bits(46)]
    to_graph_id: u64,
    #[bits(1)]
    dt_type: u8,
    #[bits(5)]
    end_day_dow: u8,
    #[bits(4)]
    end_month: u8,
    #[bits(3)]
    end_week: u8,
    #[bits(5)]
    end_hrs: u8,
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, KnownLayout)]
struct DataBitfield {
    #[bits(4)]
    restriction_type: u8,
    #[bits(12)]
    modes: u16,
    #[bits(5)]
    via_count: u8,
    #[bits(7)]
    dow: u8,
    #[bits(6)]
    begin_mins: u8,
    #[bits(6)]
    end_mins: u8,
    #[bits(7)]
    probability: u8,
    #[bits(17)]
    _spare: u32,
}

/// The fixed-size part of a complex restriction, as stored in the tile.
#[derive(FromBytes, Immutable, Unaligned, KnownLayout, Debug)]
#[repr(C)]
struct ComplexRestrictionRecord {
    from: FromBitfield,
    to: ToBitfield,
    data: DataBitfield,
}

/// A complex restriction: the fixed-size record plus its via edges.
#[derive(Clone, Copy)]
pub struct ComplexRestriction<'a> {
    record: &'a ComplexRestrictionRecord,
    vias: &'a [GraphId],
}

/// When a complex restriction applies
/// (only present when [`ComplexRestriction::date_time`] returns one).
///
/// The fields mirror those of a [`TimeDomain`](super::TimeDomain),
/// and mainline evaluates them with `DateTime::is_conditional_active`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(missing_docs)]
pub struct RestrictionDateTime {
    /// False for day-of-month ranges (`kYMD`), true for nth day of week ranges (`kNthDow`).
    pub nth_dow: bool,
    /// The days of the week, as a mask with bit 0 for Sunday.
    pub dow: u8,
    pub begin_month: u8,
    pub begin_week: u8,
    pub begin_day_dow: u8,
    pub begin_hrs: u8,
    pub begin_mins: u8,
    pub end_month: u8,
    pub end_week: u8,
    pub end_day_dow: u8,
    pub end_hrs: u8,
    pub end_mins: u8,
}

impl<'a> ComplexRestriction<'a> {
    /// The raw 46-bit *from* edge ID.
    #[inline]
    pub fn raw_from_graph_id(&self) -> u64 {
        self.record.from.from_graph_id()
    }

    /// The raw 46-bit *to* edge ID.
    #[inline]
    pub fn raw_to_graph_id(&self) -> u64 {
        self.record.to.to_graph_id()
    }

    /// The edge the restriction starts on.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored ID is invalid.
    #[inline]
    pub fn from_graph_id(&self) -> Result<GraphId, SectionDecodingError> {
        graph_id("complex restriction from edge", self.raw_from_graph_id())
    }

    /// The edge the restriction ends on.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored ID is invalid.
    #[inline]
    pub fn to_graph_id(&self) -> Result<GraphId, SectionDecodingError> {
        graph_id("complex restriction to edge", self.raw_to_graph_id())
    }

    /// The via edges, in the order mainline's `WalkVias` visits them.
    #[inline]
    pub const fn vias(&self) -> &'a [GraphId] {
        self.vias
    }

    /// The raw restriction type.
    #[inline]
    pub fn raw_restriction_type(&self) -> u8 {
        self.record.data.restriction_type()
    }

    /// The restriction type.
    ///
    /// # Errors
    ///
    /// Returns an error if the stored type is not one Valhalla 3.9.1 defines.
    #[inline]
    pub fn restriction_type(&self) -> Result<RestrictionType, SectionDecodingError> {
        let raw = self.raw_restriction_type();
        RestrictionType::try_from(raw).map_err(|_| SectionDecodingError::UnknownValue {
            field: "complex restriction type",
            value: u64::from(raw),
        })
    }

    /// The access modes the restriction applies to.
    #[inline]
    pub fn modes(&self) -> EnumSet<Access> {
        EnumSet::from_repr_truncated(self.record.data.modes())
    }

    /// The probability (from 0 to 100) of a probable restriction.
    #[inline]
    pub fn probability(&self) -> u8 {
        self.record.data.probability()
    }

    /// Whether the restriction only applies at certain times.
    #[inline]
    pub fn has_date_time(&self) -> bool {
        self.record.from.has_dt() != 0
    }

    /// When the restriction applies, or `None` if it always does.
    ///
    /// When a restriction has no date and time,
    /// mainline 3.9.1 leaves these fields uninitialized,
    /// so they hold arbitrary values that differ between builds of the same tile.
    /// This only returns them when they mean something.
    #[inline]
    pub fn date_time(&self) -> Option<RestrictionDateTime> {
        let (from, to, data) = (&self.record.from, &self.record.to, &self.record.data);
        self.has_date_time().then(|| RestrictionDateTime {
            nth_dow: to.dt_type() != 0,
            dow: data.dow(),
            begin_month: from.begin_month(),
            begin_week: from.begin_week(),
            begin_day_dow: from.begin_day_dow(),
            begin_hrs: from.begin_hrs(),
            begin_mins: data.begin_mins(),
            end_month: to.end_month(),
            end_week: to.end_week(),
            end_day_dow: to.end_day_dow(),
            end_hrs: to.end_hrs(),
            end_mins: data.end_mins(),
        })
    }
}

impl core::fmt::Debug for ComplexRestriction<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Leaves out the date and time fields when they are uninitialized (see `date_time`).
        f.debug_struct("ComplexRestriction")
            .field("from", &self.from_graph_id())
            .field("to", &self.to_graph_id())
            .field("restriction_type", &self.restriction_type())
            .field("modes", &self.modes())
            .field("vias", &self.vias)
            .field("probability", &self.probability())
            .field("date_time", &self.date_time())
            .finish()
    }
}

fn graph_id(field: &'static str, raw: u64) -> Result<GraphId, SectionDecodingError> {
    GraphId::try_from_id(raw).map_err(|_| SectionDecodingError::UnknownValue { field, value: raw })
}

/// An iterator over a list of complex restrictions.
///
/// # Errors
///
/// Yields an error (and then stops) if a restriction runs past the end of the section.
#[derive(Clone, Debug)]
pub struct ComplexRestrictions<'a> {
    bytes: &'a [u8],
    position: usize,
    section: &'static str,
    done: bool,
}

impl<'a> ComplexRestrictions<'a> {
    #[inline]
    pub(crate) const fn new(bytes: &'a [u8], section: &'static str) -> Self {
        Self {
            bytes,
            position: 0,
            section,
            done: false,
        }
    }
}

impl<'a> Iterator for ComplexRestrictions<'a> {
    type Item = Result<ComplexRestriction<'a>, SectionDecodingError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.position >= self.bytes.len() {
            return None;
        }
        let rest = &self.bytes[self.position..];
        let parsed = ComplexRestrictionRecord::ref_from_prefix(rest)
            .ok()
            .and_then(|(record, rest)| {
                <[GraphId]>::ref_from_prefix_with_elems(rest, usize::from(record.data.via_count()))
                    .ok()
                    .map(|(vias, _)| ComplexRestriction { record, vias })
            });
        if let Some(restriction) = parsed {
            self.position += size_of::<ComplexRestrictionRecord>() + size_of_val(restriction.vias);
            Some(Ok(restriction))
        } else {
            self.done = true;
            Some(Err(SectionDecodingError::MalformedSection {
                section: self.section,
                offset: self.position,
                reason: "a restriction runs past the end of the section",
            }))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};

    /// Builds a restriction record the way mainline's `ComplexRestrictionBuilder` writes it.
    pub(crate) fn encode(
        from: u64,
        to: u64,
        restriction_type: u8,
        modes: u16,
        vias: &[u64],
        date_time: Option<RestrictionDateTime>,
        probability: u8,
    ) -> Vec<u8> {
        let dt = date_time.unwrap_or(RestrictionDateTime {
            nth_dow: false,
            dow: 0,
            begin_month: 0,
            begin_week: 0,
            begin_day_dow: 0,
            begin_hrs: 0,
            begin_mins: 0,
            end_month: 0,
            end_week: 0,
            end_day_dow: 0,
            end_hrs: 0,
            end_mins: 0,
        });
        let record = ComplexRestrictionRecord {
            from: FromBitfield::new()
                .with_from_graph_id(from)
                .with_has_dt(u8::from(date_time.is_some()))
                .with_begin_day_dow(dt.begin_day_dow)
                .with_begin_month(dt.begin_month)
                .with_begin_week(dt.begin_week)
                .with_begin_hrs(dt.begin_hrs),
            to: ToBitfield::new()
                .with_to_graph_id(to)
                .with_dt_type(u8::from(dt.nth_dow))
                .with_end_day_dow(dt.end_day_dow)
                .with_end_month(dt.end_month)
                .with_end_week(dt.end_week)
                .with_end_hrs(dt.end_hrs),
            data: DataBitfield::new()
                .with_restriction_type(restriction_type)
                .with_modes(modes)
                .with_via_count(u8::try_from(vias.len()).unwrap())
                .with_dow(dt.dow)
                .with_begin_mins(dt.begin_mins)
                .with_end_mins(dt.end_mins)
                .with_probability(probability),
        };
        let mut out = record.from.into_bits().get().to_le_bytes().to_vec();
        out.extend(record.to.into_bits().get().to_le_bytes());
        out.extend(record.data.into_bits().get().to_le_bytes());
        for via in vias {
            out.extend(via.to_le_bytes());
        }
        out
    }

    #[test]
    fn test_record_size() {
        assert_eq!(size_of::<ComplexRestrictionRecord>(), 24);
    }

    #[test]
    fn test_round_trip_with_date_time() {
        let dt = RestrictionDateTime {
            nth_dow: true,
            dow: 0b011_1110,
            begin_month: 3,
            begin_week: 5,
            begin_day_dow: 1,
            begin_hrs: 7,
            begin_mins: 30,
            end_month: 10,
            end_week: 1,
            end_day_dow: 7,
            end_hrs: 19,
            end_mins: 59,
        };
        let from = GraphId::try_from_components(0, 3015, 12).unwrap();
        let to = GraphId::try_from_components(1, 47701, 2_097_151).unwrap();
        let via = GraphId::try_from_components(2, 762_485, 7).unwrap();
        let mut bytes = encode(
            from.value(),
            to.value(),
            RestrictionType::NoProbable as u8,
            0b1000_0000_0001,
            &[via.value(), from.value()],
            Some(dt),
            42,
        );
        bytes.extend(encode(to.value(), from.value(), 9, 1, &[], None, 0));

        let all = ComplexRestrictions::new(&bytes, "test")
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(all.len(), 2);
        let r = all[0];
        assert_eq!(r.from_graph_id().unwrap(), from);
        assert_eq!(r.to_graph_id().unwrap(), to);
        assert_eq!(r.vias(), &[via, from]);
        assert_eq!(r.restriction_type().unwrap(), RestrictionType::NoProbable);
        assert_eq!(r.modes(), Access::Auto | Access::Reserved);
        assert_eq!(r.probability(), 42);
        assert_eq!(r.date_time(), Some(dt));
        assert!(all[1].date_time().is_none());
        assert_eq!(all[1].restriction_type().unwrap(), RestrictionType::NoTurn);
        assert!(all[1].vias().is_empty());
    }

    #[test]
    fn test_truncated() {
        let bytes = encode(1, 2, 0, 1, &[3, 4], None, 0);
        // Cut into the vias, and into the fixed part
        for len in [bytes.len() - 1, 30, 23, 1] {
            let mut it = ComplexRestrictions::new(&bytes[..len], "complex_forward_restrictions");
            assert_eq!(
                it.next().unwrap().err(),
                Some(SectionDecodingError::MalformedSection {
                    section: "complex_forward_restrictions",
                    offset: 0,
                    reason: "a restriction runs past the end of the section",
                })
            );
            assert!(it.next().is_none());
        }
    }

    #[test]
    fn test_invalid_values() {
        let invalid_id = (1 << 46) - 1;
        let bytes = encode(invalid_id, 2, 15, 1, &[], None, 0);
        let r = ComplexRestrictions::new(&bytes, "test")
            .next()
            .unwrap()
            .unwrap();
        assert!(r.from_graph_id().is_err());
        assert!(r.restriction_type().is_err());
    }

    #[test]
    fn test_fixture_lookups() {
        // Cross-checked with GraphTile::GetComplexRestrictions in Valhalla 3.9.1:
        // tile 0/3015 has one reverse restriction and no forward ones.
        let tile = &*TEST_GRAPH_TILE_L0;
        let sections = tile.sections();
        assert_eq!(sections.forward_complex_restrictions().count(), 0);
        let reverse = sections
            .reverse_complex_restrictions()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(reverse.len(), 1);
        let from = GraphId::try_from_components(0, 3015, 2143).unwrap();
        let r = reverse[0];
        assert_eq!(r.from_graph_id().unwrap(), from);
        assert_eq!(
            r.to_graph_id().unwrap(),
            GraphId::try_from_components(2, 763_926, 5865).unwrap()
        );
        assert_eq!(
            r.vias(),
            &[GraphId::try_from_components(0, 3015, 2141).unwrap()]
        );
        assert_eq!(r.restriction_type().unwrap(), RestrictionType::NoRightTurn);
        assert_eq!(r.modes().as_repr(), 1661);
        assert!(r.date_time().is_none());

        let found = tile
            .get_complex_restrictions(false, from, EnumSet::all())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(found.len(), 1);
        // Modes must overlap: 1661 does not include pedestrians.
        assert_eq!(
            tile.get_complex_restrictions(false, from, Access::Pedestrian.into())
                .count(),
            0
        );
        // Reverse restrictions match on the from edge only.
        assert_eq!(
            tile.get_complex_restrictions(true, from, EnumSet::all())
                .count(),
            0
        );
    }
}
