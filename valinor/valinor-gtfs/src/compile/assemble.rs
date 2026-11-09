//! Groups trips into patterns, numbers stops, and writes the file's tables.

use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use valinor_transit::{
    RouteAttributes, Seconds, ServiceDate, ServiceId, StopKey, StopTime, Time, TimetableBuilder,
    TripId,
};
use zerocopy::little_endian::{F32, I32, I64, U16, U32};

use crate::compile::CompiledOutput;
use crate::compile::read::{
    AgencyModel, FeedModel, RouteModel, ServiceModel, StopModel, TripModel,
};
use crate::compile::report::FeedReport;
use crate::error::CompileError;
use crate::format::layout::{
    AgencyRow, ColorRow, FeedRow, IdIndexRow, NONE, NoonOffsetRow, PATTERN_BIKES_ALLOWED,
    PatternRow, RouteRow, SectionId, ServiceRow, ShapePointRow, ShapeRow, StopRow, TimePair,
    TripRiderRow, TripRow, ZoneRow, ZoneTransitionRow,
};
use crate::format::reader::{civil_date_of, service_date_of};
use crate::format::timetable::stop_rules;
use crate::format::writer::FileWriter;

/// A deduplicated pool of strings, with the empty string at index zero.
#[derive(Debug)]
pub(crate) struct StringPool {
    index: HashMap<String, u32>,
    data: String,
    offsets: Vec<u32>,
}

impl StringPool {
    pub(crate) fn new() -> Self {
        let mut pool = Self {
            index: HashMap::new(),
            data: String::new(),
            offsets: vec![0],
        };
        // Index zero must be the empty string, which absent fields point at.
        pool.intern_inner("");
        pool
    }

    /// The index of `text` in the pool, adding it if new.
    pub(crate) fn intern(&mut self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }
        self.intern_inner(text)
    }

    fn intern_inner(&mut self, text: &str) -> u32 {
        if let Some(&index) = self.index.get(text) {
            return index;
        }
        let index = u32::try_from(self.offsets.len() - 1).unwrap_or(NONE);
        self.data.push_str(text);
        self.offsets
            .push(u32::try_from(self.data.len()).unwrap_or(NONE));
        self.index.insert(text.to_owned(), index);
        index
    }

    fn get(&self, index: u32) -> &str {
        let start = self.offsets[index as usize] as usize;
        let end = self.offsets[index as usize + 1] as usize;
        &self.data[start..end]
    }
}

