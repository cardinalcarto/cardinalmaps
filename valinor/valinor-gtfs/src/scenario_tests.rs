//! Whole-crate scenarios: compile a synthetic feed, then load and read it back.

use core::fmt::Write as _;
use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use proptest::prelude::*;
use valinor_transit::{PairRule, ServiceDate, StopId, Timetable, TripId};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::DEFAULT_TRANSFER_SECONDS;
use crate::format::layout::{STOP_NO_CHANGE, checksum_of};
use crate::{
    Accessibility, CompileError, CompileOptions, CompiledOutput, CompiledTimetable, DecodeError,
    DirectorySource, DropReason, FORMAT_VERSION, FeedId, FeedInput, InMemorySource, LatLon,
    LoadedTimetable, LocationType, ZipSource, compile, compile_with, parse_service_date,
    service_date_of,
};

const AGENCY: &str = "agency_name,agency_timezone,agency_url,agency_phone,agency_fare_url\n\
    Metro,America/Los_Angeles,https://m.test,555-1000,https://m.test/fare\n";

fn base_feed() -> InMemorySource {
    InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with(
            "routes.txt",
            "route_id,route_short_name,route_long_name,route_desc,route_type,route_url,route_color,route_text_color\n\
             R1,1,First Avenue,Crosstown,3,https://m.test/1,FDB71A,000000\n\
             R2,2,Second Avenue,,3,,not-a-color,\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_code,stop_lat,stop_lon,wheelchair_boarding,platform_code\n\
             S1,Alpha,100,47.600,-122.330,1,A\n\
             S2,Beta,200,47.610,-122.330,2,\n\
             S3,Gamma,300,47.620,-122.330,,\n\
             S4,Delta,400,47.630,-122.330,,\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with("calendar_dates.txt", "service_id,date,exception_type\nWK,20260620,1\nWK,20260601,2\n")
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,trip_headsign,block_id,wheelchair_accessible,bikes_allowed,direction_id\n\
             R1,WK,T1,Northbound,B7,1,1,0\n\
             R1,WK,T2,Northbound,B7,1,2,0\n\
             R2,WK,T3,Eastbound,,,,1\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence,stop_headsign,pickup_type,drop_off_type\n\
             T1,08:00:00,08:00:30,S1,10,,,\n\
             T1,08:10:00,08:10:00,S2,20,Via Beta,,\n\
             T1,08:20:00,08:20:00,S3,30,,1,\n\
             T2,09:00:00,09:00:00,S1,10,,,\n\
             T2,09:10:00,09:10:00,S2,20,,,\n\
             T2,09:20:00,09:20:00,S3,30,,,2\n\
             T3,25:30:00,25:30:00,S1,1,,,\n\
             T3,25:40:00,25:40:00,S4,2,,,\n",
        )
}

fn compile_one(source: InMemorySource) -> CompiledOutput {
    compile(vec![FeedInput {
        id: FeedId::try_new("kcm".to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compile")
}

fn date(text: &str) -> ServiceDate {
    parse_service_date(text).expect("date")
}

/// Every GTFS rule the basic feed covers.
#[test]
fn compiles_and_reads_the_basic_feed() {
    let output = compile_one(base_feed());
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let feeds: Vec<_> = view.feeds().collect();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].id, "kcm");
    assert_eq!(feeds[0].time_zone, "America/Los_Angeles");

    assert_eq!(view.gtfs_route_count(), 2);
    assert_eq!(view.trip_count(), 3);
    assert_eq!(view.stop_event_count(), 8);
    assert_eq!(view.location_count(), 4);
    assert_eq!(view.stop_count(), 4);

    // Route fields, including rejected colors and an omitted agency_id on a single-agency feed.
    let route = view.route_by_gtfs_id("kcm", "R1").expect("route");
    let info = view.route(route).expect("info");
    assert_eq!(info.short_name, Some("1"));
    assert_eq!(info.long_name, Some("First Avenue"));
    assert_eq!(info.description, Some("Crosstown"));
    assert_eq!(info.route_type, 3);
    assert_eq!(info.color, Some([0xFD, 0xB7, 0x1A]));
    assert_eq!(info.text_color, Some([0, 0, 0]));
    assert_eq!(info.url, Some("https://m.test/1"));
    let agency = view
        .agency(info.agency.expect("agency"))
        .expect("agency info");
    assert_eq!(agency.name, "Metro");
    assert_eq!(agency.phone, Some("555-1000"));
    assert_eq!(agency.fare_url, Some("https://m.test/fare"));
    let other = view
        .route(view.route_by_gtfs_id("kcm", "R2").expect("route"))
        .expect("info");
    assert_eq!(other.color, None);

    // A stop and its rider fields.
    let stop_id = view.stop_by_gtfs_id("kcm", "S1").expect("stop");
    let stop = view.stop(stop_id).expect("stop");
    assert_eq!(stop.name, Some("Alpha"));
    assert_eq!(stop.code, Some("100"));
    assert_eq!(stop.platform, Some("A"));
    assert_eq!(stop.wheelchair_boarding, Accessibility::Yes);
    assert_eq!(stop.location_type, LocationType::Stop);
    assert_eq!(stop.time_zone, "America/Los_Angeles");
    let coordinate = stop.coordinate.expect("coordinate");
    assert!((coordinate.lat - 47.6).abs() < 1e-6 && (coordinate.lon + 122.33).abs() < 1e-6);
    assert_eq!(view.stop_by_gtfs_id("other", "S1"), None);

    // Calendars: the trip runs weekdays plus the added date, minus the removed date.
    let trip_id = view
        .trips_by_gtfs_id("kcm", "T1", date("20260602"))
        .next()
        .expect("run");
    assert_eq!(view.active_trip_count(date("20260602")), 3);
    assert_eq!(
        view.active_trip_count(date("20260601")),
        0,
        "removed by exception"
    );
    assert_eq!(
        view.active_trip_count(date("20260620")),
        3,
        "added by exception"
    );
    assert_eq!(view.active_trip_count(date("20260621")), 0, "a Sunday");
    assert_eq!(
        view.trips_by_gtfs_id("kcm", "T1", date("20260621")).count(),
        0
    );

    // Trip rider fields: stop sequences, pickup rules, and stop headsigns.
    let trip = view.trip(trip_id.trip).expect("trip");
    assert_eq!(trip.gtfs_id, "T1");
    assert_eq!(trip.headsign, Some("Northbound"));
    assert_eq!(trip.block_id, Some("B7"));
    assert_eq!(trip.wheelchair_accessible, Accessibility::Yes);
    assert_eq!(trip.bikes_allowed, Accessibility::Yes);
    assert_eq!(trip.direction, Some(0));
    assert!(!trip.frequency_run);
    assert_eq!(trip.stop_count(), 3);
    assert_eq!(trip.time_at(0), Some((8 * 3600, 8 * 3600 + 30)));
    assert_eq!(trip.position_of_sequence(20), Some(1));
    assert_eq!(trip.stop_sequence_at(2), Some(30));
    assert_eq!(trip.pickup_type_at(2), 1);
    assert_eq!(view.stop_headsign(&trip, 1), Some("Via Beta"));
    assert_eq!(view.stop_headsign(&trip, 0), Some("Northbound"));
    assert_eq!(trip_id.first_departure, 8 * 3600 + 30);

    // Times past midnight stay on the same service day.
    let t3 = view
        .trips_by_gtfs_id("kcm", "T3", date("20260602"))
        .next()
        .expect("run");
    assert_eq!(t3.first_departure, 25 * 3600 + 30 * 60);
    assert_eq!(
        view.trip(t3.trip).expect("trip").time_at(1),
        Some((25 * 3600 + 40 * 60, 25 * 3600 + 40 * 60))
    );

    // Unknown lookups.
    assert!(view.trip(TripId::new(99)).is_none());
    assert!(view.stop(StopId::new(99)).is_none());
    assert_eq!(view.route_by_gtfs_id("kcm", "nope"), None);
    assert_eq!(
        view.trips_by_gtfs_id("kcm", "nope", date("20260602"))
            .count(),
        0
    );
}

#[test]
fn bikes_split_patterns_but_rules_do_not() {
    let output = compile_one(base_feed());
    let report = &output.reports[0];
    assert_eq!(report.patterns, 3, "R1 splits by bikes; R2 is separate");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let route = view.route_by_gtfs_id("kcm", "R1").expect("route");
    let patterns = view.route_patterns(route).expect("patterns");
    assert_eq!(patterns.len(), 2);
    let mut bikes: Vec<bool> = patterns
        .iter()
        .map(|&pattern| view.pattern(pattern).expect("pattern").bikes_allowed)
        .collect();
    bikes.sort_unstable();
    assert_eq!(bikes, vec![false, true]);

    // A pickup rule difference alone stays in one pattern; the builder splits lanes per day.
    let same_stops = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\nR,S,Y\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence,pickup_type\n\
             X,08:00:00,08:00:00,A,1,\n\
             X,08:10:00,08:10:00,B,2,\n\
             Y,08:30:00,08:30:00,A,1,1\n\
             Y,08:40:00,08:40:00,B,2,\n",
        );
    let output = compile_one(same_stops);
    assert_eq!(output.reports[0].patterns, 1);
}

#[test]
fn timetable_replay_matches_the_compiled_trips() {
    let output = compile_one(base_feed());
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let timetable = view.to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), 4);
    assert_eq!(timetable.route_count(), view.pattern_count());
    let window = view
        .service_window(&[date("20260602"), date("20260603")])
        .expect("window");
    assert_eq!(window.days[0].offset.into_inner(), 0);
    assert_eq!(window.days[1].offset.into_inner(), 86_400);
    let materialized = timetable.materialize(&window.days).expect("materialize");
    assert_eq!(materialized.stop_count(), 4);

    let mut seen = 0;
    for pattern_index in 0..view.pattern_count() {
        let route = valinor_transit::RouteId::new(u32::try_from(pattern_index).expect("small"));
        let pattern = view.pattern(route).expect("pattern");
        // The file's stop numbering and the timetable's agree.
        assert_eq!(
            timetable.route_stops(route).expect("stops").to_vec(),
            pattern.stops().collect::<Vec<_>>()
        );
        for resolved in timetable
            .resolved_trips(route, date("20260602"), window.days[0].offset)
            .expect("trips")
        {
            seen += 1;
            let info = view.trip(resolved.trip).expect("trip");
            for (position, event) in resolved.events.iter().enumerate() {
                let (arrival, departure) = info.time_at(position).expect("time");
                assert_eq!(event.arrival.into_inner(), arrival);
                assert_eq!(event.departure.into_inner(), departure);
            }
            // The rule bytes the core receives.
            let rule = resolved.events[2.min(resolved.events.len() - 1)].rules;
            if info.gtfs_id == "T1" {
                assert!(!rule.pickup);
                assert!(rule.drop_off);
            }
            if info.gtfs_id == "T2" {
                assert!(rule.pickup, "phone or none on drop-off only");
                assert!(rule.drop_off, "drop_off_type 2 is still routable");
            }
        }
    }
    assert_eq!(seen, 3);
}

