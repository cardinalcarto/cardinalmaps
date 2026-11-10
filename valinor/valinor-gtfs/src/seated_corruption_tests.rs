//! Corrupted compiled files with combined trips must never panic a reader.

#![allow(
    clippy::cast_possible_truncation,
    reason = "a pseudo-random fuzzer truncates its words on purpose"
)]

use valinor_transit::{ServiceDate, StopId, TimetableSource, TripId};

use crate::format::layout::checksum_of;
use crate::{CompiledTimetable, FeedId, FeedInput, InMemorySource, compile};

fn seated_bytes() -> Vec<u8> {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_name,agency_timezone,agency_url\nM,America/Los_Angeles,https://m.test\n",
        )
        .with("routes.txt", "route_id,route_short_name,route_type\nR1,1,3\nR2,2,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_lat,stop_lon,wheelchair_boarding\nA,47.0,-122.0,1\nM,47.1,-122.0,2\nB,47.2,-122.0,\nC,47.3,-122.0,1\n",
        )
        .with("transfers.txt", "from_stop_id,to_stop_id,transfer_type,min_transfer_time\nA,B,2,300\nC,B,3,\nM,M,3,\n")
        .with("calendar_dates.txt", "service_id,date,exception_type\nD,20260602,1\nD,20260603,1\n")
        .with(
            "shapes.txt",
            "shape_id,shape_pt_lat,shape_pt_lon,shape_pt_sequence,shape_dist_traveled\n\
             S1,47.0,-122.0,1,0\nS1,47.1,-122.0,2,1000\nS2,47.1,-122.0,1,0\nS2,47.2,-122.0,2,1000\nS2,47.3,-122.0,3,2000\n",
        )
        .with(
            "trips.txt",
            "route_id,service_id,trip_id,block_id,trip_headsign,shape_id\n\
             R1,D,T1,X,To M,S1\nR2,D,T2,X,To C,S2\nR1,D,T3,X,Back,S1\n",
        )
        .with(
            "stop_times.txt",
            "trip_id,arrival_time,departure_time,stop_id,stop_sequence,shape_dist_traveled\n\
             T1,08:00:00,08:00:00,A,1,0\nT1,08:30:00,08:30:00,M,2,1000\n\
             T2,08:35:00,08:35:00,M,1,0\nT2,09:00:00,09:00:00,B,2,1000\nT2,09:10:00,09:10:00,C,3,2000\n\
             T3,09:12:00,09:12:00,C,1,0\nT3,09:40:00,09:40:00,A,2,1000\n",
        );
    compile(vec![FeedInput {
        id: FeedId::try_new("kcm".to_owned()).expect("id"),
        source: Box::new(source),
    }])
    .expect("compiles")
    .bytes
}

fn read_seated(bytes: &[u8]) {
    let Ok(view) = CompiledTimetable::parse(bytes) else {
        return;
    };
    let dates: Vec<ServiceDate> = (0..3).map(|day| ServiceDate::new(20_400 + day)).collect();
    for index in 0..view.trip_count() + 3 {
        let trip = TripId::new(u32::try_from(index).unwrap_or(0));
        let _ = view.trip_segments(trip);
        for from in 0..8 {
            for to in 0..8 {
                let _ = view.shape_points_between(trip, from, to);
            }
        }
    }
    for id in ["T1", "T2", "T3", "X", ""] {
        let _ = view.trips_by_gtfs_id("kcm", id, dates[0]).count();
        let _ = view.trip_run_for_start_time("kcm", id, dates[0], Some(30_000));
    }
    let _ = TimetableSource::transfer_pairs(&view);
    for stop in 0..view.stop_count() + 2 {
        let id = u32::try_from(stop).unwrap_or(0);
        let _ = (
            TimetableSource::no_change_at(&view, stop),
            view.wheelchair_stop_accessible(stop),
        );
        let _ = view.stop(StopId::new(id));
    }
    if let Ok(window) = view.service_window(&dates[..3])
        && let Ok(table) = view.to_timetable()
    {
        drop(table.materialize(&window.days));
    }
}

/// Exercises every new table of a file with merged trips against valid-checksum corruption.
#[test]
fn corrupted_seated_files_never_panic() {
    let original = seated_bytes();
    let view = CompiledTimetable::parse(&original).expect("parses");
    let combined = (0..view.trip_count())
        .filter(|&t| !view.trip_segments(TripId::new(t as u32)).is_empty())
        .count();
    assert!(combined > 0, "no combined trip in the fixture");
    read_seated(&original);
    let mut parsed = 0_u64;
    let mut state = 0x1234_5678_9ABC_DEF0_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..400_000_u32 {
        let mut bytes = original.clone();
        let count = 1 + (next() % 4) as usize;
        for _ in 0..count {
            let at = 16 + (next() as usize) % (bytes.len() - 16);
            bytes[at] = next() as u8;
        }
        let checksum = checksum_of(&bytes);
        bytes[12..16].copy_from_slice(&checksum.to_le_bytes());
        parsed += u64::from(CompiledTimetable::parse(&bytes).is_ok());
        read_seated(&bytes);
    }
    eprintln!(
        "combined trips {combined}, file {} bytes, parsed ok after corruption {parsed}",
        original.len()
    );
    assert!(parsed > 0);
}