/// Writes every table of the file from the feed models.
#[allow(
    clippy::too_many_lines,
    reason = "one linear walk through the file's tables, in order"
)]
pub(super) fn assemble(
    mut feeds: Vec<FeedModel>,
    mut pool: StringPool,
) -> Result<CompiledOutput, CompileError> {
    let time_zone = feeds
        .first()
        .ok_or_else(|| CompileError::NoAgency("(no feeds)".to_owned()))?
        .time_zone
        .clone();
    for feed in &feeds {
        if feed.time_zone != time_zone {
            return Err(CompileError::MixedFeedTimeZones {
                first: time_zone,
                second: feed.time_zone.clone(),
            });
        }
    }

    let (first_date, last_date) = coverage(&feeds);
    let mut zones = ZoneTable::new(first_date, last_date);
    let tz = zones.zone(&time_zone, &mut pool)?;
    for feed in &feeds {
        for stop in &feed.stops {
            zones.zone(&stop.timezone, &mut pool)?;
        }
    }

    let noon_offsets = noon_offset_table(zones.time_zone(tz), first_date, last_date)?;

    let mut writer = FileWriter::new();
    let mut stops = Vec::new();
    let mut feed_rows = Vec::new();
    let mut agencies = Vec::new();
    let mut routes = Vec::new();
    let mut services_table = Vec::new();
    let mut service_bits = Vec::new();
    let mut route_index = Vec::new();

    // Global indices for each feed's local indices.
    let mut bases = Vec::with_capacity(feeds.len());
    for (feed_index, feed) in feeds.iter().enumerate() {
        let feed_u32 =
            u32::try_from(feed_index).map_err(|_| CompileError::TableTooLarge("feeds"))?;
        let agency_base = table_index(agencies.len(), "agencies")?;
        let route_base = table_index(routes.len(), "routes")?;
        let service_base = table_index(services_table.len(), "services")?;
        feed_rows.push(FeedRow {
            id: U32::new(pool.intern(&feed.id)),
            version: U32::new(feed.version),
            zone: U32::new(tz),
            first_service_date: U32::new(first_service_date(feed)),
            last_service_date: U32::new(last_service_date(feed)),
        });
        for agency in &feed.agencies {
            agencies.push(agency_row(feed_u32, agency));
        }
        for route in &feed.routes {
            routes.push(route_row(feed_u32, agency_base, route));
        }
        for service in &feed.services {
            services_table.push(service_row(feed_u32, service, &mut service_bits)?);
        }
        bases.push(FeedBases {
            route_base,
            service_base,
        });
    }

    // Lay out locations. Boardable stops get builder keys; their final numbers come later.
    let mut locations: Vec<LocationRef> = Vec::new();
    for (feed_index, feed) in feeds.iter().enumerate() {
        for local in 0..feed.stops.len() {
            locations.push(LocationRef {
                feed: feed_index,
                local,
            });
        }
    }

    let mut patterns = build_patterns(&feeds, &bases);
    sort_pattern_trips(&mut patterns, &feeds, &pool);

    let (numbering, boardable_count) = number_stops(&feeds, &locations, &patterns)?;

    let mut trip_table = TripTables::default();
    let mut pattern_rows = Vec::with_capacity(patterns.len());
    let mut route_pattern_lists: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut pattern_stops: Vec<U32> = Vec::new();
    let mut loop_counts = vec![0_usize; feeds.len()];

    for (pattern_index, pattern) in patterns.iter().enumerate() {
        let pattern_u32 =
            u32::try_from(pattern_index).map_err(|_| CompileError::TableTooLarge("patterns"))?;
        route_pattern_lists
            .entry(pattern.route)
            .or_default()
            .push(pattern_u32);

        let mut seen = HashSet::with_capacity(pattern.stops.len());
        let mut is_loop = false;
        for stop in &pattern.stops {
            let final_id = numbering.final_of(pattern.feed, *stop)?;
            pattern_stops.push(U32::new(final_id));
            is_loop |= !seen.insert(final_id);
        }
        if is_loop {
            loop_counts[pattern.feed] += 1;
        }

        let trips_start = u32::try_from(trip_table.trips.len())
            .map_err(|_| CompileError::TableTooLarge("trips"))?;
        for trip in &pattern.trips {
            let model = &feeds[pattern.feed].trips[*trip];
            emit_trip(
                &mut trip_table,
                pattern.feed,
                pattern_u32,
                bases[pattern.feed].service_base,
                model,
            )?;
        }

        pattern_rows.push(PatternRow {
            route: U32::new(pattern.route),
            stops_start: U32::new(
                u32::try_from(pattern_stops.len() - pattern.stops.len())
                    .map_err(|_| CompileError::TableTooLarge("pattern stops"))?,
            ),
            stop_count: U32::new(
                u32::try_from(pattern.stops.len())
                    .map_err(|_| CompileError::TableTooLarge("pattern stops"))?,
            ),
            trips_start: U32::new(trips_start),
            trip_count: U32::new(
                u32::try_from(pattern.trips.len())
                    .map_err(|_| CompileError::TableTooLarge("trips"))?,
            ),
            attributes: if pattern.bikes {
                PATTERN_BIKES_ALLOWED
            } else {
                0
            },
            _pad: [0; 3],
        });
    }

    for (feed_index, feed) in feeds.iter_mut().enumerate() {
        feed.report.loop_patterns = loop_counts[feed_index];
        feed.report.patterns = patterns.iter().filter(|p| p.feed == feed_index).count();
    }

    // Now that patterns are numbered, fill in each route's pattern list.
    let mut route_patterns_table: Vec<U32> = Vec::new();
    for (route_index_value, route) in routes.iter_mut().enumerate() {
        let list = route_pattern_lists
            .get(&(u32::try_from(route_index_value).unwrap_or(NONE)))
            .map_or(&[][..], Vec::as_slice);
        route.patterns_start = U32::new(
            u32::try_from(route_patterns_table.len())
                .map_err(|_| CompileError::TableTooLarge("route patterns"))?,
        );
        route.pattern_count = U32::new(
            u32::try_from(list.len()).map_err(|_| CompileError::TableTooLarge("route patterns"))?,
        );
        route_patterns_table.extend(list.iter().map(|&value| U32::new(value)));
    }

    // Stops in final order, then locations that can't be boarded.
    let mut child_rows: Vec<U32> = Vec::new();
    let mut stop_rows: Vec<StopRow> = vec![];
    build_stop_rows(
        &feeds,
        &numbering,
        &zones,
        &mut stop_rows,
        &mut child_rows,
        boardable_count,
    )?;
    stops.extend(stop_rows);

    let mut stop_index = Vec::with_capacity(stops.len());
    for (row, stop) in stops.iter().enumerate() {
        stop_index.push(IdIndexRow {
            feed: stop.feed,
            id: stop.gtfs_id,
            row: U32::new(u32::try_from(row).unwrap_or(NONE)),
        });
    }

    let (shapes_table, shape_points, shape_bases) = build_shape_tables(&feeds)?;
    trip_table.fix_shape_bases(&shape_bases);

    // Sort the indexes of `route_index` over all routes.
    for (index, route) in routes.iter().enumerate() {
        route_index.push(IdIndexRow {
            feed: route.feed,
            id: route.gtfs_id,
            row: U32::new(u32::try_from(index).unwrap_or(NONE)),
        });
    }
    sort_index(&mut route_index, &pool);
    sort_index(&mut stop_index, &pool);
    sort_index(&mut trip_table.index, &pool);

    writer.table(SectionId::Feeds, &feed_rows);
    writer.table(SectionId::Zones, &zones.rows);
    writer.table(SectionId::ZoneTransitions, &zones.transitions);
    writer.table(SectionId::NoonOffsets, &noon_offsets);
    writer.bytes(SectionId::Strings, pool.data.clone().into_bytes());
    writer.table(
        SectionId::StringOffsets,
        &pool
            .offsets
            .iter()
            .map(|&value| U32::new(value))
            .collect::<Vec<_>>(),
    );
    writer.table(SectionId::Agencies, &agencies);
    writer.table(SectionId::Routes, &routes);
    writer.table(SectionId::RoutePatterns, &route_patterns_table);
    writer.table(SectionId::Stops, &stops);
    writer.table(SectionId::ChildStops, &child_rows);
    writer.table(SectionId::Services, &services_table);
    writer.bytes(SectionId::ServiceBits, service_bits);
    writer.table(SectionId::Patterns, &pattern_rows);
    writer.table(SectionId::PatternStops, &pattern_stops);
    writer.table(SectionId::Trips, &trip_table.trips);
    writer.table(SectionId::Times, &trip_table.times);
    writer.bytes(SectionId::Rules, trip_table.rules.clone());
    writer.table(SectionId::TripRiders, &trip_table.riders);
    writer.table(SectionId::StopHeadsigns, &trip_table.stop_headsigns);
    writer.table(SectionId::StopSequences, &trip_table.stop_sequences);
    writer.table(SectionId::StopDists, &trip_table.stop_dists);
    writer.table(SectionId::Shapes, &shapes_table);
    writer.table(SectionId::ShapePoints, &shape_points);
    writer.table(SectionId::TripIndex, &trip_table.index);
    writer.table(SectionId::RouteIndex, &route_index);
    writer.table(SectionId::StopIndex, &stop_index);

    let boardable =
        u32::try_from(boardable_count).map_err(|_| CompileError::TableTooLarge("stops"))?;
    let bytes = writer.finish(boardable);
    let reports: Vec<FeedReport> = feeds.into_iter().map(|feed| feed.report).collect();
    Ok(CompiledOutput { bytes, reports })
}

