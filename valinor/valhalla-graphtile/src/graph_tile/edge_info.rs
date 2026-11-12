use super::linguistic::{LinguisticMap, LinguisticRecord};
use super::sections::{SectionDecodingError, text_at, text_tail};
use super::tagged_value::{Landmark, Levels, OsmNodeIds, TaggedValue, TaggedValueKind};
use super::time_domain::ConditionalSpeedLimit;
use crate::{BicycleNetwork, graph_tile::GraphTileDecodingError, shape_codec::decode_shape};
use bitfield_struct::bitfield;
use enumset::EnumSet;
use geo::{Coord, CoordFloat};
use num_traits::FromPrimitive;
use std::borrow::Cow;
use zerocopy::{FromBytes as _, LE, U16, U32};
use zerocopy_derive::{FromBytes, Immutable, KnownLayout, Unaligned};

use crate::shape_codec::decode_first_coordinate;
#[cfg(feature = "serde")]
use serde::{Serialize, Serializer, ser::SerializeStruct};

/// The elevation of a stored mean elevation of 0, which mainline treats as "no data"
/// (`kNoElevationData`, also `kMinElevation`).
const MIN_ELEVATION: f32 = -500.0;

/// The size of a mean elevation bin, in meters.
const ELEVATION_BIN_SIZE: f32 = 2.0;

/// The maximum sampling interval for elevation along an edge, in meters
/// (`kMaxEdgeElevationSampling`).
const MAX_ELEVATION_SAMPLING: u32 = 32;

/// The precision of encoded elevation deltas, in meters (`kElevationPrecision`).
const ELEVATION_PRECISION: f32 = 0.25;

/// Information about a name in the edge's name list (mainline's `NameInfo`).
#[bitfield(u32,
    repr = U32<LE>,
    debug = false,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, Immutable, Unaligned)]
pub struct NameInfo {
    #[bits(24, from = bit_twiddling_helpers::conv_u32le::from_inner, into = bit_twiddling_helpers::conv_u32le::into_inner)]
    raw_name_offset: U32<LE>,
    /// Additional fields following the name (unused in Valhalla 3.9.1).
    #[bits(4)]
    raw_additional_fields: u8,
    #[bits(1)]
    raw_is_route_num: u8,
    /// Indicates the string is specially tagged (the first byte is the tag type;
    /// see [`TaggedValueKind`]).
    ///
    /// This doesn't have any relation to OSM tagging.
    #[bits(1)]
    raw_is_tagged: u8,
    #[bits(2)]
    _spare: u8,
}

impl NameInfo {
    /// The offset of the name (or tagged value) in the text list.
    #[inline]
    pub fn text_offset(&self) -> u32 {
        self.raw_name_offset().get()
    }

    /// Whether the name is a route number (for example, `CG-1`) rather than a name.
    #[inline]
    pub const fn is_route_number(&self) -> bool {
        self.raw_is_route_num() != 0
    }

    /// Whether this entry is a tagged value rather than a name.
    #[inline]
    pub const fn is_tagged(&self) -> bool {
        self.raw_is_tagged() != 0
    }
}

impl core::fmt::Debug for NameInfo {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NameInfo")
            .field("text_offset", &self.text_offset())
            .field("is_route_number", &self.is_route_number())
            .field("is_tagged", &self.is_tagged())
            .finish()
    }
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, Immutable, Unaligned, KnownLayout)]
struct FirstInnerBitfield {
    #[bits(12, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    mean_elevation: U16<LE>,
    #[bits(4)]
    bike_network: u8,
    #[bits(8)]
    speed_limit: u8,
    #[bits(8)]
    extended_way_id: u8,
}

#[bitfield(u32,
    repr = U32<LE>,
    from = bit_twiddling_helpers::conv_u32le::from_inner,
    into = bit_twiddling_helpers::conv_u32le::into_inner
)]
#[derive(FromBytes, Immutable, Unaligned, KnownLayout)]
struct SecondInnerBitfield {
    #[bits(4)]
    name_count: u8,
    #[bits(16, from = bit_twiddling_helpers::conv_u16le::from_inner, into = bit_twiddling_helpers::conv_u16le::into_inner)]
    encoded_shape_size: U16<LE>,
    #[bits(8)]
    extended_way_id: u8,
    #[bits(2)]
    extended_way_id_size: u8,
    #[bits(1)]
    has_elevation: u8,
    #[bits(1)]
    _spare: u8,
}

#[derive(Debug, FromBytes, Immutable, Unaligned, KnownLayout)]
#[repr(C)]
struct EdgeInfoInner {
    // The first part of the OSM way ID
    // TODO: would be nice to reshuffle this in v4.0
    way_id: U32<LE>,
    first_inner_bitfield: FirstInnerBitfield,
    second_inner_bitfield: SecondInnerBitfield,
}

