//! Loads a compiled file without copying it, and answers questions about it.
//!
//! [`CompiledTimetable`] borrows the file's bytes and holds typed slices over them.
//! Loading checks the header, the section directory, and the checksum,
//! and then validates every cross-reference (see [`crate::format::validate`]),
//! so a decoded file is safe to read, and any failure is a [`DecodeError`], never a panic.
//! Nothing a file says can make a loader allocate out of proportion to the file's size.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use jiff::civil::Date;
use memmap2::Mmap;
use self_cell::self_cell;
use valinor_transit::{RouteId, ServiceDate, ServiceDay, StopId, Time, TripId};
use zerocopy::little_endian::{F32, U32};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

use crate::error::{DecodeError, LoadError};
use crate::format::layout::{
    AgencyRow, DIRECTION_NONE, FORMAT_VERSION, FeedRow, FileHeader, IdIndexRow, MAGIC, NONE,
    NoonOffsetRow, PATTERN_BIKES_ALLOWED, PatternRow, RULE_APPROXIMATE, RULE_DROP_OFF_SHIFT,
    RULE_PICKUP_MASK, RUN_HEADWAY_FREQUENCY, RUN_SCHEDULED, RouteRow, SectionEntry, SectionId,
    ServiceRow, ShapePointRow, ShapeRow, StopRow, TimePair, TripRiderRow, TripRow,
    UNCHECKSUMMED_PREFIX, ZoneRow, ZoneTransitionRow, checksum_of,
};
use crate::format::validate::validate;

/// Days from the Unix epoch, as `valinor-transit` service dates.
///
/// # Errors
///
/// If `date` is before 1970.
pub fn service_date_of(date: Date) -> Result<ServiceDate, jiff::Error> {
    let days = date.since((jiff::Unit::Day, epoch()))?.get_days();
    let days = u32::try_from(days)
        .map_err(|_| jiff::Error::from_args(format_args!("{date} is before 1970")))?;
    Ok(ServiceDate::new(days))
}

/// The civil date of a service date.
///
/// # Errors
///
/// If the date is past the end of the supported range.
pub fn civil_date_of(date: ServiceDate) -> Result<Date, jiff::Error> {
    let days = i32::try_from(date.into_inner())
        .map_err(|_| jiff::Error::from_args(format_args!("{date} is too large")))?;
    epoch().checked_add(jiff::Span::new().try_days(days)?)
}

/// Parses a GTFS date (`YYYYMMDD`) into a service date.
///
/// `None` means that the text isn't a date or that it precedes 1970.
pub fn parse_service_date(text: &str) -> Option<ServiceDate> {
    let bytes = text.trim().as_bytes();
    if bytes.len() != 8 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let digit = |i: usize| i32::from(bytes[i] - b'0');
    let year = digit(0) * 1000 + digit(1) * 100 + digit(2) * 10 + digit(3);
    let month = digit(4) * 10 + digit(5);
    let day = digit(6) * 10 + digit(7);
    let date = Date::new(
        i16::try_from(year).ok()?,
        i8::try_from(month).ok()?,
        i8::try_from(day).ok()?,
    )
    .ok()?;
    service_date_of(date).ok()
}

const fn epoch() -> Date {
    Date::constant(1970, 1, 1)
}

/// Every table of the file, borrowed from its bytes.
///
/// The fields are reachable only inside this crate, and after validation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tables<'a> {
    pub(crate) boardable_stops: usize,
    pub(crate) feeds: &'a [FeedRow],
    pub(crate) zones: &'a [ZoneRow],
    pub(crate) zone_transitions: &'a [ZoneTransitionRow],
    pub(crate) noon_offsets: &'a [NoonOffsetRow],
    pub(crate) strings: &'a str,
    pub(crate) string_offsets: &'a [U32],
    pub(crate) agencies: &'a [AgencyRow],
    pub(crate) routes: &'a [RouteRow],
    pub(crate) route_patterns: &'a [U32],
    pub(crate) stops: &'a [StopRow],
    pub(crate) child_stops: &'a [U32],
    pub(crate) services: &'a [ServiceRow],
    pub(crate) service_bits: &'a [u8],
    pub(crate) patterns: &'a [PatternRow],
    pub(crate) pattern_stops: &'a [U32],
    pub(crate) trips: &'a [TripRow],
    pub(crate) times: &'a [TimePair],
    pub(crate) rules: &'a [u8],
    pub(crate) trip_riders: &'a [TripRiderRow],
    pub(crate) stop_headsigns: &'a [U32],
    pub(crate) stop_sequences: &'a [U32],
    pub(crate) stop_dists: &'a [F32],
    pub(crate) shapes: &'a [ShapeRow],
    pub(crate) shape_points: &'a [ShapePointRow],
    pub(crate) trip_index: &'a [IdIndexRow],
    pub(crate) route_index: &'a [IdIndexRow],
    pub(crate) stop_index: &'a [IdIndexRow],
}

/// Casts a slice of bytes to a slice of records, naming the section on failure.
fn cast_table<T>(section: SectionId, bytes: &[u8]) -> Result<&[T], DecodeError>
where
    T: FromBytes + KnownLayout + Immutable + Unaligned,
{
    <[T]>::ref_from_bytes(bytes).map_err(|_| DecodeError::Section {
        section: section.name(),
        reason: "its length is not a whole number of records",
    })
}