/// Rows of the shape table, rows of the point table, and each feed's first shape.
type ShapeTables = (Vec<ShapeRow>, Vec<ShapePointRow>, Vec<u32>);

/// The shape tables, and each feed's base index within them.
fn build_shape_tables(feeds: &[FeedModel]) -> Result<ShapeTables, CompileError> {
    let mut shapes_table = Vec::new();
    let mut shape_points = Vec::new();
    let mut shape_bases = Vec::with_capacity(feeds.len());
    for (feed_index, feed) in feeds.iter().enumerate() {
        shape_bases.push(u32::try_from(shapes_table.len()).unwrap_or(NONE));
        for shape in &feed.shapes {
            let points_start = u32::try_from(shape_points.len())
                .map_err(|_| CompileError::TableTooLarge("shape points"))?;
            for point in &shape.points {
                shape_points.push(ShapePointRow {
                    lat_e7: I32::new(to_e7(point.lat)),
                    lon_e7: I32::new(to_e7(point.lon)),
                    dist: F32::new(point.dist),
                });
            }
            shapes_table.push(ShapeRow {
                feed: U32::new(u32::try_from(feed_index).unwrap_or(NONE)),
                gtfs_id: U32::new(shape.gtfs_id),
                points_start: U32::new(points_start),
                point_count: U32::new(
                    u32::try_from(shape.points.len())
                        .map_err(|_| CompileError::TableTooLarge("shape points"))?,
                ),
            });
        }
    }
    Ok((shapes_table, shape_points, shape_bases))
}

