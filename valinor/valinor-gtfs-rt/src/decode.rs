//! Decoding GTFS-RT from bytes.
//!
//! This is the only place that reads a feed's bytes.
//! Anything malformed is an error: a feed that can't be trusted must not change what we route on.

use prost::Message;
use thiserror::Error;

use crate::proto::feed_header::Incrementality;
use crate::proto::{FeedEntity, FeedHeader, FeedMessage, TranslatedString};

/// Bounds on what one feed message may cost to decode.
///
/// A hostile or broken feed must not exhaust a phone, and protobuf amplifies:
/// a two-byte empty entity decodes into a struct of over a kilobyte.
/// Entities are therefore decoded one at a time and accounted for as they arrive,
/// so memory stays proportionate to what the message really carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeLimits {
    /// The largest message accepted, in bytes.
    pub max_feed_bytes: usize,
    /// The largest single entity, in bytes; a larger one is dropped and counted.
    ///
    /// This bounds the memory of decoding any one entity,
    /// before it can be accounted for.
    pub max_entity_bytes: usize,
    /// The most entities with a payload that a message may have.
    pub max_entities: usize,
    /// The most memory the decoded message may hold, estimated from its contents.
    pub max_decoded_bytes: usize,
}

impl Default for DecodeLimits {
    /// Generous for real feeds: King County's largest is about 400 KB and 400 entities.
    fn default() -> Self {
        Self {
            max_feed_bytes: 32 * 1024 * 1024,
            max_entity_bytes: 512 * 1024,
            max_entities: 200_000,
            max_decoded_bytes: 128 * 1024 * 1024,
        }
    }
}

/// Why a feed message was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The bytes are not a GTFS-RT protobuf message.
    #[error("decode a GTFS-RT feed: {0}")]
    Protobuf(#[from] prost::DecodeError),
    /// The message's outer framing is broken.
    #[error("read a feed's protobuf framing: {0}")]
    Malformed(&'static str),
    /// The header gave no GTFS-RT version, which the specification requires.
    #[error("read a feed header without a GTFS-RT version")]
    MissingVersion,
    /// The feed is `DIFFERENTIAL`.
    ///
    /// A differential message says nothing about trips it doesn't mention,
    /// and this crate reads each feed as the whole current state.
    #[error("use a differential feed")]
    Differential,
    /// The message exceeds a [`DecodeLimits`] bound.
    #[error("decode a feed over its {what} limit of {limit}")]
    TooLarge {
        /// Which limit was exceeded.
        what: &'static str,
        /// The limit.
        limit: usize,
    },
}

/// A decoded GTFS-RT feed message.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFeed {
    message: FeedMessage,
    dropped_entities: usize,
}

impl DecodedFeed {
    /// Decodes a feed message from its protobuf bytes, under the default [`DecodeLimits`].
    ///
    /// # Errors
    ///
    /// If `bytes` isn't a GTFS-RT message, has no version in its header, is differential,
    /// or exceeds the limits.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::decode_with(bytes, DecodeLimits::default())
    }

    /// Decodes a feed message from its protobuf bytes.
    ///
    /// Entities with no payload carry nothing to use, and are dropped.
    ///
    /// # Errors
    ///
    /// If `bytes` isn't a GTFS-RT message, has no version in its header, is differential,
    /// or exceeds `limits`.
    pub fn decode_with(bytes: &[u8], limits: DecodeLimits) -> Result<Self, DecodeError> {
        if bytes.len() > limits.max_feed_bytes {
            return Err(DecodeError::TooLarge {
                what: "bytes",
                limit: limits.max_feed_bytes,
            });
        }
        let mut header = FeedHeader::default();
        let mut entity = Vec::new();
        let mut held = 0_usize;
        let mut dropped_entities = 0_usize;
        let mut cursor = bytes;
        while !cursor.is_empty() {
            let (number, payload, rest) = next_field(cursor)?;
            cursor = rest;
            match (number, payload) {
                (1, Some(payload)) => header.merge(payload)?,
                (2, Some(payload)) => {
                    if payload.len() > limits.max_entity_bytes {
                        dropped_entities += 1;
                        continue;
                    }
                    let decoded = FeedEntity::decode(payload)?;
                    if !carries_payload(&decoded) {
                        dropped_entities += 1;
                        continue;
                    }
                    if entity.len() >= limits.max_entities {
                        return Err(DecodeError::TooLarge {
                            what: "entities",
                            limit: limits.max_entities,
                        });
                    }
                    held = held.saturating_add(held_bytes(&decoded));
                    if held > limits.max_decoded_bytes {
                        return Err(DecodeError::TooLarge {
                            what: "decoded bytes",
                            limit: limits.max_decoded_bytes,
                        });
                    }
                    entity.push(decoded);
                }
                // Unknown fields and extensions: nothing here reads them.
                _ => {}
            }
        }
        if header.gtfs_realtime_version.is_empty() {
            return Err(DecodeError::MissingVersion);
        }
        if header.incrementality() == Incrementality::Differential {
            return Err(DecodeError::Differential);
        }
        Ok(Self {
            message: FeedMessage { header, entity },
            dropped_entities,
        })
    }

    /// Wraps a message that has already been decoded.
    pub fn from_message(message: FeedMessage) -> Self {
        Self {
            message,
            dropped_entities: 0,
        }
    }

    /// How many entities were dropped for being empty or oversized.
    pub fn dropped_entities(&self) -> usize {
        self.dropped_entities
    }

    /// The message.
    pub fn message(&self) -> &FeedMessage {
        &self.message
    }

    /// The message, by value.
    pub fn into_message(self) -> FeedMessage {
        self.message
    }

    /// The header's timestamp, in POSIX seconds.
    ///
    /// Zero and values beyond `i64` count as absent, as neither says when the data is from.
    pub fn timestamp(&self) -> Option<i64> {
        self.message
            .header
            .timestamp
            .filter(|&time| time != 0)
            .and_then(|time| i64::try_from(time).ok())
    }
}