fn day_after(date: ServiceDate) -> ServiceDate {
    ServiceDate::new(date.into_inner() + 1)
}

/// Spring forward on 2027-03-14 and fall back on 2027-11-07, in the feed's own zone.
#[test]
fn dst_service_days_are_23_or_25_hours() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with("stops.txt", "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\n")
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20270312,1\nS,20270314,1\nS,20271106,1\nS,20271108,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
        );
    let output = compile_one(source);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");

    let spring = view
        .service_day_start_unix(date("20270314"))
        .expect("covered");
    let before = view
        .service_day_start_unix(date("20270313"))
        .expect("covered");
    let after = view
        .service_day_start_unix(date("20270315"))
        .expect("covered");
    // The zone changes in the small hours of the 14th, and a service day starts at
    // noon minus twelve hours, so the short day is the one that ends at that change.
    assert_eq!(
        spring - before,
        82_800,
        "the 13th, ending at 2 a.m. on the 14th, is 23 hours"
    );
    assert_eq!(
        after - spring,
        86_400,
        "the 14th is an ordinary service day"
    );
    let fall = view
        .service_day_start_unix(date("20271107"))
        .expect("covered");
    let fall_before = view
        .service_day_start_unix(date("20271106"))
        .expect("covered");
    let fall_after = view
        .service_day_start_unix(date("20271108"))
        .expect("covered");
    assert_eq!(fall - fall_before, 90_000, "the 6th is 25 hours");
    assert_eq!(
        fall_after - fall,
        86_400,
        "the 7th is an ordinary service day"
    );

    // Origin is noon minus twelve hours, whatever the zone offset.
    let la = TimeZone::get("America/Los_Angeles").expect("zone");
    for text in ["20270312", "20270314", "20271106", "20271107", "20271108"] {
        let parsed = crate::civil_date_of(date(text)).expect("civil");
        let noon = parsed.at(12, 0, 0, 0).to_zoned(la.clone()).expect("noon");
        assert_eq!(
            view.service_day_start_unix(date(text)),
            Some(noon.timestamp().as_second() - 43_200),
            "{text}",
        );
    }

    // A window across the change applies each day's own offset.
    let window = view
        .service_window(&[date("20270313"), date("20270314"), date("20270315")])
        .expect("window");
    assert_eq!(window.days[1].offset.into_inner(), 82_800);
    assert_eq!(window.days[2].offset.into_inner(), 82_800 + 86_400);

    // A trip's 10:00 on the 14th is 10 wall-clock hours after that day's noon-minus-12.
    let x = view
        .trips_by_gtfs_id("kcm", "X", date("20270314"))
        .next()
        .expect("run");
    assert_eq!(x.first_departure, 36_000);
    assert_eq!(x.service_day_start_unix, Some(spring));

    // The covered range ends two days after the last date.
    let last = date("20271108");
    assert!(view.service_day_start_unix(last).is_some());
    assert!(
        view.service_day_start_unix(day_after(day_after(last)))
            .is_some()
    );
    assert!(
        view.service_day_start_unix(day_after(day_after(day_after(last))))
            .is_none()
    );
    assert_eq!(
        view.service_window(&[ServiceDate::new(1)]),
        Err(DecodeError::DateNotCovered(1))
    );
}

proptest! {
    /// Over random dates in a DST zone, the file's service-day start equals noon minus twelve
    /// hours as `jiff` computes it, and offsets between consecutive days are 23, 24, or 25 hours.
    #[test]
    fn service_day_start_matches_jiff(
        (year, month, day) in (2020_i16..2034, 1_i8..=12, 1_i8..=28),
        zone in prop_oneof![
            Just("America/Los_Angeles"),
            Just("Europe/Stockholm"),
            Just("Asia/Kathmandu"),
            Just("Australia/Lord_Howe"),
        ],
    ) {
        let target = Date::new(year, month, day).expect("valid");
        let start = target.first_of_year();
        let end = start.yesterday().expect("date");
        let source = InMemorySource::new()
            .with(
                "agency.txt",
                &format!("agency_name,agency_timezone\nM,{zone}\n"),
            )
            .with("routes.txt", "route_id,route_type\nR,3\n")
            .with("stops.txt", "stop_id,stop_lat,stop_lon\nA,1.0,1.0\nB,1.1,1.0\n")
            .with(
                "calendar_dates.txt",
                &format!(
                    "service_id,date,exception_type\nS,{},1\nS,{},1\n",
                    end.strftime("%Y%m%d"),
                    target.last_of_year().strftime("%Y%m%d"),
                ),
            )
            .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
            .with(
                "stop_times.txt",
                "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
                 X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
            );
        let output = compile_one(source);
        let view = CompiledTimetable::parse(&output.bytes).expect("parse");
        let tz = TimeZone::get(zone).expect("zone");
        let service_date = service_date_of(target).expect("date");
        let noon = target.at(12, 0, 0, 0).to_zoned(tz.clone()).expect("noon");
        prop_assert_eq!(
            view.service_day_start_unix(service_date),
            Some(noon.timestamp().as_second() - 43_200)
        );
        let next = view
            .service_day_start_unix(day_after(service_date))
            .expect("covered");
        let this = view.service_day_start_unix(service_date).expect("covered");
        prop_assert!([82_800, 84_600, 86_400, 88_200, 90_000].contains(&(next - this)));
    }
}

