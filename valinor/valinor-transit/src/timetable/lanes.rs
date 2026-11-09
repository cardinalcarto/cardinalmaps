//! Resolved trips and lanes: the input to every block, base and overlay alike.
//!
//! A *route* is a stop sequence plus attributes.
//! A *lane* is a set of its trips that share pickup and drop-off rules
//! and never overtake one another, so a RAPTOR scan may take the first boardable trip
//! and know that no later trip in the lane can do better.
//!
//! Splitting by rules is what keeps the scan free of per-trip checks:
//! pickup and drop-off become one flag per stop position of a lane.

use std::collections::BTreeMap;

use crate::error::BuildError;
use crate::ids::{Time, TripId};

/// Bit of a lane's rule byte: a rider may board at this position.
pub(crate) const RULE_PICKUP: u8 = 1;
/// Bit of a lane's rule byte: a rider may alight at this position.
pub(crate) const RULE_DROP_OFF: u8 = 2;
/// How far the wheelchair copies of those two bits are shifted up.
pub(crate) const RULE_WHEELCHAIR_SHIFT: u8 = 2;

/// What may happen at one stop of a trip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StopRules {
    /// Riders may board here.
    pub pickup: bool,
    /// Riders may alight here.
    pub drop_off: bool,
}

impl StopRules {
    /// Boarding and alighting are both allowed.
    pub const REGULAR: Self = Self {
        pickup: true,
        drop_off: true,
    };

    /// Neither boarding nor alighting: a skipped stop.
    pub const SKIPPED: Self = Self {
        pickup: false,
        drop_off: false,
    };

    pub(crate) fn encode(self) -> u8 {
        (u8::from(self.pickup) * RULE_PICKUP) | (u8::from(self.drop_off) * RULE_DROP_OFF)
    }

    pub(crate) fn decode(bits: u8) -> Self {
        Self {
            pickup: bits & RULE_PICKUP != 0,
            drop_off: bits & RULE_DROP_OFF != 0,
        }
    }
}

/// One stop of a trip, in times already resolved for a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopEvent {
    /// When the vehicle arrives.
    pub arrival: Time,
    /// When the vehicle departs.
    pub departure: Time,
    /// What riders may do here.
    pub rules: StopRules,
}

/// A trip resolved to times in a window, ready to be laid out in a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTrip {
    /// The caller's trip identifier.
    pub trip: TripId,
    /// One event per stop of the route.
    pub events: Vec<StopEvent>,
}

/// Trips that share rules and never overtake, in travel order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLane {
    trips: Vec<ResolvedTrip>,
}

impl ResolvedLane {
    /// Validates a lane given in row order.
    ///
    /// # Errors
    ///
    /// If trips have inconsistent stop counts or rules, their times don't move forward,
    /// or one overtakes another.
    pub fn new(trips: Vec<ResolvedTrip>) -> Result<Self, BuildError> {
        for trip in &trips {
            check_trip_order(trip)?;
        }
        for pair in trips.windows(2) {
            let [earlier, later] = pair else { continue };
            if earlier.events.len() != later.events.len() {
                return Err(BuildError::WrongStopCount {
                    trip: later.trip,
                    expected: earlier.events.len(),
                    got: later.events.len(),
                });
            }
            for (position, (a, b)) in earlier.events.iter().zip(&later.events).enumerate() {
                if a.rules != b.rules {
                    return Err(BuildError::LaneOvertakes { position });
                }
                if a.arrival > b.arrival || a.departure > b.departure {
                    return Err(BuildError::LaneOvertakes { position });
                }
            }
        }
        Ok(Self { trips })
    }

    /// The lane's trips in row order.
    pub fn trips(&self) -> &[ResolvedTrip] {
        &self.trips
    }

    /// The rule at each stop position (the lane's trips all share them).
    pub(crate) fn rules_bytes(&self) -> Vec<u8> {
        self.trips.first().map_or_else(Vec::new, |trip| {
            trip.events.iter().map(|e| e.rules.encode()).collect()
        })
    }
}

/// The lanes of one route on one service day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedBlock {
    lanes: Vec<ResolvedLane>,
}

impl ResolvedBlock {
    /// Takes lanes that are already split and validated.
    ///
    /// A lane left with no trips, as when every trip in it is canceled, is dropped:
    /// a block's lanes always have at least one trip, so an empty one can't reach an arena.
    pub fn from_lanes(mut lanes: Vec<ResolvedLane>) -> Self {
        lanes.retain(|lane| !lane.trips.is_empty());
        Self { lanes }
    }