fn table_index(len: usize, name: &'static str) -> Result<u32, CompileError> {
    u32::try_from(len).map_err(|_| CompileError::TableTooLarge(name))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "validated coordinates fit in e7"
)]
fn to_e7(value: f64) -> i32 {
    (value * 1e7).round() as i32
}

/// A location's position among every feed's stops, before final numbering.
#[derive(Clone, Copy)]
struct LocationRef {
    feed: usize,
    local: usize,
}

struct FeedBases {
    route_base: u32,
    service_base: u32,
}

fn coverage(feeds: &[FeedModel]) -> (Date, Date) {
    let mut first = u32::MAX;
    let mut last = 0_u32;
    for feed in feeds {
        for service in &feed.services {
            if service.day_count > 0 {
                first = first.min(service.first_date);
                last = last.max(service.first_date + service.day_count - 1);
            }
        }
    }
    if first == u32::MAX {
        first = 0;
        last = 0;
    }
    // Extend a little: a window around the edge of the feed still needs yesterday and tomorrow.
    let first = first.saturating_sub(2);
    let last = last.saturating_add(2);
    let to_date = |days: u32| -> Date {
        civil_date_of(ServiceDate::new(days)).unwrap_or(Date::constant(1970, 1, 1))
    };
    (to_date(first), to_date(last))
}

fn first_service_date(feed: &FeedModel) -> u32 {
    feed.services
        .iter()
        .filter(|service| service.day_count > 0)
        .map(|service| service.first_date)
        .min()
        .unwrap_or(0)
}

fn last_service_date(feed: &FeedModel) -> u32 {
    feed.services
        .iter()
        .filter(|service| service.day_count > 0)
        .map(|service| service.first_date + service.day_count - 1)
        .max()
        .unwrap_or(0)
}

fn agency_row(feed: u32, agency: &AgencyModel) -> AgencyRow {
    AgencyRow {
        feed: U32::new(feed),
        gtfs_id: U32::new(agency.gtfs_id),
        name: U32::new(agency.name),
        url: U32::new(agency.url),
        phone: U32::new(agency.phone),
        fare_url: U32::new(agency.fare_url),
    }
}

fn route_row(feed: u32, agency_base: u32, route: &RouteModel) -> RouteRow {
    RouteRow {
        feed: U32::new(feed),
        agency: U32::new(route.agency.map_or(NONE, |agency| agency + agency_base)),
        gtfs_id: U32::new(route.gtfs_id),
        short_name: U32::new(route.short_name),
        long_name: U32::new(route.long_name),
        desc: U32::new(route.desc),
        url: U32::new(route.url),
        patterns_start: U32::new(0),
        pattern_count: U32::new(0),
        route_type: U16::new(route.route_type),
        color: color_row(route.color),
        text_color: color_row(route.text_color),
        _pad: [0; 2],
    }
}

fn color_row(color: Option<[u8; 3]>) -> ColorRow {
    match color {
        Some(rgb) => ColorRow { present: 1, rgb },
        None => ColorRow {
            present: 0,
            rgb: [0; 3],
        },
    }
}

fn service_row(
    feed: u32,
    service: &ServiceModel,
    bits: &mut Vec<u8>,
) -> Result<ServiceRow, CompileError> {
    let bits_start =
        u32::try_from(bits.len()).map_err(|_| CompileError::TableTooLarge("service bits"))?;
    bits.extend_from_slice(&service.bits);
    Ok(ServiceRow {
        feed: U32::new(feed),
        gtfs_id: U32::new(service.gtfs_id),
        first_date: U32::new(service.first_date),
        day_count: U32::new(service.day_count),
        bits_start: U32::new(bits_start),
    })
}

struct ZoneTable {
    start_unix: i64,
    end_unix: i64,
    by_name: HashMap<String, u32>,
    rows: Vec<ZoneRow>,
    transitions: Vec<ZoneTransitionRow>,
    zones: Vec<TimeZone>,
}

impl ZoneTable {
    fn new(first: Date, last: Date) -> Self {
        // Service days can run up to a week past their date for the longest trips, and stop
        // times shift local display around them; the margin keeps those instants covered.
        let start = date_as_unix(first) - 3 * 86_400;
        let end = date_as_unix(last) + 10 * 86_400;
        Self {
            start_unix: start,
            end_unix: end,
            by_name: HashMap::new(),
            rows: Vec::new(),
            transitions: Vec::new(),
            zones: Vec::new(),
        }
    }

