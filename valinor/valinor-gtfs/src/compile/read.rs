//! Reads one GTFS feed into a model, keeping what can be routed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use csv::ByteRecord;
use geo::{Distance, Haversine, Point};
use jiff::civil::Date;
use valinor_transit::{SECONDS_LIMIT, TIME_LIMIT};

use crate::compile::FeedInput;
use crate::compile::assemble::StringPool;
use crate::compile::interpolate::{RawTime, interpolate};
use crate::compile::report::{DropReason, FeedReport};
use crate::compile::shape_match::{ShapePoint, project};
use crate::compile::source::GtfsSource;
use crate::compile::table::{Table, field};
use crate::compile::time::{parse_date, parse_time};
use crate::error::CompileError;
use crate::format::layout::{DIRECTION_NONE, MAX_SERVICE_DAYS};
use crate::format::reader::service_date_of;

/// The most runs one row of `frequencies.txt` may expand into.
const MAX_FREQUENCY_RUNS: i32 = 200_000;

pub(super) struct AgencyModel {
    pub(super) gtfs_id: u32,
    pub(super) name: u32,
    pub(super) url: u32,
    pub(super) phone: u32,
    pub(super) fare_url: u32,
    pub(super) timezone: String,
}

pub(super) struct RouteModel {
    pub(super) gtfs_id: u32,
    pub(super) agency: Option<u32>,
    pub(super) short_name: u32,
    pub(super) long_name: u32,
    pub(super) desc: u32,
    pub(super) url: u32,
    pub(super) route_type: u16,
    pub(super) color: Option<[u8; 3]>,
    pub(super) text_color: Option<[u8; 3]>,
}

pub(super) struct StopModel {
    pub(super) gtfs_id: u32,
    pub(super) name: u32,
    pub(super) code: u32,
    pub(super) platform: u32,
    pub(super) coordinate: Option<(f64, f64)>,
    pub(super) parent: Option<u32>,
    /// The stop's own time zone, or else its parent's, or else the feed's.
    pub(super) timezone: String,
    pub(super) location_type: u8,
    pub(super) wheelchair: u8,
    pub(super) min_change: u32,
    pub(super) boardable: bool,
    pub(super) used: bool,
}

pub(super) struct ServiceModel {
    pub(super) gtfs_id: u32,
    pub(super) first_date: u32,
    pub(super) day_count: u32,
    pub(super) bits: Vec<u8>,
}

pub(super) struct ShapeModel {
    pub(super) gtfs_id: u32,
    pub(super) points: Vec<ShapePoint>,
    /// Whether the distances are the feed's, rather than computed meters.
    pub(super) dist_given: bool,
}

pub(super) struct TripModel {
    pub(super) gtfs_id: u32,
    pub(super) route: u32,
    pub(super) service: u32,
    pub(super) shape: Option<u32>,
    pub(super) headsign: u32,
    pub(super) block: u32,
    pub(super) bikes: u8,
    pub(super) wheelchair: u8,
    pub(super) direction: u8,
    pub(super) frequency_run: bool,
    pub(super) stops: Vec<u32>,
    pub(super) times: Vec<(i32, i32)>,
    pub(super) rules: Vec<u8>,
    pub(super) stop_headsigns: Vec<u32>,
    pub(super) sequences: Vec<u32>,
    pub(super) stop_dists: Option<Vec<f32>>,
}

/// One feed, with feed-local indices.
pub(super) struct FeedModel {
    pub(super) id: String,
    pub(super) version: u32,
    pub(super) time_zone: String,
    pub(super) agencies: Vec<AgencyModel>,
    pub(super) routes: Vec<RouteModel>,
    pub(super) stops: Vec<StopModel>,
    pub(super) services: Vec<ServiceModel>,
    pub(super) shapes: Vec<ShapeModel>,
    pub(super) trips: Vec<TripModel>,
    pub(super) report: FeedReport,
}

/// Reads a whole feed.
pub(super) fn read_feed(
    feed: FeedInput<'_>,
    pool: &mut StringPool,
) -> Result<FeedModel, CompileError> {
    let id = feed.id.into_inner();
    let mut source = feed.source;
    let mut report = FeedReport::new(id.clone());

    let (agencies, agency_ids) = read_agencies(source.as_mut(), pool, &id, &mut report)?;
    let time_zone = feed_time_zone(&agencies, &id)?;
    let (mut services, service_ids) = read_calendars(source.as_mut(), pool, &mut report)?;
    let (routes, route_ids) =
        read_routes(source.as_mut(), pool, &agency_ids, &agencies, &mut report)?;
    let (mut stops, stop_ids) = read_stops(source.as_mut(), pool, &time_zone, &mut report)?;
    apply_transfers(source.as_mut(), &stop_ids, &mut stops, &mut report)?;

    let shapes = read_shapes(source.as_mut(), pool, &mut report)?;
    let frequencies = read_frequencies(source.as_mut(), &mut report)?;
    let mut trips = read_trips(
        source.as_mut(),
        pool,
        &route_ids,
        &service_ids,
        &stop_ids,
        &mut stops,
        &shapes,
        &frequencies,
        &mut report,
    )?;

    // Keep only the calendars and shapes that some kept trip uses.
    let mut used_services = vec![false; services.len()];
    let mut used_shapes = vec![false; shapes.len()];
    for trip in &trips {
        used_services[trip.service as usize] = true;
        if let Some(shape) = trip.shape {
            used_shapes[shape as usize] = true;
        }
    }
    let service_remap = remap_used(&used_services);
    let shape_remap = remap_used(&used_shapes);
    for trip in &mut trips {
        trip.service = service_remap[trip.service as usize].ok_or(CompileError::OutOfRange(
            "a kept trip's service was not retained".to_owned(),
        ))?;
        if let Some(shape) = trip.shape {
            trip.shape = Some(shape_remap[shape as usize].ok_or(CompileError::OutOfRange(
                "a kept trip's shape was not retained".to_owned(),
            ))?);
        }
    }
    let mut indexed_shapes: Vec<(u32, ShapeModel)> = shapes.into_values().collect();
    indexed_shapes.sort_by_key(|(index, _)| *index);
    let shapes: Vec<ShapeModel> = indexed_shapes
        .into_iter()
        .filter(|(index, _)| used_shapes[*index as usize])
        .map(|(_, shape)| shape)
        .collect();
    report.services_used = used_services.iter().filter(|used| **used).count();
    report.shapes = shapes.len();
    report.shape_points = shapes.iter().map(|shape| shape.points.len()).sum();
    report.stops_used_by_trips = stops.iter().filter(|stop| stop.used).count();
    let mut index = 0;
    services.retain(|_| {
        let keep = used_services[index];
        index += 1;
        keep
    });

    let version = read_version(source.as_mut(), pool, &mut report)?;
    Ok(FeedModel {
        id,
        version,
        time_zone,
        agencies,
        routes,
        stops,
        services,
        shapes,
        trips,
        report,
    })
}

