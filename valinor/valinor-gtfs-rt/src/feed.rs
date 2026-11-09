//! Feeds: their namespaced identifiers, what the overlay builder takes, and their freshness.

use core::fmt;
use std::sync::Arc;

use thiserror::Error;

use crate::decode::{DecodeError, DecodedFeed};

/// The default age, in seconds, past which a trip-update or vehicle feed is stale.
pub const DEFAULT_REALTIME_STALE_AFTER: i64 = 5 * 60;

/// The default age, in seconds, past which an alerts feed is stale.
///
/// Alerts change slowly and agencies publish them less often.
pub const DEFAULT_ALERTS_STALE_AFTER: i64 = 30 * 60;

/// A short name for one data source: all identifiers in its feeds are scoped to this.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedId(Arc<str>);

/// Why a [`FeedId`] was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FeedIdError {
    /// The identifier was empty.
    #[error("use an empty feed ID")]
    Empty,
}

impl FeedId {
    /// A feed identifier.
    ///
    /// # Errors
    ///
    /// If `id` is empty.
    pub fn new(id: impl AsRef<str>) -> Result<Self, FeedIdError> {
        let id = id.as_ref();
        if id.is_empty() {
            return Err(FeedIdError::Empty);
        }
        Ok(Self(id.into()))
    }

    /// The feed's name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for FeedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for FeedId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A GTFS identifier scoped to its feed, so identifiers of different agencies never collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopedId {
    /// The feed the identifier belongs to.
    pub feed: FeedId,
    /// The identifier, as the feed spells it.
    pub id: Arc<str>,
}

impl ScopedId {
    /// Scopes `id` to `feed`.
    pub fn new(feed: &FeedId, id: impl AsRef<str>) -> Self {
        Self {
            feed: feed.clone(),
            id: id.as_ref().into(),
        }
    }
}

/// What a feed message carries, for reporting and staleness limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeedKind {
    /// Trip updates.
    TripUpdates,
    /// Vehicle positions.
    VehiclePositions,
    /// Service alerts.
    Alerts,
    /// More than one of the above in a single message.
    Combined,
}

impl FeedKind {
    /// The default staleness limit for a feed of this kind.
    pub fn default_stale_after(self) -> i64 {
        match self {
            Self::TripUpdates | Self::VehiclePositions | Self::Combined => {
                DEFAULT_REALTIME_STALE_AFTER
            }
            Self::Alerts => DEFAULT_ALERTS_STALE_AFTER,
        }
    }
}

/// One decoded feed message and what the builder needs to know about it.
#[derive(Debug, Clone)]
pub struct FeedInput {
    /// The source whose identifiers this message uses.
    pub feed: FeedId,
    /// What the message carries.
    pub kind: FeedKind,
    /// The decoded message, or why it couldn't be decoded.
    ///
    /// A feed that fails to decode still goes to the builder:
    /// it has no effect on routing, but its [`FeedStatus`] says why.
    pub decoded: Result<DecodedFeed, DecodeError>,
    /// When this client received the message, in POSIX seconds, if it knows.
    ///
    /// Age is taken from the feed header when it has a timestamp, and from this otherwise.
    pub received_at: Option<i64>,
    /// How old, in seconds, the message may be before it is ignored.
    pub stale_after: i64,
}

impl FeedInput {
    /// A feed with the default staleness limit for its kind.
    pub fn new(feed: FeedId, kind: FeedKind, decoded: DecodedFeed) -> Self {
        Self {
            feed,
            kind,
            decoded: Ok(decoded),
            received_at: None,
            stale_after: kind.default_stale_after(),
        }
    }

    /// Decodes `bytes`, keeping any failure so that it can be reported rather than lost.
    pub fn from_bytes(feed: FeedId, kind: FeedKind, bytes: &[u8]) -> Self {
        Self {
            feed,
            kind,
            decoded: DecodedFeed::decode(bytes),
            received_at: None,
            stale_after: kind.default_stale_after(),
        }
    }

    /// The feed header's timestamp, if the message decoded and has one.
    fn header_timestamp(&self) -> Option<i64> {
        self.decoded.as_ref().ok().and_then(DecodedFeed::timestamp)
    }

    /// Reports the message's age at `now`, and whether it can be used.
    ///
    /// A message with no time at all has no age and counts as stale, since nothing says it is
    /// current. So does one dated further ahead of `now` than its limit allows: that is a
    /// broken clock rather than a small skew, and it would otherwise never go stale.
    pub(crate) fn status(&self, now: i64) -> FeedStatus {
        let header_timestamp = self.header_timestamp();
        let age = header_timestamp
            .or(self.received_at)
            .map(|time| now.saturating_sub(time));
        let error = self.decoded.as_ref().err().cloned();
        FeedStatus {
            feed: self.feed.clone(),
            kind: self.kind,
            header_timestamp,
            age,
            stale_after: self.stale_after,
            stale: error.is_some() || is_stale(age, self.stale_after),
            entities_dropped: self
                .decoded
                .as_ref()
                .map_or(0, DecodedFeed::dropped_entities),
            error,
        }
    }
}