    fn time_zone(&self, index: u32) -> &TimeZone {
        &self.zones[index as usize]
    }

    fn zone(&mut self, name: &str, pool: &mut StringPool) -> Result<u32, CompileError> {
        if let Some(&index) = self.by_name.get(name) {
            return Ok(index);
        }
        let zone =
            TimeZone::get(name).map_err(|_| CompileError::UnknownTimeZone(name.to_owned()))?;
        let index =
            u32::try_from(self.rows.len()).map_err(|_| CompileError::TableTooLarge("zones"))?;
        let transitions_start = u32::try_from(self.transitions.len())
            .map_err(|_| CompileError::TableTooLarge("zone transitions"))?;

        let start = jiff::Timestamp::from_second(self.start_unix)
            .map_err(|error| CompileError::OutOfRange(error.to_string()))?;
        let end = jiff::Timestamp::from_second(self.end_unix)
            .map_err(|error| CompileError::OutOfRange(error.to_string()))?;
        self.transitions.push(ZoneTransitionRow {
            at_unix: I64::new(i64::MIN),
            offset: I32::new(zone.to_offset(start).seconds()),
        });
        for transition in zone.following(start) {
            if transition.timestamp() > end {
                break;
            }
            self.transitions.push(ZoneTransitionRow {
                at_unix: I64::new(transition.timestamp().as_second()),
                offset: I32::new(transition.offset().seconds()),
            });
        }
        let transition_count = u32::try_from(self.transitions.len() - transitions_start as usize)
            .map_err(|_| CompileError::TableTooLarge("zone transitions"))?;

        self.rows.push(ZoneRow {
            name: U32::new(pool.intern(name)),
            transitions_start: U32::new(transitions_start),
            transition_count: U32::new(transition_count),
        });
        self.by_name.insert(name.to_owned(), index);
        self.zones.push(zone);
        Ok(index)
    }
}

fn date_as_unix(date: Date) -> i64 {
    i64::from(
        date.since((jiff::Unit::Day, Date::constant(1970, 1, 1)))
            .map_or(0, |span| span.get_days()),
    ) * 86_400
}

/// Runs of the noon offset across the covered dates, plus a terminator row.
fn noon_offset_table(
    zone: &TimeZone,
    first: Date,
    last: Date,
) -> Result<Vec<NoonOffsetRow>, CompileError> {
    let mut rows: Vec<NoonOffsetRow> = Vec::new();
    let mut date = first;
    while date <= last {
        let noon = date
            .at(12, 0, 0, 0)
            .to_zoned(zone.clone())
            .map_err(|error| CompileError::OutOfRange(error.to_string()))?;
        let offset = noon.offset().seconds();
        let days = service_date_of(date)
            .map_err(|error| CompileError::OutOfRange(error.to_string()))?
            .into_inner();
        if rows
            .last()
            .is_none_or(|row| row.noon_offset.get() != offset)
        {
            rows.push(NoonOffsetRow {
                first_date: U32::new(days),
                noon_offset: I32::new(offset),
            });
        }
        let Ok(next) = date.tomorrow() else { break };
        date = next;
    }
    let terminator = service_date_of(last)
        .map_err(|error| CompileError::OutOfRange(error.to_string()))?
        .into_inner()
        + 1;
    rows.push(NoonOffsetRow {
        first_date: U32::new(terminator),
        noon_offset: I32::new(0),
    });
    Ok(rows)
}

struct Pattern {
    feed: usize,
    route: u32,
    bikes: bool,
    stops: Vec<u32>,
    trips: Vec<usize>,
}

/// Groups each feed's trips by route, stop sequence, and bike rule, in a fixed order.
fn build_patterns(feeds: &[FeedModel], bases: &[FeedBases]) -> Vec<Pattern> {
    let mut map: BTreeMap<(usize, u32, Vec<u32>, bool), Vec<usize>> = BTreeMap::new();
    for (feed_index, feed) in feeds.iter().enumerate() {
        for (trip_index, trip) in feed.trips.iter().enumerate() {
            let route = trip.route + bases[feed_index].route_base;
            let bikes = trip.bikes == 1;
            map.entry((feed_index, route, trip.stops.clone(), bikes))
                .or_default()
                .push(trip_index);
        }
    }
    map.into_iter()
        .map(|((feed, route, stops, bikes), trips)| Pattern {
            feed,
            route,
            bikes,
            stops,
            trips,
        })
        .collect()
}