/// Reads the header and the directory, and hands back the section bytes in file order.
fn read_sections(bytes: &[u8]) -> Result<(usize, Vec<&[u8]>), DecodeError> {
    let header_len = core::mem::size_of::<FileHeader>();
    if bytes.len() < UNCHECKSUMMED_PREFIX {
        return Err(DecodeError::Truncated {
            found: bytes.len(),
            expected: UNCHECKSUMMED_PREFIX as u64,
        });
    }
    if bytes[..8] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if version != FORMAT_VERSION {
        return Err(DecodeError::UnsupportedVersion {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    let (header, _) = FileHeader::ref_from_prefix(bytes).map_err(|_| DecodeError::Truncated {
        found: bytes.len(),
        expected: header_len as u64,
    })?;
    if (bytes.len() as u64) < header.file_len.get() {
        return Err(DecodeError::Truncated {
            found: bytes.len(),
            expected: header.file_len.get(),
        });
    }
    if (bytes.len() as u64) > header.file_len.get() {
        return Err(DecodeError::TrailingData {
            found: bytes.len(),
            expected: header.file_len.get(),
        });
    }
    let stored = header.checksum.get();
    let computed = checksum_of(bytes);
    if stored != computed {
        return Err(DecodeError::ChecksumMismatch { stored, computed });
    }

    let count = header.section_count.get() as usize;
    if count != SectionId::ALL.len() {
        return Err(DecodeError::Section {
            section: "directory",
            reason: "the section count is wrong for this format version",
        });
    }
    let dir_len = count * core::mem::size_of::<SectionEntry>();
    let dir_bytes = bytes
        .get(header_len..header_len + dir_len)
        .ok_or(DecodeError::Truncated {
            found: bytes.len(),
            expected: (header_len + dir_len) as u64,
        })?;
    let entries = cast_entries(dir_bytes)?;
    let mut sections = Vec::with_capacity(count);
    let mut previous_end = (header_len + dir_len) as u64;
    for (entry, id) in entries.iter().zip(SectionId::ALL) {
        if entry.id.get() != id as u32 {
            return Err(DecodeError::Section {
                section: id.name(),
                reason: "its directory entry is out of place",
            });
        }
        let start = entry.offset.get();
        let end = start
            .checked_add(entry.byte_len.get())
            .ok_or(DecodeError::Section {
                section: id.name(),
                reason: "its bounds overflow",
            })?;
        if start < previous_end || end > bytes.len() as u64 {
            return Err(DecodeError::Section {
                section: id.name(),
                reason: "it overlaps the previous section or leaves the file",
            });
        }
        previous_end = end;
        let start = usize::try_from(start).map_err(|_| DecodeError::Section {
            section: id.name(),
            reason: "its offset does not fit in memory",
        })?;
        let end = usize::try_from(end).map_err(|_| DecodeError::Section {
            section: id.name(),
            reason: "its end does not fit in memory",
        })?;
        sections.push(&bytes[start..end]);
    }
    Ok((header.boardable_stops.get() as usize, sections))
}

fn cast_entries(bytes: &[u8]) -> Result<&[SectionEntry], DecodeError> {
    cast_table(SectionId::Feeds, bytes).map_err(|_| DecodeError::Section {
        section: "directory",
        reason: "its length is not a whole number of records",
    })
}

impl<'a> Tables<'a> {
    fn read(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let (boardable_stops, sections) = read_sections(bytes)?;
        let mut sections = sections.into_iter();
        macro_rules! table {
            ($id:ident) => {
                cast_table(
                    SectionId::$id,
                    sections.next().ok_or(DecodeError::Section {
                        section: SectionId::$id.name(),
                        reason: "it is missing",
                    })?,
                )?
            };
        }
        macro_rules! raw {
            ($id:ident) => {
                sections.next().ok_or(DecodeError::Section {
                    section: SectionId::$id.name(),
                    reason: "it is missing",
                })?
            };
        }
        let feeds = table!(Feeds);
        let zones = table!(Zones);
        let zone_transitions = table!(ZoneTransitions);
        let noon_offsets = table!(NoonOffsets);
        let strings_bytes = raw!(Strings);
        let string_offsets = table!(StringOffsets);
        let agencies = table!(Agencies);
        let routes = table!(Routes);
        let route_patterns = table!(RoutePatterns);
        let stops = table!(Stops);
        let child_stops = table!(ChildStops);
        let services = table!(Services);
        let service_bits = raw!(ServiceBits);
        let patterns = table!(Patterns);
        let pattern_stops = table!(PatternStops);
        let trips = table!(Trips);
        let times = table!(Times);
        let rules = raw!(Rules);
        let trip_riders = table!(TripRiders);
        let stop_headsigns = table!(StopHeadsigns);
        let stop_sequences = table!(StopSequences);
        let stop_dists = table!(StopDists);
        let shapes = table!(Shapes);
        let shape_points = table!(ShapePoints);
        let trip_index = table!(TripIndex);
        let route_index = table!(RouteIndex);
        let stop_index = table!(StopIndex);

        let strings = core::str::from_utf8(strings_bytes)
            .map_err(|_| DecodeError::InvalidStrings("the pool is not UTF-8"))?;
        Ok(Self {
            boardable_stops,
            feeds,
            zones,
            zone_transitions,
            noon_offsets,
            strings,
            string_offsets,
            agencies,
            routes,
            route_patterns,
            stops,
            child_stops,
            services,
            service_bits,
            patterns,
            pattern_stops,
            trips,
            times,
            rules,
            trip_riders,
            stop_headsigns,
            stop_sequences,
            stop_dists,
            shapes,
            shape_points,
            trip_index,
            route_index,
            stop_index,
        })
    }

    /// The string for a reference; `None` when the reference doesn't resolve.
    pub(crate) fn string(&self, id: U32) -> Option<&'a str> {
        let index = id.get() as usize;
        let start = self.string_offsets.get(index)?.get() as usize;
        let end = self.string_offsets.get(index.checked_add(1)?)?.get() as usize;
        self.strings.get(start..end)
    }
}

/// A compiled timetable file, loaded without copying.
#[derive(Debug, Clone, Copy)]
pub struct CompiledTimetable<'a> {
    pub(crate) tables: Tables<'a>,
}

impl<'a> CompiledTimetable<'a> {
    /// Loads and validates a compiled file.
    ///
    /// # Performance
    ///
    /// Reads the whole file once to check its checksum and references, in time linear in its size.
    ///
    /// # Errors
    ///
    /// If the file is truncated, corrupt, of another format version, or inconsistent.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let tables = Tables::read(bytes)?;
        validate(&tables)?;
        Ok(Self { tables })
    }

    /// The feeds in this file.
    pub fn feeds(&self) -> impl ExactSizeIterator<Item = FeedInfo<'a>> + 'a {
        let tables = self.tables;
        tables.feeds.iter().map(move |row| FeedInfo {
            id: tables.string(row.id).unwrap_or(""),
            version: non_empty(tables.string(row.version)),
            time_zone: tables
                .zones
                .get(row.zone.get() as usize)
                .and_then(|zone| tables.string(zone.name))
                .unwrap_or(""),
            first_service_date: ServiceDate::new(row.first_service_date.get()),
            last_service_date: ServiceDate::new(row.last_service_date.get()),
        })
    }

    /// How many stops the timetable numbers.
    pub fn stop_count(&self) -> usize {
        self.tables.boardable_stops
    }

    /// How many patterns (`valinor-transit` routes) the file holds.
    pub fn pattern_count(&self) -> usize {
        self.tables.patterns.len()
    }

    /// How many GTFS routes the file holds.
    pub fn gtfs_route_count(&self) -> usize {
        self.tables.routes.len()
    }

    /// How many agencies the file holds.
    pub fn agency_count(&self) -> usize {
        self.tables.agencies.len()
    }

    /// How many trips the file holds, counting each frequency run once.
    pub fn trip_count(&self) -> usize {
        self.tables.trips.len()
    }

    /// How many rows of `stops.txt` the file holds across every feed.
    pub fn location_count(&self) -> usize {
        self.tables.stops.len()
    }

    /// How many calendars the file holds.
    pub fn service_count(&self) -> usize {
        self.tables.services.len()
    }

    /// The total number of stop events across all trips.
    pub fn stop_event_count(&self) -> usize {
        self.tables.times.len()
    }

    /// The UTC instant at which a service date starts (its noon minus twelve hours, in the
    /// feed's time zone). A device needs no time zone database to compute this.
    ///
    /// `None` means that the file doesn't cover the date.
    pub fn service_day_start_unix(&self, date: ServiceDate) -> Option<i64> {
        let days = date.into_inner();
        let offsets = self.tables.noon_offsets;
        let sentinel = offsets.last()?.first_date.get();
        if days < offsets.first()?.first_date.get() || days >= sentinel {
            return None;
        }
        let row = offsets
            .partition_point(|row| row.first_date.get() <= days)
            .checked_sub(1)
            .and_then(|i| offsets.get(i))?;
        Some(i64::from(days) * 86_400 - i64::from(row.noon_offset.get()))
    }

    /// Whether a calendar runs on a date.
    ///
    /// An unknown service runs on no date.
    pub fn service_active(&self, service: u32, date: ServiceDate) -> bool {
        let Some(row) = self.tables.services.get(service as usize) else {
            return false;
        };
        service_bit(self.tables.service_bits, row, date.into_inner())
    }

    /// The dates on which a calendar runs, in ascending order.
    pub(crate) fn service_dates(&self, row: &ServiceRow) -> impl Iterator<Item = ServiceDate> + 'a {
        let bits = self.tables.service_bits;
        let first = row.first_date.get();
        let days = row.day_count.get();
        let row = *row;
        (0..days).filter_map(move |offset| {
            let date = first.checked_add(offset)?;
            service_bit(bits, &row, date).then(|| ServiceDate::new(date))
        })
    }

    /// Lays out the service days of a window for [`valinor_transit::Timetable::materialize`].
    ///
    /// The window's clock starts at the first given date's service-day origin,
    /// and each day's offset already reflects any time zone change in between,
    /// so a day can be 23 or 25 hours long with no help from a time zone database.
    ///
    /// # Errors
    ///
    /// If `dates` is empty, a date isn't covered by the file, or an offset can't be held.
    pub fn service_window(&self, dates: &[ServiceDate]) -> Result<ServiceWindow, DecodeError> {
        let first = *dates.first().ok_or(DecodeError::InvalidValue {
            table: "dates",
            reason: "a window needs at least one date",
        })?;
        let origin_unix = self
            .service_day_start_unix(first)
            .ok_or(DecodeError::DateNotCovered(first.into_inner()))?;
        let mut days = Vec::with_capacity(dates.len());
        for &date in dates {
            let start = self
                .service_day_start_unix(date)
                .ok_or(DecodeError::DateNotCovered(date.into_inner()))?;
            let offset = i32::try_from(start - origin_unix)
                .ok()
                .and_then(|offset| Time::try_new(offset).ok())
                .ok_or(DecodeError::InvalidValue {
                    table: "window",
                    reason: "a day's offset is out of range",
                })?;
            days.push(ServiceDay { date, offset });
        }
        Ok(ServiceWindow { origin_unix, days })
    }

    /// The riders' view of a stop.
    pub fn stop(&self, stop: StopId) -> Option<StopInfo<'a>> {
        let index = stop.into_inner() as usize;
        (index < self.tables.boardable_stops)
            .then(|| self.stop_row(index))
            .flatten()
    }

    /// The riders' view of any row of `stops.txt`, including stations and entrances.
    pub fn location(&self, id: LocationId) -> Option<StopInfo<'a>> {
        self.stop_row(id.0 as usize)
    }

    fn stop_row(&self, index: usize) -> Option<StopInfo<'a>> {
        let tables = self.tables;
        let row = tables.stops.get(index)?;
        let coordinate = if row.lat_e7.get() == i32::MIN {
            None
        } else {
            Some(LatLon::from_e7(row.lat_e7.get(), row.lon_e7.get()))
        };
        Some(StopInfo {
            gtfs_id: tables.string(row.gtfs_id)?,
            feed: self.feed_of(row.feed)?,
            name: non_empty(tables.string(row.name)),
            code: non_empty(tables.string(row.code)),
            platform: non_empty(tables.string(row.platform)),
            coordinate,
            location_type: LocationType::from_byte(row.location_type),
            wheelchair_boarding: Accessibility::from_byte(row.wheelchair_boarding),
            parent_station: reference(row.parent).map(LocationId),
            time_zone: tables
                .zones
                .get(row.zone.get() as usize)
                .and_then(|zone| tables.string(zone.name))?,
        })
    }

    fn feed_of(&self, feed: U32) -> Option<&'a str> {
        self.tables
            .feeds
            .get(feed.get() as usize)
            .and_then(|row| self.tables.string(row.id))
    }

    /// The UTC offset, in seconds east, at a stop at a UTC instant.
    ///
    /// Stops report their own zone's offset, not the feed's. `None` means the stop is unknown.
    pub fn stop_utc_offset_at(&self, stop: StopId, unix: i64) -> Option<i32> {
        let index = stop.into_inner() as usize;
        if index >= self.tables.boardable_stops {
            return None;
        }
        let zone = self
            .tables
            .zones
            .get(self.tables.stops.get(index)?.zone.get() as usize)?;
        let start = zone.transitions_start.get() as usize;
        let count = zone.transition_count.get() as usize;
        let transitions = self
            .tables
            .zone_transitions
            .get(start..start.checked_add(count)?)?;
        let after = transitions.partition_point(|row| row.at_unix.get() <= unix);
        transitions
            .get(after.checked_sub(1)?)
            .map(|row| row.offset.get())
    }

    /// The stops inside a station (or other parent): every boardable descendant.
    ///
    /// A boardable stop lists itself. `None` means that the location doesn't exist.
    pub fn stops_at(&self, location: LocationId) -> Option<Vec<StopId>> {
        let row = self.tables.stops.get(location.0 as usize)?;
        if (location.0 as usize) < self.tables.boardable_stops {
            return Some(vec![StopId::new(location.0)]);
        }
        let start = row.children_start.get() as usize;
        let count = row.child_count.get() as usize;
        Some(
            self.tables
                .child_stops
                .get(start..start.checked_add(count)?)?
                .iter()
                .map(|stop| StopId::new(stop.get()))
                .collect(),
        )
    }

    /// Finds a GTFS `stop_id` within a feed, whatever its location type.
    pub fn location_by_gtfs_id(&self, feed: &str, stop_id: &str) -> Option<LocationId> {
        lookup_one(&self.tables, self.tables.stop_index, feed, stop_id).map(LocationId)
    }

    /// Finds the timetable stop for a GTFS `stop_id` within a feed.
    ///
    /// Stations and other locations that aren't boarded directly have no [`StopId`];
    /// use [`Self::location_by_gtfs_id`] and [`Self::stops_at`] for those.
    pub fn stop_by_gtfs_id(&self, feed: &str, stop_id: &str) -> Option<StopId> {
        let location = self.location_by_gtfs_id(feed, stop_id)?;
        ((location.0 as usize) < self.tables.boardable_stops).then(|| StopId::new(location.0))
    }

    /// Finds a GTFS `route_id` within a feed.
    pub fn route_by_gtfs_id(&self, feed: &str, route_id: &str) -> Option<GtfsRoute> {
        lookup_one(&self.tables, self.tables.route_index, feed, route_id).map(GtfsRoute)
    }

    /// The riders' view of a GTFS route.
    pub fn route(&self, route: GtfsRoute) -> Option<RouteInfo<'a>> {
        let tables = self.tables;
        let row = tables.routes.get(route.0 as usize)?;
        Some(RouteInfo {
            gtfs_id: tables.string(row.gtfs_id)?,
            feed: self.feed_of(row.feed)?,
            agency: reference(row.agency).map(AgencyIndex),
            short_name: non_empty(tables.string(row.short_name)),
            long_name: non_empty(tables.string(row.long_name)),
            description: non_empty(tables.string(row.desc)),
            route_type: row.route_type.get(),
            color: rgb(row.color),
            text_color: rgb(row.text_color),
            url: non_empty(tables.string(row.url)),
        })
    }

    /// The patterns (`valinor-transit` routes) of a GTFS route.
    pub fn route_patterns(&self, route: GtfsRoute) -> Option<Vec<RouteId>> {
        let row = self.tables.routes.get(route.0 as usize)?;
        let start = row.patterns_start.get() as usize;
        let count = row.pattern_count.get() as usize;
        Some(
            self.tables
                .route_patterns
                .get(start..start.checked_add(count)?)?
                .iter()
                .map(|pattern| RouteId::new(pattern.get()))
                .collect(),
        )
    }

    /// The riders' view of an agency.
    pub fn agency(&self, agency: AgencyIndex) -> Option<AgencyInfo<'a>> {
        let tables = self.tables;
        let row = tables.agencies.get(agency.0 as usize)?;
        Some(AgencyInfo {
            gtfs_id: non_empty(tables.string(row.gtfs_id)),
            feed: self.feed_of(row.feed)?,
            name: tables.string(row.name)?,
            url: non_empty(tables.string(row.url)),
            phone: non_empty(tables.string(row.phone)),
            fare_url: non_empty(tables.string(row.fare_url)),
        })
    }

    /// The GTFS route, stop sequence, and bike rule of a pattern.
    pub fn pattern(&self, pattern: RouteId) -> Option<PatternInfo<'a>> {
        let tables = self.tables;
        let row = tables.patterns.get(pattern.into_inner() as usize)?;
        let start = row.stops_start.get() as usize;
        let count = row.stop_count.get() as usize;
        let stops = tables.pattern_stops.get(start..start.checked_add(count)?)?;
        Some(PatternInfo {
            route: GtfsRoute(row.route.get()),
            stops,
            bikes_allowed: row.attributes & PATTERN_BIKES_ALLOWED != 0,
        })
    }

    fn trip_rows(&self, trip: TripId) -> Option<(&'a TripRow, &'a TripRiderRow)> {
        let index = trip.into_inner() as usize;
        Some((
            self.tables.trips.get(index)?,
            self.tables.trip_riders.get(index)?,
        ))
    }

    fn trip_pattern(&self, trip: &TripRow) -> Option<&'a PatternRow> {
        self.tables.patterns.get(trip.pattern.get() as usize)
    }

    fn trip_times(&self, row: &TripRow, count: usize) -> Option<&'a [TimePair]> {
        let start = row.times_start.get() as usize;
        self.tables.times.get(start..start.checked_add(count)?)
    }

    /// The riders' view of a trip.
    pub fn trip(&self, trip: TripId) -> Option<TripInfo<'a>> {
        let tables = self.tables;
        let (row, rider) = self.trip_rows(trip)?;
        let pattern = self.trip_pattern(row)?;
        let count = pattern.stop_count.get() as usize;
        let times = self.trip_times(row, count)?;
        let stop_headsigns = match reference(rider.stop_headsigns_start) {
            Some(start) => Some(
                tables
                    .stop_headsigns
                    .get(start as usize..(start as usize).checked_add(count)?)?,
            ),
            None => None,
        };
        let sequence_start = rider.stop_sequences_start.get() as usize;
        Some(TripInfo {
            gtfs_id: tables.string(rider.gtfs_id)?,
            route: GtfsRoute(pattern.route.get()),
            pattern: RouteId::new(row.pattern.get()),
            service: row.service.get(),
            headsign: non_empty(tables.string(rider.headsign)),
            block_id: non_empty(tables.string(rider.block)),
            wheelchair_accessible: Accessibility::from_byte(rider.wheelchair),
            bikes_allowed: Accessibility::from_byte(rider.bikes),
            direction: direction_of(rider.direction),
            frequency_run: rider.frequency_run != RUN_SCHEDULED,
            headway_based: rider.frequency_run == RUN_HEADWAY_FREQUENCY,
            has_shape: reference(rider.shape).is_some(),
            times,
            stop_headsigns,
            stop_sequences: tables
                .stop_sequences
                .get(sequence_start..sequence_start.checked_add(count)?)?,
            rules: reference(row.rules_start)
                .and_then(|start| {
                    tables
                        .rules
                        .get(start as usize..(start as usize).checked_add(count)?)
                })
                .unwrap_or(&[]),
        })
    }

    /// A headsign for a stop of a trip: the stop's own, or else the trip's.
    pub fn stop_headsign(&self, trip: &TripInfo<'a>, position: usize) -> Option<&'a str> {
        if let Some(headsigns) = trip.stop_headsigns
            && let Some(id) = headsigns.get(position)
            && let Some(text) = non_empty(self.tables.string(*id))
        {
            return Some(text);
        }
        trip.headsign
    }

    /// Runs of a GTFS trip on a service date, as GTFS-RT finds them by `trip_id` and `start_date`.
    ///
    /// A frequency-based trip has one run per expanded departure.
    /// Runs whose calendar doesn't include the date are not returned.
    ///
    /// To resolve a GTFS-RT `start_time`, use [`Self::trip_run_for_start_time`] rather than
    /// comparing [`TripRun::first_departure`]: the departures of a headway-based run are a grid
    /// laid out by the compiler, and a vehicle's real start time need not fall on it.
    pub fn trips_by_gtfs_id(
        &self,
        feed: &str,
        trip_id: &str,
        date: ServiceDate,
    ) -> impl Iterator<Item = TripRun> + 'a {
        let start = self.service_day_start_unix(date);
        let copy = *self;
        let rows = lookup_range(&self.tables, self.tables.trip_index, feed, trip_id);
        rows.into_iter().filter_map(move |row| {
            let trip_row = copy.tables.trips.get(row.get() as usize)?;
            copy.service_active(trip_row.service.get(), date)
                .then_some(())?;
            let times = copy.tables.times.get(trip_row.times_start.get() as usize)?;
            let kind = copy
                .tables
                .trip_riders
                .get(row.get() as usize)?
                .frequency_run;
            Some(TripRun {
                trip: TripId::new(row.get()),
                date,
                service_day_start_unix: start,
                first_departure: times.departure.get(),
                frequency_run: kind != RUN_SCHEDULED,
                headway_based: kind == RUN_HEADWAY_FREQUENCY,
            })
        })
    }

    /// The run of a trip that a GTFS-RT `TripDescriptor` names.
    ///
    /// `start_time` is the descriptor's `start_time`, in seconds from the start of the service day.
    ///
    /// - A trip with its own schedule has one run on a date, and `start_time` isn't needed.
    /// - An `exact_times=1` frequency trip needs `start_time`, and it must match a departure.
    /// - An `exact_times=0` frequency trip needs `start_time`, and it gets the *nearest* run.
    ///   Those runs are a grid the compiler laid out from the headway, so a real start time
    ///   usually falls between them and two vehicles can land on one run.
    ///   Treat the result as the template for a vehicle's times, not as that vehicle's identity:
    ///   key real-time state for these runs by the descriptor's `start_time`.
    ///
    /// `None` means no run fits: the trip isn't active on `date`,
    /// or a frequency trip came without a `start_time`, or nothing in the feed matches it.
    pub fn trip_run_for_start_time(
        &self,
        feed: &str,
        trip_id: &str,
        date: ServiceDate,
        start_time: Option<i32>,
    ) -> Option<TripRun> {
        let mut nearest: Option<(u32, TripRun)> = None;
        for run in self.trips_by_gtfs_id(feed, trip_id, date) {
            if !run.frequency_run {
                return Some(run);
            }
            let start = start_time?;
            if !run.headway_based {
                if run.first_departure == start {
                    return Some(run);
                }
                continue;
            }
            let gap = run.first_departure.abs_diff(start);
            if nearest.is_none_or(|(best, _)| gap < best) {
                nearest = Some((gap, run));
            }
        }
        nearest.map(|(_, run)| run)
    }

    /// How many trips run on a date, counted from the compiled file alone.
    pub fn active_trip_count(&self, date: ServiceDate) -> usize {
        self.tables
            .trips
            .iter()
            .filter(|trip| self.service_active(trip.service.get(), date))
            .count()
    }

    /// How many stop events run on a date, counted from the compiled file alone.
    pub fn active_stop_event_count(&self, date: ServiceDate) -> usize {
        self.tables
            .trips
            .iter()
            .filter(|trip| self.service_active(trip.service.get(), date))
            .filter_map(|trip| self.trip_pattern(trip))
            .map(|pattern| pattern.stop_count.get() as usize)
            .sum()
    }

    /// A trip's shape between two of its stop positions, ready for display.
    ///
    /// `None` means the trip has no shape, or a position is out of range, or `from >= to`.
    pub fn shape_between(&self, trip: TripId, from: usize, to: usize) -> Option<ShapeSlice<'a>> {
        if from >= to {
            return None;
        }
        let (_, rider) = self.trip_rows(trip)?;
        let shape = self.tables.shapes.get(reference(rider.shape)? as usize)?;
        let start = shape.points_start.get() as usize;
        let count = shape.point_count.get() as usize;
        let points = self
            .tables
            .shape_points
            .get(start..start.checked_add(count)?)?;
        let dists_start = reference(rider.stop_dists_start)? as usize;
        let from_dist = self
            .tables
            .stop_dists
            .get(dists_start.checked_add(from)?)?
            .get();
        let to_dist = self
            .tables
            .stop_dists
            .get(dists_start.checked_add(to)?)?
            .get();
        Some(slice_shape(points, from_dist, to_dist))
    }
}

