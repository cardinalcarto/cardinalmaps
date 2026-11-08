use super::sections::SectionDecodingError;
use bitfield_struct::bitfield;
use core::fmt::{self, Display, Formatter};
use enumset::{EnumSet, EnumSetType};
use zerocopy::{LE, U32};
use zerocopy_derive::{FromBytes, Immutable, IntoBytes, Unaligned};

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, IntoBytes, Immutable, Unaligned)]
struct EdgeIndex {
    #[bits(22, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    edge_index: U32<LE>,
    #[bits(10)]
    _spare: u16,
}

/// Holds turn lane information at the end of a directed edge (mainline's `TurnLanes`).
///
/// Turn lane text is stored in the graph tile text list
/// and the offset is stored within the [`TurnLane`] structure.
/// To get the lanes, use [`GraphTile::get_turn_lanes`](super::GraphTile::get_turn_lanes).
#[derive(FromBytes, IntoBytes, Immutable, Unaligned, Debug, Clone)]
#[repr(C)]
pub struct TurnLane {
    edge_index: EdgeIndex,
    /// The offset into the graph tile text list
    /// which contains the text for this turn lane.
    pub text_offset: U32<LE>,
}

impl TurnLane {
    /// Gets the index (within the same tile) of the directed edge that this sign applies to.
    #[inline]
    pub fn directed_edge_index(&self) -> u32 {
        self.edge_index.edge_index().get()
    }
}

/// A turn lane direction (one bit of a [`TurnLaneMask`]).
///
/// The order matches the bits of mainline's `kTurnLane*` constants.
#[derive(Debug, EnumSetType)]
#[enumset(repr = "u16")]
pub enum TurnLaneDirection {
    /// The lane is explicitly tagged `none`.
    None,
    Through,
    SharpLeft,
    Left,
    SlightLeft,
    SlightRight,
    Right,
    SharpRight,
    Reverse,
    MergeToLeft,
    MergeToRight,
}

impl TurnLaneDirection {
    /// The name mainline uses for the direction (for example, `slight_left`).
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Through => "through",
            Self::SharpLeft => "sharp_left",
            Self::Left => "left",
            Self::SlightLeft => "slight_left",
            Self::SlightRight => "slight_right",
            Self::Right => "right",
            Self::SharpRight => "sharp_right",
            Self::Reverse => "reverse",
            Self::MergeToLeft => "merge_to_left",
            Self::MergeToRight => "merge_to_right",
        }
    }
}

/// The directions of a single lane.
///
/// A mask of 0 means the lane has no turn information
/// (an empty entry in the OSM `turn:lanes` tag).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnLaneMask(u16);

impl TurnLaneMask {
    /// The raw mask.
    #[inline]
    pub const fn raw(&self) -> u16 {
        self.0
    }

    /// The directions in the mask.
    ///
    /// Bits beyond the defined directions are ignored, as in mainline.
    #[inline]
    pub fn directions(&self) -> EnumSet<TurnLaneDirection> {
        EnumSet::from_repr_truncated(self.0)
    }
}

/// The turn lanes at the end of a directed edge, left to right.
///
/// The text list stores them as decimal masks separated by `|` (for example, `8|2`),
/// which [`TurnLanes::masks`] decodes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TurnLanes<'a> {
    text: &'a [u8],
    offset: usize,
}

impl<'a> TurnLanes<'a> {
    #[inline]
    pub(crate) const fn new(text: &'a [u8], offset: usize) -> Self {
        Self { text, offset }
    }

    /// The raw text, such as `8|2`.
    #[inline]
    pub const fn text(&self) -> &'a [u8] {
        self.text
    }

    /// Iterates over the lane masks, as mainline's `TurnLanes::lanemasks` parses them.
    ///
    /// Like `std::getline`, a trailing `|` does not start another lane.
    /// (Mainline never writes one; it ends a trailing empty lane with an explicit `0`.)
    ///
    /// # Errors
    ///
    /// Yields an error for a lane that is not a decimal number that fits in 16 bits.
    /// (Mainline's `stoi` would throw for most of these.)
    pub fn masks(
        &self,
    ) -> impl Iterator<Item = Result<TurnLaneMask, SectionDecodingError>> + use<'a> {
        let text = self.text.strip_suffix(b"|").unwrap_or(self.text);
        let offset = self.offset;
        // An empty string has no lanes at all.
        text.split(|&b| b == b'|')
            .filter(move |_| !text.is_empty())
            .map(move |lane| {
                core::str::from_utf8(lane)
                    .ok()
                    .and_then(|lane| lane.parse::<u16>().ok())
                    .map(TurnLaneMask)
                    .ok_or(SectionDecodingError::MalformedText {
                        what: "turn lanes",
                        offset,
                        reason: "a lane is not a 16-bit decimal mask",
                    })
            })
    }

    /// Formats the lanes as mainline's `TurnLanes::turnlane_string` does
    /// (for example, `left|through;right`).
    ///
    /// # Errors
    ///
    /// Returns an error if a lane mask is malformed.
    pub fn to_mainline_string(&self) -> Result<String, SectionDecodingError> {
        let masks = self.masks().collect::<Result<Vec<_>, _>>()?;
        Ok(MainlineTurnLanes(&masks).to_string())
    }
}