/// Orders a pattern's trips by departure, then by a stable tie-break, so output is reproducible.
fn sort_pattern_trips(patterns: &mut [Pattern], feeds: &[FeedModel], pool: &StringPool) {
    for pattern in patterns {
        let trips = &feeds[pattern.feed].trips;
        pattern.trips.sort_by(|&a, &b| {
            let (a, b) = (&trips[a], &trips[b]);
            (a.times[0], a.gtfs_id_text(pool)).cmp(&(b.times[0], b.gtfs_id_text(pool)))
        });
    }
}

impl TripModel {
    fn gtfs_id_text<'p>(&self, pool: &'p StringPool) -> &'p str {
        pool.get(self.gtfs_id)
    }
}

/// Stop numbers: `final_of` gives the `StopId` of a feed's local stop.
struct StopNumbering {
    /// Per feed, per local stop.
    numbers: Vec<Vec<Option<u32>>>,
}

impl StopNumbering {
    fn final_of(&self, feed: usize, local: u32) -> Result<u32, CompileError> {
        self.numbers[feed][local as usize]
            .ok_or_else(|| CompileError::OutOfRange("a trip's stop has no number".to_owned()))
    }
}

/// Runs the timetable builder to learn the numbering it gives the stops.
///
/// The builder numbers stops along the busiest patterns first, and is deterministic,
/// so a file's trips replay into the same numbers at load.
fn number_stops(
    feeds: &[FeedModel],
    locations: &[LocationRef],
    patterns: &[Pattern],
) -> Result<(StopNumbering, usize), CompileError> {
    let mut numbers: Vec<Vec<Option<u32>>> = feeds
        .iter()
        .map(|feed| vec![None; feed.stops.len()])
        .collect();
    let mut builder = TimetableBuilder::new();

    let mut key_of: Vec<Vec<Option<StopKey>>> = feeds
        .iter()
        .map(|feed| vec![None; feed.stops.len()])
        .collect();
    let mut ordered_keys = Vec::new();
    for location in locations {
        let stop: &StopModel = &feeds[location.feed].stops[location.local];
        if !stop.boardable {
            continue;
        }
        let min_change = Seconds::try_new(stop.min_change)
            .map_err(|_| CompileError::OutOfRange("a minimum change time".to_owned()))?;
        let key = builder.add_stop(min_change)?;
        key_of[location.feed][location.local] = Some(key);
        ordered_keys.push((location.feed, location.local, key));
    }

    // Number with every trip on one synthetic service: only the patterns' shapes and sizes matter
    // for stop numbers, but `add_trip` needs real rows, so the real times go through validation.
    builder.add_service([ServiceDate::new(0)])?;
    for pattern in patterns {
        let keys: Vec<StopKey> = pattern
            .stops
            .iter()
            .map(|stop| {
                key_of[pattern.feed][*stop as usize].ok_or_else(|| {
                    CompileError::OutOfRange("a pattern's stop is not boardable".to_owned())
                })
            })
            .collect::<Result<_, _>>()?;
        let attributes = if pattern.bikes {
            RouteAttributes::BIKES_ALLOWED
        } else {
            RouteAttributes::NONE
        };
        let key = builder.add_pattern(&keys, attributes)?;
        for (position, &trip_index) in pattern.trips.iter().enumerate() {
            let trip = &feeds[pattern.feed].trips[trip_index];
            let mut times = Vec::with_capacity(trip.times.len());
            for (index, (arrival, departure)) in trip.times.iter().enumerate() {
                times.push(StopTime {
                    arrival: time_of(*arrival)?,
                    departure: time_of(*departure)?,
                    rules: stop_rules(trip.rules[index]),
                });
            }
            let id = TripId::new(u32::try_from(position).unwrap_or(0));
            builder.add_trip(key, id, ServiceId::new(0), times)?;
        }
    }

    let timetable = builder.build()?;
    let mut boardable_count = 0;
    for &(feed, local, key) in &ordered_keys {
        numbers[feed][local] = Some(timetable.stop_id(key).into_inner());
        boardable_count += 1;
    }
    Ok((StopNumbering { numbers }, boardable_count))
}

fn time_of(seconds: i32) -> Result<Time, CompileError> {
    Time::try_new(seconds)
        .map_err(|_| CompileError::OutOfRange(format!("time {seconds} is out of range")))
}