fn slice_shape(points: &[ShapePointRow], from: f32, to: f32) -> ShapeSlice<'_> {
    let first = points.partition_point(|p| p.dist.get() <= from);
    let last = points.partition_point(|p| p.dist.get() < to);
    let start = interpolate_at(points, from);
    let end = interpolate_at(points, to);
    let inner = points.get(first..last).unwrap_or(&[]);
    ShapeSlice {
        start,
        end,
        inner,
        from_dist: from,
        to_dist: to,
    }
}

/// The point at `dist` along a shape, clamping to its ends.
fn interpolate_at(points: &[ShapePointRow], dist: f32) -> Option<LatLon> {
    let after = points.partition_point(|p| p.dist.get() < dist);
    let previous = after.checked_sub(1).and_then(|i| points.get(i));
    match (previous, points.get(after)) {
        (Some(a), Some(b)) => {
            let span = b.dist.get() - a.dist.get();
            let fraction = if span > 0.0 {
                f64::from((dist - a.dist.get()) / span)
            } else {
                0.0
            };
            Some(LatLon {
                lat: lerp_e7(a.lat_e7.get(), b.lat_e7.get(), fraction),
                lon: lerp_e7(a.lon_e7.get(), b.lon_e7.get(), fraction),
            })
        }
        (Some(point), None) | (None, Some(point)) => {
            Some(LatLon::from_e7(point.lat_e7.get(), point.lon_e7.get()))
        }
        (None, None) => None,
    }
}