/// Whether an entity has anything in it to use.
fn carries_payload(entity: &FeedEntity) -> bool {
    entity.trip_update.is_some()
        || entity.vehicle.is_some()
        || entity.alert.is_some()
        || entity.stop.is_some()
        || entity.trip_modifications.is_some()
        || entity.shape.is_some()
}

/// The memory an entity's repeated fields hold, beyond its text.
///
/// Text costs what it costs on the wire. Repeated messages are what amplify,
/// so this counts them at their in-memory size.
#[allow(deprecated)]
fn held_bytes(entity: &FeedEntity) -> usize {
    let mut held = size_of::<FeedEntity>() + entity.id.len();
    if let Some(update) = &entity.trip_update {
        held += size_of_val(update.stop_time_update.as_slice());
    }
    if let Some(vehicle) = &entity.vehicle {
        held += size_of_val(vehicle.multi_carriage_details.as_slice());
    }
    if let Some(alert) = &entity.alert {
        held += size_of_val(alert.informed_entity.as_slice())
            + size_of_val(alert.active_period.as_slice())
            + size_of_val(alert.communication_period.as_slice())
            + size_of_val(alert.impact_period.as_slice());
        for text in [
            &alert.url,
            &alert.header_text,
            &alert.description_text,
            &alert.tts_header_text,
            &alert.tts_description_text,
            &alert.image_alternative_text,
            &alert.cause_detail,
            &alert.effect_detail,
        ] {
            held += translated_bytes(text.as_ref());
        }
    }
    if let Some(stop) = &entity.stop {
        for text in [
            &stop.stop_code,
            &stop.stop_name,
            &stop.tts_stop_name,
            &stop.stop_desc,
            &stop.stop_url,
            &stop.platform_code,
        ] {
            held += translated_bytes(text.as_ref());
        }
    }
    if let Some(set) = &entity.trip_modifications {
        held += size_of_val(set.selected_trips.as_slice())
            + size_of_val(set.start_times.as_slice())
            + size_of_val(set.service_dates.as_slice())
            + size_of_val(set.modifications.as_slice());
        for selected in &set.selected_trips {
            held += size_of_val(selected.trip_ids.as_slice());
        }
        for modification in &set.modifications {
            held += size_of_val(modification.replacement_stops.as_slice());
        }
    }
    held
}

fn translated_bytes(text: Option<&TranslatedString>) -> usize {
    text.map_or(0, |text| size_of_val(text.translation.as_slice()))
}

/// Splits one field off a message: its number, its bytes if length-delimited, and the rest.
fn next_field(bytes: &[u8]) -> Result<(u64, Option<&[u8]>, &[u8]), DecodeError> {
    let (key, rest) = read_varint(bytes)?;
    let number = key >> 3;
    if number == 0 {
        return Err(DecodeError::Malformed("field number zero"));
    }
    match key & 7 {
        0 => {
            let (_, rest) = read_varint(rest)?;
            Ok((number, None, rest))
        }
        1 => take(rest, 8).map(|(_, rest)| (number, None, rest)),
        2 => {
            let (length, rest) = read_varint(rest)?;
            let length =
                usize::try_from(length).map_err(|_| DecodeError::Malformed("field too long"))?;
            let (payload, rest) = take(rest, length)?;
            Ok((number, Some(payload), rest))
        }
        5 => take(rest, 4).map(|(_, rest)| (number, None, rest)),
        // Groups are absent from GTFS-RT, and an unknown wire type can't be skipped.
        _ => Err(DecodeError::Malformed("unsupported wire type")),
    }
}

fn take(bytes: &[u8], length: usize) -> Result<(&[u8], &[u8]), DecodeError> {
    bytes
        .split_at_checked(length)
        .ok_or(DecodeError::Malformed("field runs past the message"))
}

