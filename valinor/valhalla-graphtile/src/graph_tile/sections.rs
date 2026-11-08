//! # Variable-length tile sections
//!
//! A graph tile has several sections whose records point into other sections
//! or have no fixed size:
//! signs, turn lanes, and admins point into the text list,
//! complex restrictions carry a variable number of vias,
//! and lane connectivity has no count in the header.
//! [`TileSections`] bundles borrowed views of these,
//! and implements the lookups that mainline Valhalla's `GraphTile` offers on top of them
//! (`GetSigns`, `turnlanes`, `GetComplexRestrictions`, `GetLaneConnectivity`, and `admininfo`).
//! The [`GraphTile`](super::GraphTile) trait exposes the same lookups as default methods.
//!
//! Every lookup checks offsets and sizes against the tile data,
//! so a corrupt or truncated tile produces a [`SectionDecodingError`] rather than a panic.

use super::complex_restriction::{ComplexRestriction, ComplexRestrictions};
use super::lane_connectivity::LaneConnectivity;
use super::linguistic::{LinguisticMap, LinguisticRecords};
use super::{Admin, GraphId, Sign, SignType, TurnLane, TurnLanes};
use crate::{Access, AsCowStr};
use enumset::EnumSet;
use std::borrow::Cow;
use thiserror::Error;
use zerocopy::FromBytes;

/// An error decoding one of the variable-length tile sections
/// (edge info, the text list, signs, turn lanes, complex restrictions, or lane connectivity).
///
/// These always indicate a corrupt or truncated tile
/// (or a tile written by a newer, incompatible version of Valhalla).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SectionDecodingError {
    /// A record points past the end of the text list.
    #[error("read text at offset {offset}: the text list is only {len} bytes long")]
    TextOffsetOutOfBounds { offset: usize, len: usize },
    /// A string in the text list runs off the end of the text list.
    #[error(
        "read text at offset {offset}: there is no NUL terminator before the end of the text list"
    )]
    UnterminatedText { offset: usize },
    /// A structured value in the text list is malformed.
    #[error("decode the {what} at text offset {offset}: {reason}")]
    MalformedText {
        what: &'static str,
        offset: usize,
        reason: &'static str,
    },
    /// A fixed-size or variable-length record runs past the end of its section.
    #[error("decode the {section} section at byte {offset}: {reason}")]
    MalformedSection {
        section: &'static str,
        offset: usize,
        reason: &'static str,
    },
    /// A field holds a value that has no meaning in Valhalla 3.9.1.
    #[error("decode the {field} field: {value} is not a known value")]
    UnknownValue { field: &'static str, value: u64 },
    /// An index is past the end of a section.
    #[error("look up {section} {index}: the tile only has {len}")]
    IndexOutOfBounds {
        section: &'static str,
        index: usize,
        len: usize,
    },
}

/// Returns the bytes of the NUL-terminated string at `offset` in the text list,
/// without the terminator.
#[inline]
pub(crate) fn text_at(text_list: &[u8], offset: usize) -> Result<&[u8], SectionDecodingError> {
    let tail = text_tail(text_list, offset)?;
    let end = tail
        .iter()
        .position(|&b| b == 0)
        .ok_or(SectionDecodingError::UnterminatedText { offset })?;
    Ok(&tail[..end])
}

/// Returns all bytes of the text list from `offset` on.
#[inline]
pub(crate) fn text_tail(text_list: &[u8], offset: usize) -> Result<&[u8], SectionDecodingError> {
    // Mainline requires the offset to be strictly inside the list (`offset < textlist_size_`).
    if offset < text_list.len() {
        Ok(&text_list[offset..])
    } else {
        Err(SectionDecodingError::TextOffsetOutOfBounds {
            offset,
            len: text_list.len(),
        })
    }
}

/// Borrowed views of a tile's variable-length sections,
/// with the lookups that depend on them.
///
/// Get one from [`GraphTile::sections`](super::GraphTile::sections).
/// It is cheap to copy (just a handful of slices).
#[derive(Clone, Copy)]
pub struct TileSections<'a> {
    pub(super) signs: &'a [Sign],
    pub(super) turn_lanes: &'a [TurnLane],
    pub(super) admins: &'a [Admin],
    pub(super) complex_forward: &'a [u8],
    pub(super) complex_reverse: &'a [u8],
    pub(super) text_list: &'a [u8],
    pub(super) lane_connectivity: &'a [u8],
}

