//! Releases of the identity tables in a directory laid out as the bucket's
//! `releases/poi/`, as `export` writes it and `fetch` downloads it:
//!
//! ```text
//! SEQ/release.json            every checkpoint; `snapshot` names where a restore starts
//! SEQ/delta.bin.zst           that diff's changes (none for the planet)
//! snapshots/SEQ/release.json
//! snapshots/SEQ/versions.bin.zst, pois.bin.zst, status.bin.zst, checkpoints.json
//! ```
//!
//! A directory is complete once its `release.json` exists; `export` writes
//! everything else first, under a temporary name.

use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{Checkpoint, CheckpointRow, Origin};
use crate::identity::{Checksum, VersionRow};
use crate::tables::{
    self, Delta, POIS_MAGIC, PoisReader, Status, VERSIONS_MAGIC, VersionsReader, open_zst, read_header,
};

pub const RELEASE_FILE: &str = "release.json";
pub const DELTA_FILE: &str = "delta.bin.zst";
pub const VERSIONS_FILE: &str = "versions.bin.zst";
pub const POIS_FILE: &str = "pois.bin.zst";
pub const STATUS_FILE: &str = "status.bin.zst";
pub const CHECKPOINTS_FILE: &str = "checkpoints.json";
pub const SNAPSHOTS: &str = "snapshots";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// The bootstrap's checkpoint, which has no delta.
    Planet,
    Delta,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Previous {
    pub sequence: i64,
    pub next_poi_id: i64,
    pub checksum: String,
}

/// A `release.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub format: u32,
    pub kind: Kind,
    /// The OSM replication sequence of the checkpoint.
    pub sequence: i64,
    /// The checkpoint's replication timestamp.
    pub timestamp: DateTime<Utc>,
    pub next_poi_id: i64,
    /// The checkpoint's running checksum, in hex.
    pub checksum: String,
    /// The checkpoint a delta applies to.
    pub previous: Option<Previous>,
    /// The snapshot a restore up to this checkpoint starts from.
    pub snapshot: i64,
    pub ruleset: i32,
    pub planet_timestamp: DateTime<Utc>,
    pub replication_url: String,
    pub files: BTreeMap<String, FileInfo>,
    /// Rows in each table file.
    pub counts: BTreeMap<String, u64>,
}

impl Release {
    pub fn new(kind: Kind, checkpoint: &Checkpoint, origin: &Origin, snapshot: i64) -> Release {
        Release {
            format: tables::FORMAT,
            kind,
            sequence: checkpoint.sequence,
            timestamp: checkpoint.timestamp,
            next_poi_id: checkpoint.next_poi_id,
            checksum: hex::encode(&checkpoint.checksum),
            previous: None,
            snapshot,
            ruleset: origin.ruleset,
            planet_timestamp: origin.planet_timestamp,
            replication_url: origin.replication_url.clone(),
            files: BTreeMap::new(),
            counts: BTreeMap::new(),
        }
    }

    pub fn checkpoint(&self) -> Result<Checkpoint> {
        Ok(Checkpoint {
            sequence: self.sequence,
            timestamp: self.timestamp,
            next_poi_id: self.next_poi_id,
            checksum: hex::decode(&self.checksum).context("bad checksum")?,
        })
    }

    pub fn origin(&self) -> Origin {
        Origin {
            planet_timestamp: self.planet_timestamp,
            ruleset: self.ruleset,
            replication_url: self.replication_url.clone(),
        }
    }

    pub fn same_origin(&self, other: &Release) -> bool {
        self.origin() == other.origin()
    }
}

/// A `checkpoints.json` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointJson {
    pub sequence: i64,
    pub timestamp: DateTime<Utc>,
    pub next_poi_id: i64,
    pub checksum: String,
    pub applied_at: DateTime<Utc>,
    pub journaled: bool,
}

