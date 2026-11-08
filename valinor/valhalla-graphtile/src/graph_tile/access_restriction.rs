use super::sections::SectionDecodingError;
use super::time_domain::TimeDomain;
use crate::Access;
use bitfield_struct::bitfield;
use enumset::EnumSet;
use num_enum::TryFromPrimitive;
use zerocopy::{LE, U16, U32, U64};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

/// Types of access restrictions (mainline's `AccessType`).
///
/// The type field is 6 bits wide, but mainline documents 31 as the maximum supported value
/// (its mask table has 32 entries).
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum AccessRestrictionType {
    Hazmat = 0,
    MaxHeight = 1,
    MaxWidth = 2,
    MaxLength = 3,
    MaxWeight = 4,
    MaxAxleLoad = 5,
    TimedAllowed = 6,
    TimedDenied = 7,
    DestinationAllowed = 8,
    MaxAxles = 9,
}

impl AccessRestrictionType {
    /// The mask bit for this type (mainline's `kAccessRestrictionMasks`).
    ///
    /// Directed edges summarize which restriction types apply to them with these bits,
    /// so costing can skip the restriction lookup for edges without relevant ones.
    /// The timed and destination types have no mask bit (0).
    #[inline]
    pub const fn mask(self) -> u8 {
        match self {
            Self::Hazmat => 1,
            Self::MaxHeight => 2,
            Self::MaxWidth => 4,
            Self::MaxLength => 8,
            Self::MaxWeight => 16,
            Self::MaxAxleLoad => 32,
            Self::MaxAxles => 64,
            Self::TimedAllowed | Self::TimedDenied | Self::DestinationAllowed => 0,
        }
    }

    /// The name mainline uses in JSON output (for example, `max_height`).
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hazmat => "hazmat",
            Self::MaxHeight => "max_height",
            Self::MaxWidth => "max_width",
            Self::MaxLength => "max_length",
            Self::MaxWeight => "max_weight",
            Self::MaxAxleLoad => "max_axle_load",
            Self::TimedAllowed => "timed_allowed",
            Self::TimedDenied => "timed_denied",
            Self::DestinationAllowed => "destination_allowed",
            Self::MaxAxles => "max_axles",
        }
    }
}

/// The decoded value of an access restriction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessRestrictionValue {
    /// Whether hazardous materials are prohibited.
    Hazmat(bool),
    /// A limit in hundredths of a meter (height, width, and length)
    /// or of a metric ton (weight and axle load).
    Hundredths(u64),
    /// When the restriction applies (timed and destination restrictions).
    TimeDomain(TimeDomain),
    /// The maximum number of axles.
    Axles(u64),
}

#[bitfield(u64,
    repr = U64<LE>,
    from = bit_twiddling_helpers::conv_u64le::from_inner,
    into = bit_twiddling_helpers::conv_u64le::into_inner
)]
#[derive(PartialEq, Eq, FromBytes, IntoBytes, Immutable, Unaligned)]
struct AccessRestrictionBitField {
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    edge_index: U32<LE>,
    // Kept raw so that decoding an unknown type is an error rather than a panic.
    #[bits(6)]
    restriction_type: u8,
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    modes: U16<LE>,
    /// New in Valhalla 3.9 (it took one of the spare bits).
    #[bits(1)]
    except_destination: u8,
    #[bits(23)]
    _spare: u32,
}

/// Access restrictions beyond the usual access tags (mainline's `AccessRestriction`).
#[derive(PartialEq, Eq, FromBytes, IntoBytes, Immutable, Unaligned, Clone)]
#[repr(C)]
pub struct AccessRestriction {
    bitfield: AccessRestrictionBitField,
    value: U64<LE>,
}

impl AccessRestriction {
    /// Gets the edge index (within the tile) to which the restriction applies.
    #[inline]
    pub fn edge_index(&self) -> u32 {
        self.bitfield.edge_index().get()
    }

    /// Gets the raw type of access restriction.
    #[inline]
    pub const fn raw_restriction_type(&self) -> u8 {
        self.bitfield.restriction_type()
    }

    /// Gets the type of access restriction.
    ///
    /// # Errors
    ///
    /// Returns an error if the tile holds a type that Valhalla 3.9.1 does not define.
    #[inline]
    pub fn restriction_type(&self) -> Result<AccessRestrictionType, SectionDecodingError> {
        let raw = self.raw_restriction_type();
        AccessRestrictionType::try_from(raw).map_err(|_| SectionDecodingError::UnknownValue {
            field: "access restriction type",
            value: u64::from(raw),
        })
    }

    /// The mask bit for the restriction's type (see [`AccessRestrictionType::mask`]).
    ///
    /// Unknown types have no mask bit (mainline's table is zero for them).
    #[inline]
    pub fn mask(&self) -> u8 {
        self.restriction_type()
            .map_or(0, AccessRestrictionType::mask)
    }

    /// The access modes affected by this restriction.
    #[inline]
    pub fn affected_access_modes(&self) -> EnumSet<Access> {
        // The field is 12 bits, and Access has exactly 12 variants, so nothing is truncated.
        EnumSet::from_repr_truncated(self.bitfield.modes().get())
    }

    /// Whether local traffic (traffic with a destination on the edge) is exempt
    /// (from OSM tags like `maxweight:conditional=none @ destination`).
    #[inline]
    pub const fn except_destination(&self) -> bool {
        self.bitfield.except_destination() != 0
    }

    /// The raw value; see [`AccessRestriction::restriction_value`] for its meaning.
    #[inline]
    pub fn value(&self) -> u64 {
        self.value.get()
    }

