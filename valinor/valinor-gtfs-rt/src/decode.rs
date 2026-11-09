//! Decoding GTFS-RT from bytes.
//!
//! This is the only place that reads a feed's bytes.
//! Anything malformed is an error: a feed that can't be trusted must not change what we route on.

use prost::Message;
use thiserror::Error;

use crate::proto::FeedMessage;
use crate::proto::feed_header::Incrementality;

/// Why a feed message was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The bytes are not a GTFS-RT protobuf message.
    #[error("decode a GTFS-RT feed: {0}")]
    Protobuf(#[from] prost::DecodeError),
    /// The header gave no GTFS-RT version, which the specification requires.
    #[error("read a feed header without a GTFS-RT version")]
    MissingVersion,
    /// The feed is `DIFFERENTIAL`.
    ///
    /// A differential message says nothing about trips it doesn't mention,
    /// and this crate reads each feed as the whole current state.
    #[error("use a differential feed")]
    Differential,
}

/// A decoded GTFS-RT feed message.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFeed {
    message: FeedMessage,
}

impl DecodedFeed {
    /// Decodes a feed message from its protobuf bytes.
    ///
    /// # Errors
    ///
    /// If `bytes` isn't a GTFS-RT message, has no version in its header, or is differential.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let message = FeedMessage::decode(bytes)?;
        if message.header.gtfs_realtime_version.is_empty() {
            return Err(DecodeError::MissingVersion);
        }
        if message.header.incrementality() == Incrementality::Differential {
            return Err(DecodeError::Differential);
        }
        Ok(Self { message })
    }

    /// Wraps a message that has already been decoded.
    pub fn from_message(message: FeedMessage) -> Self {
        Self { message }
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
                is_deleted: None,
                trip_update: None,
                vehicle: None,
                alert: None,
                shape: None,
                stop: None,
                trip_modifications: None,
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
            Err(DecodeError::Protobuf(_))
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
