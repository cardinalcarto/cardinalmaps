//! Errors from compiling feeds and from decoding compiled files.

use thiserror::Error;
use valinor_transit::BuildError;

/// Why a compiled file could not be decoded.
///
/// Every failure while reading a file is one of these; decoding never panics.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeError {
    /// The file ends before a structure it claims to hold.
    #[error("read a file truncated at {found} bytes (expected at least {expected})")]
    Truncated {
        /// How many bytes were present.
        found: usize,
        /// How many bytes the structure requires.
        expected: u64,
    },
    /// The file holds more bytes than its header says.
    #[error("read a file with {found} bytes where the header says {expected}")]
    TrailingData {
        /// How many bytes were present.
        found: usize,
        /// How many bytes the header says there are.
        expected: u64,
    },
    /// The file doesn't start with the magic bytes.
    #[error("recognize this file as a compiled timetable")]
    BadMagic,
    /// The file's format version isn't one this build reads.
    #[error("read format version {found} (this build reads version {supported})")]
    UnsupportedVersion {
        /// The version in the file.
        found: u32,
        /// The version this build supports.
        supported: u32,
    },
    /// The file's contents don't match its checksum.
    #[error("verify the file checksum: stored {stored:#010x}, computed {computed:#010x}")]
    ChecksumMismatch {
        /// The checksum stored in the header.
        stored: u32,
        /// The checksum of the bytes on disk.
        computed: u32,
    },
    /// A section is missing, duplicated, misplaced, or out of bounds.
    #[error("locate the {section} section: {reason}")]
    Section {
        /// The section's name.
        section: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A record refers to something that doesn't exist.
    #[error("resolve a reference in {table}: {reason}")]
    InvalidReference {
        /// The table that holds the reference.
        table: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The string pool isn't valid UTF-8, or its offsets are out of order.
    #[error("read the string pool: {0}")]
    InvalidStrings(&'static str),
    /// A value in a table is outside its valid range.
    #[error("read a value in {table}: {reason}")]
    InvalidValue {
        /// The table that holds the value.
        table: &'static str,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A date isn't covered by the file.
    #[error("resolve service date {0}, which the file does not cover")]
    DateNotCovered(u32),
    /// `valinor-transit` rejected the data while building the timetable.
    #[error("build the timetable: {0}")]
    Timetable(#[from] BuildError),
    /// The file's stop numbering differs from what the timetable builder assigned.
    #[error("keep stop {stop} at its compiled number (the builder gave it {assigned})")]
    StopRenumbered {
        /// The stop's number in the file.
        stop: u32,
        /// The number the builder gave it.
        assigned: u32,
    },
}

/// Why a file could not be read from disk.
#[derive(Debug, Error)]
pub enum LoadError {
    /// The file could not be read.
    #[error("read the compiled file: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are not a valid compiled file.
    #[error(transparent)]
    Decode(#[from] DecodeError),
}

/// Why a feed could not be compiled.
#[cfg(feature = "compile")]
#[derive(Debug, Error)]
pub enum CompileError {
    /// A file could not be read.
    #[error("read {file}: {source}")]
    Io {
        /// The file in question.
        file: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The feed archive could not be read.
    #[error("read the feed archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A required file is absent from the feed.
    #[error("find required file {0} in the feed")]
    MissingFile(&'static str),
    /// A required column is absent from a file.
    #[error("find required column {column} in {file}")]
    MissingColumn {
        /// The file in question.
        file: &'static str,
        /// The column in question.
        column: &'static str,
    },
    /// The CSV in a file is malformed.
    #[error("parse {file}: {source}")]
    Csv {
        /// The file in question.
        file: &'static str,
        /// The underlying error.
        source: csv::Error,
    },
    /// A feed ID isn't usable as a namespace.
    #[error("use {0:?} as a feed ID")]
    InvalidFeedId(String),
    /// Two feeds have the same ID, so their identifiers would collide.
    #[error("compile two feeds with the same ID {0}")]
    DuplicateFeedId(String),
    /// A feed has no agencies, so it has no time zone.
    #[error("find an agency with a time zone in feed {0}")]
    NoAgency(String),
    /// A feed's agencies have different time zones, which GTFS forbids.
    #[error("compile feed {feed}, whose agencies are in different zones ({first} and {second})")]
    MixedAgencyTimeZones {
        /// The feed in question.
        feed: String,
        /// One zone.
        first: String,
        /// Another zone.
        second: String,
    },
    /// Feeds in one file have different time zones.
    #[error("compile feeds in different time zones into one file ({first} and {second})")]
    MixedFeedTimeZones {
        /// One zone.
        first: String,
        /// Another zone.
        second: String,
    },
    /// A time zone name isn't in the time zone database.
    #[error("resolve time zone {0:?}")]
    UnknownTimeZone(String),
    /// A resolved civil time or date is outside what the file can hold.
    #[error("represent a service date or time: {0}")]
    OutOfRange(String),
    /// `valinor-transit` rejected the data.
    #[error("build the timetable: {0}")]
    Timetable(#[from] BuildError),
    /// A table grew past what its `u32` indices can address.
    #[error("index more than 2^32 entries of {0}")]
    TableTooLarge(&'static str),
}