/// A name of an edge.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct EdgeName<'a> {
    text: &'a [u8],
    is_route_number: bool,
    tag: u8,
}

impl core::fmt::Debug for EdgeName<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EdgeName")
            .field("text", &self.text_lossy())
            .field("is_route_number", &self.is_route_number)
            .field("tag", &self.tag)
            .finish()
    }
}

impl<'a> EdgeName<'a> {
    /// The raw name bytes.
    #[inline]
    pub const fn text(&self) -> &'a [u8] {
        self.text
    }

    /// The name as a string (lossy if the tile holds invalid UTF-8).
    #[inline]
    pub fn text_lossy(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.text)
    }

    /// Whether the name is a route number (for example, `CG-1`).
    #[inline]
    pub const fn is_route_number(&self) -> bool {
        self.is_route_number
    }

    /// The tag byte of a tagged name, or 0 for a plain name
    /// (the third element of mainline's `GetNamesAndTypes` tuples).
    #[inline]
    pub const fn tag(&self) -> u8 {
        self.tag
    }
}

/// Elevation samples along an edge, as offsets from the previous sample.
///
/// The elevations at the two ends are stored in the nodes, not here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodedElevation<'a> {
    interval: f64,
    deltas: &'a [i8],
}

impl<'a> EncodedElevation<'a> {
    /// The distance between samples, in meters.
    ///
    /// When the edge info has no elevation (the edge is too short to sample),
    /// this is the edge length, as in mainline.
    #[inline]
    pub const fn interval(&self) -> f64 {
        self.interval
    }

    /// The elevation offsets, in units of 0.25 meters,
    /// from the start of the edge info's (forward) shape.
    #[inline]
    pub const fn deltas(&self) -> &'a [i8] {
        self.deltas
    }

    /// Decodes the elevation along the edge, given the elevations at its start and end nodes,
    /// as mainline's `decode_elevation` does.
    ///
    /// `start` and `end` are the elevations of the directed edge's start and end nodes,
    /// and `forward` is [`DirectedEdge::edge_info_is_forward`](super::DirectedEdge::edge_info_is_forward).
    /// The result runs in the direction of the directed edge,
    /// with `start` first and `end` last.
    pub fn decode(&self, start: f32, end: f32, forward: bool) -> Vec<f32> {
        let (first, last) = if forward { (start, end) } else { (end, start) };
        let mut elevation = Vec::with_capacity(self.deltas.len() + 2);
        elevation.push(first);
        let mut current = first;
        for &delta in self.deltas {
            current += f32::from(delta) * ELEVATION_PRECISION;
            elevation.push(current);
        }
        // The last sample is the end node's elevation, not the sum of the deltas.
        elevation.push(last);
        if !forward {
            elevation.reverse();
        }
        elevation
    }
}

/// Edge information that isn't required during path finding.
///
/// This includes things like road names, the OSM way ID, and geometry
/// which would be a waste of space in the [`DirectedEdge`](super::DirectedEdge).
///
/// The directed edge graph always has an opposing edge to aid in graph traversal,
/// and the access conditions and other attributes of these are not always the same.
/// However, the edge info _is_ always the same, so it's only stored once per directed edge pair.
///
/// Because of this, some properties (like the shape) depend on which direction we're going.
/// This is captured in [`DirectedEdge::edge_info_is_forward`](super::DirectedEdge::edge_info_is_forward).
///
/// # Names and tagged values
///
/// The name list holds both names and *tagged values*
/// (strings whose first byte says what they are; see [`TaggedValue`]).
/// [`EdgeInfo::names`] returns only the names, like mainline's `GetNames`;
/// the tagged values have their own accessors.
///
/// Name and tagged value accessors decode lazily and check every offset,
/// so they return errors for corrupt tiles.
pub struct EdgeInfo<'a> {
    inner: &'a EdgeInfoInner,
    name_info_list: &'a [NameInfo],
    encoded_shape: &'a [u8],
    // Last 2 bytes of the 64-bit way ID
    extended_way_id_2: u8,
    extended_way_id_3: u8,
    /// Everything after the fixed fields: the encoded elevation, then the next edge info.
    /// (The elevation has no stored size; it depends on the directed edge's length.)
    trailing: &'a [u8],
    text_list_memory: &'a [u8],
}

impl<'a> EdgeInfo<'a> {
    /// Gets the tagged speed limit along this edge (in kph).
    ///
    /// 0 means there is no tagged limit, and 255 means there is no limit
    /// (mainline's `kUnlimitedSpeedLimit`).
    #[inline]
    pub const fn speed_limit(&self) -> u8 {
        self.inner.first_inner_bitfield.speed_limit()
    }

    /// The mean elevation along the edge, in meters (with 2 meter precision),
    /// or `None` if there is no elevation data.
    #[inline]
    pub fn mean_elevation(&self) -> Option<f32> {
        match self.inner.first_inner_bitfield.mean_elevation().get() {
            0 => None,
            bin => Some(MIN_ELEVATION + f32::from(bin) * ELEVATION_BIN_SIZE),
        }
    }

