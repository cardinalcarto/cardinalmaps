//! Checks every reference and ordering in a file before anything reads it.
//!
//! A flipped byte could pass a CRC by chance, and a deliberately crafted file can always pass it,
//! so loading can't rely on the checksum for safety.
//! After this pass every accessor is a plain bounds-checked read
//! and every value is one its readers expect.

use crate::error::DecodeError;
use crate::format::layout::{NONE, PATTERN_BIKES_ALLOWED};
use crate::format::reader::Tables;
use zerocopy::little_endian::U32;

/// The longest calendar a file may hold (over a hundred years).
const MAX_SERVICE_DAYS: u32 = 40_000;

/// Maps a table's name and message to an error.
fn fail(table: &'static str, reason: &'static str) -> DecodeError {
    DecodeError::InvalidReference { table, reason }
}

fn check_range(
    table: &'static str,
    reason: &'static str,
    start: u32,
    count: u32,
    len: usize,
) -> Result<(), DecodeError> {
    let end = start
        .checked_add(count)
        .ok_or_else(|| fail(table, reason))?;
    if end as usize > len {
        return Err(fail(table, reason));
    }
    Ok(())
}

fn check_index(
    table: &'static str,
    reason: &'static str,
    index: u32,
    len: usize,
) -> Result<(), DecodeError> {
    if index as usize >= len {
        return Err(fail(table, reason));
    }
    Ok(())
}

fn check_string(table: &'static str, tables: &Tables<'_>, id: U32) -> Result<(), DecodeError> {
    check_index(
        table,
        "a string reference is out of range",
        id.get(),
        tables.string_offsets.len().saturating_sub(1),
    )
}

/// Validates a whole file. `tables` is not read until this returns.
pub(crate) fn validate(tables: &Tables<'_>) -> Result<(), DecodeError> {
    validate_strings(tables)?;
    validate_feeds_and_zones(tables)?;
    validate_stops(tables)?;
    validate_routes(tables)?;
    validate_services(tables)?;
    validate_trips(tables)?;
    validate_shapes(tables)?;
    validate_index("trip index", tables, tables.trip_index, tables.trips.len())?;
    validate_index(
        "route index",
        tables,
        tables.route_index,
        tables.routes.len(),
    )?;
    validate_index("stop index", tables, tables.stop_index, tables.stops.len())?;
    Ok(())
}

fn validate_strings(tables: &Tables<'_>) -> Result<(), DecodeError> {
    let Some((last, _)) = tables.string_offsets.split_last() else {
        return Err(DecodeError::InvalidStrings("there are no string offsets"));
    };
    if tables.string_offsets.first().map(|o| o.get()) != Some(0) {
        return Err(DecodeError::InvalidStrings("the first offset is not zero"));
    }
    if last.get() as usize != tables.strings.len() {
        return Err(DecodeError::InvalidStrings(
            "the last offset is not the end of the pool",
        ));
    }
    let mut previous = 0;
    for offset in tables.string_offsets {
        let offset = offset.get();
        if offset < previous {
            return Err(DecodeError::InvalidStrings("the offsets go backward"));
        }
        if !tables.strings.is_char_boundary(offset as usize) {
            return Err(DecodeError::InvalidStrings(
                "an offset is inside a character",
            ));
        }
        previous = offset;
    }
    // The first string is the empty string, which absent fields point at.
    if tables.string_offsets.get(1).is_none_or(|o| o.get() != 0) {
        return Err(DecodeError::InvalidStrings(
            "the first string is not the empty string",
        ));
    }
    Ok(())
}

