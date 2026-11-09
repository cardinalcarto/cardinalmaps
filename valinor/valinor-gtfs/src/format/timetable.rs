//! Replays a compiled file into `valinor-transit`'s builder.

use valinor_transit::{
    BuildError, ResolvedTrip, RouteAttributes, RouteId, Seconds, ServiceDate, ServiceId, StopEvent,
    StopId, StopRules, StopTime, Time, Timetable, TimetableBuilder, TimetableSource, TransferPair,
    TripId,
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

        for row in tables.transfer_pairs {
            let (Some(&from), Some(&to)) = (
                keys.get(row.from.get() as usize),
                keys.get(row.to.get() as usize),
            ) else {
                return Err(DecodeError::InvalidReference {
                    table: "transfer pairs",
                    reason: "a transfer names an unknown stop",
                });
            };
            let floor =
                Seconds::try_new(row.floor.get()).map_err(|_| DecodeError::InvalidValue {
                    table: "transfer pairs",
                    reason: "a floor is out of range",
                })?;
            builder.add_transfer_pair(from, to, floor)?;
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

impl CompiledTimetable<'_> {
    /// Checks everything a window needs of this file that loading doesn't,
    /// so that materializing a window from it can't fail on its contents.
    ///
    /// It checks exactly what [`Self::to_timetable`] would have refused: pattern lengths and
    /// references, and every trip's calendar, times, and rules, without copying any of them.
    ///
    /// # Performance
    ///
    /// Reads every stop event once, in the file's own pages.
    ///
    /// # Errors
    ///
    /// If a table is inconsistent, or a time is out of range or out of order along a trip.
    pub fn check_routable(&self) -> Result<(), DecodeError> {
        let tables = &self.tables;
        let boardable =
            tables
                .stops
                .get(..tables.boardable_stops)
                .ok_or(DecodeError::InvalidValue {
                    table: "stops",
                    reason: "there are more boardable stops than rows",
                })?;
        for stop in boardable {
            Seconds::try_new(stop.min_change.get()).map_err(|_| DecodeError::InvalidValue {
                table: "stops",
                reason: "a minimum change time is out of range",
            })?;
        }
        for pattern in tables.patterns {
            let count = pattern.stop_count.get() as usize;
            let start = pattern.stops_start.get() as usize;
            let stops = tables
                .pattern_stops
                .get(start..start.saturating_add(count))
                .ok_or(DecodeError::InvalidReference {
                    table: "patterns",
                    reason: "a pattern's stops leave the table",
                })?;
            if stops.len() < 2 {
                return Err(BuildError::PatternTooShort(stops.len()).into());
            }
            if stops
                .iter()
                .any(|stop| stop.get() as usize >= boardable.len())
            {
                return Err(DecodeError::InvalidReference {
                    table: "pattern stops",
                    reason: "a pattern visits an unknown stop",
                });
            }
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
                if trip.service.get() as usize >= tables.services.len() {
                    return Err(
                        BuildError::UnknownService(ServiceId::new(trip.service.get())).into(),
                    );
                }
                let times_start = trip.times_start.get() as usize;
                let times = tables
                    .times
                    .get(times_start..times_start.saturating_add(count))
                    .ok_or(DecodeError::InvalidReference {
                        table: "trips",
                        reason: "a trip's times leave the table",
                    })?;
                if trip.rules_start.get() != NONE {
                    let rules_start = trip.rules_start.get() as usize;
                    if tables
                        .rules
                        .get(rules_start..rules_start.saturating_add(count))
                        .is_none()
                    {
                        return Err(DecodeError::InvalidReference {
                            table: "trips",
                            reason: "a trip's rules leave the table",
                        });
                    }
                }
                let trip_id = trips_start.saturating_add(row);
                let out_of_order = || BuildError::TripTimesOutOfOrder {
                    trip: TripId::new(u32::try_from(trip_id).unwrap_or(u32::MAX)),
                    position: 0,
                };
                for (position, pair) in times.iter().enumerate() {
                    let (arrival, departure) = (pair.arrival.get(), pair.departure.get());
                    time_of(arrival)?;
                    time_of(departure)?;
                    if arrival > departure {
                        return Err(out_of_order().into());
                    }
                    if let Some(next) = times.get(position + 1)
                        && departure > next.arrival.get()
                    {
                        return Err(out_of_order().into());
                    }
                }
            }
        }
        Ok(())
    }
}

