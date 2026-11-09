//! Measures the overlay builder on recorded King County GTFS-RT captures.
//!
//! Reads a static GTFS directory and a directory of capture subdirectories, each holding
//! `tripupdates.pb`, `vehiclepositions.pb`, and `alerts.pb`. Run with:
//!
//! ```text
//! cargo run --release --example kcm_report -- <static-dir> <captures-dir> <utc-offset-seconds>
//! ```
//!
//! This reads `trips.txt` and `stop_times.txt` directly, and exists only to measure real feeds.
//! The feed data never goes in git.

#![allow(clippy::cast_precision_loss, clippy::too_many_lines)]

use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use valinor_gtfs_rt::{
    CalendarDate, DecodedFeed, FeedId, FeedInput, FeedKind, OverlayOptions, RouteInfo,
    ScheduledTrip, StaticLookup, TripQuery, TripRef, WindowDay, build_overlay,
};
use valinor_transit::{
    DaySlot, RouteAttributes, RouteId, Seconds, ServiceDate, ServiceDay, StopId, StopRules,
    StopTime, Time, Timetable, TimetableBuilder, TripId,
};

/// Column positions by name, and the rows.
type Csv = (HashMap<String, usize>, Vec<Vec<String>>);

/// A minimal CSV row reader: GTFS files here quote only free text, which this handles.
fn read_csv(path: &Path) -> Result<Csv, Box<dyn Error>> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or("empty file")?
        .trim_start_matches('\u{feff}');
    let columns = split_csv(header)
        .into_iter()
        .enumerate()
        .map(|(i, name)| (name, i))
        .collect();
    let rows = lines.map(split_csv).collect();
    Ok((columns, rows))
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(core::mem::take(&mut field)),
            c => field.push(c),
        }
    }
    out.push(field);
    out
}

fn parse_gtfs_time(text: &str) -> Option<i32> {
    let mut parts = text.trim().split(':');
    let hours: i32 = parts.next()?.parse().ok()?;
    let minutes: i32 = parts.next()?.parse().ok()?;
    let seconds: i32 = parts.next()?.parse().ok()?;
    Some(hours * 3600 + minutes * 60 + seconds)
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_unix(date: u32, utc_offset: i64) -> i64 {
    let year = i64::from(date / 10_000);
    let month = i64::from(date / 100 % 100);
    let day = i64::from(date % 100);
    days_from_civil(year, month, day) * 86_400 - utc_offset
}

struct Lookup {
    feed: FeedId,
    days: Vec<WindowDay>,
    service_days: Vec<ServiceDay>,
    timetable: Timetable,
    stops: HashMap<String, StopId>,
    stop_names: HashMap<StopId, String>,
    /// `trip_id` to `(TripId, route, active dates)`.
    trips: HashMap<String, (TripId, RouteId, BTreeSet<u32>)>,
    /// Per trip, its direction and stop sequences.
    trip_details: HashMap<TripId, (Option<u32>, Vec<u32>)>,
    routes: Vec<(String, Option<String>, i32)>,
    route_patterns: HashMap<String, Vec<RouteId>>,
    next_trip: u32,
}

impl StaticLookup for Lookup {
    fn window_days(&self) -> &[WindowDay] {
        &self.days
    }

    fn trip(&self, feed: &FeedId, query: &TripQuery<'_>) -> Option<TripRef> {
        if feed != &self.feed {
            return None;
        }
        let (trip, route, active) = self.trips.get(query.trip_id)?;
        if !active.contains(&query.date.digits()) {
            return None;
        }
        let slot = self.days.iter().position(|d| d.date == query.date)?;
        let service = self.service_days[slot];
        let resolved = self
            .timetable
            .resolved_trips(*route, service.date, service.offset)
            .ok()?;
        let found = resolved.into_iter().find(|t| t.trip == *trip)?;
        Some(TripRef {
            trip: *trip,
            route: *route,
            day: DaySlot::new(u8::try_from(slot).ok()?),
            first_departure: found.events.first()?.departure,
            last_arrival: found.events.last()?.arrival,
        })
    }

    fn route_patterns(&self, feed: &FeedId, route_id: &str) -> Vec<RouteId> {
        if feed != &self.feed {
            return Vec::new();
        }
        self.route_patterns
            .get(route_id)
            .cloned()
            .unwrap_or_default()
    }

    fn route_info(&self, route: RouteId) -> Option<RouteInfo<'_>> {
        let (route_id, agency, route_type) = self.routes.get(route.into_inner() as usize)?;
        Some(RouteInfo {
            feed: &self.feed,
            route_id,
            agency_id: agency.as_deref(),
            route_type: *route_type,
        })
    }

    fn stop(&self, feed: &FeedId, stop_id: &str) -> Option<StopId> {
        (feed == &self.feed)
            .then(|| self.stops.get(stop_id).copied())
            .flatten()
    }

    fn stop_gtfs_id(&self, feed: &FeedId, stop: StopId) -> Option<&str> {
        (feed == &self.feed)
            .then(|| self.stop_names.get(&stop).map(String::as_str))
            .flatten()
    }

    fn scheduled_block(&self, route: RouteId, day: DaySlot) -> Vec<ScheduledTrip> {
        let service = self.service_days[usize::from(day.into_inner())];
        self.timetable
            .resolved_trips(route, service.date, service.offset)
            .unwrap_or_default()
            .into_iter()
            .map(|trip| {
                let (direction, sequences) = self
                    .trip_details
                    .get(&trip.trip)
                    .cloned()
                    .unwrap_or_default();
                ScheduledTrip {
                    trip: trip.trip,
                    direction_id: direction,
                    events: trip.events,
                    stop_sequences: sequences,
                }
            })
            .collect()
    }

    fn first_unused_trip_id(&self) -> u32 {
        self.next_trip
    }
}