fn validate_feeds_and_zones(tables: &Tables<'_>) -> Result<(), DecodeError> {
    if tables.feeds.is_empty() {
        return Err(fail("feeds", "the file has no feeds"));
    }
    for feed in tables.feeds {
        check_string("feeds", tables, feed.id)?;
        check_string("feeds", tables, feed.version)?;
        check_index(
            "feeds",
            "a zone reference is out of range",
            feed.zone.get(),
            tables.zones.len(),
        )?;
    }
    for zone in tables.zones {
        check_string("zones", tables, zone.name)?;
        if zone.transition_count.get() == 0 {
            return Err(fail("zones", "a zone has no transitions"));
        }
        check_range(
            "zones",
            "a zone's transitions leave the table",
            zone.transitions_start.get(),
            zone.transition_count.get(),
            tables.zone_transitions.len(),
        )?;
        let start = zone.transitions_start.get() as usize;
        let rows = tables
            .zone_transitions
            .get(start..start + zone.transition_count.get() as usize)
            .ok_or_else(|| fail("zones", "a zone's transitions leave the table"))?;
        if !rows.is_sorted_by_key(|row| row.at_unix.get()) {
            return Err(fail("zones", "a zone's transitions are out of order"));
        }
    }
    if tables.noon_offsets.len() < 2 {
        return Err(fail(
            "noon offsets",
            "the table needs at least one row and a terminator",
        ));
    }
    if !tables
        .noon_offsets
        .is_sorted_by_key(|row| row.first_date.get())
    {
        return Err(fail("noon offsets", "the table is out of order"));
    }
    Ok(())
}

fn validate_stops(tables: &Tables<'_>) -> Result<(), DecodeError> {
    if tables.boardable_stops > tables.stops.len() {
        return Err(fail("stops", "there are more boardable stops than rows"));
    }
    for stop in tables.stops {
        check_index(
            "stops",
            "a feed reference is out of range",
            stop.feed.get(),
            tables.feeds.len(),
        )?;
        check_string("stops", tables, stop.gtfs_id)?;
        check_string("stops", tables, stop.name)?;
        check_string("stops", tables, stop.code)?;
        check_string("stops", tables, stop.platform)?;
        if stop.parent.get() != NONE {
            check_index(
                "stops",
                "a parent reference is out of range",
                stop.parent.get(),
                tables.stops.len(),
            )?;
        }
        check_index(
            "stops",
            "a zone reference is out of range",
            stop.zone.get(),
            tables.zones.len(),
        )?;
        check_range(
            "stops",
            "a stop's children leave the table",
            stop.children_start.get(),
            stop.child_count.get(),
            tables.child_stops.len(),
        )?;
        if stop.location_type > 4 {
            return Err(DecodeError::InvalidValue {
                table: "stops",
                reason: "a location type is not defined by GTFS",
            });
        }
        let (lat, lon) = (stop.lat_e7.get(), stop.lon_e7.get());
        let absent = lat == i32::MIN && lon == i32::MIN;
        let valid = (-900_000_000..=900_000_000).contains(&lat)
            && (-1_800_000_000..=1_800_000_000).contains(&lon);
        if !absent && !valid {
            return Err(DecodeError::InvalidValue {
                table: "stops",
                reason: "a coordinate is out of range",
            });
        }
    }
    for child in tables.child_stops {
        check_index(
            "child stops",
            "a child is not a boardable stop",
            child.get(),
            tables.boardable_stops,
        )?;
    }
    Ok(())
}

fn validate_routes(tables: &Tables<'_>) -> Result<(), DecodeError> {
    for agency in tables.agencies {
        check_index(
            "agencies",
            "a feed reference is out of range",
            agency.feed.get(),
            tables.feeds.len(),
        )?;
        for id in [
            agency.gtfs_id,
            agency.name,
            agency.url,
            agency.phone,
            agency.fare_url,
        ] {
            check_string("agencies", tables, id)?;
        }
    }
    for route in tables.routes {
        check_index(
            "routes",
            "a feed reference is out of range",
            route.feed.get(),
            tables.feeds.len(),
        )?;
        if route.agency.get() != NONE {
            check_index(
                "routes",
                "an agency reference is out of range",
                route.agency.get(),
                tables.agencies.len(),
            )?;
        }
        for id in [
            route.gtfs_id,
            route.short_name,
            route.long_name,
            route.desc,
            route.url,
        ] {
            check_string("routes", tables, id)?;
        }
        check_range(
            "routes",
            "a route's patterns leave the table",
            route.patterns_start.get(),
            route.pattern_count.get(),
            tables.route_patterns.len(),
        )?;
    }
    for pattern in tables.route_patterns {
        check_index(
            "route patterns",
            "a pattern reference is out of range",
            pattern.get(),
            tables.patterns.len(),
        )?;
    }
    Ok(())
}

