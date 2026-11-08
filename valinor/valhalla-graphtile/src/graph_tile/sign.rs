use super::sections::SectionDecodingError;
use bitfield_struct::bitfield;
use num_enum::TryFromPrimitive;
use zerocopy::{LE, U32};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

/// The type of a sign (mainline's `Sign::Type`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum SignType {
    ExitNumber = 0,
    ExitBranch = 1,
    ExitToward = 2,
    ExitName = 3,
    GuideBranch = 4,
    GuideToward = 5,
    /// A named junction; attached to a node.
    JunctionName = 6,
    GuidanceViewJunction = 7,
    GuidanceViewSignboard = 8,
    /// A toll booth name; attached to a node.
    TollName = 9,
    /// Pronunciations for the other signs on the same edge or node.
    Linguistic = 255,
}

impl SignType {
    /// Converts a raw sign type.
    ///
    /// # Errors
    ///
    /// Returns the raw value if it is not a type Valhalla 3.9.1 defines.
    #[inline]
    pub fn try_from_raw(value: u8) -> Result<Self, u8> {
        Self::try_from(value).map_err(|e| e.number)
    }
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct SignBitField {
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    edge_or_node_index: U32<LE>,
    // Kept raw so that decoding an unknown type is an error rather than a panic.
    #[bits(8)]
    sign_type: u8,
    #[bits(1)]
    is_route_num_type: u8,
    #[bits(1)]
    is_text_tagged: u8,
}

/// A sign on a directed edge or node (mainline's `Sign`).
///
/// Text is stored in the graph tile text list,
/// and the offset is stored within the sign.
/// Exit and guide signs belong to directed edges;
/// junction and toll names belong to nodes.
/// Both kinds share one list, sorted by index,
/// so that signs can be found via either the directed edge or node index.
///
/// To get the text, use [`GraphTile::get_signs`](super::GraphTile::get_signs).
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
#[repr(C)]
pub struct Sign {
    bitfield: SignBitField,
    /// The offset into the graph tile text list
    /// which contains the text for this sign.
    pub text_offset: U32<LE>,
}

impl Sign {
    /// Gets the index (within the same tile) of the directed edge or node this sign applies to.
    #[inline]
    pub fn edge_or_node_index(&self) -> u32 {
        self.bitfield.edge_or_node_index().get()
    }

    /// Gets the raw sign type.
    #[inline]
    pub const fn raw_sign_type(&self) -> u8 {
        self.bitfield.sign_type()
    }

    /// Gets the type of the sign.
    ///
    /// # Errors
    ///
    /// Returns an error if the tile holds a type that Valhalla 3.9.1 does not define.
    #[inline]
    pub fn sign_type(&self) -> Result<SignType, SectionDecodingError> {
        SignType::try_from_raw(self.raw_sign_type()).map_err(|value| {
            SectionDecodingError::UnknownValue {
                field: "sign type",
                value: u64::from(value),
            }
        })
    }

    /// Does this sign indicate a route number?
    ///
    /// Mainline overloads this flag:
    /// for a linguistic sign, it says the pronunciations belong to a node
    /// (rather than an edge),
    /// and for guidance view signs, it distinguishes the base and overlay images.
    #[inline]
    pub const fn is_route_num_type(&self) -> bool {
        self.bitfield.is_route_num_type() != 0
    }

    /// Is the sign text tagged?
    ///
    /// In Valhalla 3.9.1, only linguistic signs are tagged.
    #[inline]
    pub const fn is_text_tagged(&self) -> bool {
        self.bitfield.is_text_tagged() != 0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph_tile::{
        GraphTile, SectionDecodingError, SignInfo, TEST_GRAPH_TILE_L0, TileSections,
    };
    use zerocopy::FromBytes;

    /// Builds a sign the way mainline lays it out.
    pub(crate) fn sign(
        index: u32,
        sign_type: u8,
        route_num: bool,
        tagged: bool,
        offset: u32,
    ) -> Sign {
        let bits = index
            | u32::from(sign_type) << 22
            | u32::from(route_num) << 30
            | u32::from(tagged) << 31;
        let mut bytes = bits.to_le_bytes().to_vec();
        bytes.extend(offset.to_le_bytes());
        Sign::read_from_bytes(&bytes).unwrap()
    }

    #[test]
    fn test_parse_sign_count() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        assert_eq!(
            tile_view.signs.len(),
            tile_view.header.sign_count() as usize
        );
    }