fn remap_used(used: &[bool]) -> Vec<Option<u32>> {
    let mut next = 0;
    used.iter()
        .map(|used| {
            if *used {
                let index = next;
                next += 1;
                Some(index)
            } else {
                None
            }
        })
        .collect()
}

fn index_of(values: usize, name: &'static str) -> Result<u32, CompileError> {
    u32::try_from(values).map_err(|_| CompileError::TableTooLarge(name))
}

fn feed_time_zone(agencies: &[AgencyModel], feed: &str) -> Result<String, CompileError> {
    let first = agencies
        .first()
        .ok_or_else(|| CompileError::NoAgency(feed.to_owned()))?
        .timezone
        .clone();
    for agency in agencies {
        if agency.timezone != first {
            return Err(CompileError::MixedAgencyTimeZones {
                feed: feed.to_owned(),
                first,
                second: agency.timezone.clone(),
            });
        }
    }
    Ok(first)
}

type AgencyIds = HashMap<String, u32>;

fn read_agencies(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    feed: &str,
    report: &mut FeedReport,
) -> Result<(Vec<AgencyModel>, AgencyIds), CompileError> {
    let mut table = Table::require(source, "agency.txt")?;
    let name = table.column("agency_name")?;
    let timezone = table.column("agency_timezone")?;
    let (id, url, phone, fare_url) = (
        table.optional("agency_id"),
        table.optional("agency_url"),
        table.optional("agency_phone"),
        table.optional("agency_fare_url"),
    );
    table.optional_identifier(id);
    let mut agencies = Vec::new();
    let mut ids = AgencyIds::new();
    let mut row = ByteRecord::new();
    while table.read(&mut row, report)? {
        let gtfs_id = field(&row, id);
        let zone = field(&row, Some(timezone));
        if zone.is_empty() {
            report.warn(|| format!("agency {gtfs_id:?} has no time zone"));
            continue;
        }
        let index = index_of(agencies.len(), "agencies")?;
        if ids.insert(gtfs_id.to_owned(), index).is_some() {
            report.warn(|| format!("duplicate agency_id {gtfs_id:?}"));
        }
        agencies.push(AgencyModel {
            gtfs_id: pool.intern(gtfs_id),
            name: pool.intern(field(&row, Some(name))),
            url: pool.intern(field(&row, url)),
            phone: pool.intern(field(&row, phone)),
            fare_url: pool.intern(field(&row, fare_url)),
            timezone: zone.to_owned(),
        });
    }
    if agencies.is_empty() {
        return Err(CompileError::NoAgency(feed.to_owned()));
    }
    report.agencies = agencies.len();
    Ok((agencies, ids))
}

type ServiceIds = HashMap<String, u32>;

/// A service's rule before expansion.
#[derive(Default)]
struct ServiceSpec {
    /// Bit 0 is Monday.
    weekdays: u8,
    start: Option<Date>,
    end: Option<Date>,
    added: BTreeSet<Date>,
    removed: BTreeSet<Date>,
}