    /// Whether the edge info has elevation samples along the edge.
    ///
    /// Short edges (under 32 meters) have none,
    /// nor do edges whose end elevations are missing.
    #[inline]
    pub const fn has_elevation(&self) -> bool {
        self.inner.second_inner_bitfield.has_elevation() != 0
    }

    /// Gets the elevation samples along the edge,
    /// as mainline's `EdgeInfo::encoded_elevation` does.
    ///
    /// The number of samples depends on the edge length
    /// (`length` is [`DirectedEdge::length`](super::DirectedEdge::length);
    /// both directions have the same length):
    /// the edge is split into equal intervals of at most 32 meters,
    /// and there is a sample at each interior split point.
    /// Without elevation, there are no samples, and the interval is the edge length.
    ///
    /// # Errors
    ///
    /// Returns an error if the samples run past the end of the edge info section.
    pub fn encoded_elevation(
        &self,
        length: u32,
    ) -> Result<EncodedElevation<'a>, SectionDecodingError> {
        if !self.has_elevation() {
            return Ok(EncodedElevation {
                interval: f64::from(length),
                deltas: &[],
            });
        }
        let (interval, count) = elevation_sampling(length);
        let count = count as usize;
        let deltas = self
            .trailing
            .get(..count)
            .and_then(|bytes| <[i8]>::ref_from_bytes(bytes).ok())
            .ok_or(SectionDecodingError::MalformedSection {
                section: "edge_info",
                offset: count,
                reason: "the encoded elevation runs past the end of the section",
            })?;
        Ok(EncodedElevation { interval, deltas })
    }

    /// The raw varint-encoded shape bytes.
    #[inline]
    pub const fn encoded_shape(&self) -> &'a [u8] {
        self.encoded_shape
    }

    /// Decodes the geometry for an edge pair.
    ///
    /// # Directionality
    ///
    /// Edges exist in both forward and backward variants in the directed graph,
    /// If your use case depends on the coordinates being in a particular order
    /// (e.g. generating a full-route step profile / polyline),
    /// you may need to reverse the result.
    ///
    /// This method always returns the "raw" (forward) coordinate order.
    /// If [`DirectedEdge::edge_info_is_forward`](super::DirectedEdge::edge_info_is_forward)
    /// is `false`, then you need to reverse the result if your use is order-dependent.
    ///
    /// # Errors
    ///
    /// See [`decode_shape`] for a description of possible errors.
    ///
    /// # Performance
    ///
    /// This requires decoding the shape from its packed representation (varint).
    /// If expect to reuse geometries many times, you may want to cache the decoded geometries.
    #[inline]
    pub fn decode_raw_shape<T: CoordFloat + FromPrimitive>(
        &self,
    ) -> std::io::Result<Vec<Coord<T>>> {
        decode_shape(self.encoded_shape)
    }

    /// Decodes the _first_ coordinate of the geometry for an edge pair.
    ///
    /// # Directionality
    ///
    /// The same notes for [`EdgeInfo::decode_raw_shape`] apply.
    /// This means that if you are trying to get the _starting_ coordinate for a directed edge,
    /// you must first check whether the edge info is forward or reverse.
    ///
    /// # Errors
    ///
    /// See [`decode_shape`] for a description of possible errors.
    ///
    /// # Performance
    ///
    /// This is going to be pretty fast compared to [`EdgeInfo::decode_raw_shape`],
    /// consisting of basically 2 memory accesses and 2 varint decodes.
    #[inline]
    pub fn decode_first_coordinate<T: CoordFloat + FromPrimitive>(
        &self,
    ) -> std::io::Result<Coord<T>> {
        decode_first_coordinate(self.encoded_shape)
    }

    /// The raw name list (names and tagged values).
    #[inline]
    pub const fn name_infos(&self) -> &'a [NameInfo] {
        self.name_info_list
    }

    /// A bit mask of which entries in the name list are route numbers
    /// (bit `i` for entry `i`), as mainline's `GetTypes` returns it.
    #[inline]
    pub fn types(&self) -> u16 {
        self.name_info_list
            .iter()
            .zip(0u32..)
            .fold(0, |acc, (ni, i)| acc | u16::from(ni.is_route_number()) << i)
    }

    /// Iterates over the names of the edge (excluding tagged values),
    /// as mainline's `GetNames` and `GetNamesAndTypes(false)` return them.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a name outside the text list.
    #[inline]
    pub fn names(
        &self,
    ) -> impl Iterator<Item = Result<EdgeName<'a>, SectionDecodingError>> + use<'a> {
        self.names_and_types(false)
    }

    /// Iterates over the names of the edge,
    /// optionally including the non-linguistic tagged values,
    /// as mainline's `GetNamesAndTypes(include_tagged_values)` returns them.
    ///
    /// A tagged value is returned with its tag (see [`EdgeName::tag`]),
    /// and its text is the C string after the tag byte.
    /// That is only meaningful for the string kinds (like tunnel names);
    /// for binary kinds (like levels), it is whatever precedes the first NUL byte,
    /// exactly as in mainline.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for an entry outside the text list.
    pub fn names_and_types(
        &self,
        include_tagged_values: bool,
    ) -> impl Iterator<Item = Result<EdgeName<'a>, SectionDecodingError>> + use<'a> {
        let text_list = self.text_list_memory;
        self.name_info_list.iter().filter_map(move |ni| {
            let offset = ni.text_offset() as usize;
            if !ni.is_tagged() {
                return Some(text_at(text_list, offset).map(|text| EdgeName {
                    text,
                    is_route_number: ni.is_route_number(),
                    tag: 0,
                }));
            }
            if !include_tagged_values {
                return None;
            }
            let tag = match text_tail(text_list, offset) {
                Ok(tail) => tail[0],
                Err(e) => return Some(Err(e)),
            };
            if tag == TaggedValueKind::Linguistic as u8 {
                return None;
            }
            Some(text_at(text_list, offset + 1).map(|text| EdgeName {
                text,
                is_route_number: false,
                tag,
            }))
        })
    }

    /// Gets all names for this edge, lossily converted to strings.
    ///
    /// Names outside the text list are skipped;
    /// use [`EdgeInfo::names`] to see those errors.
    pub fn get_names(&self) -> Vec<Cow<'a, str>> {
        self.names()
            .filter_map(Result::ok)
            .map(|name| name.text_lossy())
            .collect()
    }

    /// Iterates over every tagged value in the name list, in name list order,
    /// including linguistic values and values with tags Valhalla 3.9.1 does not define.
    ///
    /// Mainline's `GetTags` holds the same values (minus the linguistic ones) in a multimap
    /// sorted by tag, with values of the same tag in name list order.
    /// So the first value of a given kind here is the one mainline finds.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a value outside the text list,
    /// or one that runs past its end.
    pub fn tagged_values(
        &self,
    ) -> impl Iterator<Item = Result<TaggedValue<'a>, SectionDecodingError>> + use<'a> {
        let text_list = self.text_list_memory;
        self.name_info_list
            .iter()
            .filter(|ni| ni.is_tagged())
            .map(move |ni| TaggedValue::parse(text_list, ni.text_offset() as usize))
    }

    /// Iterates over the tagged values of one kind.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for any malformed tagged value
    /// (since its kind cannot be trusted).
    pub fn tagged_values_of(
        &self,
        kind: TaggedValueKind,
    ) -> impl Iterator<Item = Result<TaggedValue<'a>, SectionDecodingError>> + use<'a> {
        self.tagged_values()
            .filter(move |v| v.as_ref().map_or(true, |v| v.tag() == kind as u8))
    }

    /// The first tagged value of one kind, if any.
    fn first_of(
        &self,
        kind: TaggedValueKind,
    ) -> Result<Option<TaggedValue<'a>>, SectionDecodingError> {
        self.tagged_values_of(kind).next().transpose()
    }

    /// Iterates over all linguistic records of the edge's names,
    /// as mainline's `GetLinguisticTaggedValues` returns them.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed value.
    pub fn linguistic_records(
        &self,
    ) -> impl Iterator<Item = Result<LinguisticRecord<'a>, SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::Linguistic)
            .flat_map(|value| {
                // Either the records of a value, or the error that kept us from reading them.
                let (records, error) = match value.and_then(|v| v.linguistic_records()) {
                    Ok(records) => (Some(records), None),
                    Err(e) => (None, Some(Err(e))),
                };
                records.into_iter().flatten().chain(error)
            })
    }

    /// The pronunciations and languages of the edge's names, keyed by name index,
    /// as mainline's `GetLinguisticMap` builds them.
    ///
    /// The name index counts all entries in the name list, tagged values included.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed value.
    pub fn linguistic_map(&self) -> Result<LinguisticMap<'a>, SectionDecodingError> {
        let mut map = LinguisticMap::default();
        for record in self.linguistic_records() {
            map.insert(record?);
        }
        Ok(map)
    }

    /// The OSM layer of the edge relative to other edges (0 if untagged),
    /// as mainline's `EdgeInfo::layer` returns it.
    ///
    /// # Errors
    ///
    /// Returns an error if the layer value is malformed.
    #[inline]
    pub fn layer(&self) -> Result<i8, SectionDecodingError> {
        self.first_of(TaggedValueKind::Layer)?
            .map_or(Ok(0), |value| value.layer())
    }

    /// The indoor levels of the edge (from OSM `level`), if any,
    /// as mainline's `EdgeInfo::levels` returns them.
    ///
    /// # Errors
    ///
    /// Returns an error if the levels value is malformed.
    #[inline]
    pub fn levels(&self) -> Result<Option<Levels<'a>>, SectionDecodingError> {
        self.first_of(TaggedValueKind::Levels)?
            .map(|value| value.levels())
            .transpose()
    }

    /// Whether the edge connects the given level,
    /// as mainline's `EdgeInfo::includes_level` decides it (false without levels).
    ///
    /// # Errors
    ///
    /// Returns an error if the levels value is malformed.
    #[inline]
    pub fn includes_level(&self, level: f32) -> Result<bool, SectionDecodingError> {
        match self.levels()? {
            Some(levels) => levels.includes_level(level),
            None => Ok(false),
        }
    }

    /// Iterates over the edge's `level:ref` values, as mainline's `EdgeInfo::level_ref` returns them.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed tagged value.
    #[inline]
    pub fn level_refs(
        &self,
    ) -> impl Iterator<Item = Result<&'a [u8], SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::LevelRef)
            .map(|value| value.map(|v| v.value()))
    }

    /// Iterates over the edge's tunnel names.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed tagged value.
    #[inline]
    pub fn tunnel_names(
        &self,
    ) -> impl Iterator<Item = Result<&'a [u8], SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::Tunnel)
            .map(|value| value.map(|v| v.value()))
    }

    /// Iterates over the edge's bridge names.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed tagged value.
    #[inline]
    pub fn bridge_names(
        &self,
    ) -> impl Iterator<Item = Result<&'a [u8], SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::Bridge)
            .map(|value| value.map(|v| v.value()))
    }

    /// The OSM node IDs along the edge, if the tiles were built to keep them,
    /// as mainline's `EdgeInfo::osm_node_ids` returns them.
    ///
    /// # Errors
    ///
    /// Returns an error if the value is malformed.
    #[inline]
    pub fn osm_node_ids(&self) -> Result<Option<OsmNodeIds<'a>>, SectionDecodingError> {
        self.first_of(TaggedValueKind::OsmNodeIds)?
            .map(|value| value.osm_node_ids())
            .transpose()
    }

    /// Iterates over the edge's conditional speed limits,
    /// as mainline's `EdgeInfo::conditional_speed_limits` returns them.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed value.
    #[inline]
    pub fn conditional_speed_limits(
        &self,
    ) -> impl Iterator<Item = Result<ConditionalSpeedLimit, SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::ConditionalSpeedLimits)
            .map(|value| value.and_then(|v| v.conditional_speed_limit()))
    }

    /// Iterates over the landmarks near the edge.
    ///
    /// # Errors
    ///
    /// The iterator yields an error for a malformed landmark.
    #[inline]
    pub fn landmarks(
        &self,
    ) -> impl Iterator<Item = Result<Landmark<'a>, SectionDecodingError>> + use<'a> {
        self.tagged_values_of(TaggedValueKind::Landmark)
            .map(|value| value.and_then(|v| v.landmark()))
    }

    /// The bicycle network membership mask for this edge.
    #[inline]
    pub fn bicycle_network(&self) -> EnumSet<BicycleNetwork> {
        // The field is 4 bits and BicycleNetwork has exactly 4 variants.
        EnumSet::from_repr_truncated(self.inner.first_inner_bitfield.bike_network())
    }

    /// The way ID of the edge.
    #[inline]
    pub fn way_id(&self) -> u64 {
        u64::from(self.extended_way_id_3) << 56
            | u64::from(self.extended_way_id_2) << 48
            | u64::from(self.inner.second_inner_bitfield.extended_way_id()) << 40
            | u64::from(self.inner.first_inner_bitfield.extended_way_id()) << 32
            | u64::from(self.inner.way_id)
    }
}