    /// The value, interpreted according to the restriction type
    /// (as mainline's `AccessRestriction::json` does).
    ///
    /// # Errors
    ///
    /// Returns an error if the type is unknown.
    #[inline]
    pub fn restriction_value(&self) -> Result<AccessRestrictionValue, SectionDecodingError> {
        let value = self.value();
        Ok(match self.restriction_type()? {
            AccessRestrictionType::Hazmat => AccessRestrictionValue::Hazmat(value != 0),
            AccessRestrictionType::MaxHeight
            | AccessRestrictionType::MaxWidth
            | AccessRestrictionType::MaxLength
            | AccessRestrictionType::MaxWeight
            | AccessRestrictionType::MaxAxleLoad => AccessRestrictionValue::Hundredths(value),
            AccessRestrictionType::TimedAllowed
            | AccessRestrictionType::TimedDenied
            | AccessRestrictionType::DestinationAllowed => {
                AccessRestrictionValue::TimeDomain(TimeDomain::from_raw(value))
            }
            AccessRestrictionType::MaxAxles => AccessRestrictionValue::Axles(value),
        })
    }
}

impl core::fmt::Debug for AccessRestriction {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AccessRestriction")
            .field("edge_index", &self.edge_index())
            .field("restriction_type", &self.restriction_type())
            .field("modes", &self.affected_access_modes())
            .field("except_destination", &self.except_destination())
            .field("value", &self.restriction_value())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{AccessRestriction, AccessRestrictionType, AccessRestrictionValue};
    use crate::Access;
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};
    use enumset::EnumSet;
    use zerocopy::FromBytes;

    /// Lays out a restriction as mainline's `AccessRestriction` constructor does.
    fn restriction(
        edge: u64,
        kind: u64,
        modes: u64,
        except: bool,
        value: u64,
    ) -> AccessRestriction {
        let bits = edge | kind << 22 | modes << 28 | u64::from(except) << 40;
        let mut bytes = bits.to_le_bytes().to_vec();
        bytes.extend(value.to_le_bytes());
        AccessRestriction::read_from_bytes(&bytes).unwrap()
    }

    #[test]
    fn test_parse_access_restrictions_count() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        assert_eq!(
            tile_view.access_restrictions.len(),
            tile.header().access_restriction_count() as usize
        );
    }

    #[test]
    fn test_parse_access_restrictions() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!(
                "first_access_restriction",
                tile_view.access_restrictions[0]
            );
        }

        // Cross-checked with GraphTile::GetAccessRestrictions in Valhalla 3.9.1
        // (type 1 with modes 233 and value 440 on edge 1456; the section oracle test covers the rest).
        let first = &tile_view.access_restrictions[0];
        assert_eq!(first.edge_index(), 1456);
        assert_eq!(
            first.restriction_type().unwrap(),
            AccessRestrictionType::MaxHeight
        );
        assert_eq!(
            first.affected_access_modes(),
            Access::Auto | Access::Truck | Access::Taxi | Access::Bus | Access::HOV
        );
        assert!(!first.except_destination());
        assert_eq!(
            first.restriction_value().unwrap(),
            AccessRestrictionValue::Hundredths(440)
        );
        assert_eq!(first.mask(), 2);

        if !cfg!(miri) {
            insta::assert_debug_snapshot!(
                "last_access_restriction",
                tile_view.access_restrictions.last().unwrap()
            );
        }
    }

    #[test]
    fn test_except_destination_bit() {
        // Bit 40, right after the 12 mode bits.
        let r = restriction(7, 4, 4095, true, 350);
        assert!(r.except_destination());
        assert_eq!(r.affected_access_modes(), EnumSet::all());
        assert_eq!(
            r.restriction_type().unwrap(),
            AccessRestrictionType::MaxWeight
        );
        assert_eq!(r.edge_index(), 7);
        let r = restriction(7, 4, 4095, false, 350);
        assert!(!r.except_destination());
    }

    #[test]
    fn test_values_and_masks() {
        let timed = restriction(0, 6, 1, false, (0b011_1110 << 1) | (7 << 8) | (9 << 31));
        match timed.restriction_value().unwrap() {
            AccessRestrictionValue::TimeDomain(td) => {
                assert_eq!(td.to_string(), "Mo-Fr 07:00-09:00");
            }
            other => panic!("unexpected value {other:?}"),
        }
        assert_eq!(timed.mask(), 0);
        assert_eq!(
            restriction(0, 0, 1, false, 1).restriction_value().unwrap(),
            AccessRestrictionValue::Hazmat(true)
        );
        assert_eq!(
            restriction(0, 9, 1, false, 3).restriction_value().unwrap(),
            AccessRestrictionValue::Axles(3)
        );
        assert_eq!(restriction(0, 9, 1, false, 3).mask(), 64);
        let unknown = restriction(0, 40, 1, false, 3);
        assert!(unknown.restriction_type().is_err());
        assert!(unknown.restriction_value().is_err());
        assert_eq!(unknown.mask(), 0);

        let masks = (0..10)
            .map(|t| AccessRestrictionType::try_from(t).unwrap().mask())
            .collect::<Vec<_>>();
        // kAccessRestrictionMasks in Valhalla 3.9.1
        assert_eq!(masks, [1, 2, 4, 8, 16, 32, 0, 0, 0, 64]);
    }

    #[test]
    fn test_get_access_restrictions() {
        // Cross-checked with GraphTile::GetAccessRestrictions(idx, access) in Valhalla 3.9.1.
        let tile = &*TEST_GRAPH_TILE_L0;
        assert_eq!(
            tile.get_access_restrictions(1456, Access::Auto.into())
                .len(),
            1
        );
        assert!(
            tile.get_access_restrictions(1456, Access::Pedestrian.into())
                .is_empty()
        );
        assert!(
            tile.get_access_restrictions(1455, EnumSet::all())
                .is_empty()
        );
    }
}