fn lerp_e7(from: i32, to: i32, fraction: f64) -> f64 {
    let (from, to) = (f64::from(from) / 1e7, f64::from(to) / 1e7);
    from + (to - from) * fraction
}

fn service_bit(bits: &[u8], row: &ServiceRow, date: u32) -> bool {
    let first = row.first_date.get();
    if date < first || date - first >= row.day_count.get() {
        return false;
    }
    let bit = (date - first) as usize;
    let start = row.bits_start.get() as usize;
    bits.get(start.saturating_add(bit / 8))
        .is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
}

fn non_empty(text: Option<&str>) -> Option<&str> {
    text.filter(|t| !t.is_empty())
}

fn reference(value: U32) -> Option<u32> {
    let value = value.get();
    (value != NONE).then_some(value)
}

fn rgb(color: crate::format::layout::ColorRow) -> Option<[u8; 3]> {
    (color.present != 0).then_some(color.rgb)
}

fn direction_of(byte: u8) -> Option<u8> {
    (byte != DIRECTION_NONE).then_some(byte)
}

fn lookup_feed(tables: &Tables<'_>, feed: &str) -> Option<u32> {
    tables
        .feeds
        .iter()
        .position(|row| tables.string(row.id) == Some(feed))
        .and_then(|index| u32::try_from(index).ok())
}