/// Formats lane masks as mainline's `TurnLanes::turnlane_string` does.
struct MainlineTurnLanes<'a>(&'a [TurnLaneMask]);

impl Display for MainlineTurnLanes<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // Mainline appends a `|` after every lane and pops the last one;
        // an empty first lane is a lone `|` (with no second `|` after it).
        let mut out = String::new();
        for mask in self.0 {
            if out.is_empty() && mask.raw() == 0 {
                out.push('|');
            } else {
                let mut first = true;
                for direction in mask.directions() {
                    if !first {
                        out.push(';');
                    }
                    out.push_str(direction.as_str());
                    first = false;
                }
                out.push('|');
            }
        }
        out.pop();
        f.write_str(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};

    #[test]
    fn test_parse_turn_lane_count() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        assert_eq!(
            tile_view.turn_lanes.len(),
            tile.header().turn_lane_count() as usize
        );
    }

    #[test]
    fn test_parse_turn_lanes() {
        let tile = &*TEST_GRAPH_TILE_L0;
        let tile_view = tile.borrow_dependent();

        // insta internally does a fork operation, which is not supported under Miri
        if !cfg!(miri) {
            insta::assert_debug_snapshot!(tile_view.turn_lanes);
        }
    }

    #[test]
    fn test_get_turn_lanes() {
        // Cross-checked with GraphTile::turnlanes and TurnLanes::turnlane_string
        // in Valhalla 3.9.1 (the section oracle test covers every turn lane in the fixtures).
        let tile = &*TEST_GRAPH_TILE_L0;
        let lanes = tile.get_turn_lanes(2215).unwrap().unwrap();
        assert_eq!(lanes.text(), b"2|66");
        assert_eq!(lanes.to_mainline_string().unwrap(), "through|through;right");
        let masks = lanes.masks().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(
            masks[1].directions(),
            TurnLaneDirection::Through | TurnLaneDirection::Right
        );
        assert_eq!(
            tile.get_turn_lanes(2865)
                .unwrap()
                .unwrap()
                .to_mainline_string()
                .unwrap(),
            "|merge_to_left"
        );
        assert_eq!(
            tile.get_turn_lanes(1487)
                .unwrap()
                .unwrap()
                .to_mainline_string()
                .unwrap(),
            "merge_to_right|"
        );
        // Mainline would return the next edge's lanes here; we return none.
        assert!(tile.get_turn_lanes(2214).unwrap().is_none());
        assert!(tile.get_turn_lanes(u32::MAX).unwrap().is_none());
    }

    fn mainline_string(text: &[u8]) -> Result<String, SectionDecodingError> {
        TurnLanes::new(text, 0).to_mainline_string()
    }

    #[test]
    fn test_mainline_string() {
        assert_eq!(mainline_string(b"").unwrap(), "");
        assert_eq!(mainline_string(b"0").unwrap(), "");
        assert_eq!(mainline_string(b"0|0").unwrap(), "|");
        assert_eq!(mainline_string(b"1|1024").unwrap(), "none|merge_to_right");
        assert_eq!(mainline_string(b"2047").unwrap().split(';').count(), 11);
        // Unknown bits are dropped, leaving an empty lane.
        assert_eq!(mainline_string(b"2048|2").unwrap(), "|through");
        // A trailing delimiter does not add a lane.
        assert_eq!(mainline_string(b"8|").unwrap(), "left");
        assert!(mainline_string(b"8||2").is_err());
        assert!(mainline_string(b"x").is_err());
        assert!(mainline_string(b"70000").is_err());
    }
}