fn read_calendars(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    report: &mut FeedReport,
) -> Result<(Vec<ServiceModel>, ServiceIds), CompileError> {
    let mut specs: BTreeMap<String, ServiceSpec> = BTreeMap::new();
    let mut found_any = false;

    if let Some(mut table) = Table::open(source, "calendar.txt")? {
        found_any = true;
        let service = table.column("service_id")?;
        table.identifier(service);
        let start = table.column("start_date")?;
        let end = table.column("end_date")?;
        let days = [
            table.column("monday")?,
            table.column("tuesday")?,
            table.column("wednesday")?,
            table.column("thursday")?,
            table.column("friday")?,
            table.column("saturday")?,
            table.column("sunday")?,
        ];
        let mut row = ByteRecord::new();
        while table.read(&mut row, report)? {
            let id = field(&row, Some(service));
            if specs.contains_key(id) {
                report.warn(|| format!("duplicate service_id {id:?}; keeping the first row"));
                continue;
            }
            let spec = specs.entry(id.to_owned()).or_default();
            for (bit, column) in days.iter().enumerate() {
                if field(&row, Some(*column)) == "1" {
                    spec.weekdays |= 1 << bit;
                }
            }
            spec.start = parse_date(field(&row, Some(start)));
            spec.end = parse_date(field(&row, Some(end)));
            if spec.start.is_none() || spec.end.is_none() {
                report.warn(|| format!("calendar {id:?} has an invalid date range"));
                spec.weekdays = 0;
            }
        }
    }

    if let Some(mut table) = Table::open(source, "calendar_dates.txt")? {
        found_any = true;
        let service = table.column("service_id")?;
        table.identifier(service);
        let date = table.column("date")?;
        let kind = table.column("exception_type")?;
        let mut row = ByteRecord::new();
        while table.read(&mut row, report)? {
            let id = field(&row, Some(service));
            let Some(day) = parse_date(field(&row, Some(date))) else {
                report.warn(|| format!("calendar_dates {id:?} has an invalid date"));
                continue;
            };
            let spec = specs.entry(id.to_owned()).or_default();
            match field(&row, Some(kind)) {
                "1" => {
                    spec.added.insert(day);
                    spec.removed.remove(&day);
                }
                "2" => {
                    spec.removed.insert(day);
                    spec.added.remove(&day);
                }
                other => {
                    report.warn(|| format!("calendar_dates {id:?} has exception type {other:?}"));
                }
            }
        }
    }

    if !found_any {
        return Err(CompileError::MissingFile(
            "calendar.txt or calendar_dates.txt",
        ));
    }

    let mut services = Vec::with_capacity(specs.len());
    let mut ids = ServiceIds::new();
    report.services_listed = specs.len();
    for (id, spec) in specs {
        let index = index_of(services.len(), "services")?;
        services.push(expand_service(&id, &spec, pool, report)?);
        ids.insert(id, index);
    }
    Ok((services, ids))
}

/// Expands a service's rule into a bit per date.
fn expand_service(
    id: &str,
    spec: &ServiceSpec,
    pool: &mut StringPool,
    report: &mut FeedReport,
) -> Result<ServiceModel, CompileError> {
    let mut dates = BTreeSet::new();
    if let (Some(start), Some(end)) = (spec.start, spec.end) {
        if end < start {
            report.warn(|| format!("calendar {id:?} ends before it starts, so it never runs"));
        } else if spec.weekdays != 0 {
            insert_regular_days(id, spec, start, end, &mut dates, report)?;
        }
    }
    dates.extend(spec.added.iter().copied());
    for day in &spec.removed {
        dates.remove(day);
    }
    // Service dates are days since 1970, and no real timetable reaches back that far.
    let before_epoch = dates.iter().take_while(|day| day.year() < 1970).count();
    if before_epoch > 0 {
        report.warn(|| format!("calendar {id:?} has {before_epoch} dates before 1970"));
        dates.retain(|day| day.year() >= 1970);
    }
    fit_to_format(id, &mut dates, report)?;
    let gtfs_id = pool.intern(id);

    let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
        return Ok(ServiceModel {
            gtfs_id,
            first_date: 0,
            day_count: 0,
            bits: Vec::new(),
        });
    };
    let first_days = day_number(*first)?;
    let last_days = day_number(*last)?;
    let day_count = last_days - first_days + 1;
    let mut bits = vec![0_u8; day_count.div_ceil(8) as usize];
    for day in &dates {
        let bit = (day_number(*day)? - first_days) as usize;
        bits[bit / 8] |= 1 << (bit % 8);
    }
    Ok(ServiceModel {
        gtfs_id,
        first_date: first_days,
        day_count,
        bits,
    })
}

/// Adds the dates of a service's weekly pattern.
///
/// An open-ended range, such as one ending in `99991231`, is clamped to the longest span
/// the format holds, so a feed's regular service survives it.
fn insert_regular_days(
    id: &str,
    spec: &ServiceSpec,
    start: Date,
    end: Date,
    dates: &mut BTreeSet<Date>,
    report: &mut FeedReport,
) -> Result<(), CompileError> {
    let first = start.max(Date::constant(1970, 1, 1));
    let span = jiff::Span::new()
        .try_days(i64::from(MAX_SERVICE_DAYS - 1))
        .map_err(|error| CompileError::OutOfRange(error.to_string()))?;
    let last = first.checked_add(span).map_or(end, |limit| end.min(limit));
    if last < end {
        report.warn(|| format!("calendar {id:?} runs past {last}; ignoring the rest of its range"));
    }
    let mut day = first;
    while day <= last {
        if spec.weekdays & (1 << day.weekday().to_monday_zero_offset()) != 0 {
            dates.insert(day);
        }
        let Ok(next) = day.tomorrow() else { break };
        day = next;
    }
    Ok(())
}

/// Keeps a calendar within the span the format can hold.
///
/// When exceptions stretch a calendar further than that, the span with the most dates is kept
/// (the earliest one if several tie), which keeps the bulk of the service over stray outliers.
fn fit_to_format(
    id: &str,
    dates: &mut BTreeSet<Date>,
    report: &mut FeedReport,
) -> Result<(), CompileError> {
    let numbers: Vec<u32> = dates
        .iter()
        .map(|day| day_number(*day))
        .collect::<Result<_, _>>()?;
    let (Some(first), Some(last)) = (numbers.first(), numbers.last()) else {
        return Ok(());
    };
    if last - first < MAX_SERVICE_DAYS {
        return Ok(());
    }
    let (mut left, mut best_start, mut best_len) = (0, 0, 0);
    for (right, &day) in numbers.iter().enumerate() {
        while day - numbers[left] >= MAX_SERVICE_DAYS {
            left += 1;
        }
        if right - left + 1 > best_len {
            (best_start, best_len) = (left, right - left + 1);
        }
    }
    let (low, high) = (numbers[best_start], numbers[best_start + best_len - 1]);
    dates.retain(|day| day_number(*day).is_ok_and(|number| (low..=high).contains(&number)));
    let dropped = numbers.len() - best_len;
    report.warn(|| format!("calendar {id:?} spans too many days; dropped {dropped} of its dates"));
    Ok(())
}

