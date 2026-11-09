//! Identifiers and time values.
//!
//! Everything a caller holds is a newtype, so a stop can't be passed where a route is expected.
//! Inside the route scan these unwrap to plain integers.

use nutype::nutype;

/// The largest absolute value a [`Time`] may hold, in seconds (about 17 years).
///
/// Keeps every sum of a time and a duration well inside `i32`,
/// and below the sentinel the route scan uses for "unreachable".
pub const TIME_LIMIT: i32 = 1 << 29;

/// The longest [`Seconds`] may be (about 8 years).
pub const SECONDS_LIMIT: u32 = 1 << 28;

// `nutype` validators need literals; these tie them to the limits above.
const _: () = assert!(TIME_LIMIT == 536_870_912 && SECONDS_LIMIT == 268_435_456);

/// A point in time: integer seconds from an origin chosen for the query's window.
///
/// Timetables hold seconds from the start of a service day
/// until they are materialized for a window (see [`ServiceDay::offset`]).
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display),
    validate(greater_or_equal = -536_870_912, less_or_equal = 536_870_912)
)]
pub struct Time(i32);

/// A non-negative span of integer seconds.
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display),
    validate(less_or_equal = 268_435_456)
)]
pub struct Seconds(u32);

impl Seconds {
    /// Zero seconds.
    pub const ZERO: Self = match Self::try_new(0) {
        Ok(zero) => zero,
        // Unreachable: zero satisfies the limit; a const context can't use unwrap.
        Err(_) => panic!("zero is below the limit"),
    };

    /// The span as a signed value, for arithmetic on search clocks.
    #[inline]
    pub(crate) fn as_i32(self) -> i32 {
        // The limit is far below `i32::MAX`.
        self.into_inner().cast_signed()
    }
}

/// A timetable stop.
///
/// Stops are numbered when a timetable is built; see [`crate::Timetable::stop_id`].
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct StopId(u32);

impl StopId {
    /// The stop's position in flat per-stop arrays.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.into_inner() as usize
    }
}

/// A route: one stop sequence with one set of attributes.
///
/// Routes are numbered by the order their patterns were added to the builder.
/// An overlay's added routes are numbered after all base routes.
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct RouteId(u32);

impl RouteId {
    /// The route's position in flat per-route arrays.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.into_inner() as usize
    }
}

/// A trip: the caller's own identifier, carried through to journeys.
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct TripId(u32);

/// A calendar: the set of dates on which some trips run.
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct ServiceId(u32);

impl ServiceId {
    /// The calendar's position in flat per-service arrays.
    #[inline]
    pub(crate) fn index(self) -> usize {
        self.into_inner() as usize
    }
}

/// A service date, in the caller's own numbering (for example, days since an epoch).
///
/// `valinor-transit` only compares these for equality.
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct ServiceDate(u32);

/// A slot in a materialized window: an index into [`crate::BaseWindow::days`].
#[nutype(
    const_fn,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Display)
)]
pub struct DaySlot(u8);

impl DaySlot {
    /// The slot's position in flat per-day arrays.
    #[inline]
    pub(crate) fn index(self) -> usize {
        usize::from(self.into_inner())
    }
}

/// A lane within a snapshot: which arena it is in, and its index there.
///
/// Journeys carry this so a response layer can find the lane again in the same snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LaneRef(u32);

impl LaneRef {
    const OVERLAY_BIT: u32 = 1 << 31;

    /// A lane of the base arena (`arena == 0`) or the overlay arena (`arena == 1`).
    #[inline]
    pub(crate) fn new(arena: u8, index: u32) -> Self {
        if arena == 0 {
            Self(index)
        } else {
            Self(index | Self::OVERLAY_BIT)
        }
    }

    /// Whether the lane is in the overlay arena rather than the base window.
    #[inline]
    pub fn is_overlay(self) -> bool {
        self.0 & Self::OVERLAY_BIT != 0
    }

    #[inline]
    pub(crate) fn arena(self) -> usize {
        usize::from(self.is_overlay())
    }

    /// The lane's index in its arena.
    #[inline]
    pub fn index(self) -> u32 {
        self.0 & !Self::OVERLAY_BIT
    }
}

/// One day of a materialized window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceDay {
    /// The date whose calendars select this day's trips.
    pub date: ServiceDate,
    /// The seconds from the window's origin to the start of this service day.
    ///
    /// A trip's service-day-relative times are shifted by this when it is materialized.
    pub offset: Time,
}

/// Which way a search moves through time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Time grows along the search; edges are followed as they are directed.
    Forward,
    /// Time shrinks along the search; edges are followed against their direction.
    Backward,
}

/// A trip as it ran on one service date.
///
/// GTFS-RT matches trips by this pair, never by feed entity ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TripKey {
    /// The caller's trip identifier.
    pub trip: TripId,
    /// The service date of this run of the trip.
    pub date: ServiceDate,
}