/// The sampling interval and the number of interior samples for an edge of a given length
/// (mainline's `sampling_interval` and `encoded_elevation_count`).
///
/// Mainline computes the count in floating point as
/// `round(length / (length / (n + 1))) - 1` with `n = floor(length / 32)`,
/// which is `n` for every positive length
/// (and undefined behavior for a length of 0, which we treat as no samples).
fn elevation_sampling(length: u32) -> (f64, u32) {
    let count = length / MAX_ELEVATION_SAMPLING;
    let interval = f64::from(length) / f64::from(count + 1);
    (interval, count)
}

impl core::fmt::Debug for EdgeInfo<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EdgeInfo")
            .field("way_id", &self.way_id())
            .field("mean_elevation", &self.mean_elevation())
            .field("bicycle_network", &self.bicycle_network())
            .field("speed_limit", &self.speed_limit())
            .field("has_elevation", &self.has_elevation())
            .field("name_infos", &self.name_info_list)
            .field("names", &self.names().collect::<Vec<_>>())
            .field("tagged_values", &self.tagged_values().collect::<Vec<_>>())
            .field("encoded_shape_bytes", &self.encoded_shape.len())
            .finish_non_exhaustive()
    }
}

impl<'a> TryFrom<(&'a [u8], &'a [u8])> for EdgeInfo<'a> {
    type Error = GraphTileDecodingError;