fn day_number(date: Date) -> Result<u32, CompileError> {
    service_date_of(date)
        .map(valinor_transit::ServiceDate::into_inner)
        .map_err(|error| CompileError::OutOfRange(error.to_string()))
}

type RouteIds = HashMap<String, u32>;

fn read_routes(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    agency_ids: &AgencyIds,
    agencies: &[AgencyModel],
    report: &mut FeedReport,
) -> Result<(Vec<RouteModel>, RouteIds), CompileError> {
    let mut table = Table::require(source, "routes.txt")?;
    let id = table.column("route_id")?;
    table.identifier(id);
    let route_type = table.column("route_type")?;
    let (agency_id, short, long, desc, url, color, text_color) = (
        table.optional("agency_id"),
        table.optional("route_short_name"),
        table.optional("route_long_name"),
        table.optional("route_desc"),
        table.optional("route_url"),
        table.optional("route_color"),
        table.optional("route_text_color"),
    );
    let mut routes = Vec::new();
    let mut ids = RouteIds::new();
    let mut row = ByteRecord::new();
    while table.read(&mut row, report)? {
        let gtfs_id = field(&row, Some(id));
        let agency_text = field(&row, agency_id);
        let agency = if agency_text.is_empty() && agencies.len() == 1 {
            Some(0)
        } else if let Some(&index) = agency_ids.get(agency_text) {
            Some(index)
        } else {
            report.warn(|| format!("route {gtfs_id:?} names unknown agency {agency_text:?}"));
            None
        };
        let Ok(kind) = field(&row, Some(route_type)).parse::<u16>() else {
            report.warn(|| format!("route {gtfs_id:?} has an invalid route_type"));
            continue;
        };
        if ids.contains_key(gtfs_id) {
            report.warn(|| format!("duplicate route_id {gtfs_id:?}"));
            continue;
        }
        let index = index_of(routes.len(), "routes")?;
        ids.insert(gtfs_id.to_owned(), index);
        routes.push(RouteModel {
            gtfs_id: pool.intern(gtfs_id),
            agency,
            short_name: pool.intern(field(&row, short)),
            long_name: pool.intern(field(&row, long)),
            desc: pool.intern(field(&row, desc)),
            url: pool.intern(field(&row, url)),
            route_type: kind,
            color: parse_color(field(&row, color)),
            text_color: parse_color(field(&row, text_color)),
        });
    }
    report.routes = routes.len();
    Ok((routes, ids))
}

fn parse_color(text: &str) -> Option<[u8; 3]> {
    if text.len() != 6 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: core::ops::Range<usize>| u8::from_str_radix(text.get(range)?, 16).ok();
    Some([channel(0..2)?, channel(2..4)?, channel(4..6)?])
}

type StopIds = HashMap<String, u32>;

/// A row of `stops.txt` before its parent is resolved.
struct StopRow {
    model: StopModel,
    parent: String,
}

#[allow(
    clippy::too_many_lines,
    reason = "one linear pass over the columns of stops.txt"
)]
fn read_stops(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    feed_zone: &str,
    report: &mut FeedReport,
) -> Result<(Vec<StopModel>, StopIds), CompileError> {
    let mut table = Table::require(source, "stops.txt")?;
    let id = table.column("stop_id")?;
    table.identifier(id);
    let (name, code, platform, lat, lon, location_type, parent, zone, wheelchair) = (
        table.optional("stop_name"),
        table.optional("stop_code"),
        table.optional("platform_code"),
        table.optional("stop_lat"),
        table.optional("stop_lon"),
        table.optional("location_type"),
        table.optional("parent_station"),
        table.optional("stop_timezone"),
        table.optional("wheelchair_boarding"),
    );
    let mut rows: Vec<StopRow> = Vec::new();
    let mut ids = StopIds::new();
    let mut record = ByteRecord::new();
    while table.read(&mut record, report)? {
        let gtfs_id = field(&record, Some(id));
        if ids.contains_key(gtfs_id) {
            report.warn(|| format!("duplicate stop_id {gtfs_id:?}"));
            continue;
        }
        let index = index_of(rows.len(), "stops")?;
        ids.insert(gtfs_id.to_owned(), index);
        let coordinate = match (
            field(&record, lat).parse::<f64>(),
            field(&record, lon).parse::<f64>(),
        ) {
            (Ok(lat), Ok(lon)) if lat.abs() <= 90.0 && lon.abs() <= 180.0 => Some((lat, lon)),
            _ => None,
        };
        let kind = match field(&record, location_type) {
            "" | "0" => 0,
            "1" => 1,
            "2" => 2,
            "3" => 3,
            "4" => 4,
            other => {
                report.warn(|| format!("stop {gtfs_id:?} has location_type {other:?}; assuming 0"));
                0
            }
        };
        let wheelchair = match field(&record, wheelchair) {
            "1" => 1,
            "2" => 2,
            _ => 0,
        };
        rows.push(StopRow {
            model: StopModel {
                gtfs_id: pool.intern(gtfs_id),
                name: pool.intern(field(&record, name)),
                code: pool.intern(field(&record, code)),
                platform: pool.intern(field(&record, platform)),
                coordinate,
                parent: None,
                timezone: field(&record, zone).to_owned(),
                location_type: kind,
                wheelchair,
                min_change: 0,
                boardable: false,
                used: false,
            },
            parent: field(&record, parent).to_owned(),
        });
    }
    report.stops_listed = rows.len();

    // Resolve parents, then inherit what GTFS says a child inherits.
    let parent_index: Vec<Option<u32>> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if row.parent.is_empty() {
                return None;
            }
            let found = ids.get(row.parent.as_str()).copied();
            if found.is_none() {
                let name = &row.parent;
                report.warn(|| format!("stop {index} names unknown parent {name:?}"));
            }
            found
        })
        .collect();
    let mut stops: Vec<StopModel> = rows.into_iter().map(|row| row.model).collect();
    for (index, parent) in parent_index.iter().enumerate() {
        stops[index].parent = *parent;
    }
    // Walk up a bounded chain, because a parent can have a parent (a boarding area, a platform,
    // a station), and a feed could make a cycle, which must end rather than hang.
    let limit = stops.len();
    for index in 0..stops.len() {
        if stops[index].coordinate.is_some()
            && !stops[index].timezone.is_empty()
            && stops[index].wheelchair != 0
        {
            continue;
        }
        let mut seen = 0;
        let mut cursor = stops[index].parent;
        while let Some(parent) = cursor {
            seen += 1;
            if seen > limit {
                break;
            }
            let (coordinate, zone, wheelchair, next) = {
                let parent = &stops[parent as usize];
                (
                    parent.coordinate,
                    parent.timezone.clone(),
                    parent.wheelchair,
                    parent.parent,
                )
            };
            let stop = &mut stops[index];
            if stop.coordinate.is_none() {
                stop.coordinate = coordinate;
            }
            if stop.timezone.is_empty() {
                stop.timezone = zone;
            }
            if stop.wheelchair == 0 {
                stop.wheelchair = wheelchair;
            }
            cursor = next;
        }
    }
    let mut boardable = 0;
    for stop in &mut stops {
        if stop.timezone.is_empty() {
            feed_zone.clone_into(&mut stop.timezone);
        }
        // A stop's zone is only used to show local times, so a bad name doesn't lose the stop.
        if jiff::tz::TimeZone::get(&stop.timezone).is_err() {
            let bad = stop.timezone.clone();
            report.warn(|| format!("a stop has unknown time zone {bad:?}; using the feed's"));
            feed_zone.clone_into(&mut stop.timezone);
        }
        // Riders can board at stops, platforms, and boarding areas.
        // A stop without a position can't be reached on foot, so it can't be used.
        stop.boardable = matches!(stop.location_type, 0 | 4) && stop.coordinate.is_some();
        boardable += usize::from(stop.boardable);
    }
    report.boardable_stops = boardable;
    Ok((stops, ids))
}