/// Dates each service runs on, restricted to the window.
fn active_dates(
    static_dir: &Path,
    window: &[u32],
    utc_offset: i64,
) -> Result<HashMap<String, BTreeSet<u32>>, Box<dyn Error>> {
    let mut active: HashMap<String, BTreeSet<u32>> = HashMap::new();
    let (columns, rows) = read_csv(&static_dir.join("calendar.txt"))?;
    let col = |row: &Vec<String>, name: &str| row[columns[name]].clone();
    for row in &rows {
        let start: u32 = col(row, "start_date").parse()?;
        let end: u32 = col(row, "end_date").parse()?;
        for &date in window {
            if date < start || date > end {
                continue;
            }
            // 1970-01-01 was a Thursday, so the day index offset is 3 from Monday.
            let days = date_unix(date, utc_offset).div_euclid(86_400) + 3;
            let weekday = match days.rem_euclid(7) {
                0 => "monday",
                1 => "tuesday",
                2 => "wednesday",
                3 => "thursday",
                4 => "friday",
                5 => "saturday",
                _ => "sunday",
            };
            if col(row, weekday) == "1" {
                active
                    .entry(col(row, "service_id"))
                    .or_default()
                    .insert(date);
            }
        }
    }
    let (columns, rows) = read_csv(&static_dir.join("calendar_dates.txt"))?;
    for row in &rows {
        let service = row[columns["service_id"]].clone();
        let date: u32 = row[columns["date"]].parse()?;
        if !window.contains(&date) {
            continue;
        }
        let entry = active.entry(service).or_default();
        match row[columns["exception_type"]].as_str() {
            "1" => {
                entry.insert(date);
            }
            "2" => {
                entry.remove(&date);
            }
            _ => {}
        }
    }
    Ok(active)
}

struct Row {
    sequence: u32,
    stop_id: String,
    arrival: i32,
    departure: i32,
}

