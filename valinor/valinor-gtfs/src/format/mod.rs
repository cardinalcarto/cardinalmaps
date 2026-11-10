//! The compiled timetable file.
//!
//! One file holds one or more feeds. It is a header, a section directory, and then flat tables
//! of fixed-size little-endian records, plus a pool of UTF-8 strings.
//! Records have alignment one, so a table is a slice cast directly over the file's bytes:
//! nothing is parsed or copied, and a memory map works on any device.
//!
//! The file stores service-day-relative times and calendars,
//! with the UTC offsets that each service day needs already resolved,
//! so that a device needs no time zone database.
//!
//! The on-disk records live in [`layout`], and [`writer`] and [`reader`] are their two sides.

pub(crate) mod layout;
pub(crate) mod reader;
pub(crate) mod set;
pub(crate) mod timetable;
pub(crate) mod validate;
#[cfg(feature = "compile")]
pub(crate) mod writer;

pub use layout::{DEFAULT_TRANSFER_SECONDS, FORMAT_VERSION, MAGIC};