fn apply_transfers(
    source: &mut dyn GtfsSource,
    stop_ids: &StopIds,
    stops: &mut [StopModel],
    report: &mut FeedReport,
) -> Result<(), CompileError> {
    let Some(mut table) = Table::open(source, "transfers.txt")? else {
        return Ok(());
    };
    let from = table.column("from_stop_id")?;
    let to = table.column("to_stop_id")?;
    let kind = table.column("transfer_type")?;
    let min_time = table.optional("min_transfer_time");
    let scoped: Vec<Option<usize>> = ["from_route_id", "to_route_id", "from_trip_id", "to_trip_id"]
        .into_iter()
        .map(|name| table.optional(name))
        .collect();
    let children = children_of(stops);
    let mut row = ByteRecord::new();
    while table.read(&mut row, report)? {
        let (from_id, to_id) = (field(&row, Some(from)), field(&row, Some(to)));
        let time = field(&row, min_time).parse::<u32>().ok();
        let unscoped = scoped.iter().all(|column| field(&row, *column).is_empty());
        let same_stop = !from_id.is_empty() && from_id == to_id;
        let applies = same_stop
            && unscoped
            && field(&row, Some(kind)) == "2"
            && time.is_some_and(|time| time <= SECONDS_LIMIT);
        let target = applies.then(|| stop_ids.get(from_id).copied()).flatten();
        let Some(index) = target else {
            report.transfers_ignored += 1;
            continue;
        };
        report.transfers_applied += 1;
        let seconds = time.unwrap_or(0);
        // A station's minimum applies to every stop inside it.
        let mut pending = vec![index];
        let mut visited = 0;
        while let Some(stop) = pending.pop() {
            visited += 1;
            if visited > stops.len() {
                break;
            }
            let model = &mut stops[stop as usize];
            model.min_change = model.min_change.max(seconds);
            if let Some(list) = children.get(&stop) {
                pending.extend(list.iter().copied());
            }
        }
    }
    Ok(())
}

/// Each location's direct children, for walking a station's hierarchy.
fn children_of(stops: &[StopModel]) -> HashMap<u32, Vec<u32>> {
    let mut out: HashMap<u32, Vec<u32>> = HashMap::new();
    for (index, stop) in stops.iter().enumerate() {
        if let Some(parent) = stop.parent
            && let Ok(index) = u32::try_from(index)
        {
            out.entry(parent).or_default().push(index);
        }
    }
    out
}