fn load(
    static_dir: &Path,
    feed: &FeedId,
    window: &[u32],
    utc_offset: i64,
) -> Result<(Lookup, Arc<valinor_transit::BaseWindow>), Box<dyn Error>> {
    let active = active_dates(static_dir, window, utc_offset)?;

    // Route metadata.
    let (columns, rows) = read_csv(&static_dir.join("routes.txt"))?;
    let mut route_meta = HashMap::new();
    for row in &rows {
        route_meta.insert(
            row[columns["route_id"]].clone(),
            (
                row.get(columns["agency_id"]).cloned(),
                row[columns["route_type"]].parse::<i32>().unwrap_or(3),
            ),
        );
    }

    // Trips running in the window.
    let (columns, rows) = read_csv(&static_dir.join("trips.txt"))?;
    let mut trips: HashMap<String, (String, String, Option<u32>)> = HashMap::new();
    for row in &rows {
        let service = row[columns["service_id"]].clone();
        if active.get(&service).is_none_or(BTreeSet::is_empty) {
            continue;
        }
        trips.insert(
            row[columns["trip_id"]].clone(),
            (
                row[columns["route_id"]].clone(),
                service,
                row[columns["direction_id"]].parse().ok(),
            ),
        );
    }

    // Stop times of those trips.
    let (columns, rows) = read_csv(&static_dir.join("stop_times.txt"))?;
    let mut times: HashMap<String, Vec<Row>> = HashMap::new();
    for row in &rows {
        let trip_id = &row[columns["trip_id"]];
        if !trips.contains_key(trip_id) {
            continue;
        }
        times.entry(trip_id.clone()).or_default().push(Row {
            sequence: row[columns["stop_sequence"]].parse()?,
            stop_id: row[columns["stop_id"]].clone(),
            arrival: parse_gtfs_time(&row[columns["arrival_time"]]).unwrap_or_default(),
            departure: parse_gtfs_time(&row[columns["departure_time"]]).unwrap_or_default(),
        });
    }

    // Numbers.
    let service_date = |date: u32| {
        let index = window.iter().position(|&d| d == date).unwrap_or_default();
        ServiceDate::new(u32::try_from(index).unwrap_or_default())
    };
    let mut builder = TimetableBuilder::new();
    let mut service_ids = HashMap::new();
    for (service, dates) in &active {
        let id = builder.add_service(dates.iter().map(|&d| service_date(d)))?;
        service_ids.insert(service.clone(), id);
    }
    let mut stop_keys = HashMap::new();
    // Routes are numbered in the order their patterns are added.
    let mut patterns: HashMap<(String, Vec<String>), (valinor_transit::PatternKey, RouteId)> =
        HashMap::new();
    let mut route_of_pattern = Vec::new();
    let mut trip_details = HashMap::new();
    let mut trip_index: HashMap<String, (TripId, RouteId, BTreeSet<u32>)> = HashMap::new();
    let mut next_trip = 0_u32;

    let mut sorted: Vec<_> = trips.into_iter().collect();
    sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    for (trip_id, (route_id, service, direction)) in sorted {
        let Some(mut rows) = times.remove(&trip_id) else {
            continue;
        };
        rows.sort_by_key(|r| r.sequence);
        if rows.len() < 2 {
            continue;
        }
        let stops: Vec<String> = rows.iter().map(|r| r.stop_id.clone()).collect();
        for stop in &stops {
            if !stop_keys.contains_key(stop) {
                stop_keys.insert(stop.clone(), builder.add_stop(Seconds::ZERO)?);
            }
        }
        let key = (route_id.clone(), stops.clone());
        let (pattern, route) = if let Some(&found) = patterns.get(&key) {
            found
        } else {
            let keys: Vec<_> = stops.iter().map(|s| stop_keys[s]).collect();
            let pattern = builder.add_pattern(&keys, RouteAttributes::NONE)?;
            let route = RouteId::new(u32::try_from(route_of_pattern.len())?);
            patterns.insert(key, (pattern, route));
            route_of_pattern.push(route_id.clone());
            (pattern, route)
        };
        let events: Vec<StopTime> = rows
            .iter()
            .map(|r| StopTime {
                arrival: Time::try_new(r.arrival).unwrap_or(Time::try_new(0).expect("zero")),
                departure: Time::try_new(r.departure).unwrap_or(Time::try_new(0).expect("zero")),
                rules: StopRules::REGULAR,
            })
            .collect();
        let trip = TripId::new(next_trip);
        next_trip += 1;
        builder.add_trip(pattern, trip, service_ids[&service], events)?;
        trip_details.insert(trip, (direction, rows.iter().map(|r| r.sequence).collect()));
        let dates = active[&service].clone();
        trip_index.insert(trip_id, (trip, route, dates));
    }

    let timetable = builder.build()?;
    let days: Vec<WindowDay> = window
        .iter()
        .map(|&d| WindowDay {
            date: CalendarDate::parse(&d.to_string()).expect("window date"),
            start_unix: date_unix(d, utc_offset),
        })
        .collect();
    let first_unix = days[0].start_unix;
    let service_days: Vec<ServiceDay> = days
        .iter()
        .zip(window)
        .map(|(day, &d)| ServiceDay {
            date: service_date(d),
            offset: Time::try_new(i32::try_from(day.start_unix - first_unix).unwrap_or_default())
                .unwrap_or(Time::try_new(0).expect("zero")),
        })
        .collect();
    let base = Arc::new(timetable.materialize(&service_days)?);

    let stops: HashMap<String, StopId> = stop_keys
        .iter()
        .map(|(name, &key)| (name.clone(), timetable.stop_id(key)))
        .collect();
    let stop_names = stops.iter().map(|(n, &s)| (s, n.clone())).collect();
    let routes: Vec<(String, Option<String>, i32)> = route_of_pattern
        .iter()
        .map(|r| {
            let (agency, route_type) = route_meta.get(r).cloned().unwrap_or((None, 3));
            (r.clone(), agency, route_type)
        })
        .collect();
    let mut route_patterns: HashMap<String, Vec<RouteId>> = HashMap::new();
    for (i, r) in route_of_pattern.iter().enumerate() {
        route_patterns
            .entry(r.clone())
            .or_default()
            .push(RouteId::new(u32::try_from(i)?));
    }
    Ok((
        Lookup {
            feed: feed.clone(),
            days,
            service_days,
            timetable,
            stops,
            stop_names,
            trips: trip_index,
            trip_details,
            routes,
            route_patterns,
            next_trip,
        },
        base,
    ))
}