fn lookup_range(tables: &Tables<'_>, index: &[IdIndexRow], feed: &str, id: &str) -> Vec<U32> {
    let Some(feed) = lookup_feed(tables, feed) else {
        return Vec::new();
    };
    let key = (feed, id);
    let lower = index.partition_point(|row| index_key(tables, row) < key);
    index[lower..]
        .iter()
        .take_while(|row| index_key(tables, row) == key)
        .map(|row| row.row)
        .collect()
}

fn lookup_one(tables: &Tables<'_>, index: &[IdIndexRow], feed: &str, id: &str) -> Option<u32> {
    let feed = lookup_feed(tables, feed)?;
    let key = (feed, id);
    let position = index.partition_point(|row| index_key(tables, row) < key);
    let row = index.get(position)?;
    (index_key(tables, row) == key).then(|| row.row.get())
}

fn index_key<'a>(tables: &Tables<'a>, row: &IdIndexRow) -> (u32, &'a str) {
    (row.feed.get(), tables.string(row.id).unwrap_or(""))
}

/// One feed of a compiled file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedInfo<'a> {
    /// The namespace of this feed's identifiers.
    pub id: &'a str,
    /// `feed_info.txt`'s `feed_version`.
    pub version: Option<&'a str>,
    /// The feed's time zone.
    pub time_zone: &'a str,
    /// The first service date of any of the feed's trips.
    pub first_service_date: ServiceDate,
    /// The last service date of any of the feed's trips.
    pub last_service_date: ServiceDate,
}