#[test]
fn stations_keep_platforms_and_inherit_attributes() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon,location_type,parent_station,wheelchair_boarding,stop_timezone,platform_code\n\
             STATION,Hub,47.6,-122.3,1,,1,America/Chicago,\n\
             PLAT1,Hub Bay 1,47.6001,-122.3001,0,STATION,,,1\n\
             PLAT2,Hub Bay 2,47.6002,-122.3002,0,STATION,,,2\n\
             ENTRANCE,Hub North Door,47.6003,-122.3003,2,STATION,,,\n\
             AREA,Bay 1 Waiting,,,4,PLAT1,,,\n",
        )
        .with("calendar_dates.txt", "service_id,date,exception_type\nS,20260602,1\n")
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,PLAT1,1\nX,11:00:00,11:00:00,PLAT2,2\n",
        );
    let output = compile_one(source);
    let report = &output.reports[0];
    assert_eq!(report.stops_listed, 5);
    assert_eq!(report.boardable_stops, 3, "the area inherits coordinates");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.stop_count(), 3);
    assert_eq!(view.location_count(), 5);

    let platform = view.stop_by_gtfs_id("kcm", "PLAT1").expect("platform");
    let stop = view.stop(platform).expect("stop");
    assert_eq!(
        stop.wheelchair_boarding,
        Accessibility::Yes,
        "inherited from the station"
    );
    assert_eq!(
        stop.time_zone, "America/Chicago",
        "inherited from the station"
    );
    assert_eq!(stop.location_type, LocationType::Stop);
    assert_eq!(stop.platform, Some("1"));

    let area = view.stop_by_gtfs_id("kcm", "AREA").expect("boarding area");
    let coordinate = view
        .stop(area)
        .expect("area")
        .coordinate
        .expect("inherited");
    assert!((coordinate.lat - 47.6001).abs() < 1e-6);

    assert_eq!(
        view.stop_by_gtfs_id("kcm", "STATION"),
        None,
        "stations have no StopId"
    );
    assert_eq!(view.stop_by_gtfs_id("kcm", "ENTRANCE"), None);
    let station = view
        .location_by_gtfs_id("kcm", "STATION")
        .expect("location");
    assert_eq!(
        view.location(station).expect("info").location_type,
        LocationType::Station
    );
    let mut members = view.stops_at(station).expect("members");
    members.sort_unstable();
    let mut expected = vec![
        platform,
        view.stop_by_gtfs_id("kcm", "PLAT2").expect("platform"),
        area,
    ];
    expected.sort_unstable();
    assert_eq!(members, expected, "every boardable stop in the station");
    assert_eq!(
        view.stops_at(view.location_by_gtfs_id("kcm", "PLAT1").expect("loc")),
        Some(vec![platform, area]),
        "a platform is listed first, with its boarding areas after it"
    );
}

#[test]
fn trips_with_unusable_data_are_dropped_and_counted() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\nBAD,notatype\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,location_type\nA,47.0,-122.0,\nB,47.1,-122.0,\nSTATION,47.2,-122.0,1\n",
        )
        .with("calendar_dates.txt", "service_id,date,exception_type\nS,20260602,1\n")
        .with(
            "trips.txt",
            "route_id,service_id,trip_id\n\
             R,S,GOOD\nGHOST,S,NOROUTE\nR,NOSERVICE,NOSERVICE\n\
             R,S,SINGLE\nR,S,NOTIMES\nR,S,UNBOARDABLE\nR,S,DUP\nR,S,DUP\nR,S,BACKWARD\nR,S,SAMESEQ\nR,S,UNKNOWNSTOP\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             GOOD,08:00:00,08:00:00,A,1\nGOOD,08:10:00,08:10:00,B,2\n\
             NOROUTE,08:00:00,08:00:00,A,1\nNOROUTE,08:10:00,08:10:00,B,2\n\
             NOSERVICE,08:00:00,08:00:00,A,1\nNOSERVICE,08:10:00,08:10:00,B,2\n\
             SINGLE,08:00:00,08:00:00,A,1\n\
             NOTIMES,,,A,1\nNOTIMES,08:10:00,08:10:00,B,2\n\
             UNBOARDABLE,08:00:00,08:00:00,STATION,1\nUNBOARDABLE,08:10:00,08:10:00,B,2\n\
             DUP,08:00:00,08:00:00,A,1\nDUP,08:10:00,08:10:00,B,2\n\
             BACKWARD,08:10:00,08:10:00,A,1\nBACKWARD,08:00:00,08:00:00,B,2\n\
             SAMESEQ,08:00:00,08:00:00,A,1\nSAMESEQ,08:10:00,08:10:00,B,1\n\
             UNKNOWNSTOP,08:00:00,08:00:00,A,1\nUNKNOWNSTOP,08:10:00,08:10:00,NOPE,2\n",
        );
    let output = compile_one(source);
    let report = &output.reports[0];
    assert_eq!(report.trips_listed, 11);
    assert_eq!(report.trips_kept, 2, "GOOD and the first DUP");
    let expected: BTreeMap<DropReason, usize> = [
        (DropReason::UnknownRoute, 1),
        (DropReason::UnknownService, 1),
        (DropReason::TooFewStops, 1),
        (DropReason::MissingEndpointTimes, 1),
        (DropReason::UnboardableStop, 1),
        (DropReason::DuplicateTripId, 1),
        (DropReason::TimesOutOfOrder, 1),
        (DropReason::DuplicateStopSequence, 1),
        (DropReason::UnknownStop, 1),
    ]
    .into_iter()
    .collect();
    assert_eq!(report.dropped_trips, expected);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("duplicate trip_id"))
    );
    assert_eq!(report.routes, 1, "the route with a bad type is dropped");
}

#[test]
fn missing_stop_times_are_interpolated() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.0005,-122.0\nC,47.002,-122.0\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,,,B,2\nX,10:20:00,10:20:00,C,3\n",
        );
    let output = compile_one(source);
    assert_eq!(output.reports[0].interpolated_stop_times, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let run = view
        .trips_by_gtfs_id("kcm", "X", date("20260602"))
        .next()
        .expect("run");
    let trip = view.trip(run.trip).expect("trip");
    let (arrival, departure) = trip.time_at(1).expect("time");
    assert_eq!(arrival, departure);
    // Stop B is a quarter of the way there by straight-line distance.
    assert_eq!(arrival, 10 * 3600 + 5 * 60);
}

#[test]
fn frequencies_expand_into_timetabled_runs() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id\nR,S,FREQ\nR,S,PLAIN\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             FREQ,10:00:00,10:00:00,A,1\nFREQ,10:30:00,10:30:00,B,2\n\
             PLAIN,12:00:00,12:00:00,A,1\nPLAIN,12:30:00,12:30:00,B,2\n",
        )
        .with(
            "frequencies.txt",
            "trip_id,start_time,end_time,headway_secs,exact_times\nFREQ,10:00:00,11:00:00,1800,0\n",
        );
    let output = compile_one(source);
    assert_eq!(
        output.reports[0].frequency_runs, 2,
        "start times 10:00 and 10:30"
    );
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.trip_count(), 3);
    let mut runs: Vec<i32> = view
        .trips_by_gtfs_id("kcm", "FREQ", date("20260602"))
        .map(|run| {
            let trip = view.trip(run.trip).expect("trip");
            assert!(trip.frequency_run);
            assert!(
                trip.headway_based,
                "exact_times=0 promises only the headway"
            );
            assert_eq!(trip.time_is_approximate_at(0), Some(true));
            assert_eq!(trip.time_at(1).expect("time").1 - run.first_departure, 1800);
            run.first_departure
        })
        .collect();
    runs.sort_unstable();
    assert_eq!(runs, vec![10 * 3600, 10 * 3600 + 1800]);
    assert!(
        !view
            .trip(
                view.trips_by_gtfs_id("kcm", "PLAIN", date("20260602"))
                    .next()
                    .expect("run")
                    .trip
            )
            .expect("trip")
            .frequency_run
    );
}