fn read_shapes(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    report: &mut FeedReport,
) -> Result<HashMap<String, (u32, ShapeModel)>, CompileError> {
    let Some(mut table) = Table::open(source, "shapes.txt")? else {
        return Ok(HashMap::new());
    };
    let shape = table.column("shape_id")?;
    table.identifier(shape);
    let lat = table.column("shape_pt_lat")?;
    let lon = table.column("shape_pt_lon")?;
    let sequence = table.column("shape_pt_sequence")?;
    let dist = table.optional("shape_dist_traveled");

    let mut raw: BTreeMap<String, Vec<RawPoint>> = BTreeMap::new();
    let mut row = ByteRecord::new();
    while table.read(&mut row, report)? {
        let id = field(&row, Some(shape));
        let parsed = (
            field(&row, Some(sequence)).parse::<u32>(),
            field(&row, Some(lat)).parse::<f64>(),
            field(&row, Some(lon)).parse::<f64>(),
        );
        let (Ok(sequence), Ok(lat), Ok(lon)) = parsed else {
            report.warn(|| format!("shape {id:?} has an unreadable point"));
            continue;
        };
        if lat.abs() > 90.0 || lon.abs() > 180.0 {
            report.warn(|| format!("shape {id:?} has a point off the globe"));
            continue;
        }
        raw.entry(id.to_owned()).or_default().push(RawPoint {
            sequence,
            lat,
            lon,
            dist: field(&row, dist)
                .parse::<f32>()
                .ok()
                .filter(|d| d.is_finite()),
        });
    }

    let mut shapes = HashMap::with_capacity(raw.len());
    for (index, (id, mut points)) in raw.into_iter().enumerate() {
        points.sort_by_key(|point| point.sequence);
        let given: Option<Vec<f32>> = points.iter().map(|point| point.dist).collect();
        let given = given.filter(|dists| dists.windows(2).all(|pair| pair[0] <= pair[1]));
        let dist_given = given.is_some();
        let mut total = 0.0_f64;
        let mut converted = Vec::with_capacity(points.len());
        let mut previous: Option<(f64, f64)> = None;
        for (position, point) in points.iter().enumerate() {
            if let Some(dists) = &given {
                converted.push(ShapePoint {
                    lat: point.lat,
                    lon: point.lon,
                    dist: dists[position],
                });
            } else {
                if let Some((lat, lon)) = previous {
                    total += haversine((lat, lon), (point.lat, point.lon));
                }
                previous = Some((point.lat, point.lon));
                converted.push(ShapePoint {
                    lat: point.lat,
                    lon: point.lon,
                    #[expect(clippy::cast_possible_truncation, reason = "shape-scale meters")]
                    dist: total as f32,
                });
            }
        }
        let gtfs_id = pool.intern(&id);
        let index = index_of(index, "shapes")?;
        shapes.insert(
            id,
            (
                index,
                ShapeModel {
                    gtfs_id,
                    points: converted,
                    dist_given,
                },
            ),
        );
    }
    Ok(shapes)
}

/// A shape point before its distance is resolved.
struct RawPoint {
    sequence: u32,
    lat: f64,
    lon: f64,
    dist: Option<f32>,
}

fn haversine(a: (f64, f64), b: (f64, f64)) -> f64 {
    Haversine.distance(Point::new(a.1, a.0), Point::new(b.1, b.0))
}

/// One row of `frequencies.txt`.
struct Frequency {
    start: i32,
    end: i32,
    headway: i32,
}

fn read_frequencies(
    source: &mut dyn GtfsSource,
    report: &mut FeedReport,
) -> Result<HashMap<String, Vec<Frequency>>, CompileError> {
    let Some(mut table) = Table::open(source, "frequencies.txt")? else {
        return Ok(HashMap::new());
    };
    let trip = table.column("trip_id")?;
    table.identifier(trip);
    let start = table.column("start_time")?;
    let end = table.column("end_time")?;
    let headway = table.column("headway_secs")?;
    let mut out: HashMap<String, Vec<Frequency>> = HashMap::new();
    let mut row = ByteRecord::new();
    while table.read(&mut row, report)? {
        let id = field(&row, Some(trip)).to_owned();
        let parsed = (
            parse_time(field(&row, Some(start))),
            parse_time(field(&row, Some(end))),
            field(&row, Some(headway)).parse::<i32>().ok(),
        );
        let (Some(start), Some(end), Some(headway)) = parsed else {
            report.warn(|| format!("frequency row for trip {id:?} is unreadable"));
            continue;
        };
        if headway <= 0 || end <= start {
            report.warn(|| format!("frequency row for trip {id:?} has no runs"));
            continue;
        }
        if end > TIME_LIMIT / 4 || (end - start) / headway > MAX_FREQUENCY_RUNS {
            report.warn(|| format!("frequency row for trip {id:?} is implausibly long"));
            continue;
        }
        out.entry(id).or_default().push(Frequency {
            start,
            end,
            headway,
        });
    }
    Ok(out)
}

fn read_version(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    report: &mut FeedReport,
) -> Result<u32, CompileError> {
    let Some(mut table) = Table::open(source, "feed_info.txt")? else {
        return Ok(0);
    };
    let Some(version) = table.optional("feed_version") else {
        return Ok(0);
    };
    let mut row = ByteRecord::new();
    if table.read(&mut row, report)? {
        return Ok(pool.intern(field(&row, Some(version))));
    }
    Ok(0)
}

#[derive(Clone, Copy)]
struct RawStopTime {
    sequence: u32,
    stop: u32,
    arrival: Option<i32>,
    departure: Option<i32>,
    pickup: u8,
    drop_off: u8,
    headsign: u32,
    shape_dist: Option<f32>,
}