/// A window of service days, with the file's offsets applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceWindow {
    /// The UTC instant of the window's clock zero.
    pub origin_unix: i64,
    /// The days for [`valinor_transit::Timetable::materialize`], in the order they were given.
    pub days: Vec<ServiceDay>,
}

/// A location from `stops.txt`, numbered across all of a file's feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocationId(pub(crate) u32);

/// A GTFS route in a file. `valinor-transit` calls its patterns routes; this is the GTFS kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GtfsRoute(pub(crate) u32);

/// An agency in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgencyIndex(pub(crate) u32);

/// A GTFS `location_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocationType {
    /// A stop or platform where riders board.
    Stop,
    /// A physical structure with platforms inside.
    Station,
    /// A station entrance or exit.
    Entrance,
    /// A generic node within a station.
    GenericNode,
    /// A boarding area on a platform.
    BoardingArea,
}

impl LocationType {
    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Station,
            2 => Self::Entrance,
            3 => Self::GenericNode,
            4 => Self::BoardingArea,
            _ => Self::Stop,
        }
    }
}

/// A GTFS tri-state accessibility flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Accessibility {
    /// The feed gives no information.
    Unknown,
    /// Allowed or accessible.
    Yes,
    /// Not allowed or not accessible.
    No,
}