#[test]
fn min_transfer_times_set_where_the_core_can_use_them() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,location_type,parent_station\n\
             A,47.0,-122.0,,\nB,47.1,-122.0,,\nC,47.2,-122.0,,\n\
             HUB,47.3,-122.0,1,\nP1,47.3,-122.0,,HUB\nP2,47.3,-122.0,,HUB\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
        )
        .with(
            "transfers.txt",
            "from_stop_id,to_stop_id,transfer_type,min_transfer_time,from_route_id\n\
             A,A,2,300,\n\
             A,A,2,240,\n\
             A,B,2,60,\n\
             B,B,1,,\n\
             C,C,2,120,R\n\
             HUB,HUB,2,90,\n",
        );
    let output = compile_one(source);
    assert_eq!(output.reports[0].transfers_applied, 4);
    assert_eq!(output.reports[0].transfers_ignored, 2);
    // A to B, and each platform of the hub to the other.
    assert_eq!(output.reports[0].transfer_pairs, 3);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let change = |id: &str| {
        let stop = view.stop_by_gtfs_id("kcm", id).expect("stop");
        view.tables.stops[stop.into_inner() as usize]
            .min_change
            .get()
    };
    assert_eq!(change("A"), 240, "same stop, type 2: the smallest row wins");
    assert_eq!(
        change("B"),
        DEFAULT_TRANSFER_SECONDS,
        "a row between stops sets no time"
    );
    assert_eq!(change("C"), DEFAULT_TRANSFER_SECONDS, "route-scoped");
    assert_eq!(
        change("P1"),
        90,
        "a station's minimum reaches its platforms"
    );
    assert_eq!(change("P2"), 90);
    let stop = |id: &str| view.stop_by_gtfs_id("kcm", id).expect("stop");
    let pairs: Vec<_> = valinor_transit::TimetableSource::transfer_pairs(&view)
        .into_iter()
        .map(|pair| (pair.from, pair.to, pair.rule))
        .collect();
    assert_eq!(pairs.len(), 3);
    let floor = |s: u32| PairRule::Floor(valinor_transit::Seconds::try_new(s).expect("in range"));
    assert!(pairs.contains(&(stop("A"), stop("B"), floor(60))));
    assert!(pairs.contains(&(stop("P1"), stop("P2"), floor(90))));
    assert!(pairs.contains(&(stop("P2"), stop("P1"), floor(90))));
    let timetable = view.to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), view.stop_count());
}

#[test]
fn forbidden_transfers_apply_to_stops_and_pairs_and_qualified_rows_are_counted() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,location_type,parent_station\n\
             A,47.0,-122.0,,\nB,47.1,-122.0,,\nC,47.2,-122.0,,\n\
             HUB,47.3,-122.0,1,\nP1,47.3,-122.0,,HUB\nP2,47.3,-122.0,,HUB\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
        )
        .with(
            "transfers.txt",
            "from_stop_id,to_stop_id,transfer_type,min_transfer_time,from_route_id\n\
             A,A,3,,\n\
             A,B,2,90,\n\
             A,B,3,,\n\
             B,C,3,,\n\
             C,B,3,,R\n\
             HUB,HUB,3,,\n",
        );
    let output = compile_one(source);
    let report = &output.reports[0];
    assert_eq!(report.transfers_forbidden_qualified, 1);
    // A, and both platforms of the hub.
    assert_eq!(report.transfers_forbidden_same_stop, 3);
    // A to B, B to C, and the two platform pairs inside the hub.
    assert_eq!(report.transfers_forbidden_pairs, 4);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let stop = |id: &str| view.stop_by_gtfs_id("kcm", id).expect("stop");
    let no_change =
        |id: &str| view.tables.stops[stop(id).into_inner() as usize].flags & STOP_NO_CHANGE != 0;
    assert!(no_change("A"));
    assert!(no_change("P1"));
    assert!(no_change("P2"));
    assert!(!no_change("B"));
    let pairs: Vec<_> = valinor_transit::TimetableSource::transfer_pairs(&view)
        .into_iter()
        .map(|pair| (pair.from, pair.to, pair.rule))
        .collect();
    assert!(pairs.contains(&(stop("A"), stop("B"), PairRule::Forbidden)));
    assert!(pairs.contains(&(stop("B"), stop("C"), PairRule::Forbidden)));
    assert!(pairs.contains(&(stop("P1"), stop("P2"), PairRule::Forbidden)));
    assert!(pairs.contains(&(stop("P2"), stop("P1"), PairRule::Forbidden)));
    // A qualified row forbids nothing, only reports itself.
    assert!(
        !pairs
            .iter()
            .any(|&(from, to, _)| (from, to) == (stop("C"), stop("B")))
    );
    let timetable = view.to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), view.stop_count());
}

#[test]
fn trips_split_by_wheelchair_rule_and_stops_inherit_from_their_station() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,location_type,parent_station,wheelchair_boarding\n\
             HUB,47.0,-122.0,1,,2\n\
             P1,47.0,-122.0,,HUB,\n\
             P2,47.0,-122.0,,HUB,1\n\
             B,47.1,-122.0,,,\n\
             C,47.2,-122.0,,,2\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,wheelchair_accessible\nR,S,OPEN,1\nR,S,UNKNOWN,\nR,S,CLOSED,2\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             OPEN,10:00:00,10:00:00,P1,1\nOPEN,11:00:00,11:00:00,B,2\n\
             UNKNOWN,10:30:00,10:30:00,P1,1\nUNKNOWN,11:30:00,11:30:00,B,2\n\
             CLOSED,10:15:00,10:15:00,P1,1\nCLOSED,11:15:00,11:15:00,B,2\n",
        );
    let output = compile_one(source);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let stop = |id: &str| view.stop_by_gtfs_id("kcm", id).expect("stop");
    let accessible = |id: &str| {
        valinor_transit::TimetableSource::wheelchair_stop_accessible(
            &view,
            stop(id).into_inner() as usize,
        )
    };
    assert!(!accessible("P1"), "an empty child inherits its station's 2");
    assert!(
        accessible("P2"),
        "a child's own value wins over its station's"
    );
    assert!(accessible("B"), "unknown is allowed");
    assert!(!accessible("C"), "an explicit 2 excludes the stop");
    // Trips with and without the rule are in separate patterns; unknown shares the open pattern.
    let wheelchair_patterns: Vec<_> = (0..view.pattern_count())
        .filter(|&p| {
            view.pattern(valinor_transit::RouteId::new(
                u32::try_from(p).expect("few"),
            ))
            .expect("pattern")
            .wheelchair_accessible
        })
        .collect();
    assert_eq!(view.pattern_count(), 2);
    assert_eq!(wheelchair_patterns.len(), 1);
    let trip = |id: &str| {
        view.trip_run_for_start_time(
            "kcm",
            id,
            parse_service_date("20260602").expect("date"),
            None,
        )
        .expect("trip")
        .trip
    };
    let pattern_of = |id: &str| view.trip(trip(id)).expect("trip").pattern;
    assert_eq!(pattern_of("OPEN"), pattern_of("UNKNOWN"));
    assert_ne!(pattern_of("OPEN"), pattern_of("CLOSED"));
    let timetable = view.to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), view.stop_count());
}

/// Two trips of one vehicle: `T1` (route `R1`, A to M) and `T2` (route `R2`, M to B).
fn seated_feed(
    t2_start: &str,
    t2_end: &str,
    t2_first_stop: &str,
    trips: &str,
    transfers: Option<&str>,
    calendar_dates: &str,
) -> InMemorySource {
    let mut source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with(
            "routes.txt",
            "route_id,route_short_name,route_type\nR1,1,3\nR2,2,3\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nM,47.1,-122.0\nN,47.11,-122.0\nB,47.2,-122.0\n",
        )
        .with("calendar_dates.txt", calendar_dates)
        .with("trips.txt", trips)
        .with(
            "stop_times.txt",
            &format!(
                "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
                 T1,08:00:00,08:00:00,A,1\nT1,08:30:00,08:30:00,M,2\n\
                 T2,{t2_start},{t2_start},{t2_first_stop},1\nT2,{t2_end},{t2_end},B,2\n"
            ),
        );
    if let Some(transfers) = transfers {
        source = source.with("transfers.txt", transfers);
    }
    source
}