#[derive(Default)]
struct TripTables {
    trips: Vec<TripRow>,
    riders: Vec<TripRiderRow>,
    times: Vec<TimePair>,
    rules: Vec<u8>,
    stop_headsigns: Vec<U32>,
    stop_sequences: Vec<U32>,
    stop_dists: Vec<F32>,
    index: Vec<IdIndexRow>,
    rule_starts: HashMap<Vec<u8>, u32>,
    headsign_starts: HashMap<Vec<u32>, u32>,
    sequence_starts: HashMap<Vec<u32>, u32>,
    dist_starts: HashMap<Vec<u32>, u32>,
    /// Per emitted trip: feed index and shape, patched once shape bases are known.
    shapes: Vec<(usize, Option<u32>)>,
}

impl TripTables {
    fn fix_shape_bases(&mut self, bases: &[u32]) {
        for (rider, (feed, shape)) in self.riders.iter_mut().zip(&self.shapes) {
            if let Some(shape) = shape {
                rider.shape = U32::new(bases[*feed] + shape);
            }
        }
    }
}

fn dedupe_start<K: Eq + core::hash::Hash>(
    starts: &mut HashMap<K, u32>,
    key: K,
    next: usize,
    name: &'static str,
) -> Result<(u32, bool), CompileError> {
    if let Some(&start) = starts.get(&key) {
        return Ok((start, false));
    }
    let start = u32::try_from(next).map_err(|_| CompileError::TableTooLarge(name))?;
    starts.insert(key, start);
    Ok((start, true))
}

fn emit_trip(
    tables: &mut TripTables,
    feed: usize,
    pattern: u32,
    service_base: u32,
    model: &TripModel,
) -> Result<(), CompileError> {
    let trip_row =
        u32::try_from(tables.trips.len()).map_err(|_| CompileError::TableTooLarge("trips"))?;
    let times_start =
        u32::try_from(tables.times.len()).map_err(|_| CompileError::TableTooLarge("times"))?;
    tables
        .times
        .extend(model.times.iter().map(|(arrival, departure)| TimePair {
            arrival: I32::new(*arrival),
            departure: I32::new(*departure),
        }));

    let rules_start = if model.rules.iter().all(|rule| *rule == 0) {
        NONE
    } else {
        let (start, fresh) = dedupe_start(
            &mut tables.rule_starts,
            model.rules.clone(),
            tables.rules.len(),
            "rules",
        )?;
        if fresh {
            tables.rules.extend_from_slice(&model.rules);
        }
        start
    };
    tables.trips.push(TripRow {
        pattern: U32::new(pattern),
        service: U32::new(model.service + service_base),
        times_start: U32::new(times_start),
        rules_start: U32::new(rules_start),
    });

    let headsigns_start = if model.stop_headsigns.is_empty() {
        NONE
    } else {
        let (start, fresh) = dedupe_start(
            &mut tables.headsign_starts,
            model.stop_headsigns.clone(),
            tables.stop_headsigns.len(),
            "stop headsigns",
        )?;
        if fresh {
            tables
                .stop_headsigns
                .extend(model.stop_headsigns.iter().map(|&id| U32::new(id)));
        }
        start
    };
    let (sequences_start, fresh) = dedupe_start(
        &mut tables.sequence_starts,
        model.sequences.clone(),
        tables.stop_sequences.len(),
        "stop sequences",
    )?;
    if fresh {
        tables
            .stop_sequences
            .extend(model.sequences.iter().map(|&value| U32::new(value)));
    }
    let dists_start = match &model.stop_dists {
        None => NONE,
        Some(dists) => {
            let key: Vec<u32> = dists.iter().map(|d| d.to_bits()).collect();
            let (start, fresh) = dedupe_start(
                &mut tables.dist_starts,
                key,
                tables.stop_dists.len(),
                "stop distances",
            )?;
            if fresh {
                tables.stop_dists.extend(dists.iter().map(|&d| F32::new(d)));
            }
            start
        }
    };

    tables.riders.push(TripRiderRow {
        gtfs_id: U32::new(model.gtfs_id),
        headsign: U32::new(model.headsign),
        block: U32::new(model.block),
        shape: U32::new(NONE),
        stop_headsigns_start: U32::new(headsigns_start),
        stop_sequences_start: U32::new(sequences_start),
        stop_dists_start: U32::new(dists_start),
        wheelchair: model.wheelchair,
        bikes: model.bikes,
        direction: model.direction,
        frequency_run: u8::from(model.frequency_run),
    });
    tables.shapes.push((feed, model.shape));

    let feed_row = u32::try_from(feed).map_err(|_| CompileError::TableTooLarge("feeds"))?;
    tables.index.push(IdIndexRow {
        feed: U32::new(feed_row),
        id: U32::new(model.gtfs_id),
        row: U32::new(trip_row),
    });
    Ok(())
}