    /// Reads the edge info at the start of `bytes`
    /// (the edge info section from the edge's offset on),
    /// with names in `text_list_memory`.
    fn try_from((bytes, text_list_memory): (&'a [u8], &'a [u8])) -> Result<Self, Self::Error> {
        let cast_error = |field: &str, description: &str| GraphTileDecodingError::CastError {
            field: field.to_string(),
            error_description: description.to_string(),
        };
        let (inner, bytes) = EdgeInfoInner::ref_from_prefix(bytes)
            .map_err(|e| cast_error("inner", &e.to_string()))?;

        let (name_info_list, bytes) = <[NameInfo]>::ref_from_prefix_with_elems(
            bytes,
            inner.second_inner_bitfield.name_count() as usize,
        )
        .map_err(|e| cast_error("name_info_list", &e.to_string()))?;

        let shape_size = inner.second_inner_bitfield.encoded_shape_size().get() as usize;
        let (encoded_shape, bytes) = bytes.split_at_checked(shape_size).ok_or_else(|| {
            cast_error(
                "encoded_shape",
                "the shape runs past the end of the section",
            )
        })?;

        // Maybe read up to two more way ID bytes; the data structure on disk is tightly packed
        // and drops bytes when possible in exchange for bits that are otherwise unused.
        let extended_size = inner.second_inner_bitfield.extended_way_id_size();
        let (extended_way_id_2, bytes) = match (extended_size > 0, bytes) {
            (true, [byte, rest @ ..]) => (*byte, rest),
            (true, []) => return Err(cast_error("extended_way_id_2", "missing byte")),
            (false, _) => (0, bytes),
        };
        let (extended_way_id_3, trailing) = match (extended_size > 1, bytes) {
            (true, [byte, rest @ ..]) => (*byte, rest),
            (true, []) => return Err(cast_error("extended_way_id_3", "missing byte")),
            (false, _) => (0, bytes),
        };

        Ok(Self {
            inner,
            name_info_list,
            encoded_shape,
            extended_way_id_2,
            extended_way_id_3,
            trailing,
            text_list_memory,
        })
    }
}