const BLOCK_TRIPS: &str = "route_id,service_id,trip_id,block_id,trip_headsign\n\
R1,D,T1,X,To M\nR2,D,T2,X,To B\n";
const DAY_ONE: &str = "service_id,date,exception_type\nD,20260602,1\n";

#[test]
fn a_block_merges_consecutive_trips_within_the_wait_limit() {
    let output = compile_one(seated_feed(
        "08:35:00",
        "09:00:00",
        "M",
        BLOCK_TRIPS,
        None,
        DAY_ONE,
    ));
    let report = &output.reports[0];
    assert_eq!(report.seated_trips_merged, 1);
    assert_eq!(report.seated_trips_absorbed, 2);
    assert_eq!(report.seated_trips_partial, 0);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    // One combined trip, keeping every position of both trips: M twice, so RT finds each.
    assert_eq!(view.trip_count(), 1);
    assert_eq!(view.pattern_count(), 1);
    let date = date("20260602");
    let first = view
        .trip_run_for_start_time("kcm", "T1", date, None)
        .expect("T1 resolves");
    let second = view
        .trip_run_for_start_time("kcm", "T2", date, None)
        .expect("T2 resolves");
    assert_eq!(first.trip, second.trip, "one row serves both trips");
    assert_eq!(first.segment, Some((0, 1)));
    assert_eq!(second.segment, Some((2, 3)));
    let info = view.trip(first.trip).expect("trip");
    assert_eq!(info.stop_count(), 4);
    assert_eq!(info.time_at(1), Some((30 * 60 + 28_800, 30 * 60 + 28_800)));
    assert_eq!(second.first_departure, 35 * 60 + 28_800);
    // No boarding at the end of T1, no alighting at the start of T2.
    assert_eq!(info.pickup_type_at(1), 1);
    assert_eq!(info.drop_off_type_at(2), 1);
    let segments = view.trip_segments(first.trip);
    let summary: Vec<_> = segments
        .iter()
        .map(|s| {
            (
                s.gtfs_id,
                view.route(s.route).expect("route").gtfs_id,
                s.headsign,
                s.first_position,
                s.last_position,
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("T1", "R1", Some("To M"), 0, 1),
            ("T2", "R2", Some("To B"), 2, 3)
        ]
    );
    let timetable = view.to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), view.stop_count());
}

#[test]
fn a_block_leaves_trips_apart_past_the_wait_limit_or_stop() {
    // Eleven minutes is past the default ten.
    let output = compile_one(seated_feed(
        "08:41:00",
        "09:00:00",
        "M",
        BLOCK_TRIPS,
        None,
        DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 0);
    assert_eq!(
        CompiledTimetable::parse(&output.bytes)
            .expect("parse")
            .trip_count(),
        2
    );
    // The next trip starts at a different stop.
    let output = compile_one(seated_feed(
        "08:35:00",
        "09:00:00",
        "N",
        BLOCK_TRIPS,
        None,
        DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 0);
}

#[test]
fn a_type_4_row_merges_past_the_wait_limit_and_beats_a_block() {
    // 25 minutes is well past ten, and the block says one thing: the row decides.
    let transfers = "from_stop_id,to_stop_id,from_trip_id,to_trip_id,transfer_type,min_transfer_time\n\
                     M,M,T1,T2,4,0\n";
    let output = compile_one(seated_feed(
        "08:55:00",
        "09:20:00",
        "M",
        BLOCK_TRIPS,
        Some(transfers),
        DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 1);
    assert_eq!(output.reports[0].seated_rows, 1);

    // A type 4 row names trips at different stops, as nigiri allows.
    let output = compile_one(seated_feed(
        "08:40:00",
        "09:20:00",
        "N",
        BLOCK_TRIPS,
        Some(transfers),
        DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let run = view
        .trip_run_for_start_time("kcm", "T2", date("20260602"), None)
        .expect("resolves");
    assert_eq!(run.segment, Some((2, 3)));
}

#[test]
fn a_type_5_row_forbids_a_merge() {
    let transfers = "from_stop_id,to_stop_id,from_trip_id,to_trip_id,transfer_type\n\
                     M,M,T1,T2,5\n";
    let output = compile_one(seated_feed(
        "08:35:00",
        "09:00:00",
        "M",
        BLOCK_TRIPS,
        Some(transfers),
        DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 0);
    assert_eq!(output.reports[0].seated_rows, 1);
}

#[test]
fn a_merge_only_covers_the_days_both_trips_run() {
    // T1 runs two days; T2 only the first.
    let trips = "route_id,service_id,trip_id,block_id\nR1,D,T1,X\nR2,E,T2,X\n";
    let dates = "service_id,date,exception_type\nD,20260602,1\nD,20260603,1\nE,20260602,1\n";
    let output = compile_one(seated_feed("08:35:00", "09:00:00", "M", trips, None, dates));
    let report = &output.reports[0];
    assert_eq!(report.seated_trips_merged, 1);
    assert_eq!(report.seated_trips_absorbed, 1, "T2 only runs merged");
    assert_eq!(report.seated_trips_partial, 1, "T1 also runs alone");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let merged_day = date("20260602");
    let alone_day = date("20260603");
    let merged = view
        .trip_run_for_start_time("kcm", "T1", merged_day, None)
        .expect("merged");
    let alone = view
        .trip_run_for_start_time("kcm", "T1", alone_day, None)
        .expect("alone");
    assert_eq!(merged.segment, Some((0, 1)));
    assert_eq!(alone.segment, None);
    assert_ne!(merged.trip, alone.trip);
    // T2 doesn't exist on the second day.
    assert!(
        view.trip_run_for_start_time("kcm", "T2", alone_day, None)
            .is_none()
    );
}

#[test]
fn long_chains_are_cut_at_the_limit_and_reported() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\nC,47.2,-122.0\nD,47.3,-122.0\n",
        )
        .with("calendar_dates.txt", DAY_ONE)
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,block_id\nR,D,T1,X\nR,D,T2,X\nR,D,T3,X\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             T1,08:00:00,08:00:00,A,1\nT1,08:30:00,08:30:00,B,2\n\
             T2,08:31:00,08:31:00,B,1\nT2,09:00:00,09:00:00,C,2\n\
             T3,09:01:00,09:01:00,C,1\nT3,09:30:00,09:30:00,D,2\n",
        );
    let output = compile_with(
        vec![FeedInput {
            id: FeedId::try_new("kcm".to_owned()).expect("id"),
            source: Box::new(source),
        }],
        CompileOptions {
            max_seated_chain: 2,
            ..CompileOptions::default()
        },
    )
    .expect("compile");
    let report = &output.reports[0];
    assert_eq!(report.seated_chains_cut, 1);
    assert_eq!(report.seated_trips_merged, 1, "T1+T2, with T3 left over");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.trip_count(), 2);
}

#[test]
fn trips_with_different_bike_rules_stay_apart() {
    let trips = "route_id,service_id,trip_id,block_id,bikes_allowed\n\
                 R1,D,T1,X,1\nR2,D,T2,X,2\n";
    let output = compile_one(seated_feed(
        "08:35:00", "09:00:00", "M", trips, None, DAY_ONE,
    ));
    assert_eq!(output.reports[0].seated_trips_merged, 0);
}

#[test]
fn a_block_can_be_turned_off() {
    let output = compile_with(
        vec![FeedInput {
            id: FeedId::try_new("kcm".to_owned()).expect("id"),
            source: Box::new(seated_feed(
                "08:35:00",
                "09:00:00",
                "M",
                BLOCK_TRIPS,
                None,
                DAY_ONE,
            )),
        }],
        CompileOptions {
            merge_seated_trips: false,
            ..CompileOptions::default()
        },
    )
    .expect("compile");
    assert_eq!(output.reports[0].seated_trips_merged, 0);
}

#[test]
fn a_rule_between_stops_keeps_the_first_row() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
        )
        .with(
            "transfers.txt",
            "from_stop_id,to_stop_id,transfer_type,min_transfer_time\n\
             A,B,2,200\n\
             A,B,2,100\n",
        );
    let output = compile_one(source);
    assert_eq!(output.reports[0].transfer_pairs, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let pairs = valinor_transit::TimetableSource::transfer_pairs(&view);
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].rule,
        PairRule::Floor(valinor_transit::Seconds::try_new(200).expect("in range"))
    );
}