impl TimetableSource for CompiledTimetable<'_> {
    fn stop_count(&self) -> usize {
        self.tables.boardable_stops
    }

    fn route_count(&self) -> usize {
        self.tables.patterns.len()
    }

    fn min_change(&self, stop: usize) -> i32 {
        self.tables.stops.get(stop).map_or(0, |row| {
            i32::try_from(row.min_change.get()).unwrap_or(i32::MAX)
        })
    }

    fn transfer_pairs(&self) -> Vec<TransferPair> {
        self.tables
            .transfer_pairs
            .iter()
            .filter_map(|row| {
                Some(TransferPair {
                    from: StopId::new(row.from.get()),
                    to: StopId::new(row.to.get()),
                    floor: Seconds::try_new(row.floor.get()).ok()?,
                })
            })
            .collect()
    }

    fn route_stops_of(&self, route: RouteId) -> Result<Vec<StopId>, BuildError> {
        let pattern = self
            .tables
            .patterns
            .get(route.into_inner() as usize)
            .ok_or(BuildError::UnknownRoute(route))?;
        let start = pattern.stops_start.get() as usize;
        let count = pattern.stop_count.get() as usize;
        let stops = self
            .tables
            .pattern_stops
            .get(start..start.saturating_add(count))
            .ok_or(BuildError::UnknownRoute(route))?;
        Ok(stops.iter().map(|stop| StopId::new(stop.get())).collect())
    }

    fn route_attributes_of(&self, route: RouteId) -> Result<RouteAttributes, BuildError> {
        let pattern = self
            .tables
            .patterns
            .get(route.into_inner() as usize)
            .ok_or(BuildError::UnknownRoute(route))?;
        Ok(if pattern.attributes & PATTERN_BIKES_ALLOWED != 0 {
            RouteAttributes::BIKES_ALLOWED
        } else {
            RouteAttributes::NONE
        })
    }

    fn resolved_trips_on(
        &self,
        route: RouteId,
        date: ServiceDate,
        offset: Time,
    ) -> Result<Vec<ResolvedTrip>, BuildError> {
        let tables = &self.tables;
        let pattern = tables
            .patterns
            .get(route.into_inner() as usize)
            .ok_or(BuildError::UnknownRoute(route))?;
        let stop_count = pattern.stop_count.get() as usize;
        let trips_start = pattern.trips_start.get() as usize;
        let trip_count = pattern.trip_count.get() as usize;
        let trips = tables
            .trips
            .get(trips_start..trips_start.saturating_add(trip_count))
            .ok_or(BuildError::UnknownRoute(route))?;
        let mut resolved = Vec::new();
        for (row, trip) in trips.iter().enumerate() {
            if !self.service_active(trip.service.get(), date) {
                continue;
            }
            let times_start = trip.times_start.get() as usize;
            let times = tables
                .times
                .get(times_start..times_start.saturating_add(stop_count))
                .ok_or(BuildError::UnknownRoute(route))?;
            let rules: &[u8] = if trip.rules_start.get() == NONE {
                &[]
            } else {
                let start = trip.rules_start.get() as usize;
                tables
                    .rules
                    .get(start..start.saturating_add(stop_count))
                    .ok_or(BuildError::UnknownRoute(route))?
            };
            let mut events = Vec::with_capacity(stop_count);
            for (position, pair) in times.iter().enumerate() {
                let rule = rules.get(position).copied().unwrap_or(0);
                events.push(StopEvent {
                    arrival: shifted(pair.arrival.get(), offset)?,
                    departure: shifted(pair.departure.get(), offset)?,
                    rules: stop_rules(rule),
                });
            }
            let trip_id =
                u32::try_from(trips_start + row).map_err(|_| BuildError::TableTooLarge)?;
            resolved.push(ResolvedTrip {
                trip: TripId::new(trip_id),
                events,
            });
        }
        Ok(resolved)
    }
}

/// A file's service-day-relative time on a window's clock.
fn shifted(seconds: i32, offset: Time) -> Result<Time, BuildError> {
    let sum = i64::from(seconds) + i64::from(offset.into_inner());
    i32::try_from(sum)
        .ok()
        .and_then(|sum| Time::try_new(sum).ok())
        .ok_or(BuildError::TimeOutOfRange(sum))
}

fn time_of(seconds: i32) -> Result<Time, DecodeError> {
    Time::try_new(seconds).map_err(|_| DecodeError::InvalidValue {
        table: "times",
        reason: "a time is out of range",
    })
}
