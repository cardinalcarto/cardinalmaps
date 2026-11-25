//! What became of `poi:` IDs, from the identity status file that
//! `poi_service status` writes (format: `poi_service/README.md`, "Status
//! file").
//!
//! The file is memory-mapped. It's replaced by renaming a new file over it;
//! lookups notice the new file (a different inode) within
//! [`PoiStatus::check_interval`] and map it, without a restart. Writing into
//! the mapped file instead of replacing it would change what readers see
//! underneath them, so it must never be done.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use log::{info, warn};
use memmap2::Mmap;

const MAGIC: &[u8; 8] = b"CPOISTAT";
const FORMAT: u32 = 1;
const HEADER: usize = 128;
const RECORD: usize = 8;

/// What a `poi:` ID's identity is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Its element is a POI in its latest version.
    Active,
    /// Its element was deleted, or its latest version isn't a POI.
    Inactive,
    /// Its element has a newer identity: it's another place now.
    Replaced,
}

impl State {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            State::Active => "active",
            State::Inactive => "inactive",
            State::Replaced => "replaced",
        }
    }
}

/// The checkpoint a status file describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsOf {
    /// The OSM replication sequence.
    pub sequence: i64,
    /// Its replication timestamp, in seconds since the epoch.
    pub timestamp: i64,
}

/// A `poi:` ID's identity, as of a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No identity had this number yet.
    Unknown,
    Known {
        state: State,
        /// `node`, `way` or `relation`.
        osm_type: &'static str,
        osm_id: i64,
        /// The last OSM version that had this identity.
        version: u32,
    },
}

struct Mapped {
    map: Mmap,
    /// The file's device and inode, to notice it being replaced.
    file: (u64, u64),
    as_of: AsOf,
    records: u64,
}

impl Mapped {
    fn open(path: &Path) -> Result<Mapped> {
        let file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let meta = file.metadata()?;
        // SAFETY: the file is only ever replaced by a rename, never written
        // in place (see the module docs), so the mapping doesn't change.
        #[allow(unsafe_code)]
        let map = unsafe { Mmap::map(&file)? };
        ensure!(
            map.len() >= HEADER && &map[..8] == MAGIC,
            "{} isn't a POI status file",
            path.display()
        );
        let u32_at = |i: usize| u32::from_le_bytes(map[i..i + 4].try_into().unwrap());
        let i64_at = |i: usize| i64::from_le_bytes(map[i..i + 8].try_into().unwrap());
        ensure!(
            u32_at(8) == FORMAT && u32_at(12) as usize == RECORD,
            "{} is status format {}, with {}-byte records; this build reads format {FORMAT}",
            path.display(),
            u32_at(8),
            u32_at(12)
        );
        let records = i64_at(40).cast_unsigned();
        ensure!(
            usize::try_from(records)
                .ok()
                .and_then(|n| n.checked_mul(RECORD))
                .is_some_and(|n| n + HEADER == map.len()),
            "{} is {} bytes, but says it has {records} records",
            path.display(),
            map.len()
        );
        Ok(Mapped {
            file: (meta.dev(), meta.ino()),
            as_of: AsOf {
                sequence: i64_at(16),
                timestamp: i64_at(24),
            },
            records,
            map,
        })
    }

    fn get(&self, poi_id: u64) -> Status {
        if poi_id >= self.records {
            return Status::Unknown;
        }
        let i = HEADER + usize::try_from(poi_id).expect("checked against the file's size") * RECORD;
        let record = u64::from_le_bytes(self.map[i..i + RECORD].try_into().unwrap());
        let state = match record & 3 {
            0 => return Status::Unknown,
            1 => State::Active,
            2 => State::Inactive,
            _ => State::Replaced,
        };
        let osm_type = match (record >> 2) & 3 {
            0 => "node",
            1 => "way",
            _ => "relation",
        };
        #[allow(clippy::cast_possible_truncation)]
        let version = ((record >> 4) & ((1 << 20) - 1)) as u32;
        Status::Known {
            state,
            osm_type,
            osm_id: (record >> 24).cast_signed(),
            version,
        }
    }
}

/// The status file, reloaded when it's replaced.
pub struct PoiStatus {
    path: PathBuf,
    current: RwLock<Arc<Mapped>>,
    last_check: Mutex<Instant>,
    /// How often lookups check whether the file was replaced.
    pub check_interval: Duration,
}

impl PoiStatus {
    /// Maps the status file at `path`.
    ///
    /// # Errors
    /// If it can't be read or isn't a status file of this format.
    pub fn open(path: &Path) -> Result<PoiStatus> {
        let mapped = Mapped::open(path)?;
        info!(
            "POI status from {}: {} identities as of sequence {}",
            path.display(),
            mapped.records.saturating_sub(1),
            mapped.as_of.sequence
        );
        Ok(PoiStatus {
            path: path.to_owned(),
            current: RwLock::new(Arc::new(mapped)),
            last_check: Mutex::new(Instant::now()),
            check_interval: Duration::from_secs(5),
        })
    }

    /// Maps the file again if it was replaced since it was last mapped. A
    /// file that can't be read is logged and the old one kept.
    ///
    /// # Panics
    /// If another thread panicked holding the lock.
    pub fn reload_if_replaced(&self) {
        let current = self.current.read().unwrap().file;
        let replaced = match std::fs::metadata(&self.path) {
            Ok(meta) => (meta.dev(), meta.ino()) != current,
            Err(e) => {
                warn!(
                    "Can't read {}: {e}; keeping the POI status already loaded",
                    self.path.display()
                );
                false
            }
        };
        if !replaced {
            return;
        }
        match Mapped::open(&self.path) {
            Ok(mapped) => {
                info!(
                    "Reloaded POI status from {}: as of sequence {}",
                    self.path.display(),
                    mapped.as_of.sequence
                );
                *self.current.write().unwrap() = Arc::new(mapped);
            }
            Err(e) => warn!(
                "Can't load {}: {e:#}; keeping the POI status already loaded",
                self.path.display()
            ),
        }
    }

    /// The status of `poi:<poi_id>`, and the checkpoint it's as of.
    ///
    /// # Panics
    /// If another thread panicked holding a lock.
    #[must_use]
    pub fn lookup(&self, poi_id: u64) -> (Status, AsOf) {
        let check = {
            let mut last = self.last_check.lock().unwrap();
            let due = last.elapsed() >= self.check_interval;
            if due {
                *last = Instant::now();
            }
            due
        };
        if check {
            self.reload_if_replaced();
        }
        let mapped = self.current.read().unwrap().clone();
        (mapped.get(poi_id), mapped.as_of)
    }
}

/// Seconds since the epoch as RFC 3339, in UTC.
#[must_use]
pub fn rfc3339(seconds: i64) -> String {
    let (days, secs) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_158_403), "2026-10-05T00:00:03Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }
}