impl core::fmt::Debug for TileSections<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The raw byte sections are large and unreadable; their sizes are what matter.
        f.debug_struct("TileSections")
            .field("signs", &self.signs.len())
            .field("turn_lanes", &self.turn_lanes.len())
            .field("admins", &self.admins.len())
            .field("complex_forward_bytes", &self.complex_forward.len())
            .field("complex_reverse_bytes", &self.complex_reverse.len())
            .field("text_list_bytes", &self.text_list.len())
            .field("lane_connectivity_bytes", &self.lane_connectivity.len())
            .finish()
    }
}

/// A sign as mainline's `GraphTile::GetSigns` returns it:
/// the sign record plus its text.
#[derive(Clone, Copy, Debug)]
pub struct SignInfo<'a> {
    sign: &'a Sign,
    text: &'a [u8],
}

impl<'a> SignInfo<'a> {
    /// The underlying sign record.
    #[inline]
    pub const fn sign(&self) -> &'a Sign {
        self.sign
    }

    /// The type of the sign.
    ///
    /// # Errors
    ///
    /// Returns an error if the tile holds a sign type that Valhalla 3.9.1 does not define.
    #[inline]
    pub fn sign_type(&self) -> Result<SignType, SectionDecodingError> {
        self.sign.sign_type()
    }

    /// Whether the sign text is a route number (for example, `CG-1`).
    ///
    /// For linguistic signs, this flag instead says whether the pronunciations
    /// belong to a node (junction or toll names) rather than an edge.
    #[inline]
    pub const fn is_route_number(&self) -> bool {
        self.sign.is_route_num_type()
    }

    /// Whether the sign text is tagged.
    ///
    /// In Valhalla 3.9.1, only linguistic signs are tagged.
    #[inline]
    pub const fn is_tagged(&self) -> bool {
        self.sign.is_text_tagged()
    }

    /// The raw sign text, without a NUL terminator.
    ///
    /// For a linguistic sign, this is the sequence of linguistic records
    /// (each a 3-byte header followed by a pronunciation),
    /// which is exactly what mainline puts in `SignInfo::text()`.
    /// Use [`SignInfo::linguistic_records`] to decode them.
    #[inline]
    pub const fn text(&self) -> &'a [u8] {
        self.text
    }

    /// The sign text as a string (lossy if the tile holds invalid UTF-8).
    #[inline]
    pub fn text_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.text)
    }

    /// Decodes the linguistic records of a linguistic sign.
    ///
    /// For other signs, the result is meaningless.
    #[inline]
    pub fn linguistic_records(&self) -> LinguisticRecords<'a> {
        LinguisticRecords::new(self.text, 0)
    }
}

/// Administrative area information, with the names resolved from the text list.
///
/// This is the equivalent of mainline's `AdminInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminInfo<'a> {
    /// The country name.
    pub country_text: Cow<'a, str>,
    /// The principal subdivision (state) name.
    pub state_text: Cow<'a, str>,
    /// The ISO 3166-1 country code.
    pub country_iso: Cow<'a, str>,
    /// The principal subdivision part of the ISO 3166-2 code.
    pub state_iso: Cow<'a, str>,
}

/// Is this sign type one that mainline attaches to a node rather than an edge?
const fn is_node_sign_type(sign_type: Result<SignType, u8>) -> bool {
    matches!(sign_type, Ok(SignType::JunctionName | SignType::TollName))
}