#[test]
fn stop_time_zones_are_available_without_a_database() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,stop_timezone\nA,47.0,-122.0,America/Los_Angeles\nB,45.5,-73.5,America/Toronto\n",
        )
        .with("calendar_dates.txt", "service_id,date,exception_type\nS,20260602,1\n")
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,11:00:00,11:00:00,B,2\n",
        );
    let output = compile_one(source);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let id = view.stop_by_gtfs_id("kcm", "B").expect("stop");
    let stop = view.stop(id).expect("stop");
    assert_eq!(stop.time_zone, "America/Toronto");
    // A noon in early June, when Toronto is on UTC-4.
    let noon_unix = view
        .service_day_start_unix(date("20260602"))
        .expect("covered")
        + 12 * 3600;
    assert_eq!(view.stop_utc_offset_at(id, noon_unix), Some(-4 * 3600));
    let la = view.stop_by_gtfs_id("kcm", "A").expect("stop");
    assert_eq!(view.stop_utc_offset_at(la, noon_unix), Some(-7 * 3600));
}

#[test]
fn feeds_are_namespaced() {
    let second = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type,route_short_name\nR1,2,Other Line\n")
        .with("stops.txt", "stop_id,stop_lat,stop_lon,stop_name\nS1,48.0,-121.0,Elsewhere\nS9,48.1,-121.0,Beyond\n")
        .with("calendar_dates.txt", "service_id,date,exception_type\nWK,20260610,1\n")
        .with("trips.txt", "route_id,service_id,trip_id,trip_headsign\nR1,WK,T1,Other\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             T1,06:00:00,06:00:00,S1,1\nT1,06:30:00,06:30:00,S9,2\n",
        );
    let output = compile(vec![
        FeedInput {
            id: FeedId::try_new("kcm".to_owned()).expect("id"),
            source: Box::new(base_feed()),
        },
        FeedInput {
            id: FeedId::try_new("st".to_owned()).expect("id"),
            source: Box::new(second),
        },
    ])
    .expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");

    let feeds: Vec<_> = view.feeds().map(|feed| feed.id).collect();
    assert_eq!(feeds, vec!["kcm", "st"]);
    assert_eq!(view.gtfs_route_count(), 3, "each feed keeps its own R1");

    let kcm_stop = view.stop_by_gtfs_id("kcm", "S1").expect("stop");
    let st_stop = view.stop_by_gtfs_id("st", "S1").expect("stop");
    assert_ne!(kcm_stop, st_stop);
    assert_eq!(view.stop(st_stop).expect("stop").name, Some("Elsewhere"));
    assert_eq!(view.stop(st_stop).expect("stop").feed, "st");

    let st_route = view.route_by_gtfs_id("st", "R1").expect("route");
    assert_eq!(
        view.route(st_route).expect("info").short_name,
        Some("Other Line")
    );
    let st_run = view
        .trips_by_gtfs_id("st", "T1", date("20260610"))
        .next()
        .expect("run");
    let trip = view.trip(st_run.trip).expect("trip");
    assert_eq!(trip.headsign, Some("Other"));
    assert_eq!(
        view.trips_by_gtfs_id("kcm", "T1", date("20260610")).count(),
        1,
        "the other feed's T1"
    );
    let kcm_run = view
        .trips_by_gtfs_id("kcm", "T1", date("20260602"))
        .next()
        .expect("run");
    assert_ne!(kcm_run.trip, st_run.trip);
}

#[test]
fn compiling_feeds_with_duplicate_ids_or_zones_fails() {
    let result = compile(vec![
        FeedInput {
            id: FeedId::try_new("a".to_owned()).expect("id"),
            source: Box::new(base_feed()),
        },
        FeedInput {
            id: FeedId::try_new("a".to_owned()).expect("id"),
            source: Box::new(base_feed()),
        },
    ]);
    assert!(matches!(result, Err(CompileError::DuplicateFeedId(_))));

    let other_zone = base_feed().with(
        "agency.txt",
        "agency_name,agency_timezone\nM,America/New_York\n",
    );
    let result = compile(vec![
        FeedInput {
            id: FeedId::try_new("a".to_owned()).expect("id"),
            source: Box::new(base_feed()),
        },
        FeedInput {
            id: FeedId::try_new("b".to_owned()).expect("id"),
            source: Box::new(other_zone),
        },
    ]);
    assert!(matches!(
        result,
        Err(CompileError::MixedFeedTimeZones { .. })
    ));

    let mixed = base_feed().with(
        "agency.txt",
        "agency_id,agency_name,agency_timezone\n1,M,America/Los_Angeles\n2,N,America/New_York\n",
    );
    let result = compile(vec![FeedInput {
        id: FeedId::try_new("a".to_owned()).expect("id"),
        source: Box::new(mixed),
    }]);
    assert!(matches!(
        result,
        Err(CompileError::MixedAgencyTimeZones { .. })
    ));
}

#[test]
fn an_unknown_time_zone_is_fatal_for_the_feed_zone_and_tolerated_for_a_stop() {
    let bad_agency = base_feed().with(
        "agency.txt",
        "agency_name,agency_timezone\nM,Mars/Olympus\n",
    );
    let result = compile(vec![FeedInput {
        id: FeedId::try_new("a".to_owned()).expect("id"),
        source: Box::new(bad_agency),
    }]);
    assert!(matches!(result, Err(CompileError::UnknownTimeZone(_))));

    let bad_stop = base_feed().with(
        "stops.txt",
        "stop_id,stop_lat,stop_lon,stop_timezone\nS1,47.0,-122.0,Mars/Olympus\nS2,47.1,-122.0,\nS3,47.2,-122.0,\nS4,47.3,-122.0,\n",
    );
    let output = compile_one(bad_stop);
    assert_eq!(output.reports[0].warning_count, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let stop = view.stop_by_gtfs_id("kcm", "S1").expect("stop");
    assert_eq!(
        view.stop(stop).expect("info").time_zone,
        "America/Los_Angeles"
    );
}

#[test]
fn compile_rejects_a_feed_with_no_required_files() {
    let result = compile(vec![FeedInput {
        id: FeedId::try_new("a".to_owned()).expect("id"),
        source: Box::new(InMemorySource::new()),
    }]);
    assert!(matches!(
        result,
        Err(CompileError::MissingFile("agency.txt"))
    ));

    let result = compile(vec![FeedInput {
        id: FeedId::try_new("a".to_owned()).expect("id"),
        source: Box::new(base_feed().with("agency.txt", "agency_name\nX\n")),
    }]);
    assert!(matches!(result, Err(CompileError::MissingColumn { .. })));
}

#[test]
fn a_utf8_bom_is_tolerated() {
    let source = base_feed().with("routes.txt", "\u{feff}route_id,route_type\nR1,3\nR2,3\n");
    let output = compile_one(source);
    assert_eq!(output.reports[0].routes, 2);
}

#[test]
fn loop_patterns_keep_each_visit() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with("stops.txt", "stop_id,stop_lat,stop_lon\nA,47.0,-122.0\nB,47.1,-122.0\n")
        .with("calendar_dates.txt", "service_id,date,exception_type\nS,20260602,1\n")
        .with("trips.txt", "route_id,service_id,trip_id\nR,S,X\n")
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,10:10:00,10:10:00,B,2\nX,10:20:00,10:20:00,A,3\nX,10:30:00,10:30:00,B,4\n",
        );
    let output = compile_one(source);
    assert_eq!(output.reports[0].loop_patterns, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let timetable = view.to_timetable().expect("timetable");
    let stops = timetable
        .route_stops(valinor_transit::RouteId::new(0))
        .expect("stops")
        .to_vec();
    assert_eq!(stops.len(), 4);
    assert_eq!(stops[0], stops[2]);
    assert_eq!(stops[1], stops[3]);
    let window = view.service_window(&[date("20260602")]).expect("window");
    let trips = timetable
        .resolved_trips(
            valinor_transit::RouteId::new(0),
            date("20260602"),
            window.days[0].offset,
        )
        .expect("trips");
    assert_eq!(trips.len(), 1);
}