fn read_varint(bytes: &[u8]) -> Result<(u64, &[u8]), DecodeError> {
    let mut value = 0_u64;
    for (index, &byte) in bytes.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            return Ok((value, &bytes[index + 1..]));
        }
    }
    Err(DecodeError::Malformed("varint"))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::proto::{FeedEntity, FeedHeader};

    fn message(version: &str, incrementality: Option<i32>) -> Vec<u8> {
        FeedMessage {
            header: FeedHeader {
                gtfs_realtime_version: version.into(),
                incrementality,
                timestamp: Some(10),
                feed_version: None,
            },
            entity: vec![FeedEntity {
                id: "1".into(),
                alert: Some(Default::default()),
                ..Default::default()
            }],
        }
        .encode_to_vec()
    }

    #[test]
    fn a_valid_message_decodes() {
        let decoded = DecodedFeed::decode(&message("2.0", None)).expect("valid");
        assert_eq!(decoded.timestamp(), Some(10));
        assert_eq!(decoded.message().entity.len(), 1);
    }

    #[test]
    fn missing_version_is_an_error() {
        assert_eq!(
            DecodedFeed::decode(&message("", None)),
            Err(DecodeError::MissingVersion)
        );
    }

    #[test]
    fn a_differential_feed_is_an_error() {
        assert_eq!(
            DecodedFeed::decode(&message("2.0", Some(Incrementality::Differential as i32))),
            Err(DecodeError::Differential)
        );
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(matches!(
            DecodedFeed::decode(&[0xff, 0xff, 0xff, 0xff]),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn a_broken_entity_is_an_error() {
        // Field 2, length-delimited, holding a truncated varint.
        let mut bytes = message("2.0", None);
        bytes.extend_from_slice(&[0x12, 0x02, 0x08, 0xff]);
        assert!(matches!(
            DecodedFeed::decode(&bytes),
            Err(DecodeError::Protobuf(_))
        ));
    }

    #[test]
    fn empty_entities_are_dropped_and_counted() {
        let mut bytes = message("2.0", None);
        bytes.extend_from_slice(&[0x12, 0x00, 0x12, 0x00]);
        let decoded = DecodedFeed::decode(&bytes).expect("valid");
        assert_eq!(decoded.message().entity.len(), 1);
        assert_eq!(decoded.dropped_entities(), 2);
    }

    #[test]
    fn a_message_over_the_byte_limit_is_rejected_unread() {
        let limits = DecodeLimits {
            max_feed_bytes: 4,
            ..DecodeLimits::default()
        };
        assert!(matches!(
            DecodedFeed::decode_with(&message("2.0", None), limits),
            Err(DecodeError::TooLarge { what: "bytes", .. })
        ));
    }

    #[test]
    fn an_oversized_entity_is_dropped_and_counted() {
        let limits = DecodeLimits {
            max_entity_bytes: 1,
            ..DecodeLimits::default()
        };
        let decoded = DecodedFeed::decode_with(&message("2.0", None), limits).expect("valid");
        assert!(decoded.message().entity.is_empty());
        assert_eq!(decoded.dropped_entities(), 1);
    }

    #[test]
    fn too_many_entities_are_rejected() {
        let limits = DecodeLimits {
            max_entities: 0,
            ..DecodeLimits::default()
        };
        assert!(matches!(
            DecodedFeed::decode_with(&message("2.0", None), limits),
            Err(DecodeError::TooLarge {
                what: "entities",
                ..
            })
        ));
    }

    #[test]
    fn repeated_messages_count_toward_the_memory_limit() {
        let limits = DecodeLimits {
            max_decoded_bytes: 1,
            ..DecodeLimits::default()
        };
        assert!(matches!(
            DecodedFeed::decode_with(&message("2.0", None), limits),
            Err(DecodeError::TooLarge {
                what: "decoded bytes",
                ..
            })
        ));
    }

    #[test]
    fn a_timestamp_beyond_i64_is_absent() {
        let bytes = FeedMessage {
            header: FeedHeader {
                gtfs_realtime_version: "2.0".into(),
                incrementality: None,
                timestamp: Some(u64::MAX),
                feed_version: None,
            },
            entity: Vec::new(),
        }
        .encode_to_vec();
        assert_eq!(
            DecodedFeed::decode(&bytes).expect("valid").timestamp(),
            None
        );
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_are_never_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let _ = DecodedFeed::decode(&bytes);
        }

        #[test]
        fn flipped_bytes_are_never_a_panic(index in 0_usize..64, value in any::<u8>()) {
            let mut bytes = message("2.0", None);
            if let Some(byte) = bytes.get_mut(index) {
                *byte = value;
            }
            let _ = DecodedFeed::decode(&bytes);
        }
    }
}
