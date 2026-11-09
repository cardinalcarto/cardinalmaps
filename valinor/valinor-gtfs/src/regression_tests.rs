//! Regression tests for problems an independent review found.

use core::fmt::Write as _;
use std::collections::HashMap;
use std::io::{Cursor, Read};

use valinor_transit::ServiceDate;
use zerocopy::little_endian::U32;
use zerocopy::{FromBytes, IntoBytes};

use crate::format::layout::{FileHeader, PatternRow, RouteRow, SectionId, ServiceRow, StopRow};
use crate::format::writer::FileWriter;
use crate::{
    CompileError, CompiledOutput, CompiledTimetable, DecodeError, DropReason, FeedId, FeedInput,
    GtfsSource, InMemorySource, compile, parse_service_date,
};

fn compile_one(source: impl GtfsSource + 'static) -> Result<CompiledOutput, CompileError> {
    compile(vec![FeedInput {
        id: FeedId::try_new("f".to_owned()).expect("id"),
        source: Box::new(source),
    }])
}

fn d(text: &str) -> ServiceDate {
    parse_service_date(text).expect("date")
}

const CAL_HEADER: &str =
    "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n";
const NO_DATES: &str = "service_id,date,exception_type\n";
const ST: &str = "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
    T,08:00:00,08:00:00,S1,1\nT,08:10:00,08:10:00,S2,2\n";

fn daily() -> String {
    format!("{CAL_HEADER}W,1,1,1,1,1,1,1,20260101,20260201\n")
}

fn minimal(zone: &str, calendar: &str, cal_dates: &str, stop_times: &str) -> InMemorySource {
    InMemorySource::new()
        .with(
            "agency.txt",
            &format!("agency_name,agency_timezone\nA,{zone}\n"),
        )
        .with("routes.txt", "route_id,route_type\nR,3\n")
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             S1,a,47.60,-122.33\nS2,b,47.61,-122.33\nS3,c,47.62,-122.33\n",
        )
        .with("calendar.txt", calendar)
        .with("calendar_dates.txt", cal_dates)
        .with("trips.txt", "route_id,service_id,trip_id\nR,W,T\n")
        .with("stop_times.txt", stop_times)
}

/// A compiled file, split into sections so a test can rewrite them and fix the checksum.
struct HostileFile {
    sections: Vec<Vec<u8>>,
    boardable: u32,
}

impl HostileFile {
    fn from_source(source: impl GtfsSource + 'static) -> Self {
        let bytes = compile_one(source).expect("compile").bytes;
        let (header, _) = FileHeader::ref_from_prefix(&bytes).expect("header");
        let boardable = header.boardable_stops.get();
        let start = core::mem::size_of::<FileHeader>();
        let mut sections = Vec::new();
        for index in 0..SectionId::ALL.len() {
            let entry = start + index * 24;
            let word = |at: usize| {
                let raw: [u8; 8] = bytes[entry + at..entry + at + 8]
                    .try_into()
                    .expect("eight bytes");
                usize::try_from(u64::from_le_bytes(raw)).expect("fits")
            };
            let (offset, length) = (word(8), word(16));
            sections.push(bytes[offset..offset + length].to_vec());
        }
        Self {
            sections,
            boardable,
        }
    }

    fn section(&mut self, id: SectionId) -> &mut Vec<u8> {
        let index = SectionId::ALL
            .iter()
            .position(|section| *section == id)
            .expect("section");
        &mut self.sections[index]
    }

    fn rows<T: FromBytes + IntoBytes + zerocopy::KnownLayout>(
        &mut self,
        id: SectionId,
    ) -> &mut [T] {
        <[T]>::mut_from_bytes(self.section(id).as_mut_slice()).expect("rows")
    }

    fn finish(self) -> Vec<u8> {
        let mut writer = FileWriter::new();
        for (id, bytes) in SectionId::ALL.into_iter().zip(self.sections) {
            writer.bytes(id, bytes);
        }
        writer.finish(self.boardable)
    }
}

fn basic_feed() -> HostileFile {
    HostileFile::from_source(minimal("UTC", &daily(), NO_DATES, ST))
}

fn assert_rejected(file: &[u8]) {
    let result = CompiledTimetable::parse(file).map(|_| ());
    assert!(
        matches!(
            result,
            Err(DecodeError::InvalidReference { .. } | DecodeError::InvalidValue { .. })
        ),
        "{result:?}",
    );
}

fn len_u32(len: usize) -> u32 {
    u32::try_from(len).expect("fits")
}