fn sort_index(index: &mut [IdIndexRow], pool: &StringPool) {
    index.sort_by(|a, b| {
        (a.feed.get(), pool.get(a.id.get()), a.row.get()).cmp(&(
            b.feed.get(),
            pool.get(b.id.get()),
            b.row.get(),
        ))
    });
}

fn build_stop_rows(
    feeds: &[FeedModel],
    numbering: &StopNumbering,
    zones: &ZoneTable,
    rows: &mut Vec<StopRow>,
    children: &mut Vec<U32>,
    boardable_count: usize,
) -> Result<(), CompileError> {
    // Assign final locations: boardable stops by their number, others after in feed order.
    let mut location_of: Vec<Vec<u32>> =
        feeds.iter().map(|feed| vec![0; feed.stops.len()]).collect();
    let mut next_other = boardable_count;
    for (feed_index, feed) in feeds.iter().enumerate() {
        for (local, stop) in feed.stops.iter().enumerate() {
            location_of[feed_index][local] = if stop.boardable {
                numbering.numbers[feed_index][local]
                    .ok_or_else(|| CompileError::OutOfRange("a stop has no number".to_owned()))?
            } else {
                let index =
                    u32::try_from(next_other).map_err(|_| CompileError::TableTooLarge("stops"))?;
                next_other += 1;
                index
            };
        }
    }

    let total: usize = feeds.iter().map(|feed| feed.stops.len()).sum();
    let blank = StopRow {
        feed: U32::new(0),
        gtfs_id: U32::new(0),
        name: U32::new(0),
        code: U32::new(0),
        platform: U32::new(0),
        lat_e7: I32::new(i32::MIN),
        lon_e7: I32::new(i32::MIN),
        parent: U32::new(NONE),
        zone: U32::new(0),
        min_change: U32::new(0),
        children_start: U32::new(0),
        child_count: U32::new(0),
        location_type: 0,
        wheelchair_boarding: 0,
        _pad: [0; 2],
    };
    rows.resize(total, blank);

    // Children: each boardable stop lists itself under all its ancestors.
    let mut descendant_lists: Vec<Vec<u32>> = vec![Vec::new(); total];
    for (feed_index, feed) in feeds.iter().enumerate() {
        for (local, stop) in feed.stops.iter().enumerate() {
            if !stop.boardable {
                continue;
            }
            let number = location_of[feed_index][local];
            let mut cursor = stop.parent;
            let mut hops = 0;
            while let Some(parent) = cursor {
                hops += 1;
                if hops > feed.stops.len() {
                    break;
                }
                let parent_location = location_of[feed_index][parent as usize] as usize;
                descendant_lists[parent_location].push(number);
                cursor = feed.stops[parent as usize].parent;
            }
        }
    }

    for (feed_index, feed) in feeds.iter().enumerate() {
        for (local, stop) in feed.stops.iter().enumerate() {
            let location = location_of[feed_index][local] as usize;
            let (lat, lon) = stop
                .coordinate
                .map_or((i32::MIN, i32::MIN), |(lat, lon)| (to_e7(lat), to_e7(lon)));
            let zone = zones
                .by_name
                .get(stop.timezone.as_str())
                .copied()
                .unwrap_or(0);
            rows[location] = StopRow {
                feed: U32::new(u32::try_from(feed_index).unwrap_or(NONE)),
                gtfs_id: U32::new(stop.gtfs_id),
                name: U32::new(stop.name),
                code: U32::new(stop.code),
                platform: U32::new(stop.platform),
                lat_e7: I32::new(lat),
                lon_e7: I32::new(lon),
                parent: U32::new(
                    stop.parent
                        .map_or(NONE, |parent| location_of[feed_index][parent as usize]),
                ),
                zone: U32::new(zone),
                min_change: U32::new(stop.min_change),
                children_start: U32::new(0),
                child_count: U32::new(0),
                location_type: stop.location_type,
                wheelchair_boarding: stop.wheelchair,
                _pad: [0; 2],
            };
        }
    }

    // Children are laid out in location order, so each location owns the next run of the table.
    for (row, list) in rows.iter_mut().zip(&mut descendant_lists) {
        list.sort_unstable();
        row.children_start = U32::new(
            u32::try_from(children.len())
                .map_err(|_| CompileError::TableTooLarge("child stops"))?,
        );
        row.child_count = U32::new(
            u32::try_from(list.len()).map_err(|_| CompileError::TableTooLarge("child stops"))?,
        );
        children.extend(list.iter().map(|&value| U32::new(value)));
    }
    Ok(())
}