fn validate_services(tables: &Tables<'_>) -> Result<(), DecodeError> {
    for service in tables.services {
        check_index(
            "services",
            "a feed reference is out of range",
            service.feed.get(),
            tables.feeds.len(),
        )?;
        check_string("services", tables, service.gtfs_id)?;
        let days = service.day_count.get();
        if days > MAX_SERVICE_DAYS {
            return Err(DecodeError::InvalidValue {
                table: "services",
                reason: "a calendar spans more days than a file may hold",
            });
        }
        if service.first_date.get().checked_add(days).is_none() {
            return Err(DecodeError::InvalidValue {
                table: "services",
                reason: "a calendar's dates overflow",
            });
        }
        check_range(
            "services",
            "a calendar's bits leave the table",
            service.bits_start.get(),
            days.div_ceil(8),
            tables.service_bits.len(),
        )?;
    }
    Ok(())
}

/// Walks patterns and trips together.
///
/// Trips must partition by pattern, and each stop event must have exactly one row in
/// the time table, so that loading can't be made to allocate more events than the file holds.
fn validate_trips(tables: &Tables<'_>) -> Result<(), DecodeError> {
    if tables.trip_riders.len() != tables.trips.len() {
        return Err(fail("trip riders", "there is not one row per trip"));
    }
    let mut next_trip = 0_usize;
    let mut next_time = 0_usize;
    for (index, pattern) in tables.patterns.iter().enumerate() {
        check_index(
            "patterns",
            "a route reference is out of range",
            pattern.route.get(),
            tables.routes.len(),
        )?;
        let stop_count = pattern.stop_count.get() as usize;
        if stop_count < 2 {
            return Err(DecodeError::InvalidValue {
                table: "patterns",
                reason: "a pattern has fewer than two stops",
            });
        }
        if pattern.attributes & !PATTERN_BIKES_ALLOWED != 0 {
            return Err(DecodeError::InvalidValue {
                table: "patterns",
                reason: "a pattern has an attribute this version does not define",
            });
        }
        check_range(
            "patterns",
            "a pattern's stops leave the table",
            pattern.stops_start.get(),
            pattern.stop_count.get(),
            tables.pattern_stops.len(),
        )?;
        if pattern.trips_start.get() as usize != next_trip {
            return Err(fail("patterns", "trips do not partition by pattern"));
        }
        let trip_count = pattern.trip_count.get() as usize;
        let trips = tables
            .trips
            .get(next_trip..next_trip.saturating_add(trip_count))
            .ok_or_else(|| fail("patterns", "a pattern's trips leave the table"))?;
        for trip in trips {
            if trip.pattern.get() as usize != index {
                return Err(fail("trips", "a trip is not filed under its pattern"));
            }
            check_index(
                "trips",
                "a service reference is out of range",
                trip.service.get(),
                tables.services.len(),
            )?;
            if trip.times_start.get() as usize != next_time {
                return Err(fail("trips", "times do not partition by trip"));
            }
            next_time = next_time.saturating_add(stop_count);
            if trip.rules_start.get() != NONE {
                check_range(
                    "trips",
                    "a trip's rules leave the table",
                    trip.rules_start.get(),
                    pattern.stop_count.get(),
                    tables.rules.len(),
                )?;
            }
        }
        next_trip += trip_count;
    }
    if next_trip != tables.trips.len() {
        return Err(fail("trips", "some trips belong to no pattern"));
    }
    if next_time != tables.times.len() {
        return Err(fail("times", "some times belong to no trip"));
    }
    for stop in tables.pattern_stops {
        check_index(
            "pattern stops",
            "a pattern visits an unboardable stop",
            stop.get(),
            tables.boardable_stops,
        )?;
    }
    validate_trip_riders(tables)
}