/// Overlapping, tripless patterns would make `to_timetable` allocate quadratically in the file.
#[test]
fn overlapping_patterns_are_rejected() {
    const COUNT: usize = 2000;
    const STOPS: usize = 2000;
    let mut file = basic_feed();
    let trips = file.section(SectionId::Trips).len() / 16;
    let base_stops = file.section(SectionId::PatternStops).len() / 4;
    file.section(SectionId::PatternStops)
        .extend(vec![0_u8; STOPS * 4]);
    let row = PatternRow {
        route: U32::new(0),
        stops_start: U32::new(len_u32(base_stops)),
        stop_count: U32::new(len_u32(STOPS)),
        trips_start: U32::new(len_u32(trips)),
        trip_count: U32::new(0),
        attributes: 0,
        _pad: [0; 3],
    };
    for _ in 0..COUNT {
        file.section(SectionId::Patterns)
            .extend_from_slice(row.as_bytes());
    }
    assert_rejected(&file.finish());
}

#[test]
fn a_pattern_without_trips_is_rejected() {
    let mut file = basic_feed();
    let trips = file.section(SectionId::Trips).len() / 16;
    let base_stops = file.section(SectionId::PatternStops).len() / 4;
    file.section(SectionId::PatternStops)
        .extend(vec![0_u8; 2 * 4]);
    let row = PatternRow {
        route: U32::new(0),
        stops_start: U32::new(len_u32(base_stops)),
        stop_count: U32::new(2),
        trips_start: U32::new(len_u32(trips)),
        trip_count: U32::new(0),
        attributes: 0,
        _pad: [0; 3],
    };
    file.section(SectionId::Patterns)
        .extend_from_slice(row.as_bytes());
    let bytes = file.finish();
    assert_eq!(
        CompiledTimetable::parse(&bytes).map(|_| ()),
        Err(DecodeError::InvalidValue {
            table: "patterns",
            reason: "a pattern has no trips",
        }),
    );
}

#[test]
fn pattern_stops_must_be_contiguous_and_complete() {
    let mut file = basic_feed();
    file.rows::<PatternRow>(SectionId::Patterns)[0].stops_start = U32::new(1);
    file.section(SectionId::PatternStops).extend(vec![0_u8; 4]);
    assert_rejected(&file.finish());

    let mut file = basic_feed();
    file.section(SectionId::PatternStops).extend(vec![0_u8; 4]);
    assert_rejected(&file.finish());
}

/// Many calendars sharing one bit array would each collect up to 40,000 dates.
#[test]
fn overlapping_calendars_are_rejected() {
    let mut file = basic_feed();
    let start = file.section(SectionId::ServiceBits).len();
    file.section(SectionId::ServiceBits)
        .extend(vec![0xff_u8; 5000]);
    let row = ServiceRow {
        feed: U32::new(0),
        gtfs_id: U32::new(0),
        first_date: U32::new(20_000),
        day_count: U32::new(40_000),
        bits_start: U32::new(len_u32(start)),
    };
    for _ in 0..500 {
        file.section(SectionId::Services)
            .extend_from_slice(row.as_bytes());
    }
    assert_rejected(&file.finish());
}

#[test]
fn calendar_bits_must_be_contiguous_and_complete() {
    let mut file = basic_feed();
    file.rows::<ServiceRow>(SectionId::Services)[0].bits_start = U32::new(1);
    file.section(SectionId::ServiceBits).push(0);
    assert_rejected(&file.finish());

    let mut file = basic_feed();
    file.section(SectionId::ServiceBits).push(0);
    assert_rejected(&file.finish());
}

#[test]
fn route_patterns_must_be_contiguous_and_complete() {
    let mut file = basic_feed();
    file.section(SectionId::RoutePatterns)
        .extend_from_slice(U32::new(0).as_bytes());
    assert_rejected(&file.finish());

    let mut file = basic_feed();
    file.rows::<RouteRow>(SectionId::Routes)[0].patterns_start = U32::new(1);
    file.section(SectionId::RoutePatterns)
        .extend_from_slice(U32::new(0).as_bytes());
    assert_rejected(&file.finish());
}

fn feed_with_a_station() -> InMemorySource {
    minimal("UTC", &daily(), NO_DATES, ST).with(
        "stops.txt",
        "stop_id,stop_name,stop_lat,stop_lon,location_type,parent_station\n\
         HUB,h,47.60,-122.33,1,\n\
         S1,a,47.60,-122.33,0,HUB\nS2,b,47.61,-122.33,0,HUB\n",
    )
}

#[test]
fn child_stops_must_be_contiguous_and_complete() {
    let mut file = HostileFile::from_source(feed_with_a_station());
    let station = file
        .rows::<StopRow>(SectionId::Stops)
        .iter()
        .position(|row| row.child_count.get() > 0)
        .expect("station");
    file.rows::<StopRow>(SectionId::Stops)[station].children_start = U32::new(1);
    assert_rejected(&file.finish());

    let mut file = HostileFile::from_source(feed_with_a_station());
    file.section(SectionId::ChildStops)
        .extend_from_slice(U32::new(0).as_bytes());
    assert_rejected(&file.finish());
}

