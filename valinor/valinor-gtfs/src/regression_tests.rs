//! Regression tests for problems an independent review found.

use valinor_transit::ServiceDate;
use zerocopy::little_endian::U32;
use zerocopy::{FromBytes, IntoBytes};

use crate::format::layout::{FileHeader, PatternRow, RouteRow, SectionId, ServiceRow, StopRow};
use crate::format::writer::FileWriter;
use crate::{
    CompileError, CompiledOutput, CompiledTimetable, DecodeError, FeedId, FeedInput, GtfsSource,
    InMemorySource, compile, parse_service_date,
};

fn compile_one(source: impl GtfsSource + 'static) -> Result<CompiledOutput, CompileError> {
    compile(vec![FeedInput {
        id: FeedId::try_new("f".to_owned()).expect("id"),
        source: Box::new(source),
    }])
}

#[expect(dead_code, reason = "used by the tests added with each fix")]
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