#[expect(
    clippy::too_many_arguments,
    reason = "every parameter is a distinct table"
)]
#[allow(
    clippy::too_many_lines,
    reason = "one linear pass over trips and their stop times"
)]
fn read_trips(
    source: &mut dyn GtfsSource,
    pool: &mut StringPool,
    route_ids: &RouteIds,
    service_ids: &ServiceIds,
    stop_ids: &StopIds,
    stops: &mut [StopModel],
    shapes: &HashMap<String, (u32, ShapeModel)>,
    frequencies: &HashMap<String, Vec<Frequency>>,
    report: &mut FeedReport,
) -> Result<Vec<TripModel>, CompileError> {
    struct TripHeader {
        gtfs_id: u32,
        route: u32,
        service: u32,
        shape: Option<u32>,
        headsign: u32,
        block: u32,
        bikes: u8,
        wheelchair: u8,
        direction: u8,
        drop: Option<DropReason>,
        rows: Vec<RawStopTime>,
        name: String,
    }

    let mut headers: Vec<TripHeader> = Vec::new();
    let mut trip_ids: HashMap<String, u32> = HashMap::new();
    {
        let mut table = Table::require(source, "trips.txt")?;
        let (trip_id, route_id, service_id) = (
            table.column("trip_id")?,
            table.column("route_id")?,
            table.column("service_id")?,
        );
        table.identifier(trip_id);
        let (headsign, direction, block, shape, wheelchair, bikes) = (
            table.optional("trip_headsign"),
            table.optional("direction_id"),
            table.optional("block_id"),
            table.optional("shape_id"),
            table.optional("wheelchair_accessible"),
            table.optional("bikes_allowed"),
        );
        let mut row = ByteRecord::new();
        while table.read(&mut row, report)? {
            report.trips_listed += 1;
            let id = field(&row, Some(trip_id));
            let route = route_ids.get(field(&row, Some(route_id))).copied();
            let service = service_ids.get(field(&row, Some(service_id))).copied();
            let drop = if route.is_none() {
                Some(DropReason::UnknownRoute)
            } else if service.is_none() {
                Some(DropReason::UnknownService)
            } else {
                None
            };
            let (route, service) = (route.unwrap_or(0), service.unwrap_or(0));
            let shape_text = field(&row, shape);
            let shape = if shape_text.is_empty() {
                None
            } else if let Some((index, _)) = shapes.get(shape_text) {
                Some(*index)
            } else {
                report.warn(|| format!("trip {id:?} names unknown shape {shape_text:?}"));
                None
            };
            if trip_ids.contains_key(id) {
                report.dropped(DropReason::DuplicateTripId);
                report.warn(|| format!("duplicate trip_id {id:?}"));
                continue;
            }
            let index = index_of(headers.len(), "trips")?;
            trip_ids.insert(id.to_owned(), index);
            let direction = match field(&row, direction) {
                "0" => 0,
                "1" => 1,
                _ => DIRECTION_NONE,
            };
            headers.push(TripHeader {
                gtfs_id: pool.intern(id),
                route,
                service,
                shape,
                headsign: pool.intern(field(&row, headsign)),
                block: pool.intern(field(&row, block)),
                bikes: tri_state(field(&row, bikes)),
                wheelchair: tri_state(field(&row, wheelchair)),
                direction,
                drop,
                rows: Vec::new(),
                name: id.to_owned(),
            });
        }
    }

    {
        let mut table = Table::require(source, "stop_times.txt")?;
        let trip_id = table.column("trip_id")?;
        table.identifier(trip_id);
        let stop_id = table.column("stop_id")?;
        let sequence = table.column("stop_sequence")?;
        let (arrival, departure, pickup, drop_off, headsign, dist) = (
            table.optional("arrival_time"),
            table.optional("departure_time"),
            table.optional("pickup_type"),
            table.optional("drop_off_type"),
            table.optional("stop_headsign"),
            table.optional("shape_dist_traveled"),
        );
        let mut row = ByteRecord::new();
        while table.read(&mut row, report)? {
            report.stop_times_listed += 1;
            let id = field(&row, Some(trip_id));
            let Some(&header_index) = trip_ids.get(id) else {
                report.warn(|| format!("stop time names unknown trip {id:?}"));
                continue;
            };
            let header = &mut headers[header_index as usize];
            if header.drop.is_some() {
                continue;
            }
            let Some(&stop) = stop_ids.get(field(&row, Some(stop_id))) else {
                header.drop = Some(DropReason::UnknownStop);
                continue;
            };
            if !stops[stop as usize].boardable {
                header.drop = Some(DropReason::UnboardableStop);
                continue;
            }
            let Ok(sequence) = field(&row, Some(sequence)).parse::<u32>() else {
                header.drop = Some(DropReason::BadTime);
                continue;
            };
            let arrival_text = field(&row, arrival);
            let departure_text = field(&row, departure);
            let arrival = parse_optional_time(arrival_text);
            let departure = parse_optional_time(departure_text);
            if arrival.is_err() || departure.is_err() {
                header.drop = Some(DropReason::BadTime);
                continue;
            }
            let (Ok(arrival), Ok(departure)) = (arrival, departure) else {
                continue;
            };
            let stop_headsign = field(&row, headsign);
            if !stop_headsign.is_empty() {
                report.stop_headsigns += 1;
            }
            header.rows.push(RawStopTime {
                sequence,
                stop,
                arrival,
                departure,
                pickup: rule_type(field(&row, pickup)),
                drop_off: rule_type(field(&row, drop_off)),
                headsign: pool.intern(stop_headsign),
                shape_dist: field(&row, dist)
                    .parse::<f32>()
                    .ok()
                    .filter(|d| d.is_finite()),
            });
        }
    }

    let mut trips = Vec::with_capacity(headers.len());
    let shape_by_index: HashMap<u32, &ShapeModel> = shapes
        .values()
        .map(|(index, shape)| (*index, shape))
        .collect();
    let mut dist_cache: HashMap<(u32, Vec<u32>), Vec<f32>> = HashMap::new();

    for mut header in headers {
        if let Some(reason) = header.drop {
            report.dropped(reason);
            continue;
        }
        if header.rows.len() < 2 {
            report.dropped(DropReason::TooFewStops);
            continue;
        }
        header.rows.sort_by_key(|row| row.sequence);
        if header
            .rows
            .windows(2)
            .any(|pair| pair[0].sequence == pair[1].sequence)
        {
            report.dropped(DropReason::DuplicateStopSequence);
            continue;
        }

        let raw: Vec<RawTime> = header
            .rows
            .iter()
            .map(|row| RawTime {
                arrival: row.arrival,
                departure: row.departure,
                shape_dist: row.shape_dist,
                position: stops[row.stop as usize].coordinate,
            })
            .collect();
        let (times, interpolated) = match interpolate(&raw) {
            Ok(result) => result,
            Err(reason) => {
                report.dropped(reason);
                continue;
            }
        };
        report.interpolated_stop_times += interpolated;
        if times
            .iter()
            .any(|(a, d)| *a > TIME_LIMIT / 2 || *d > TIME_LIMIT / 2)
        {
            report.dropped(DropReason::TimeOutOfRange);
            continue;
        }

        let stop_seq: Vec<u32> = header.rows.iter().map(|row| row.stop).collect();
        let rules: Vec<u8> = header
            .rows
            .iter()
            .map(|row| (row.pickup & 0b11) | ((row.drop_off & 0b11) << 2))
            .collect();
        let sequences: Vec<u32> = header.rows.iter().map(|row| row.sequence).collect();
        let headsigns: Vec<u32> = if header.rows.iter().all(|row| row.headsign == 0) {
            Vec::new()
        } else {
            header.rows.iter().map(|row| row.headsign).collect()
        };

        // Riders see the whole shape; the stops' distances go with it.
        let stop_dists = match header.shape {
            None => None,
            Some(index) => {
                let shape = shape_by_index.get(&index).copied();
                let feed_dists: Option<Vec<f32>> =
                    header.rows.iter().map(|row| row.shape_dist).collect();
                shape.map(|shape| {
                    resolve_stop_dists(feed_dists, shape, index, &stop_seq, stops, &mut dist_cache)
                })
            }
        };

        report.trips_kept += 1;
        let last_arrival = times.last().map_or(0, |pair| pair.0);
        if last_arrival >= 86_400 {
            report.trips_past_midnight += 1;
        }
        report.latest_time = report
            .latest_time
            .max(times.iter().map(|t| t.1).max().unwrap_or(0));
        for stop in &stop_seq {
            stops[*stop as usize].used = true;
        }

        let base = TripModel {
            gtfs_id: header.gtfs_id,
            route: header.route,
            service: header.service,
            shape: header.shape,
            headsign: header.headsign,
            block: header.block,
            bikes: header.bikes,
            wheelchair: header.wheelchair,
            direction: header.direction,
            frequency_run: false,
            stops: stop_seq,
            times,
            rules,
            stop_headsigns: headsigns,
            sequences,
            stop_dists,
        };

        if let Some(spans) = frequencies.get(header.name.as_str()) {
            let first_departure = base.times[0].1;
            let mut emitted = 0;
            for span in spans {
                let mut run_start = span.start;
                while run_start < span.end {
                    let delta = run_start - first_departure;
                    let mut run = base.clone_with_shift(delta);
                    run.frequency_run = true;
                    trips.push(run);
                    emitted += 1;
                    run_start += span.headway;
                }
            }
            report.frequency_runs += emitted;
            report.trip_runs += emitted;
        } else {
            report.trip_runs += 1;
            trips.push(base);
        }
    }
    report.stop_events = trips.iter().map(|trip| trip.times.len()).sum();
    Ok(trips)
}