#[derive(Default)]
struct Series(Vec<f64>);
impl Series {
    fn push(&mut self, value: f64) {
        self.0.push(value);
    }
    fn summary(&mut self) -> String {
        if self.0.is_empty() {
            return "n/a".into();
        }
        self.0.sort_by(f64::total_cmp);
        let n = self.0.len();
        format!(
            "min {:.2} / median {:.2} / max {:.2}",
            self.0[0],
            self.0[n / 2],
            self.0[n - 1]
        )
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let static_dir = args.next().ok_or("static dir")?;
    let captures_dir = args.next().ok_or("captures dir")?;
    let utc_offset: i64 = args.next().ok_or("utc offset")?.parse()?;
    let feed = FeedId::new("kcm")?;

    let mut captures: Vec<_> = fs::read_dir(&captures_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    captures.sort();
    println!("captures found: {}", captures.len());

    let window: Vec<u32> = vec![20_261_107, 20_261_108, 20_261_109, 20_261_110];
    let started = Instant::now();
    let (lookup, base) = load(Path::new(&static_dir), &feed, &window, utc_offset)?;
    println!(
        "loaded static in {:.1?}: {} stops, {} patterns, {} trips in the window",
        started.elapsed(),
        base.stop_count(),
        base.route_count(),
        lookup.trips.len()
    );

    let mut decoded_ok = 0_u64;
    let mut decode_errors = Vec::new();
    let mut partial = Vec::new();
    let mut build_times = Vec::new();
    let mut last = None;

    let mut entities = Series::default();
    let mut matched = Series::default();
    let mut routes_changed = Series::default();
    let mut trips_changed = Series::default();
    let mut overtaking_blocks = Series::default();
    let mut overtaking_new = Series::default();
    let mut lanes = Series::default();
    let mut max_lanes = Series::default();
    let mut arena_mb = Series::default();
    let mut skipped_stops = Series::default();
    let mut raw_skipped = Series::default();
    let mut no_service_alerts = Series::default();
    let mut tu_routes = Series::default();
    let mut tu_trips = Series::default();
    let mut tu_overtaking = Series::default();
    let mut tu_overtaking_lanes = Series::default();
    let mut tu_lanes = Series::default();
    let mut tu_max_lanes = Series::default();
    let mut tu_arena_mb = Series::default();
    let mut captures_with_overtaking = 0_usize;
    let mut measured = 0_usize;

    for capture in &captures {
        let paths = [
            capture.join("tripupdates.pb"),
            capture.join("vehiclepositions.pb"),
            capture.join("alerts.pb"),
        ];
        if !paths.iter().all(|p| p.is_file()) {
            partial.push(capture.clone());
            continue;
        }
        let mut inputs = Vec::new();
        let mut failed = false;
        for (path, kind) in paths.iter().zip([
            FeedKind::TripUpdates,
            FeedKind::VehiclePositions,
            FeedKind::Alerts,
        ]) {
            let bytes = fs::read(path)?;
            match DecodedFeed::decode(&bytes) {
                Ok(decoded) => {
                    decoded_ok += 1;
                    inputs.push(FeedInput::new(feed.clone(), kind, decoded));
                }
                Err(error) => {
                    failed = true;
                    decode_errors.push(format!("{}: {error}", path.display()));
                }
            }
        }
        if failed {
            continue;
        }
        let trip_entities = inputs[0].decoded.message().entity.len();
        raw_skipped.push(
            inputs[0]
                .decoded
                .message()
                .entity
                .iter()
                .filter_map(|e| e.trip_update.as_ref())
                .flat_map(|u| &u.stop_time_update)
                .filter(|s| {
                    s.schedule_relationship()
                        == valinor_gtfs_rt::proto::trip_update::stop_time_update::ScheduleRelationship::Skipped
                })
                .count() as f64,
        );
        let now = inputs[0].decoded.timestamp().ok_or("no header timestamp")?;

        let started = Instant::now();
        let overlay = build_overlay(
            &lookup,
            Arc::clone(&base),
            inputs,
            now,
            OverlayOptions::default(),
        )?;
        build_times.push(started.elapsed().as_secs_f64() * 1000.0);

        let stats = &overlay.stats;
        no_service_alerts.push(stats.no_service_alerts as f64);
        entities.push(trip_entities as f64);
        matched.push(stats.trip_updates_matched as f64);
        routes_changed.push(stats.routes_changed as f64);
        trips_changed.push((stats.trips_updated + stats.trips_removed + stats.trips_added) as f64);
        overtaking_blocks.push(stats.overtaking_blocks as f64);
        overtaking_new.push(stats.overtaking_blocks_new as f64);
        lanes.push(stats.lanes as f64);
        max_lanes.push(stats.max_lanes as f64);
        arena_mb.push(stats.arena.bytes as f64 / 1_048_576.0);
        skipped_stops.push(stats.stops_skipped as f64);

        // The same trip updates alone, to separate the effect of delays from that of alerts.
        let only_updates = build_overlay(
            &lookup,
            Arc::clone(&base),
            vec![FeedInput::new(
                feed.clone(),
                FeedKind::TripUpdates,
                DecodedFeed::decode(&fs::read(&paths[0])?)?,
            )],
            now,
            OverlayOptions::default(),
        )?;
        let tu = &only_updates.stats;
        tu_routes.push(tu.routes_changed as f64);
        tu_trips.push((tu.trips_updated + tu.trips_removed + tu.trips_added) as f64);
        tu_overtaking.push(tu.overtaking_blocks_new as f64);
        tu_overtaking_lanes.push(tu.overtaking_lanes as f64);
        tu_lanes.push(tu.lanes as f64);
        tu_max_lanes.push(tu.max_lanes as f64);
        tu_arena_mb.push(tu.arena.bytes as f64 / 1_048_576.0);
        measured += 1;
        if tu.overtaking_blocks_new > 0 {
            captures_with_overtaking += 1;
        }

        last = Some(overlay);
    }

    println!(
        "decoded ok: {decoded_ok} feeds; errors: {}",
        decode_errors.len()
    );
    for error in &decode_errors {
        println!("  {error}");
    }
    println!("partial captures skipped: {}", partial.len());
    println!("trip update entities: {}", entities.summary());
    println!("matched to static trips: {}", matched.summary());
    println!("routes changed: {}", routes_changed.summary());
    println!("trips changed: {}", trips_changed.summary());
    println!("stop visits skipped: {}", skipped_stops.summary());
    println!(
        "blocks with overtaking lanes: {}",
        overtaking_blocks.summary()
    );
    println!("  beyond the schedule's own: {}", overtaking_new.summary());
    println!("lanes laid out: {}", lanes.summary());
    println!("largest lane count in a block: {}", max_lanes.summary());
    println!("arena MB: {}", arena_mb.summary());
    println!("build ms: {}", {
        let mut b = Series(build_times.clone());
        b.summary()
    });
    println!("raw SKIPPED stop updates: {}", raw_skipped.summary());
    println!("NO_SERVICE alerts: {}", no_service_alerts.summary());
    println!("\nwith trip updates only (no alerts):");
    println!("  routes changed: {}", tu_routes.summary());
    println!("  trips changed: {}", tu_trips.summary());
    println!(
        "  blocks that overtake beyond their schedule: {}",
        tu_overtaking.summary()
    );
    println!("  overtaking lanes: {}", tu_overtaking_lanes.summary());
    println!("  lanes laid out: {}", tu_lanes.summary());
    println!("  largest lane count: {}", tu_max_lanes.summary());
    println!("  arena MB: {}", tu_arena_mb.summary());
    println!("  captures with new overtaking: {captures_with_overtaking} of {measured}");

    if let Some(overlay) = last {
        let stats = &overlay.stats;
        println!("\nlast capture details:");
        println!(
            "  trip updates: {} matched {}",
            stats.trip_updates, stats.trip_updates_matched
        );
        println!(
            "  unmatched {} outside window {} duplicates {} without trip {}",
            stats.trip_updates_unmatched,
            stats.trip_updates_outside_window,
            stats.duplicate_trip_updates,
            stats.trip_updates_without_trip
        );
        println!(
            "  stop updates unmatched {} times repaired {} out of range {}",
            stats.stop_updates_unmatched, stats.times_repaired, stats.times_out_of_range
        );
        println!(
            "  vehicles {} matched {}",
            stats.vehicles, stats.vehicles_matched
        );
        println!(
            "  blocks replaced {} removed {}; lanes {} (max {}), overtaking blocks {} (new {}), overtaking lanes {}",
            stats.blocks_replaced,
            stats.blocks_removed,
            stats.lanes,
            stats.max_lanes,
            stats.overtaking_blocks,
            stats.overtaking_blocks_new,
            stats.overtaking_lanes
        );
        println!(
            "  added routes {} new stops {}",
            stats.added_routes, stats.new_stops
        );
        println!("  arena: {:?}", stats.arena);
        println!(
            "  alerts {} NO_SERVICE {}",
            stats.alerts, stats.no_service_alerts
        );
        let mut impacts: Vec<_> = stats.alert_impacts.iter().collect();
        impacts.sort_by_key(|(_, i)| core::cmp::Reverse((i.trips_removed, i.stops_removed)));
        for (index, impact) in impacts {
            let alert = &overlay.rider.alerts()[*index];
            let scope = alert
                .informed
                .iter()
                .map(|e| {
                    if e.trip_id.is_some() {
                        "trip"
                    } else if e.stop_id.is_some() {
                        "stop"
                    } else if e.route_id.is_some() {
                        "route"
                    } else if e.agency_id.is_some() {
                        "agency"
                    } else {
                        "other"
                    }
                })
                .collect::<BTreeSet<_>>();
            println!(
                "    alert {index}: trips removed {}, stop visits removed {}, informs {} entities ({scope:?}), periods {}",
                impact.trips_removed,
                impact.stops_removed,
                alert.informed.len(),
                alert.active_periods.len() + alert.impact_periods.len(),
            );
        }
    }
    Ok(())
}