impl From<&CheckpointRow> for CheckpointJson {
    fn from(row: &CheckpointRow) -> Self {
        let c = &row.checkpoint;
        CheckpointJson {
            sequence: c.sequence,
            timestamp: c.timestamp,
            next_poi_id: c.next_poi_id,
            checksum: hex::encode(&c.checksum),
            applied_at: row.applied_at,
            journaled: row.journaled,
        }
    }
}

impl CheckpointJson {
    pub fn row(&self) -> Result<CheckpointRow> {
        Ok(CheckpointRow {
            checkpoint: Checkpoint {
                sequence: self.sequence,
                timestamp: self.timestamp,
                next_poi_id: self.next_poi_id,
                checksum: hex::decode(&self.checksum)?,
            },
            applied_at: self.applied_at,
            journaled: self.journaled,
        })
    }
}

/// Pretty JSON with a trailing newline, as people read these too.
pub fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn file_info(path: &Path) -> Result<FileInfo> {
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    let mut size = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok(FileInfo { size, sha256: hex::encode(hasher.finalize()) })
}

/// A complete snapshot, its files checked against its `release.json`.
pub struct Snapshot {
    pub release: Release,
    pub dir: PathBuf,
    pub checkpoints: Vec<CheckpointRow>,
}

impl Snapshot {
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn latest(&self) -> &Checkpoint {
        &self.checkpoints.last().expect("a snapshot has checkpoints").checkpoint
    }

    /// Reads every POI version in key order, checking the checksum chain and
    /// identifier counter of every checkpoint (see [`ChainCheck`]).
    pub fn for_each_version(&self, mut f: impl FnMut(&VersionRow, Option<i64>) -> Result<()>) -> Result<()> {
        let mut chain = ChainCheck::new(&self.checkpoints)?;
        let (mut input, mut reader) = self.versions()?;
        while let Some((row, sequence)) = reader.row(&mut input)? {
            chain.add(&row, sequence)?;
            f(&row, sequence)?;
        }
        ensure!(input.fill_buf()?.is_empty(), "data after the versions table");
        chain.finish()
    }

    pub fn versions(&self) -> Result<(impl BufRead, VersionsReader)> {
        let mut input = open_zst(&self.path(VERSIONS_FILE))?;
        read_header(&mut input, VERSIONS_MAGIC)?;
        let base = tables::read_i64_le(&mut input)?;
        ensure!(base == self.checkpoints[0].checkpoint.sequence, "versions table starts at {base}");
        Ok((input, VersionsReader::new(Some(base))))
    }

    pub fn pois(&self) -> Result<(impl BufRead, PoisReader)> {
        let mut input = open_zst(&self.path(POIS_FILE))?;
        read_header(&mut input, POIS_MAGIC)?;
        Ok((input, PoisReader::new(false)))
    }

    /// Reads the snapshot's status file.
    pub fn status(&self) -> Result<Status> {
        let (status, header) = Status::read(&mut open_zst(&self.path(STATUS_FILE))?)?;
        let latest = self.latest();
        ensure!(
            (header.sequence, header.timestamp.timestamp(), header.next_poi_id, &header.checksum)
                == (latest.sequence, latest.timestamp.timestamp(), latest.next_poi_id, &latest.checksum),
            "the status file is for another checkpoint"
        );
        Ok(status)
    }