#[test]
fn shapes_slice_between_stops_with_feed_distances() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.000,-122.000\nB,47.000,-121.995\nC,47.000,-121.990\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,shape_id\nR,S,X,SH\nR,S,NOSHAPE,\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence,shape_dist_traveled\n\
             X,10:00:00,10:00:00,A,1,0\nX,10:10:00,10:10:00,B,2,500\nX,10:20:00,10:20:00,C,3,1000\n\
             NOSHAPE,10:00:00,10:00:00,A,1,\nNOSHAPE,10:10:00,10:10:00,C,2,\n",
        )
        .with(
            "shapes.txt",
            "shape_id,shape_pt_sequence,shape_pt_lat,shape_pt_lon,shape_dist_traveled\n\
             SH,1,47.000,-122.000,0\nSH,2,47.000,-121.9975,250\nSH,3,47.000,-121.995,500\n\
             SH,4,47.001,-121.9925,750\nSH,5,47.000,-121.990,1000\n",
        );
    let output = compile_one(source);
    assert_eq!(output.reports[0].shape_points, 5);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let run = view
        .trips_by_gtfs_id("kcm", "X", date("20260602"))
        .next()
        .expect("run");
    let slice = view.shape_between(run.trip, 0, 1).expect("slice");
    let points = slice.points();
    assert!(slice.from_dist.abs() < 1e-6);
    assert!((slice.to_dist - 500.0).abs() < 1e-3);
    assert_eq!(
        points.first(),
        Some(&LatLon {
            lat: 47.0,
            lon: -122.0
        })
    );
    assert_eq!(
        points.last(),
        Some(&LatLon {
            lat: 47.0,
            lon: -121.995
        })
    );
    // The middle point along the slice, and only that point.
    assert_eq!(points.len(), 3);
    assert_eq!(
        points[1],
        LatLon {
            lat: 47.0,
            lon: -121.9975
        }
    );

    // A slice can end between shape points; the end is interpolated.
    let tail = view.shape_between(run.trip, 1, 2).expect("slice").points();
    assert!((tail.last().expect("end").lon + 121.99).abs() < 1e-9);

    assert!(view.shape_between(run.trip, 1, 1).is_none());
    assert!(view.shape_between(run.trip, 0, 9).is_none());
    let bare = view
        .trips_by_gtfs_id("kcm", "NOSHAPE", date("20260602"))
        .next()
        .expect("run");
    assert!(view.shape_between(bare.trip, 0, 1).is_none());
    assert!(!view.trip(bare.trip).expect("trip").has_shape);
}

#[test]
fn shapes_project_stops_when_the_feed_omits_distances() {
    let source = InMemorySource::new()
        .with("agency.txt", AGENCY)
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon\nA,47.000,-122.000\nB,47.000,-121.995\nC,47.000,-121.990\n",
        )
        .with(
            "calendar_dates.txt",
            "service_id,date,exception_type\nS,20260602,1\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,shape_id\nR,S,X,SH\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             X,10:00:00,10:00:00,A,1\nX,10:10:00,10:10:00,B,2\nX,10:20:00,10:20:00,C,3\n",
        )
        .with(
            "shapes.txt",
            "shape_id,shape_pt_sequence,shape_pt_lat,shape_pt_lon\n\
             SH,1,47.000,-122.000\nSH,2,47.000,-121.995\nSH,3,47.000,-121.990\n",
        );
    let output = compile_one(source);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let run = view
        .trips_by_gtfs_id("kcm", "X", date("20260602"))
        .next()
        .expect("run");
    let slice = view.shape_between(run.trip, 0, 1).expect("slice");
    assert!(slice.from_dist.abs() < 1.0, "{slice:?}");
    assert!(
        (slice.to_dist - slice_mid_meters()).abs() < 5.0,
        "{slice:?}"
    );
    assert_eq!(slice.points().len(), 2);
}

/// Roughly how far 0.005 degrees of longitude is at 47N.
fn slice_mid_meters() -> f32 {
    379.0
}

fn compile_bytes() -> Vec<u8> {
    compile_one(base_feed()).bytes
}

#[test]
fn compiling_is_deterministic() {
    assert_eq!(compile_bytes(), compile_bytes());
}

#[test]
fn every_truncation_is_an_error() {
    let bytes = compile_bytes();
    for length in 0..bytes.len() {
        let result = CompiledTimetable::parse(&bytes[..length]);
        assert!(result.is_err(), "truncated to {length} bytes");
    }
    // Trailing garbage is not accepted either.
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(CompiledTimetable::parse(&longer).is_err());
    assert!(CompiledTimetable::parse(&bytes).is_ok());
}

#[test]
fn a_wrong_version_is_named_and_a_foreign_file_rejected() {
    let mut bytes = compile_bytes();
    bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(matches!(
        CompiledTimetable::parse(&bytes),
        Err(DecodeError::UnsupportedVersion { found, supported })
            if found == FORMAT_VERSION + 1 && supported == FORMAT_VERSION
    ));
    assert_eq!(
        CompiledTimetable::parse(b"this is a text file with some words in it!").err(),
        Some(DecodeError::BadMagic)
    );
}

/// Exercises every accessor, to prove none panics on a file that made it past validation.
fn read_everything(bytes: &[u8]) {
    let Ok(view) = CompiledTimetable::parse(bytes) else {
        return;
    };
    for feed in view.feeds() {
        let _ = (feed.id, feed.time_zone);
    }
    let dates: Vec<ServiceDate> = (0..20).map(|day| ServiceDate::new(20_400 + day)).collect();
    for date in &dates {
        let _ = view.active_trip_count(*date);
        let _ = view.active_stop_event_count(*date);
        let _ = view.service_day_start_unix(*date);
    }
    let _ = view.service_window(&dates);
    for index in 0..view.stop_count() + 3 {
        let id = StopId::new(u32::try_from(index).unwrap_or(0));
        let _ = view.stop(id);
        let _ = view.stop_utc_offset_at(id, 0);
    }
    for index in 0..view.location_count() + 3 {
        let location = crate::LocationId(u32::try_from(index).unwrap_or(0));
        let _ = view.location(location);
        let _ = view.stops_at(location);
    }
    for index in 0..view.gtfs_route_count() + 3 {
        let route = crate::GtfsRoute(u32::try_from(index).unwrap_or(0));
        let _ = view.route(route);
        let _ = view.route_patterns(route);
    }
    for index in 0..view.agency_count() + 3 {
        let _ = view.agency(crate::AgencyIndex(u32::try_from(index).unwrap_or(0)));
    }
    for index in 0..view.pattern_count() + 3 {
        let _ = view.pattern(valinor_transit::RouteId::new(
            u32::try_from(index).unwrap_or(0),
        ));
    }
    for index in 0..view.trip_count() + 3 {
        let trip = TripId::new(u32::try_from(index).unwrap_or(0));
        if let Some(info) = view.trip(trip) {
            let _ = view.stop_headsign(&info, 0);
            let _ = info.time_at(0);
            let _ = info.stop_sequence_at(0);
            let _ = info.position_of_sequence(1);
            let _ = info.pickup_type_at(0);
            let _ = info.drop_off_type_at(0);
            if let Some(slice) = view.shape_between(trip, 0, 1) {
                let _ = slice.points();
            }
        }
    }
    let _ = view.stop_by_gtfs_id("kcm", "S1");
    let _ = view.location_by_gtfs_id("kcm", "S1");
    let _ = view.route_by_gtfs_id("kcm", "R1");
    let _ = view.trips_by_gtfs_id("kcm", "T1", dates[0]).count();
    if let Ok(timetable) = view.to_timetable()
        && let Ok(window) = view.service_window(&dates[..3])
    {
        drop(timetable.materialize(&window.days));
    }
}

fn fix_checksum(bytes: &mut [u8]) {
    let checksum = checksum_of(bytes);
    bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
}

/// Without the checksum fixed up, a flip of any byte is caught.
#[test]
fn every_flipped_byte_is_rejected() {
    let original = compile_bytes();
    for position in 16..original.len() {
        let mut bytes = original.clone();
        bytes[position] ^= 0xff;
        let result = CompiledTimetable::parse(&bytes);
        // The header's length field, checked first, reports a truncation instead.
        if position >= 24 {
            assert!(
                matches!(result, Err(DecodeError::ChecksumMismatch { .. })),
                "byte {position}",
            );
        } else {
            assert!(result.is_err(), "byte {position}");
        }
    }
}