impl Accessibility {
    fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Yes,
            2 => Self::No,
            _ => Self::Unknown,
        }
    }
}

/// A WGS84 coordinate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    /// Degrees north.
    pub lat: f64,
    /// Degrees east.
    pub lon: f64,
}

impl LatLon {
    fn from_e7(lat: i32, lon: i32) -> Self {
        Self {
            lat: f64::from(lat) / 1e7,
            lon: f64::from(lon) / 1e7,
        }
    }
}

/// The riders' view of a stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopInfo<'a> {
    /// The stop's `stop_id`.
    pub gtfs_id: &'a str,
    /// The stop's feed.
    pub feed: &'a str,
    /// The name riders see.
    pub name: Option<&'a str>,
    /// The short code riders may enter.
    pub code: Option<&'a str>,
    /// The platform within a station.
    pub platform: Option<&'a str>,
    /// Where the stop is; `None` for locations that may omit it.
    pub coordinate: Option<LatLon>,
    /// What kind of location this is.
    pub location_type: LocationType,
    /// Whether a rider in a wheelchair can board here.
    pub wheelchair_boarding: Accessibility,
    /// The stop's parent station.
    pub parent_station: Option<LocationId>,
    /// The stop's time zone.
    pub time_zone: &'a str,
}

/// The riders' view of a GTFS route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteInfo<'a> {
    /// The route's `route_id`.
    pub gtfs_id: &'a str,
    /// The route's feed.
    pub feed: &'a str,
    /// The route's agency.
    pub agency: Option<AgencyIndex>,
    /// The short name, such as a number.
    pub short_name: Option<&'a str>,
    /// The long name.
    pub long_name: Option<&'a str>,
    /// `route_desc`.
    pub description: Option<&'a str>,
    /// `route_type`, including extended values.
    pub route_type: u16,
    /// `route_color`.
    pub color: Option<[u8; 3]>,
    /// `route_text_color`.
    pub text_color: Option<[u8; 3]>,
    /// `route_url`.
    pub url: Option<&'a str>,
}

/// The riders' view of an agency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgencyInfo<'a> {
    /// The `agency_id`, which single-agency feeds may omit.
    pub gtfs_id: Option<&'a str>,
    /// The agency's feed.
    pub feed: &'a str,
    /// The name riders see.
    pub name: &'a str,
    /// `agency_url`.
    pub url: Option<&'a str>,
    /// `agency_phone`.
    pub phone: Option<&'a str>,
    /// `agency_fare_url`.
    pub fare_url: Option<&'a str>,
}

/// The GTFS route, stop sequence, and bike rule of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternInfo<'a> {
    /// The GTFS route.
    pub route: GtfsRoute,
    stops: &'a [U32],
    /// Whether bicycles are allowed on this pattern's trips.
    pub bikes_allowed: bool,
}

impl PatternInfo<'_> {
    /// The stops of the pattern, in order.
    pub fn stops(&self) -> impl ExactSizeIterator<Item = StopId> + '_ {
        self.stops.iter().map(|stop| StopId::new(stop.get()))
    }
}

/// The riders' view of a trip.
#[derive(Debug, Clone, Copy)]
pub struct TripInfo<'a> {
    /// The GTFS `trip_id`, which every run of a frequency-based trip shares.
    pub gtfs_id: &'a str,
    /// The GTFS route.
    pub route: GtfsRoute,
    /// The pattern the trip runs on.
    pub pattern: RouteId,
    /// The calendar the trip runs on.
    pub service: u32,
    /// `trip_headsign`.
    pub headsign: Option<&'a str>,
    /// `block_id`.
    pub block_id: Option<&'a str>,
    /// `wheelchair_accessible`.
    pub wheelchair_accessible: Accessibility,
    /// `bikes_allowed`.
    pub bikes_allowed: Accessibility,
    /// `direction_id`.
    pub direction: Option<u8>,
    /// Whether this is one run of a `frequencies.txt` trip.
    pub frequency_run: bool,
    /// Whether the feed gives only a headway for this run (`exact_times=0`).
    ///
    /// Such trips are expanded to fixed runs so that they can be routed,
    /// but only the headway is promised, so a response should call their times approximate.
    pub headway_based: bool,
    /// Whether the trip has a shape to slice.
    pub has_shape: bool,
    times: &'a [TimePair],
    stop_headsigns: Option<&'a [U32]>,
    stop_sequences: &'a [U32],
    rules: &'a [u8],
}