/// The layouts above are the ones the compiler writes, including for a station's children.
#[test]
fn compiled_files_satisfy_the_contiguity_rules() {
    let output = compile_one(feed_with_a_station()).expect("compile");
    CompiledTimetable::parse(&output.bytes).expect("parse");
}

#[test]
fn a_calendar_stretched_past_the_format_limit_still_loads() {
    let dates = "service_id,date,exception_type\nW,19800101,1\nW,20990101,1\nW,20260105,1\n";
    let output = compile_one(minimal("UTC", CAL_HEADER, dates, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("a compiled file must load");
    assert_eq!(view.active_trip_count(d("20260105")), 1);
    assert!(
        output.reports[0]
            .warnings
            .iter()
            .any(|warning| warning.contains("too many days")),
        "{:?}",
        output.reports[0].warnings,
    );
}

/// The densest span survives a lone outlier, whichever side it is on.
#[test]
fn the_span_with_the_most_dates_is_kept() {
    let mut rows = String::from("service_id,date,exception_type\nW,21500101,1\n");
    for day in 1..=9 {
        writeln!(rows, "W,2026010{day},1").expect("write");
    }
    let output = compile_one(minimal("UTC", CAL_HEADER, &rows, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.active_trip_count(d("20260105")), 1);
    assert_eq!(
        view.active_trip_count(d("21500101")),
        0,
        "the lone outlier goes"
    );

    let mut rows = String::from("service_id,date,exception_type\nW,19800101,1\n");
    for day in 1..=9 {
        writeln!(rows, "W,2130010{day},1").expect("write");
    }
    let output = compile_one(minimal("UTC", CAL_HEADER, &rows, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.active_trip_count(d("21300105")), 1);
    assert_eq!(
        view.active_trip_count(d("19800101")),
        0,
        "the lone outlier goes"
    );
}

#[test]
fn a_date_at_the_end_of_time_alongside_real_service_still_loads() {
    let dates = "service_id,date,exception_type\nW,99991231,1\nW,20260105,1\n";
    let output = compile_one(minimal("UTC", CAL_HEADER, dates, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("a compiled file must load");
    assert_eq!(view.active_trip_count(d("20260105")), 1);
}

#[test]
fn service_only_at_the_end_of_time_is_an_error() {
    let dates = "service_id,date,exception_type\nW,99991231,1\n";
    let result = compile_one(minimal("UTC", CAL_HEADER, dates, ST));
    assert!(
        matches!(result, Err(CompileError::OutOfRange(_))),
        "{result:?}"
    );
}

#[test]
fn an_open_ended_calendar_keeps_service_up_to_the_limit() {
    let calendar = format!("{CAL_HEADER}W,1,1,1,1,1,1,1,20260101,99991231\n");
    let output = compile_one(minimal("UTC", &calendar, NO_DATES, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert!(output.reports[0].warning_count >= 1);
    assert_eq!(view.active_trip_count(d("20260105")), 1);
    // 40,000 days from the start, inclusive of the first day.
    let last = d("20260101").into_inner() + 39_999;
    assert_eq!(view.active_trip_count(ServiceDate::new(last)), 1);
    assert_eq!(view.active_trip_count(ServiceDate::new(last + 1)), 0);
}

#[test]
fn a_calendar_starting_before_1970_keeps_its_recent_service() {
    let calendar = format!("{CAL_HEADER}W,1,1,1,1,1,1,1,19000101,99991231\n");
    let output = compile_one(minimal("UTC", &calendar, NO_DATES, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.active_trip_count(d("20260105")), 1);
}

#[test]
fn a_reversed_calendar_range_warns() {
    let calendar = format!("{CAL_HEADER}W,1,1,1,1,1,1,1,20260201,20260101\n");
    let output = compile_one(minimal("UTC", &calendar, NO_DATES, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(view.active_trip_count(d("20260105")), 0);
    assert!(output.reports[0].warning_count >= 1);
}

#[test]
fn a_duplicate_calendar_row_warns_and_keeps_the_first() {
    let calendar = format!(
        "{CAL_HEADER}W,1,1,1,1,1,1,1,20260101,20260131\nW,1,1,1,1,1,1,1,20260201,20260228\n"
    );
    let output = compile_one(minimal("UTC", &calendar, NO_DATES, ST)).expect("compile");
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    assert_eq!(output.reports[0].warning_count, 1);
    assert!(view.service_active(0, d("20260110")));
    assert!(!view.service_active(0, d("20260210")));
}

/// A source of raw bytes, for files that aren't UTF-8.
struct RawSource(HashMap<String, Vec<u8>>);

impl GtfsSource for RawSource {
    fn open(&mut self, name: &str) -> Result<Option<Box<dyn Read + '_>>, CompileError> {
        Ok(self
            .0
            .get(name)
            .map(|bytes| Box::new(Cursor::new(bytes.as_slice())) as Box<dyn Read + '_>))
    }
}

fn latin1_feed(stops: &[u8], stop_times: &[u8]) -> RawSource {
    let mut files = HashMap::new();
    let mut insert = |name: &str, bytes: Vec<u8>| files.insert(name.to_owned(), bytes);
    insert(
        "agency.txt",
        b"agency_name,agency_timezone\nA,UTC\n".to_vec(),
    );
    insert("routes.txt", b"route_id,route_type\nR,3\n".to_vec());
    insert("stops.txt", stops.to_vec());
    insert("calendar.txt", daily().into_bytes());
    insert(
        "trips.txt",
        b"route_id,service_id,trip_id\nR,W,T\n".to_vec(),
    );
    insert("stop_times.txt", stop_times.to_vec());
    RawSource(files)
}

#[test]
fn trips_at_non_utf8_stop_ids_are_not_conflated() {
    let source = latin1_feed(
        b"stop_id,stop_name,stop_lat,stop_lon\n\
          S\xe9,a,47.60,-122.33\nS\xe8,b,47.70,-122.33\nZ,z,47.80,-122.33\n",
        b"trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
          T,08:00:00,08:00:00,S\xe8,1\nT,08:10:00,08:10:00,Z,2\n",
    );
    let output = compile_one(source).expect("compile");
    let report = &output.reports[0];
    assert_eq!(report.stops_listed, 1, "only Z survives");
    assert_eq!(report.trips_kept, 0, "{:?}", report.warnings);
    assert_eq!(report.dropped_trips.get(&DropReason::UnknownStop), Some(&1));
    assert!(report.warnings.iter().any(|w| w.contains("identifier")));
}

#[test]
fn a_non_utf8_name_reads_as_empty_and_warns() {
    let source = latin1_feed(
        b"stop_id,stop_name,stop_lat,stop_lon\nA,Caf\xe9,47.60,-122.33\nB,b,47.70,-122.33\n",
        b"trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
          T,08:00:00,08:00:00,A,1\nT,08:10:00,08:10:00,B,2\n",
    );
    let output = compile_one(source).expect("compile");
    assert_eq!(output.reports[0].trips_kept, 1);
    assert_eq!(output.reports[0].warning_count, 1);
    let view = CompiledTimetable::parse(&output.bytes).expect("parse");
    let stop = view.stop_by_gtfs_id("f", "A").expect("stop");
    assert_eq!(view.stop(stop).expect("info").name, None);
}

#[test]
fn an_empty_stop_id_never_resolves() {
    let stops = "stop_id,stop_lat,stop_lon\n,47.60,-122.33\nS2,47.70,-122.33\n";
    let stop_times = "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
                      T,08:00:00,08:00:00,,1\nT,08:10:00,08:10:00,S2,2\n";
    let output =
        compile_one(minimal("UTC", &daily(), NO_DATES, stop_times).with("stops.txt", stops))
            .expect("compile");
    let report = &output.reports[0];
    assert_eq!(
        report.stops_listed, 1,
        "the row with an empty ID is dropped"
    );
    assert_eq!(report.trips_kept, 0);
    assert_eq!(report.dropped_trips.get(&DropReason::UnknownStop), Some(&1));
}

#[test]
fn zero_byte_optional_files_count_as_absent() {
    for name in [
        "calendar_dates.txt",
        "frequencies.txt",
        "transfers.txt",
        "shapes.txt",
        "feed_info.txt",
    ] {
        let source = minimal("UTC", &daily(), NO_DATES, ST).with(name, "");
        let output = compile_one(source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(output.reports[0].trips_kept, 1, "{name}");
    }
}

#[test]
fn a_zero_byte_required_file_is_missing() {
    let source = minimal("UTC", &daily(), NO_DATES, ST).with("routes.txt", "");
    assert!(matches!(
        compile_one(source),
        Err(CompileError::MissingFile("routes.txt"))
    ));
}

#[test]
fn a_short_row_can_omit_an_optional_identifier() {
    let source = minimal("UTC", &daily(), NO_DATES, ST).with(
        "agency.txt",
        "agency_name,agency_timezone,agency_id\nA,UTC\n",
    );
    let output = compile_one(source).expect("compile");
    assert_eq!(output.reports[0].agencies, 1);
    assert_eq!(output.reports[0].warning_count, 0);
}