/// Whether a message of this age, in seconds, is past its limit.
///
/// A message of unknown age, or dated further ahead of the clock than its limit allows, is stale.
fn is_stale(age: Option<i64>, stale_after: i64) -> bool {
    age.is_none_or(|age| age > stale_after || age < stale_after.saturating_neg())
}

/// How current one feed message was when an overlay was built.
///
/// Each response reports the age of its real-time data from this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedStatus {
    /// The source.
    pub feed: FeedId,
    /// What the message carries.
    pub kind: FeedKind,
    /// The feed header's own timestamp, if it gave one.
    pub header_timestamp: Option<i64>,
    /// The message's age in seconds at `now`; `None` when the message carried no time at all.
    ///
    /// Negative if the feed's clock is ahead of ours.
    pub age: Option<i64>,
    /// The limit this message was held to.
    pub stale_after: i64,
    /// Whether the message was stale or undecodable, and so had no effect on the overlay.
    pub stale: bool,
    /// Why the message couldn't be decoded, if it couldn't.
    pub error: Option<DecodeError>,
    /// How many entities were dropped while decoding, for being empty or oversized.
    pub entities_dropped: usize,
}

impl FeedStatus {
    /// This status as of `now`, for an overlay built at `built_at`.
    ///
    /// An overlay is built once but read for a while,
    /// so responses report age at the moment of the query rather than at the build.
    /// The feed's effect on routing was decided at the build; see [`Self::stale`].
    #[must_use]
    pub fn aged_to(&self, built_at: i64, now: i64) -> Self {
        let age = self
            .age
            .map(|age| age.saturating_add(now.saturating_sub(built_at)));
        Self {
            age,
            stale: self.error.is_some() || is_stale(age, self.stale_after),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{FeedHeader, FeedMessage};

    fn input(timestamp: Option<u64>, received: Option<i64>) -> FeedInput {
        let decoded = DecodedFeed::from_message(FeedMessage {
            header: FeedHeader {
                gtfs_realtime_version: "2.0".into(),
                incrementality: None,
                timestamp,
                feed_version: None,
            },
            entity: Vec::new(),
        });
        let mut input = FeedInput::new(
            FeedId::new("kcm").expect("non-empty"),
            FeedKind::Alerts,
            decoded,
        );
        input.received_at = received;
        input.stale_after = 60;
        input
    }

    #[test]
    fn empty_feed_ids_are_rejected() {
        assert_eq!(FeedId::new("").err(), Some(FeedIdError::Empty));
    }

    #[test]
    fn age_prefers_the_header() {
        let status = input(Some(1_000), Some(900)).status(1_030);
        assert_eq!(status.age, Some(30));
        assert!(!status.stale);
    }

    #[test]
    fn age_falls_back_to_receipt_time() {
        let status = input(None, Some(900)).status(1_000);
        assert_eq!(status.age, Some(100));
        assert!(status.stale);
    }

    #[test]
    fn feeds_of_unknown_age_count_as_stale() {
        let status = input(None, None).status(1_000);
        assert_eq!(status.age, None);
        assert!(status.stale);
    }

    #[test]
    fn a_small_clock_skew_is_not_stale() {
        let status = input(Some(1_030), None).status(1_000);
        assert_eq!(status.age, Some(-30));
        assert!(!status.stale);
    }

    #[test]
    fn a_far_future_header_is_stale() {
        let status = input(Some(2_000), None).status(1_000);
        assert_eq!(status.age, Some(-1_000));
        assert!(status.stale);
    }

    #[test]
    fn aging_a_status_keeps_its_rules() {
        let status = input(Some(1_000), None).status(1_030);
        let later = status.aged_to(1_030, 1_100);
        assert_eq!(later.age, Some(100));
        assert!(later.stale);

        let undated = input(None, None).status(1_000).aged_to(1_000, 1_001);
        assert_eq!(undated.age, None);
        assert!(undated.stale);
    }

    #[test]
    fn a_feed_that_failed_to_decode_reports_why() {
        let input = FeedInput::from_bytes(
            FeedId::new("kcm").expect("non-empty"),
            FeedKind::Alerts,
            &[0xff, 0xff, 0xff],
        );
        let status = input.status(1_000);
        assert!(status.stale);
        assert!(matches!(status.error, Some(DecodeError::Malformed(_))));
    }
}