impl TripInfo<'_> {
    /// How many stops the trip makes.
    pub fn stop_count(&self) -> usize {
        self.times.len()
    }

    /// The arrival and departure at a stop position, in seconds from the service day's start.
    pub fn time_at(&self, position: usize) -> Option<(i32, i32)> {
        self.times
            .get(position)
            .map(|pair| (pair.arrival.get(), pair.departure.get()))
    }

    /// The GTFS `stop_sequence` at a position.
    pub fn stop_sequence_at(&self, position: usize) -> Option<u32> {
        self.stop_sequences.get(position).map(|value| value.get())
    }

    /// The position of a GTFS `stop_sequence` value, as GTFS-RT addresses stops.
    pub fn position_of_sequence(&self, sequence: u32) -> Option<usize> {
        self.stop_sequences
            .iter()
            .position(|value| value.get() == sequence)
    }

    /// The GTFS `pickup_type` at a position (`0` regular, `1` none, `2` phone, `3` driver).
    pub fn pickup_type_at(&self, position: usize) -> u8 {
        self.rules
            .get(position)
            .map_or(0, |byte| byte & RULE_PICKUP_MASK)
    }

    /// Whether the time at a position is approximate, so a response can say so.
    ///
    /// True when the feed marks it `timepoint=0`, when it was interpolated,
    /// or when the whole trip is [`Self::headway_based`].
    /// `None` means the position is out of range.
    pub fn time_is_approximate_at(&self, position: usize) -> Option<bool> {
        self.times.get(position)?;
        let marked = self
            .rules
            .get(position)
            .is_some_and(|byte| byte & RULE_APPROXIMATE != 0);
        Some(self.headway_based || marked)
    }

    /// The GTFS `drop_off_type` at a position.
    pub fn drop_off_type_at(&self, position: usize) -> u8 {
        self.rules
            .get(position)
            .map_or(0, |byte| (byte >> RULE_DROP_OFF_SHIFT) & RULE_PICKUP_MASK)
    }
}

/// One run of a trip on one service date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripRun {
    /// The trip's identifier in the timetable.
    pub trip: TripId,
    /// The run's service date.
    pub date: ServiceDate,
    /// The UTC instant its service day starts at, when the file covers that date.
    pub service_day_start_unix: Option<i64>,
    /// When the run leaves its first stop, in seconds from the service day's start.
    ///
    /// For a [`Self::headway_based`] run this is a position on a grid, not a real departure.
    pub first_departure: i32,
    /// Whether this is one run of a `frequencies.txt` trip.
    pub frequency_run: bool,
    /// Whether the feed gives only a headway for this run (`exact_times=0`).
    pub headway_based: bool,
}

/// A piece of a shape between two stops of a trip.
#[derive(Debug, Clone, Copy)]
pub struct ShapeSlice<'a> {
    start: Option<LatLon>,
    end: Option<LatLon>,
    inner: &'a [ShapePointRow],
    /// Distance along the shape where the slice starts.
    pub from_dist: f32,
    /// Distance along the shape where the slice ends.
    pub to_dist: f32,
}

impl ShapeSlice<'_> {
    /// The slice as coordinates, including interpolated ends.
    pub fn points(&self) -> Vec<LatLon> {
        let mut out = Vec::with_capacity(self.inner.len() + 2);
        out.extend(self.start);
        out.extend(
            self.inner
                .iter()
                .map(|point| LatLon::from_e7(point.lat_e7.get(), point.lon_e7.get())),
        );
        out.extend(self.end);
        out
    }
}

/// The bytes backing a loaded file: a memory map or an owned buffer.
#[derive(Debug)]
enum Backing {
    Mapped(Mmap),
    Owned(Vec<u8>),
}

impl AsRef<[u8]> for Backing {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(bytes) => bytes,
        }
    }
}

self_cell!(
    struct LoadedInner {
        owner: Arc<Backing>,
        #[covariant]
        dependent: CompiledTimetable,
    }
    impl {Debug}
);

/// A compiled timetable file that owns its bytes.
#[derive(Debug)]
pub struct LoadedTimetable {
    inner: LoadedInner,
}

impl LoadedTimetable {
    /// Maps a compiled file from disk and validates it.
    ///
    /// # Safety
    ///
    /// The file must not be modified while it is mapped;
    /// the operating system can make a truncated mapping fault at any later read.
    /// Publish new files by writing to a temporary name and renaming over the old one.
    ///
    /// # Errors
    ///
    /// If the file can't be read or isn't a valid compiled timetable.
    pub unsafe fn map(path: &Path) -> Result<Self, LoadError> {
        let file = File::open(path)?;
        // SAFETY: the caller promises not to modify the file while it is mapped,
        // which is the only way this mapping could change or fault underneath us.
        let map = unsafe { Mmap::map(&file) }?;
        Ok(Self::from_backing(Backing::Mapped(map))?)
    }

    /// Takes ownership of a buffer holding a compiled file.
    ///
    /// # Errors
    ///
    /// If the bytes aren't a valid compiled timetable.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, DecodeError> {
        Self::from_backing(Backing::Owned(bytes))
    }

    fn from_backing(backing: Backing) -> Result<Self, DecodeError> {
        Ok(Self {
            inner: LoadedInner::try_new(Arc::new(backing), |backing| {
                CompiledTimetable::parse((**backing).as_ref())
            })?,
        })
    }

    /// The loaded file.
    pub fn timetable(&self) -> &CompiledTimetable<'_> {
        self.inner.borrow_dependent()
    }
}
