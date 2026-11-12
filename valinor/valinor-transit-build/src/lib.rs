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

/// Writes `bytes` to `path` under a temporary name, then renames it into place durably,
/// so a reader never sees half a file, even after a power loss.
///
/// # Errors
///
/// If the file can't be written, synced, or renamed.
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
    write_synced(&partial, bytes).map_err(write_error)?;
    fs::rename(&partial, path).map_err(write_error)?;
    if let Some(parent) = path.parent() {
        sync_dir(parent)?;
    }
    Ok(())
}

/// Writes a whole file and flushes it to the disk.
///
/// # Errors
///
/// If it can't be created, written, or synced.
pub(crate) fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flushes a directory's entries to the disk, so a rename or new file in it survives a power loss.
///
/// # Errors
///
/// If it can't be opened or synced.
pub(crate) fn sync_dir(path: &Path) -> Result<(), BuildError> {
    let sync_error = |source| BuildError::Write {
        path: path.to_path_buf(),
        source,
    };
    fs::File::open(path)
        .and_then(|dir| dir.sync_all())
        .map_err(sync_error)
}