    /// Checks everything in the snapshot agrees: the versions table against
    /// every checkpoint's checksum and counter, each `pois` row against its
    /// element's versions, and the status file against both. Returns the
    /// status built from the tables.
    pub fn verify(&self) -> Result<Status> {
        let mut chain = ChainCheck::new(&self.checkpoints)?;
        let mut status = Status::new();
        let (mut versions_in, mut versions) = self.versions()?;
        let (mut pois_in, mut pois) = self.pois()?;
        let mut next = versions.row(&mut versions_in)?;
        let mut count = 0u64;
        while let Some(poi) = pois.row(&mut pois_in)? {
            let mut last = None;
            while let Some((row, sequence)) = next.filter(|(row, _)| row.key <= poi.key) {
                ensure!(row.key == poi.key, "{:?} has POI versions but no pois row", row.key);
                chain.add(&row, sequence)?;
                status.version(&row)?;
                last = Some(row);
                next = versions.row(&mut versions_in)?;
            }
            let last = last.with_context(|| format!("{:?} has a pois row but no POI versions", poi.key))?;
            let state = &poi.state;
            ensure!(last.poi_id == state.poi_id, "{:?}'s identity isn't its last POI version's", poi.key);
            ensure!(
                if state.active { last.version == state.version } else { last.version < state.version },
                "{:?} is at v{} but its last POI version is v{}",
                poi.key,
                state.version,
                last.version
            );
            status.poi(&poi)?;
            count += 1;
        }
        ensure!(next.is_none(), "{:?} has POI versions but no pois row", next.map(|(r, _)| r.key));
        ensure!(versions_in.fill_buf()?.is_empty() && pois_in.fill_buf()?.is_empty(), "data after a table");
        chain.finish()?;
        ensure!(Some(&count) == self.release.counts.get("pois"), "the pois table has {count} rows");
        let mut expected = self.status()?;
        status.records.resize(expected.records.len(), 0);
        if status != expected {
            let i = (0..status.records.len()).find(|&i| status.records[i] != expected.records[i]);
            expected.records.clear();
            bail!("the status file differs from the tables, first at poi_id {i:?}");
        }
        Ok(status)
    }
}

/// Checks a snapshot's versions table, read in key order, against its
/// checkpoints: each checkpoint's checksum is `SHA-256(the previous one ||
/// its rows in key order)`, and each identity first appears in the
/// checkpoint whose counter range holds it. Rows from one diff are in key
/// order within the whole table too, so this needs one hasher per
/// checkpoint, seeded with the previous one's claimed checksum.
pub struct ChainCheck<'a> {
    checkpoints: &'a [CheckpointRow],
    hashers: Vec<Checksum>,
    seen: Vec<u64>,
    seen_count: u64,
}

impl<'a> ChainCheck<'a> {
    pub fn new(checkpoints: &'a [CheckpointRow]) -> Result<Self> {
        ensure!(!checkpoints.is_empty(), "no checkpoints");
        let base = checkpoints[0].checkpoint.sequence;
        for (i, row) in checkpoints.iter().enumerate() {
            ensure!(row.checkpoint.sequence == base + i as i64, "checkpoints aren't consecutive at {i}");
        }
        let hashers = checkpoints
            .iter()
            .enumerate()
            .map(|(i, _)| Checksum::new(if i == 0 { &[0; 32] } else { &checkpoints[i - 1].checkpoint.checksum }))
            .collect();
        let next = checkpoints.last().unwrap().checkpoint.next_poi_id;
        Ok(ChainCheck { checkpoints, hashers, seen: vec![0; (next as usize).div_ceil(64)], seen_count: 0 })
    }

    pub fn add(&mut self, row: &VersionRow, sequence: Option<i64>) -> Result<()> {
        let base = self.checkpoints[0].checkpoint.sequence;
        let i = match sequence {
            None => 0,
            Some(s) => usize::try_from(s - base)
                .ok()
                .filter(|&i| i > 0 && i < self.checkpoints.len())
                .with_context(|| format!("{row:?} is from sequence {s}, which has no checkpoint"))?,
        };
        self.hashers[i].add(row);
        let id = usize::try_from(row.poi_id)?;
        let (word, bit) = (id / 64, 1u64 << (id % 64));
        ensure!(word < self.seen.len(), "{row:?} is beyond next_poi_id");
        if self.seen[word] & bit == 0 {
            let low = if i == 0 { 1 } else { self.checkpoints[i - 1].checkpoint.next_poi_id };
            let high = self.checkpoints[i].checkpoint.next_poi_id;
            ensure!((low..high).contains(&row.poi_id), "{row:?} starts an identity outside {low}..{high}");
            self.seen[word] |= bit;
            self.seen_count += 1;
        }
        Ok(())
    }