    /// Groups trips by their rules, and splits each group into non-overtaking lanes.
    ///
    /// Trips within a group are ordered by their first departure,
    /// then each goes into the first lane it doesn't overtake.
    ///
    /// # Errors
    ///
    /// If a trip's times don't move forward along the trip.
    pub fn from_trips(trips: Vec<ResolvedTrip>) -> Result<Self, BuildError> {
        let mut by_rules: BTreeMap<Vec<u8>, Vec<ResolvedTrip>> = BTreeMap::new();
        for trip in trips {
            check_trip_order(&trip)?;
            let key = trip.events.iter().map(|e| e.rules.encode()).collect();
            by_rules.entry(key).or_default().push(trip);
        }
        let mut lanes: Vec<ResolvedLane> = Vec::new();
        for (_rules, mut group) in by_rules {
            group.sort_by_key(|trip| {
                (
                    trip.events.first().map(|e| e.departure),
                    trip.events.last().map(|e| e.arrival),
                )
            });
            let mut split: Vec<Vec<ResolvedTrip>> = Vec::new();
            for trip in group {
                match split
                    .iter_mut()
                    .find(|lane| lane.last().is_some_and(|last| !overtakes(last, &trip)))
                {
                    Some(lane) => lane.push(trip),
                    None => split.push(vec![trip]),
                }
            }
            for trips in split {
                lanes.push(ResolvedLane { trips });
            }
        }
        Ok(Self { lanes })
    }

    /// The block's lanes.
    pub fn lanes(&self) -> &[ResolvedLane] {
        &self.lanes
    }

    /// The trip count of all lanes combined.
    pub fn trip_count(&self) -> usize {
        self.lanes.iter().map(|lane| lane.trips.len()).sum()
    }
}

/// Whether placing `next` after `last` in a lane breaks non-overtaking.
fn overtakes(last: &ResolvedTrip, next: &ResolvedTrip) -> bool {
    last.events.len() != next.events.len()
        || last
            .events
            .iter()
            .zip(&next.events)
            .any(|(a, b)| a.departure > b.departure || a.arrival > b.arrival)
}

/// Requires each trip to move forward: arrival no later than departure at each stop,
/// and departure no later than the next arrival.
fn check_trip_order(trip: &ResolvedTrip) -> Result<(), BuildError> {
    for (position, event) in trip.events.iter().enumerate() {
        if event.arrival > event.departure {
            return Err(BuildError::TripTimesOutOfOrder {
                trip: trip.trip,
                position,
            });
        }
        if let Some(next) = trip.events.get(position + 1)
            && event.departure > next.arrival
        {
            return Err(BuildError::TripTimesOutOfOrder {
                trip: trip.trip,
                position,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(seconds: i32) -> Time {
        Time::try_new(seconds).expect("in range")
    }

    fn trip(id: u32, times: &[i32], rules: StopRules) -> ResolvedTrip {
        ResolvedTrip {
            trip: TripId::new(id),
            events: times
                .iter()
                .map(|&time| StopEvent {
                    arrival: t(time),
                    departure: t(time),
                    rules,
                })
                .collect(),
        }
    }

    #[test]
    fn overtaking_trips_split_into_lanes() {
        let block = ResolvedBlock::from_trips(vec![
            trip(1, &[0, 100, 200], StopRules::REGULAR),
            // Departs later but arrives earlier: it can't share a lane with trip 1.
            trip(2, &[10, 50, 90], StopRules::REGULAR),
            trip(3, &[20, 120, 220], StopRules::REGULAR),
        ])
        .expect("valid");
        assert_eq!(block.lanes().len(), 2);
        assert_eq!(
            block
                .lanes()
                .iter()
                .map(|lane| lane.trips().iter().map(|t| t.trip.into_inner()).collect())
                .collect::<Vec<Vec<u32>>>(),
            vec![vec![1, 3], vec![2]],
        );
    }

    #[test]
    fn differing_rules_never_share_a_lane() {
        let skipped = StopRules {
            pickup: false,
            drop_off: true,
        };
        let block = ResolvedBlock::from_trips(vec![
            trip(1, &[0, 100], StopRules::REGULAR),
            trip(2, &[10, 110], skipped),
        ])
        .expect("valid");
        assert_eq!(block.lanes().len(), 2);
    }

    #[test]
    fn out_of_order_trip_is_rejected() {
        let bad = trip(9, &[100, 50], StopRules::REGULAR);
        assert_eq!(
            ResolvedBlock::from_trips(vec![bad]),
            Err(BuildError::TripTimesOutOfOrder {
                trip: TripId::new(9),
                position: 0,
            })
        );
    }

    #[test]
    fn lane_constructor_rejects_overtaking() {
        let err = ResolvedLane::new(vec![
            trip(1, &[0, 100], StopRules::REGULAR),
            trip(2, &[10, 90], StopRules::REGULAR),
        ]);
        assert_eq!(err, Err(BuildError::LaneOvertakes { position: 1 }));
    }

    #[test]
    fn empty_lanes_are_dropped_from_a_block() {
        let lane = ResolvedLane::new(vec![trip(1, &[0, 100], StopRules::REGULAR)]).expect("valid");
        let empty = ResolvedLane::new(Vec::new()).expect("a lane may have lost all of its trips");
        let block = ResolvedBlock::from_lanes(vec![empty.clone(), lane, empty]);
        assert_eq!(block.lanes().len(), 1);
        assert_eq!(block.trip_count(), 1);
    }

    #[test]
    fn identical_times_share_a_lane() {
        let block = ResolvedBlock::from_trips(vec![
            trip(1, &[0, 100], StopRules::REGULAR),
            trip(2, &[0, 100], StopRules::REGULAR),
        ])
        .expect("valid");
        assert_eq!(block.lanes().len(), 1);
    }
}