#[cfg(feature = "serde")]
impl Serialize for EdgeInfo<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let num_fields = 4;

        let speed_limit = self.speed_limit();
        let num_fields = if speed_limit == 0 {
            num_fields - 1
        } else {
            num_fields
        };

        let names = self.get_names().join(" / ");
        let num_fields = if names.is_empty() {
            num_fields - 1
        } else {
            num_fields
        };

        let mut state = serializer.serialize_struct("EdgeInfo", num_fields)?;
        if !names.is_empty() {
            state.serialize_field("names", &names)?;
        }

        state.serialize_field(
            "bike_network",
            &self
                .bicycle_network()
                .iter()
                .map(|v| v.as_char())
                .collect::<String>(),
        )?;

        if speed_limit != 0 {
            state.serialize_field("speed_limit", &self.speed_limit())?;
        }

        state.serialize_field("way_id", &self.way_id())?;

        state.end()
    }
}

#[cfg(test)]
// Elevations are sums of multiples of 0.25 (exact in binary)
// and must match mainline's bit for bit, so exact comparisons are intended.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::graph_tile::tagged_value::LandmarkType;
    use crate::graph_tile::tagged_value::tests::{encode_landmark, encode_levels};
    use crate::graph_tile::{GraphTile, TEST_GRAPH_TILE_L0};
    use proptest::prelude::*;

    /// Builds an edge info record and its text list, as `EdgeInfoBuilder` and `AddName` would.
    struct Builder {
        names: Vec<(Vec<u8>, bool, bool)>,
        shape: Vec<u8>,
        elevation: Vec<u8>,
        way_id: u64,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                names: Vec::new(),
                shape: vec![0, 0],
                elevation: Vec::new(),
                way_id: 0,
            }
        }

        fn name(mut self, text: &[u8], route_num: bool) -> Self {
            let mut text = text.to_vec();
            text.push(0);
            self.names.push((text, route_num, false));
            self
        }

        /// A tagged value, including its terminator.
        fn tagged(mut self, value: Vec<u8>) -> Self {
            self.names.push((value, false, true));
            self
        }

        fn build(&self) -> (Vec<u8>, Vec<u8>) {
            // Offset 0 of a real text list is always an empty string.
            let mut text = vec![0];
            let mut infos = Vec::new();
            for (value, route_num, tagged) in &self.names {
                let offset = u32::try_from(text.len()).unwrap();
                text.extend(value);
                infos.push(offset | u32::from(*route_num) << 28 | u32::from(*tagged) << 29);
            }
            #[allow(clippy::cast_possible_truncation)]
            let (low, b4, b5, b6, b7) = (
                self.way_id as u32,
                (self.way_id >> 32) as u8,
                (self.way_id >> 40) as u8,
                (self.way_id >> 48) as u8,
                (self.way_id >> 56) as u8,
            );
            let extended_size: u32 = if b7 != 0 { 2 } else { u32::from(b6 != 0) };
            let mut bytes = low.to_le_bytes().to_vec();
            // A mean elevation of 1020 m, no bike network, and a speed limit of 50
            let (mean_elevation_bin, speed_limit): (u32, u32) = (760, 50);
            bytes.extend(
                (mean_elevation_bin | speed_limit << 16 | u32::from(b4) << 24).to_le_bytes(),
            );
            let second = u32::try_from(self.names.len()).unwrap()
                | u32::try_from(self.shape.len()).unwrap() << 4
                | u32::from(b5) << 20
                | extended_size << 28
                | u32::from(!self.elevation.is_empty()) << 30;
            bytes.extend(second.to_le_bytes());
            for info in infos {
                bytes.extend(info.to_le_bytes());
            }
            bytes.extend(&self.shape);
            if extended_size > 0 {
                bytes.push(b6);
            }
            if extended_size > 1 {
                bytes.push(b7);
            }
            bytes.extend(&self.elevation);
            (bytes, text)
        }
    }

    #[test]
    fn test_names_and_tagged_values() {
        let mut ling = vec![TaggedValueKind::Linguistic as u8];
        ling.extend([9, 0, 5 | 1 << 3]);
        ling.extend([17, 2, 1 | 2 << 3, b'a', b'b']);
        ling.push(0);
        let mut cond = vec![TaggedValueKind::ConditionalSpeedLimits as u8];
        cond.extend(((30u64 << 54) | (0b011_1110 << 1) | (7 << 8) | (9 << 31)).to_le_bytes());
        cond.push(0);

        let builder = Builder::new()
            .name(b"Carrer Major", false)
            .tagged(b"1T\xc3\xbanel\0".to_vec())
            .name(b"CG-1", true)
            .tagged(vec![TaggedValueKind::Layer as u8, 0xfe, 0])
            .tagged(ling)
            .tagged(encode_levels(&[(0, 2), (4, 4)], 0))
            .tagged(vec![TaggedValueKind::LevelRef as u8, b'P', b'1', 0])
            .tagged(vec![TaggedValueKind::LevelRef as u8, b'P', b'2', 0])
            .tagged(cond)
            .tagged(encode_landmark(
                1.5,
                42.5,
                LandmarkType::Fuel as u8,
                b"Repsol",
            ))
            .tagged(vec![77, b'?', 0]);
        let (bytes, text) = builder.build();
        let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();

        let names = ei.names().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].text_lossy(), "Carrer Major");
        assert!(!names[0].is_route_number());
        assert_eq!(names[1].text(), b"CG-1");
        assert!(names[1].is_route_number());
        assert_eq!(ei.get_names(), ["Carrer Major", "CG-1"]);
        assert_eq!(ei.types(), 0b100);

        // Tagged names: everything but the linguistic value, as C strings after the tag.
        let tagged = ei
            .names_and_types(true)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tagged.len(), 10);
        assert_eq!(tagged[1].text_lossy(), "Túnel");
        assert_eq!(tagged[1].tag(), b'1');
        assert_eq!(tagged[3].text(), &[0xfe]);

        assert_eq!(ei.tagged_values().count(), 9);
        assert_eq!(
            ei.tunnel_names().next().unwrap().unwrap(),
            "Túnel".as_bytes()
        );
        assert_eq!(ei.bridge_names().count(), 0);
        assert_eq!(ei.layer().unwrap(), -2);
        let levels = ei.levels().unwrap().unwrap();
        assert_eq!(
            levels.ranges().collect::<Result<Vec<_>, _>>().unwrap(),
            [(0.0, 2.0), (4.0, 4.0)]
        );
        assert!(ei.includes_level(1.0).unwrap());
        assert!(!ei.includes_level(3.0).unwrap());
        assert_eq!(
            ei.level_refs().collect::<Result<Vec<_>, _>>().unwrap(),
            [b"P1", b"P2"]
        );
        let limits = ei
            .conditional_speed_limits()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].speed(), 30);
        assert_eq!(limits[0].time_domain().to_string(), "Mo-Fr 07:00-09:00");
        let landmark = ei.landmarks().next().unwrap().unwrap();
        assert_eq!(landmark.name(), b"Repsol");
        assert_eq!(landmark.landmark_type(), LandmarkType::Fuel);
        assert!(ei.osm_node_ids().unwrap().is_none());

        // The unknown tag is skipped by every accessor but still listed.
        let unknown = ei
            .tagged_values()
            .filter_map(Result::ok)
            .find(|v| v.kind().is_none());
        assert_eq!(unknown.unwrap().tag(), 77);

        let records = ei
            .linguistic_records()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records.len(), 2);
        let map = ei.linguistic_map().unwrap();
        assert_eq!(map.get(1).unwrap().language().unwrap().as_str(), "ca");
        assert_eq!(map.get(2).unwrap().pronunciation(), b"ab");

        assert_eq!(ei.speed_limit(), 50);
        assert_eq!(ei.mean_elevation(), Some(1020.0));
    }

    #[test]
    fn test_way_id_bytes() {
        let mut builder = Builder::new();
        builder.way_id = 0x0102_0304_0506_0708;
        let (bytes, text) = builder.build();
        let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();
        assert_eq!(ei.way_id(), 0x0102_0304_0506_0708);

        // Truncated before the extended way ID bytes
        let cut = bytes.len() - 1;
        assert!(EdgeInfo::try_from((&bytes[..cut], text.as_slice())).is_err());
        // Truncated in the shape
        assert!(EdgeInfo::try_from((&bytes[..13], text.as_slice())).is_err());
        // Truncated in the fixed part
        assert!(EdgeInfo::try_from((&bytes[..11], text.as_slice())).is_err());
    }

    #[test]
    fn test_corrupt_offsets() {
        let (mut bytes, text) = Builder::new()
            .name(b"x", false)
            .tagged(vec![TaggedValueKind::Layer as u8, 1, 0])
            .build();
        // Point both entries past the end of the text list.
        bytes[12..15].copy_from_slice(&[0xff, 0xff, 0x00]);
        bytes[16..19].copy_from_slice(&[0xff, 0xff, 0x00]);
        let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();
        assert!(matches!(
            ei.names().next(),
            Some(Err(SectionDecodingError::TextOffsetOutOfBounds { .. }))
        ));
        assert!(ei.get_names().is_empty());
        assert!(ei.layer().is_err());
        assert!(ei.names_and_types(true).nth(1).unwrap().is_err());
        // Debug formatting must not panic either.
        assert!(format!("{ei:?}").contains("TextOffsetOutOfBounds"));
    }

    #[test]
    fn test_encoded_elevation() {
        let mut builder = Builder::new();
        builder.elevation = vec![4, 0xfc, 1, 2, 0xff];
        let (bytes, text) = builder.build();
        let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();
        assert!(ei.has_elevation());
        // 100 m splits into 4 intervals of 25 m, with 3 interior samples.
        let elevation = ei.encoded_elevation(100).unwrap();
        assert_eq!(elevation.interval(), 25.0);
        assert_eq!(elevation.deltas(), &[4, -4, 1]);
        assert_eq!(
            elevation.decode(10.0, 20.0, true),
            [10.0, 11.0, 10.0, 10.25, 20.0]
        );
        assert_eq!(
            elevation.decode(10.0, 20.0, false),
            [10.0, 20.25, 20.0, 21.0, 20.0]
        );
        // Too long for the bytes that remain
        assert!(ei.encoded_elevation(1000).is_err());

        let (bytes, text) = Builder::new().build();
        let ei = EdgeInfo::try_from((bytes.as_slice(), text.as_slice())).unwrap();
        let elevation = ei.encoded_elevation(20).unwrap();
        assert_eq!(elevation.interval(), 20.0);
        assert!(elevation.deltas().is_empty());
        assert_eq!(elevation.decode(1.0, 2.0, true), [1.0, 2.0]);
    }

    #[test]
    fn test_fixture_edge_info() {
        // Cross-checked with EdgeInfo in Valhalla 3.9.1
        // (the section oracle test covers every edge info in the fixtures).
        let tile = &*TEST_GRAPH_TILE_L0;
        let edge = &tile.directed_edges()[1];
        let ei = tile.get_edge_info(edge).unwrap();
        assert_eq!(ei.way_id(), 24_374_057);
        assert_eq!(ei.mean_elevation(), Some(1312.0));
        assert!(ei.has_elevation());
        let elevation = ei.encoded_elevation(edge.length()).unwrap();
        assert!(!elevation.deltas().is_empty());
        // The first edge has no elevation samples, but a mean elevation.
        let ei = tile.get_edge_info(&tile.directed_edges()[0]).unwrap();
        assert_eq!(ei.mean_elevation(), Some(1130.0));
        assert!(!ei.has_elevation());
    }

    proptest! {
        /// The integer sample count is mainline's floating point computation.
        #[test]
        fn test_elevation_count_matches_mainline(length in 1u32..(1 << 24)) {
            let (interval, count) = elevation_sampling(length);
            let sample_count = (f64::from(length) / f64::from(MAX_ELEVATION_SAMPLING)).floor();
            let mainline_interval = f64::from(length) / (sample_count + 1.0);
            let mainline_count = (f64::from(length) / mainline_interval).round() - 1.0;
            prop_assert_eq!(interval, mainline_interval);
            prop_assert_eq!(f64::from(count), mainline_count);
        }
    }
}