    pub fn finish(self) -> Result<()> {
        for (hasher, row) in self.hashers.into_iter().zip(self.checkpoints) {
            ensure!(
                hasher.finish() == row.checkpoint.checksum,
                "checkpoint {}'s checksum doesn't match its rows",
                row.checkpoint.sequence
            );
        }
        let next = self.checkpoints.last().unwrap().checkpoint.next_poi_id;
        ensure!(self.seen_count == (next - 1) as u64, "{} identities, but next_poi_id is {next}", self.seen_count);
        Ok(())
    }
}

/// A directory of releases.
pub struct Releases {
    pub root: PathBuf,
}

/// The releases to apply to reach a checkpoint: a snapshot (unless starting
/// from a database), then each delta after it, in order.
pub struct Chain {
    /// The snapshot's `release.json`; [`Releases::snapshot`] opens it.
    pub snapshot: Option<Release>,
    pub dailies: Vec<Release>,
    /// The checkpoint reached.
    pub target: Checkpoint,
}

impl Releases {
    pub fn new(root: &Path) -> Releases {
        Releases { root: root.to_owned() }
    }

    pub fn daily_dir(&self, sequence: i64) -> PathBuf {
        self.root.join(sequence.to_string())
    }

    pub fn snapshot_dir(&self, sequence: i64) -> PathBuf {
        self.root.join(SNAPSHOTS).join(sequence.to_string())
    }