impl TripModel {
    fn clone_with_shift(&self, delta: i32) -> Self {
        Self {
            gtfs_id: self.gtfs_id,
            route: self.route,
            service: self.service,
            shape: self.shape,
            headsign: self.headsign,
            block: self.block,
            bikes: self.bikes,
            wheelchair: self.wheelchair,
            direction: self.direction,
            frequency_run: self.frequency_run,
            stops: self.stops.clone(),
            times: self
                .times
                .iter()
                .map(|(a, d)| (a + delta, d + delta))
                .collect(),
            rules: self.rules.clone(),
            stop_headsigns: self.stop_headsigns.clone(),
            sequences: self.sequences.clone(),
            stop_dists: self.stop_dists.clone(),
        }
    }
}

/// Where each stop of a trip is along its shape, in the shape's distance units.
///
/// A feed's own stop distances are used when the shape also has its own distances
/// (so the units agree), and they exist for every stop and move forward.
/// Otherwise stops are placed by geometry; the cache avoids repeating that for each trip.
fn resolve_stop_dists(
    feed_dists: Option<Vec<f32>>,
    shape: &ShapeModel,
    shape_index: u32,
    stop_seq: &[u32],
    stops: &[StopModel],
    cache: &mut HashMap<(u32, Vec<u32>), Vec<f32>>,
) -> Vec<f32> {
    if shape.dist_given
        && let Some(given) = feed_dists
        && given.windows(2).all(|pair| pair[0] <= pair[1])
    {
        return given;
    }
    cache
        .entry((shape_index, stop_seq.to_vec()))
        .or_insert_with(|| {
            let positions: Vec<(f64, f64)> = stop_seq
                .iter()
                .filter_map(|&stop| stops[stop as usize].coordinate)
                .collect();
            project(&shape.points, &positions)
        })
        .clone()
}

fn parse_optional_time(text: &str) -> Result<Option<i32>, ()> {
    if text.is_empty() {
        return Ok(None);
    }
    parse_time(text).map(Some).ok_or(())
}

/// A GTFS `pickup_type` or `drop_off_type`; anything else is regular service.
fn rule_type(text: &str) -> u8 {
    match text {
        "1" => 1,
        "2" => 2,
        "3" => 3,
        _ => 0,
    }
}

/// A GTFS tri-state flag: `0` for empty or anything else, `1` yes, `2` no.
fn tri_state(text: &str) -> u8 {
    match text {
        "1" => 1,
        "2" => 2,
        _ => 0,
    }
}