    #[test]
    fn test_fixture_signs() {
        // Cross-checked with GraphTile::GetSigns in Valhalla 3.9.1 (see the section oracle test,
        // which covers every sign in the fixtures).
        let tile = &*TEST_GRAPH_TILE_L0;
        let signs = tile.get_signs(2723, false).unwrap();
        let texts = signs
            .iter()
            .filter(|s| s.sign_type().unwrap() == SignType::GuideToward)
            .map(SignInfo::text_lossy)
            .collect::<Vec<_>>();
        assert_eq!(texts, ["França", "Encamp", "Ordino", "La Massana"]);
        // Each guide sign has a linguistic sign giving its language (Catalan) and no pronunciation.
        let linguistic = signs
            .iter()
            .filter(|s| s.sign_type().unwrap() == SignType::Linguistic)
            .collect::<Vec<_>>();
        assert_eq!(linguistic.len(), 4);
        assert_eq!(linguistic[1].text(), b"\x09\x00\x0d");
        let record = linguistic[1].linguistic_records().next().unwrap().unwrap();
        assert_eq!(record.name_index(), 1);
        assert_eq!(record.language().unwrap().as_str(), "ca");

        let (signs, map) = tile.get_signs_with_linguistics(2723, false).unwrap();
        assert_eq!(signs.len(), 4);
        assert_eq!(map.len(), 4);
        // No node signs here.
        assert!(tile.get_signs(2723, true).unwrap().is_empty());

        // A route number sign
        let signs = tile.get_signs(1194, false).unwrap();
        assert_eq!(signs[0].text_lossy(), "CG-1");
        assert!(signs[0].is_route_number());

        // An index without signs
        assert!(tile.get_signs(0, false).unwrap().is_empty());
    }

    #[test]
    fn test_node_and_edge_filtering() {
        // Text list: "Exit\0", "Toll\0", then node and edge linguistic records.
        let mut text = b"Exit\0Toll\0".to_vec();
        let node_records = text.len();
        text.extend([17, 2, 1, b'a', b'b', 0]);
        let edge_records = text.len();
        text.extend([17, 0, 5, 0]);
        let signs = [
            sign(7, SignType::ExitName as u8, false, false, 0),
            sign(7, SignType::TollName as u8, false, false, 5),
            sign(
                7,
                SignType::Linguistic as u8,
                true,
                true,
                u32::try_from(node_records).unwrap(),
            ),
            sign(
                7,
                SignType::Linguistic as u8,
                false,
                true,
                u32::try_from(edge_records).unwrap(),
            ),
            // An unknown type counts as an edge sign, as in mainline.
            sign(7, 100, false, false, 0),
            sign(8, SignType::ExitName as u8, false, false, 0),
        ];
        let sections = TileSections {
            signs: &signs,
            turn_lanes: &[],
            admins: &[],
            complex_forward: &[],
            complex_reverse: &[],
            text_list: &text,
            lane_connectivity: &[],
        };

        let edge = sections.get_signs(7, false).unwrap();
        assert_eq!(
            edge.iter()
                .map(|s| s.sign().raw_sign_type())
                .collect::<Vec<_>>(),
            vec![3, 255, 100]
        );
        assert_eq!(edge[1].text(), &[17, 0, 5]);
        let node = sections.get_signs(7, true).unwrap();
        assert_eq!(
            node.iter()
                .map(|s| s.sign().raw_sign_type())
                .collect::<Vec<_>>(),
            vec![9, 255]
        );
        assert_eq!(node[0].text_lossy(), "Toll");
        assert_eq!(node[1].text(), &[17, 2, 1, b'a', b'b']);

        let (node, map) = sections.get_signs_with_linguistics(7, true).unwrap();
        assert_eq!(node.len(), 1);
        assert_eq!(map.get(0).unwrap().pronunciation(), b"ab");
        let (edge, map) = sections.get_signs_with_linguistics(7, false).unwrap();
        assert_eq!(edge.len(), 2);
        assert!(map.get(0).unwrap().pronunciation().is_empty());
        assert!(edge[1].sign_type().is_err());

        // A text offset past the end of the text list
        let bad = [sign(1, SignType::ExitName as u8, false, false, 1000)];
        let sections = TileSections {
            signs: &bad,
            ..sections
        };
        assert_eq!(
            sections.get_signs(1, true).unwrap_err(),
            SectionDecodingError::TextOffsetOutOfBounds {
                offset: 1000,
                len: text.len()
            }
        );
    }
}
