//! Replays a compiled file into `valinor-transit`'s builder.

use valinor_transit::{
    RouteAttributes, Seconds, ServiceDate, ServiceId, StopId, StopRules, StopTime, Time, Timetable,
    TimetableBuilder, TripId,
};

use crate::error::DecodeError;
use crate::format::layout::{NONE, PATTERN_BIKES_ALLOWED, RULE_DROP_OFF_SHIFT, RULE_PICKUP_MASK};
use crate::format::reader::CompiledTimetable;

/// Does a GTFS pickup or drop-off type admit riders?
///
/// `1` is the only value that means no boarding at all.
/// Phone and driver-coordinated stops stay routable; riders see the requirement from the type.
pub(crate) fn admits_rider(gtfs_type: u8) -> bool {
    gtfs_type != 1
}

/// Turns a rule byte into the routing rules the core understands.
pub(crate) fn stop_rules(byte: u8) -> StopRules {
    StopRules {
        pickup: admits_rider(byte & RULE_PICKUP_MASK),
        drop_off: admits_rider((byte >> RULE_DROP_OFF_SHIFT) & RULE_PICKUP_MASK),
    }
}

impl CompiledTimetable<'_> {
    /// Builds a [`Timetable`] from this file.
    ///
    /// Trip IDs in the timetable are row numbers in this file,
    /// and each timetable stop has the [`StopId`] it has in this file.
    ///
    /// # Performance
    ///
    /// Copies every stop event into the timetable's own allocations.
    /// The file's checks guarantee that this allocates no more than a fixed multiple of its size.
    ///
    /// # Errors
    ///
    /// If `valinor-transit` rejects the data or numbers a stop differently from the file.
    #[allow(
        clippy::too_many_lines,
        reason = "one linear replay of the file's tables"
    )]
    pub fn to_timetable(&self) -> Result<Timetable, DecodeError> {
        let tables = &self.tables;
        let mut builder = TimetableBuilder::new();

        let boardable =
            tables
                .stops
                .get(..tables.boardable_stops)
                .ok_or(DecodeError::InvalidValue {
                    table: "stops",
                    reason: "there are more boardable stops than rows",
                })?;
        let mut keys = Vec::with_capacity(boardable.len());
        for stop in boardable {
            let min_change =
                Seconds::try_new(stop.min_change.get()).map_err(|_| DecodeError::InvalidValue {
                    table: "stops",
                    reason: "a minimum change time is out of range",
                })?;
            keys.push(builder.add_stop(min_change)?);
        }

        for service in tables.services {
            builder.add_service(self.service_dates(service).collect::<Vec<ServiceDate>>())?;
        }

        let mut scratch = Vec::new();
        for pattern in tables.patterns {
            let start = pattern.stops_start.get() as usize;
            let count = pattern.stop_count.get() as usize;
            let stops = tables
                .pattern_stops
                .get(start..start.saturating_add(count))
                .ok_or(DecodeError::InvalidReference {
                    table: "patterns",
                    reason: "a pattern's stops leave the table",
                })?;
            scratch.clear();
            for stop in stops {
                let key = keys
                    .get(stop.get() as usize)
                    .ok_or(DecodeError::InvalidReference {
                        table: "pattern stops",
                        reason: "a pattern visits an unknown stop",
                    })?;
                scratch.push(*key);
            }
            let attributes = if pattern.attributes & PATTERN_BIKES_ALLOWED != 0 {
                RouteAttributes::BIKES_ALLOWED
            } else {
                RouteAttributes::NONE
            };
            let key = builder.add_pattern(&scratch, attributes)?;

            let trips_start = pattern.trips_start.get() as usize;
            let trip_count = pattern.trip_count.get() as usize;
            let trips = tables
                .trips
                .get(trips_start..trips_start.saturating_add(trip_count))
                .ok_or(DecodeError::InvalidReference {
                    table: "patterns",
                    reason: "a pattern's trips leave the table",
                })?;
            for (row, trip) in trips.iter().enumerate() {
                let times_start = trip.times_start.get() as usize;
                let times = tables
                    .times
                    .get(times_start..times_start.saturating_add(count))
                    .ok_or(DecodeError::InvalidReference {
                        table: "trips",
                        reason: "a trip's times leave the table",
                    })?;
                let rules: &[u8] = if trip.rules_start.get() == NONE {
                    &[]
                } else {
                    let start = trip.rules_start.get() as usize;
                    tables.rules.get(start..start.saturating_add(count)).ok_or(
                        DecodeError::InvalidReference {
                            table: "trips",
                            reason: "a trip's rules leave the table",
                        },
                    )?
                };
                let mut events = Vec::with_capacity(count);
                for (position, pair) in times.iter().enumerate() {
                    let rule = rules.get(position).copied().unwrap_or(0);
                    events.push(StopTime {
                        arrival: time_of(pair.arrival.get())?,
                        departure: time_of(pair.departure.get())?,
                        rules: stop_rules(rule),
                    });
                }
                let trip_index = u32::try_from(trips_start.saturating_add(row)).map_err(|_| {
                    DecodeError::InvalidValue {
                        table: "trips",
                        reason: "a trip index does not fit",
                    }
                })?;
                builder.add_trip(
                    key,
                    TripId::new(trip_index),
                    ServiceId::new(trip.service.get()),
                    events,
                )?;
            }
        }

        let timetable = builder.build()?;
        for (index, key) in keys.iter().enumerate() {
            let assigned = timetable.stop_id(*key);
            let expected = u32::try_from(index).map_err(|_| DecodeError::InvalidValue {
                table: "stops",
                reason: "a stop index does not fit",
            })?;
            if assigned != StopId::new(expected) {
                return Err(DecodeError::StopRenumbered {
                    stop: expected,
                    assigned: assigned.into_inner(),
                });
            }
        }
        Ok(timetable)
    }
}

fn time_of(seconds: i32) -> Result<Time, DecodeError> {
    Time::try_new(seconds).map_err(|_| DecodeError::InvalidValue {
        table: "times",
        reason: "a time is out of range",
    })
}