/// Checks the rider data of each trip against that trip's pattern.
fn validate_trip_riders(tables: &Tables<'_>) -> Result<(), DecodeError> {
    for (trip, rider) in tables.trips.iter().zip(tables.trip_riders) {
        let pattern = tables
            .patterns
            .get(trip.pattern.get() as usize)
            .ok_or_else(|| fail("trip riders", "a trip's pattern doesn't exist"))?;
        let count = pattern.stop_count.get();
        for id in [rider.gtfs_id, rider.headsign, rider.block] {
            check_string("trip riders", tables, id)?;
        }
        if rider.shape.get() != NONE {
            check_index(
                "trip riders",
                "a shape reference is out of range",
                rider.shape.get(),
                tables.shapes.len(),
            )?;
        }
        if rider.stop_headsigns_start.get() != NONE {
            check_range(
                "trip riders",
                "a trip's stop headsigns leave the table",
                rider.stop_headsigns_start.get(),
                count,
                tables.stop_headsigns.len(),
            )?;
        }
        check_range(
            "trip riders",
            "a trip's stop sequences leave the table",
            rider.stop_sequences_start.get(),
            count,
            tables.stop_sequences.len(),
        )?;
        if rider.stop_dists_start.get() != NONE {
            check_range(
                "trip riders",
                "a trip's stop distances leave the table",
                rider.stop_dists_start.get(),
                count,
                tables.stop_dists.len(),
            )?;
        }
        if rider.wheelchair > 2 || rider.bikes > 2 {
            return Err(DecodeError::InvalidValue {
                table: "trip riders",
                reason: "an accessibility flag is not defined by GTFS",
            });
        }
    }
    for id in tables.stop_headsigns {
        check_string("stop headsigns", tables, *id)?;
    }
    Ok(())
}

fn validate_shapes(tables: &Tables<'_>) -> Result<(), DecodeError> {
    for shape in tables.shapes {
        check_index(
            "shapes",
            "a feed reference is out of range",
            shape.feed.get(),
            tables.feeds.len(),
        )?;
        check_string("shapes", tables, shape.gtfs_id)?;
        check_range(
            "shapes",
            "a shape's points leave the table",
            shape.points_start.get(),
            shape.point_count.get(),
            tables.shape_points.len(),
        )?;
        let start = shape.points_start.get() as usize;
        let points = tables
            .shape_points
            .get(start..start.saturating_add(shape.point_count.get() as usize))
            .ok_or_else(|| fail("shapes", "a shape's points leave the table"))?;
        let mut previous = f32::NEG_INFINITY;
        for point in points {
            let dist = point.dist.get();
            if !dist.is_finite() || dist < previous {
                return Err(DecodeError::InvalidValue {
                    table: "shape points",
                    reason: "distances are not finite and non-decreasing",
                });
            }
            previous = dist;
        }
    }
    for dist in tables.stop_dists {
        if !dist.get().is_finite() {
            return Err(DecodeError::InvalidValue {
                table: "stop distances",
                reason: "a distance is not finite",
            });
        }
    }
    Ok(())
}

fn validate_index(
    table: &'static str,
    tables: &Tables<'_>,
    index: &[crate::format::layout::IdIndexRow],
    rows: usize,
) -> Result<(), DecodeError> {
    let mut previous: Option<(u32, &str, u32)> = None;
    for row in index {
        check_index(
            table,
            "a feed reference is out of range",
            row.feed.get(),
            tables.feeds.len(),
        )?;
        check_string(table, tables, row.id)?;
        check_index(
            table,
            "a row reference is out of range",
            row.row.get(),
            rows,
        )?;
        let key = (
            row.feed.get(),
            tables.string(row.id).unwrap_or(""),
            row.row.get(),
        );
        if let Some(previous) = previous
            && previous >= key
        {
            return Err(fail(table, "the index is out of order"));
        }
        previous = Some(key);
    }
    Ok(())
}