impl<'a> TileSections<'a> {
    /// The tile's signs, sorted by edge or node index.
    #[inline]
    pub const fn signs(&self) -> &'a [Sign] {
        self.signs
    }

    /// The tile's turn lane records, sorted by directed edge index.
    #[inline]
    pub const fn turn_lanes(&self) -> &'a [TurnLane] {
        self.turn_lanes
    }

    /// The raw text list.
    #[inline]
    pub const fn text_list(&self) -> &'a [u8] {
        self.text_list
    }

    /// Gets the NUL-terminated string at an offset in the text list
    /// (mainline's `GraphTile::GetName`), without the terminator.
    ///
    /// # Errors
    ///
    /// Returns an error if the offset is outside the text list
    /// or the string is not terminated.
    #[inline]
    pub fn text(&self, offset: u32) -> Result<&'a [u8], SectionDecodingError> {
        text_at(self.text_list, offset as usize)
    }

    /// The sign records for an edge or node index (a contiguous run, since they are sorted).
    ///
    /// The index is an edge index for edge signs
    /// and a node index for junction and toll names;
    /// both kinds share this list.
    #[inline]
    pub fn signs_for_index(&self, index: u32) -> &'a [Sign] {
        let start = self
            .signs
            .partition_point(|s| s.edge_or_node_index() < index);
        let len = self.signs[start..]
            .iter()
            .take_while(|s| s.edge_or_node_index() == index)
            .count();
        &self.signs[start..start + len]
    }

    /// Resolves a sign's text.
    fn sign_text(&self, sign: &'a Sign) -> Result<&'a [u8], SectionDecodingError> {
        let offset = sign.text_offset.get() as usize;
        if sign.raw_sign_type() == SignType::Linguistic as u8 {
            // Linguistic records are length-prefixed and may contain NUL bytes
            // (a record with no pronunciation has a zero length byte),
            // so walk the records rather than looking for the first NUL.
            let tail = text_tail(self.text_list, offset)?;
            let mut records = LinguisticRecords::new(tail, offset);
            for record in &mut records {
                record?;
            }
            Ok(&tail[..records.consumed()])
        } else {
            text_at(self.text_list, offset)
        }
    }

    /// Gets the signs for a directed edge (or node) index,
    /// as mainline's `GraphTile::GetSigns(idx, signs_on_node)` does.
    ///
    /// With `signs_on_node` false, this returns the edge signs
    /// (everything except junction and toll names) and the edge pronunciations;
    /// with it true, the junction and toll names and the node pronunciations.
    /// Linguistic signs are returned as signs whose text is the raw linguistic records.
    ///
    /// # Errors
    ///
    /// Returns an error if a sign's text is outside the text list or malformed.
    pub fn get_signs(
        &self,
        index: u32,
        signs_on_node: bool,
    ) -> Result<Vec<SignInfo<'a>>, SectionDecodingError> {
        let mut result = Vec::new();
        for sign in self.signs_for_index(index) {
            let sign_type = SignType::try_from_raw(sign.raw_sign_type());
            let is_linguistic = matches!(sign_type, Ok(SignType::Linguistic));
            let is_node_sign = is_node_sign_type(sign_type);
            // A linguistic sign's route number flag says whether it belongs to a node.
            let on_node = is_node_sign || (is_linguistic && sign.is_route_num_type());
            let on_edge =
                (!is_node_sign && !is_linguistic) || (is_linguistic && !sign.is_route_num_type());
            // Mainline checks the text offset of every sign, but only reads the included ones.
            text_tail(self.text_list, sign.text_offset.get() as usize)?;
            if (on_node && signs_on_node) || (on_edge && !signs_on_node) {
                let text = self.sign_text(sign)?;
                result.push(SignInfo { sign, text });
            }
        }
        Ok(result)
    }

    /// Gets the signs for a directed edge (or node) index along with their pronunciations,
    /// as mainline's `GraphTile::GetSigns(idx, index_linguistic_map, signs_on_node)` does.
    ///
    /// Unlike [`TileSections::get_signs`],
    /// linguistic signs are not returned as signs;
    /// their records are collected into the map, keyed by the index of the sign they describe
    /// (among all signs on the edge or node, in tile order).
    ///
    /// # Errors
    ///
    /// Returns an error if a sign's text is outside the text list or malformed.
    pub fn get_signs_with_linguistics(
        &self,
        index: u32,
        signs_on_node: bool,
    ) -> Result<(Vec<SignInfo<'a>>, LinguisticMap<'a>), SectionDecodingError> {
        let mut result = Vec::new();
        let mut map = LinguisticMap::default();
        for sign in self.signs_for_index(index) {
            let sign_type = SignType::try_from_raw(sign.raw_sign_type());
            let offset = sign.text_offset.get() as usize;
            if sign.is_text_tagged() && matches!(sign_type, Ok(SignType::Linguistic)) {
                let tail = text_tail(self.text_list, offset)?;
                if sign.is_route_num_type() == signs_on_node {
                    for record in LinguisticRecords::new(tail, offset) {
                        map.insert(record?);
                    }
                }
                continue;
            }

            text_tail(self.text_list, offset)?;
            if is_node_sign_type(sign_type) == signs_on_node {
                // Note that this text is the plain C string even for an untagged linguistic sign;
                // mainline only walks the records of tagged ones.
                let text = text_at(self.text_list, offset)?;
                result.push(SignInfo { sign, text });
            }
        }
        Ok((result, map))
    }

    /// Gets the turn lanes at the end of a directed edge.
    ///
    /// Returns `None` if the edge has no turn lanes.
    ///
    /// Mainline's `GraphTile::turnlanes_offset` returns the text of the first record
    /// whose edge index is *at least* the requested one,
    /// which is only correct when the edge has turn lanes;
    /// mainline callers check `DirectedEdge::turnlanes()` first.
    /// This lookup requires an exact match instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the turn lane text is outside the text list.
    pub fn get_turn_lanes(
        &self,
        directed_edge_index: u32,
    ) -> Result<Option<TurnLanes<'a>>, SectionDecodingError> {
        let start = self
            .turn_lanes
            .partition_point(|tl| tl.directed_edge_index() < directed_edge_index);
        match self.turn_lanes.get(start) {
            Some(tl) if tl.directed_edge_index() == directed_edge_index => {
                let offset = tl.text_offset.get();
                // Mainline treats offset 0 as "no turn lanes".
                if offset == 0 {
                    return Ok(None);
                }
                Ok(Some(TurnLanes::new(self.text(offset)?, offset as usize)))
            }
            _ => Ok(None),
        }
    }

    /// Iterates over all forward complex restrictions in the tile.
    ///
    /// Forward restrictions are stored in the tile of their *to* edge,
    /// for use by forward path searches.
    #[inline]
    pub fn forward_complex_restrictions(&self) -> ComplexRestrictions<'a> {
        ComplexRestrictions::new(self.complex_forward, "complex_forward_restrictions")
    }

    /// Iterates over all reverse complex restrictions in the tile.
    ///
    /// Reverse restrictions are stored in the tile of their *from* edge,
    /// for use by reverse path searches.
    #[inline]
    pub fn reverse_complex_restrictions(&self) -> ComplexRestrictions<'a> {
        ComplexRestrictions::new(self.complex_reverse, "complex_reverse_restrictions")
    }

    /// Iterates over the complex restrictions that apply to an edge and any of the given modes,
    /// as mainline's `GraphTile::GetComplexRestrictions(forward, id, modes)` does.
    ///
    /// Forward restrictions match on their *to* edge, and reverse restrictions on their *from* edge.
    ///
    /// # Errors
    ///
    /// The iterator yields an error (and then stops) if the section is truncated.
    ///
    /// # Performance
    ///
    /// Like mainline, this scans the whole section, which is usually tiny.
    #[inline]
    pub fn get_complex_restrictions(
        &self,
        forward: bool,
        edge_id: GraphId,
        modes: EnumSet<Access>,
    ) -> impl Iterator<Item = Result<ComplexRestriction<'a>, SectionDecodingError>> + use<'a> {
        let restrictions = if forward {
            self.forward_complex_restrictions()
        } else {
            self.reverse_complex_restrictions()
        };
        restrictions.filter(move |r| match r {
            Ok(r) => {
                let id = if forward {
                    r.raw_to_graph_id()
                } else {
                    r.raw_from_graph_id()
                };
                id == edge_id.value() && !r.modes().is_disjoint(modes)
            }
            // Always surface errors.
            Err(_) => true,
        })
    }

    /// All lane connectivity records in the tile, sorted by *to* edge index.
    ///
    /// The header has no record count;
    /// like mainline, we take as many whole records as fit in the section
    /// and ignore the rest.
    /// That remainder is not an error:
    /// the lane connectivity section runs to the end of the tile when there are no predicted speeds,
    /// and mainline pads the end of the tile to a multiple of 8 bytes
    /// (for example, after adding 4-byte bounding circles).
    #[inline]
    pub fn lane_connectivity(&self) -> &'a [LaneConnectivity] {
        let count = self.lane_connectivity.len() / size_of::<LaneConnectivity>();
        // With an unaligned type and a count that fits, this cannot fail.
        <[LaneConnectivity]>::ref_from_prefix_with_elems(self.lane_connectivity, count)
            .map_or(&[], |(records, _padding)| records)
    }

    /// Gets the lane connections that end on a directed edge,
    /// as mainline's `GraphTile::GetLaneConnectivity` does.
    #[inline]
    pub fn get_lane_connectivity(&self, directed_edge_index: u32) -> &'a [LaneConnectivity] {
        let all = self.lane_connectivity();
        let start = all.partition_point(|lc| lc.to() < directed_edge_index);
        let len = all[start..]
            .iter()
            .take_while(|lc| lc.to() == directed_edge_index)
            .count();
        &all[start..start + len]
    }

    /// Gets the admin at an index with its names resolved,
    /// as mainline's `GraphTile::admininfo` does.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is out of bounds
    /// or the names are outside the text list.
    pub fn get_admin_info(&self, index: usize) -> Result<AdminInfo<'a>, SectionDecodingError> {
        let admin = self
            .admins
            .get(index)
            .ok_or(SectionDecodingError::IndexOutOfBounds {
                section: "admins",
                index,
                len: self.admins.len(),
            })?;
        Ok(AdminInfo {
            country_text: self.text(admin.country_name_offset.get())?.as_cow_str(),
            state_text: self
                .text(admin.principal_subdivision_offset.get())?
                .as_cow_str(),
            country_iso: admin.country_iso(),
            state_iso: admin.principal_subdivision_iso(),
        })
    }
}