    fn read_release(dir: &Path) -> Result<Option<Release>> {
        let path = dir.join(RELEASE_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let release: Release =
                    serde_json::from_slice(&bytes).with_context(|| format!("reading {}", path.display()))?;
                ensure!(release.format == tables::FORMAT, "{} is format {}", path.display(), release.format);
                Ok(Some(release))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn daily(&self, sequence: i64) -> Result<Option<Release>> {
        let release = Self::read_release(&self.daily_dir(sequence))?;
        ensure!(
            release.as_ref().is_none_or(|r| r.sequence == sequence && r.kind != Kind::Snapshot),
            "bad release {sequence}"
        );
        Ok(release)
    }

    /// Sequences of the complete releases in `dir`, in order.
    fn sequences(dir: &Path) -> Result<Vec<i64>> {
        let mut found = Vec::new();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(found),
            Err(e) => return Err(e).with_context(|| format!("listing {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            if let Some(sequence) = entry.file_name().to_str().and_then(|n| n.parse::<i64>().ok())
                && entry.path().join(RELEASE_FILE).is_file()
            {
                found.push(sequence);
            }
        }
        found.sort_unstable();
        Ok(found)
    }

    pub fn dailies(&self) -> Result<Vec<i64>> {
        Self::sequences(&self.root)
    }

    pub fn snapshots(&self) -> Result<Vec<i64>> {
        Self::sequences(&self.root.join(SNAPSHOTS))
    }

    pub fn snapshot_release(&self, sequence: i64) -> Result<Option<Release>> {
        let release = Self::read_release(&self.snapshot_dir(sequence))?;
        ensure!(
            release.as_ref().is_none_or(|r| r.sequence == sequence && r.kind == Kind::Snapshot),
            "bad snapshot {sequence}"
        );
        Ok(release)
    }

    /// Opens a snapshot, checking its files' sizes and SHA-256s.
    pub fn snapshot(&self, sequence: i64) -> Result<Snapshot> {
        let dir = self.snapshot_dir(sequence);
        let release = self
            .snapshot_release(sequence)?
            .with_context(|| format!("no snapshot {sequence} in {}", self.root.display()))?;
        for name in [VERSIONS_FILE, POIS_FILE, STATUS_FILE, CHECKPOINTS_FILE] {
            check_file(&dir, &release, name)?;
        }
        let checkpoints: Vec<CheckpointJson> = serde_json::from_slice(&std::fs::read(dir.join(CHECKPOINTS_FILE))?)?;
        let checkpoints = checkpoints.iter().map(CheckpointJson::row).collect::<Result<Vec<_>>>()?;
        let latest = checkpoints.last().context("a snapshot without checkpoints")?;
        ensure!(latest.checkpoint == release.checkpoint()?, "snapshot {sequence}'s checkpoints end elsewhere");
        Ok(Snapshot { release, dir, checkpoints })
    }

    /// Reads a delta, checking it against its `release.json` and itself.
    pub fn delta(&self, release: &Release) -> Result<Delta> {
        let dir = self.daily_dir(release.sequence);
        check_file(&dir, release, DELTA_FILE)?;
        let delta = Delta::read(&mut open_zst(&dir.join(DELTA_FILE))?)
            .with_context(|| format!("reading delta {}", release.sequence))?;
        let previous = release.previous.as_ref().context("a delta without a previous checkpoint")?;
        ensure!(
            delta.checkpoint == release.checkpoint()?
                && (delta.previous.sequence, delta.previous.next_poi_id, hex::encode(&delta.previous.checksum))
                    == (previous.sequence, previous.next_poi_id, previous.checksum.clone()),
            "delta {} differs from its release.json",
            release.sequence
        );
        ensure!(
            release.counts.get("versions") == Some(&(delta.versions.len() as u64))
                && release.counts.get("pois") == Some(&(delta.pois.len() as u64)),
            "delta {}'s row counts differ from its release.json",
            release.sequence
        );
        delta.verify()?;
        Ok(delta)
    }

    /// The releases that reach checkpoint `to` (default: the latest), from
    /// `from` if given (a database's latest checkpoint), else from the
    /// snapshot the target's release names. Checks every link of the chain.
    pub fn chain(&self, to: Option<i64>, from: Option<&Checkpoint>) -> Result<Chain> {
        let to = match to {
            Some(to) => to,
            None => *self.dailies()?.last().with_context(|| format!("no releases in {}", self.root.display()))?,
        };
        let target = self.daily(to)?.with_context(|| format!("no release {to} in {}", self.root.display()))?;
        let (snapshot, mut previous) = match from {
            Some(from) => {
                ensure!(from.sequence <= to, "the database is at {}, after {to}", from.sequence);
                (None, from.clone())
            }
            None => {
                let snapshot = self
                    .snapshot_release(target.snapshot)?
                    .with_context(|| format!("no snapshot {} in {}", target.snapshot, self.root.display()))?;
                ensure!(snapshot.same_origin(&target), "snapshot {} has another origin", target.snapshot);
                let latest = snapshot.checkpoint()?;
                (Some(snapshot), latest)
            }
        };
        let mut dailies = Vec::new();
        for sequence in previous.sequence + 1..=to {
            let release =
                self.daily(sequence)?.with_context(|| format!("no release {sequence} in {}", self.root.display()))?;
            ensure!(release.same_origin(&target), "release {sequence} has another origin");
            let link = release.previous.as_ref().context("a release after the first without a previous checkpoint")?;
            ensure!(
                link.sequence == previous.sequence
                    && link.next_poi_id == previous.next_poi_id
                    && link.checksum == hex::encode(&previous.checksum),
                "release {sequence} doesn't follow checkpoint {} (checksum {})",
                previous.sequence,
                hex::encode(&previous.checksum)
            );
            previous = release.checkpoint()?;
            dailies.push(release);
        }
        if let Some(from) = from
            && from.sequence == to
        {
            ensure!(from == &target.checkpoint()?, "the database's checkpoint {to} differs from the release's");
        }
        ensure!(previous == target.checkpoint()?, "release {to} differs from the chain that reaches it");
        Ok(Chain { snapshot, dailies, target: previous })
    }
}

fn check_file(dir: &Path, release: &Release, name: &str) -> Result<()> {
    let expected = release.files.get(name).with_context(|| format!("{} doesn't list {name}", dir.display()))?;
    let found = file_info(&dir.join(name))?;
    ensure!(&found == expected, "{}/{name} is {found:?}, but its release says {expected:?}", dir.display());
    Ok(())
}

/// Writes `bytes` to `path` atomically.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let partial = path.with_extension("partial");
    std::fs::write(&partial, bytes)?;
    std::fs::File::open(&partial)?.sync_all()?;
    std::fs::rename(&partial, path)?;
    Ok(())
}
