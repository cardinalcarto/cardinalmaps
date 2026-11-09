//! Offline stages of the transit pipeline, built to run on a large machine.
//!
//! - [`compile`] turns each fetched GTFS feed into its own compiled timetable.
//! - [`transfers`] computes walking and cycling transfers between every stop of every feed
//!   and writes them tiled by the origin stop's zoom 12 tile.
//! - The [`manifest`] ties them together for clients: feeds, bounds, tiles, and per-feed tile lists.
//!
//! Each stage is a plain command with explicit input and output directories
//! (see the `valinor-transit-build` binary and `docs/transit.md`).

pub mod compile;
pub mod error;
pub mod feeds;
pub mod manifest;
pub mod slippy;
pub mod stops;
pub mod transfers;

#[cfg(test)]
mod pipeline_tests;

use std::fs;
use std::io::Write;
use std::path::Path;

use error::BuildError;

/// Writes `bytes` to `path` under a temporary name, so a reader never sees half a file.
///
/// # Errors
///
/// If the file can't be written or renamed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BuildError> {
    let mut name = path
        .file_name()
        .map_or_else(Default::default, std::ffi::OsStr::to_os_string);
    name.push(".part");
    let partial = path.with_file_name(name);
    let write_error = |source| BuildError::Write {
        path: path.to_path_buf(),
        source,
    };
    let mut file = fs::File::create(&partial).map_err(write_error)?;
    file.write_all(bytes).map_err(write_error)?;
    file.flush().map_err(write_error)?;
    fs::rename(&partial, path).map_err(write_error)
}