/// With the checksum recomputed, a file with any byte changed still can't panic a reader.
#[test]
fn corrupted_files_with_valid_checksums_never_panic() {
    let original = compile_bytes();
    let masks = [0x01_u8, 0x40, 0xff];
    let mut buffer = original.clone();
    for position in 16..original.len() {
        for mask in masks {
            buffer[position] = original[position] ^ mask;
            fix_checksum(&mut buffer);
            read_everything(&buffer);
        }
        buffer[position] = original[position];
    }
    assert!(CompiledTimetable::parse(&original).is_ok());
}

proptest! {
    /// Random byte changes, with the checksum recomputed, never panic a reader.
    #[test]
    fn mutated_files_never_panic(changes in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..16)) {
        let mut bytes = compile_bytes();
        for (position, value) in changes {
            let index = 16 + position % (bytes.len() - 16);
            bytes[index] = value;
        }
        fix_checksum(&mut bytes);
        read_everything(&bytes);
    }

    /// Reading a window always gives the trips of that day, with their exact times.
    #[test]
    fn round_trip_of_generated_trips(
        trips in proptest::collection::vec(
            (0_i32..100_000, proptest::collection::vec(0_i32..3_000, 1..6), 0_u8..3),
            1..12,
        ),
    ) {
        let mut stop_rows = String::from("stop_id,stop_lat,stop_lon\n");
        for s in 0..8 {
            writeln!(stop_rows, "S{s},{}.5,{}.5", 40 + s, -120 + s).expect("write");
        }
        let mut trips_txt = String::from("route_id,service_id,trip_id\n");
        let mut stop_times = String::from("trip_id,arrival_time,departure_time,stop_id,stop_sequence\n");
        let mut expected: Vec<(String, Vec<i32>)> = Vec::new();
        for (index, (start, gaps, stops)) in trips.iter().enumerate() {
            let id = format!("T{index}");
            writeln!(trips_txt, "R,S,{id}").expect("write");
            let mut clock = *start;
            let mut times = vec![clock];
            for gap in gaps {
                clock += gap;
                times.push(clock);
            }
            let first = usize::from(*stops);
            for (sequence, time) in times.iter().enumerate() {
                let stop = (first + sequence) % 8;
                let text = gtfs_time(*time);
                writeln!(stop_times, "{id},{text},{text},S{stop},{sequence}").expect("write");
            }
            expected.push((id, times));
        }
        let source = InMemorySource::new()
            .with("agency.txt", AGENCY)
            .with("routes.txt", "route_id,route_type\nR,3\n")
            .with("stops.txt", &stop_rows)
            .with("calendar_dates.txt", "service_id,date,exception_type\nS,20260602,1\n")
            .with("trips.txt", &trips_txt)
            .with("stop_times.txt", &stop_times);
        let output = compile_one(source);
        let view = CompiledTimetable::parse(&output.bytes).expect("parse");
        prop_assert_eq!(view.trip_count(), expected.len());
        let timetable = view.to_timetable().expect("timetable");
        for (id, times) in &expected {
            let run = view.trips_by_gtfs_id("kcm", id, date("20260602")).next().expect("run");
            let info = view.trip(run.trip).expect("trip");
            prop_assert_eq!(info.stop_count(), times.len());
            for (position, time) in times.iter().enumerate() {
                prop_assert_eq!(info.time_at(position), Some((*time, *time)));
            }
        }
        let window = view.service_window(&[date("20260602")]).expect("window");
        let materialized = timetable.materialize(&window.days).expect("materialize");
        prop_assert_eq!(materialized.days(), window.days.as_slice());
    }
}

fn gtfs_time(seconds: i32) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[test]
fn compiling_from_a_directory_source_works() {
    let source = DirectorySource::new("fixtures/minimal");
    let output = compile(vec![FeedInput {
        id: FeedId::try_new("x".to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compile");
    assert_eq!(output.reports[0].trips_kept, 1);
    assert!(CompiledTimetable::parse(&output.bytes).is_ok());
}

#[test]
fn compiling_from_a_zip_source_works_even_when_files_are_nested() {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default();
        for name in [
            "agency.txt",
            "routes.txt",
            "stops.txt",
            "calendar.txt",
            "trips.txt",
            "stop_times.txt",
        ] {
            writer
                .start_file(format!("feed/{name}"), options)
                .expect("start");
            let bytes = std::fs::read(format!("fixtures/minimal/{name}")).expect("read");
            writer.write_all(&bytes).expect("write");
        }
        writer.finish().expect("finish");
    }
    let output = compile(vec![FeedInput {
        id: FeedId::try_new("x".to_owned()).expect("id"),
        source: Box::new(ZipSource::new(Cursor::new(buffer.into_inner())).expect("zip")),
    }])
    .expect("compile");
    assert_eq!(output.reports[0].trips_kept, 1);
}

#[test]
fn loaded_timetables_own_their_bytes() {
    let bytes = compile_bytes();
    let loaded = LoadedTimetable::from_bytes(bytes.clone()).expect("load");
    assert_eq!(loaded.timetable().trip_count(), 3);
    assert!(LoadedTimetable::from_bytes(vec![1, 2, 3]).is_err());
}

#[test]
fn loaded_timetables_can_be_mapped_from_disk() {
    let directory = std::env::temp_dir().join(format!("vgtfs-map-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("dir");
    let path = directory.join("t.vgtfs");
    std::fs::write(&path, compile_bytes()).expect("write");
    // SAFETY: the test owns this file and doesn't touch it while mapped.
    let loaded = unsafe { LoadedTimetable::map(&path) }.expect("map");
    assert_eq!(loaded.timetable().trip_count(), 3);
    let timetable: Timetable = loaded.timetable().to_timetable().expect("timetable");
    assert_eq!(timetable.stop_count(), 4);
    std::fs::remove_file(&path).expect("remove");
}

/// Compiles the real feed, when the machine has it, and pins its counts.
#[test]
#[ignore = "needs the King County feed at KCM_GTFS_ZIP"]
fn the_king_county_feed_reconciles() {
    let Ok(path) = std::env::var("KCM_GTFS_ZIP") else {
        panic!("set KCM_GTFS_ZIP to the path of the King County feed archive");
    };
    let output = compile(vec![FeedInput {
        id: FeedId::try_new("kcm".to_owned()).expect("id"),
        source: Box::new(ZipSource::from_path(std::path::Path::new(&path)).expect("zip")),
    }])
    .expect("compile");
    let report = &output.reports[0];
    assert_eq!(report.trips_listed, 32_635);
    assert_eq!(report.trips_kept, 32_635);
    assert_eq!(report.stop_times_listed, 1_126_734);
    assert_eq!(report.stop_events, 1_126_734);
    assert_eq!(report.routes, 142);
    assert_eq!(report.stops_listed, 6_265);
    assert_eq!(report.services_used, 15);
    assert_eq!(report.loop_patterns, 2);
    assert_eq!(report.trips_past_midnight, 1_433);
    assert_eq!(report.stop_headsigns, 266_075);
    assert_eq!(report.approximate_stop_times, 912_227);
    assert!(
        report.dropped_trips.is_empty(),
        "{:?}",
        report.dropped_trips
    );
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let tuesday = date("20261110");
    assert_eq!(view.active_trip_count(tuesday), 12_938);
    assert_eq!(view.active_stop_event_count(tuesday), 449_039);

    // The timetable sees the same day as the compiled file does.
    let timetable = view.to_timetable().expect("timetable");
    let window = view.service_window(&[tuesday]).expect("window");
    let mut trips = 0_usize;
    let mut events = 0_usize;
    for route in 0..u32::try_from(timetable.route_count()).expect("routes") {
        for trip in timetable
            .resolved_trips(
                valinor_transit::RouteId::new(route),
                tuesday,
                window.days[0].offset,
            )
            .expect("trips")
        {
            trips += 1;
            events += trip.events.len();
        }
    }
    assert_eq!((trips, events), (12_938, 449_039));
}
