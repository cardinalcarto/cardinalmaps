//! The published identity tables' file formats: what `export` writes and
//! `import`, `patch`, `covers` and `status` read without PostgreSQL. README.md
//! ("Published tables") describes each file.
//!
//! The row files are a zstd frame of LEB128 varints. Rows are in key order,
//! in one section per OSM type; within a section, each row's ID is a delta
//! from the previous row's, and each `poi_id` a zigzag delta from the
//! previous row's, so a planet's mostly sequential identifiers take about a
//! byte. The status file is uncompressed fixed-size records, to be memory
//! mapped.

use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};

use crate::db::Checkpoint;
use crate::identity::{Checksum, PoiChange, Tracked, VersionRow};
use crate::osm::{ElementKey, OsmType};
use crate::rules::Tags;

/// The version of every format here; readers refuse others.
pub const FORMAT: u32 = 1;

pub const VERSIONS_MAGIC: &[u8; 8] = b"CPOIVERS";
pub const POIS_MAGIC: &[u8; 8] = b"CPOIPOIS";
pub const DELTA_MAGIC: &[u8; 8] = b"CPOIDLTA";
pub const STATUS_MAGIC: &[u8; 8] = b"CPOISTAT";

/// zstd's level and worker count. Both are fixed because they change the
/// output, and reruns must give the same bytes to be recognized as published.
const ZSTD_LEVEL: i32 = 15;
const ZSTD_WORKERS: u32 = 8;

/// A zstd-compressing writer for a file being created.
pub fn create_zst(path: &Path) -> Result<zstd::Encoder<'static, BufWriter<std::fs::File>>> {
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = zstd::Encoder::new(BufWriter::with_capacity(1 << 20, file), ZSTD_LEVEL)?;
    encoder.include_checksum(true)?;
    encoder.multithread(ZSTD_WORKERS)?;
    Ok(encoder)
}

/// Finishes a file from [`create_zst`], flushing it to disk.
pub fn finish_zst(encoder: zstd::Encoder<'static, BufWriter<std::fs::File>>) -> Result<()> {
    let file = encoder.finish()?.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(())
}

pub fn open_zst(path: &Path) -> Result<BufReader<zstd::Decoder<'static, BufReader<std::fs::File>>>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(BufReader::with_capacity(1 << 20, zstd::Decoder::new(file)?))
}

pub fn write_varint(out: &mut impl Write, mut value: u64) -> io::Result<()> {
    let mut bytes = [0u8; 10];
    let mut n = 0;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            bytes[n] = byte;
            n += 1;
            break;
        }
        bytes[n] = byte | 0x80;
        n += 1;
    }
    out.write_all(&bytes[..n])
}

pub fn read_varint(input: &mut impl BufRead) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let mut byte = [0u8];
        input.read_exact(&mut byte).context("truncated table")?;
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("varint longer than 64 bits")
}

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn write_bytes(out: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    write_varint(out, bytes.len() as u64)?;
    out.write_all(bytes)
}

fn read_string(input: &mut impl BufRead) -> Result<String> {
    let len = read_varint(input)? as usize;
    ensure!(len < 1 << 24, "string of {len} bytes");
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes).context("truncated table")?;
    String::from_utf8(bytes).context("tag that isn't UTF-8")
}

fn read_array<const N: usize>(input: &mut impl Read) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes).context("truncated table")?;
    Ok(bytes)
}

pub fn read_i64_le(input: &mut impl Read) -> Result<i64> {
    Ok(i64::from_le_bytes(read_array(input)?))
}

fn read_u64(input: &mut impl Read) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(input)?))
}

pub fn write_header(out: &mut impl Write, magic: &[u8; 8]) -> io::Result<()> {
    out.write_all(magic)?;
    out.write_all(&FORMAT.to_le_bytes())
}

pub fn read_header(input: &mut impl Read, magic: &[u8; 8]) -> Result<()> {
    let found: [u8; 8] = read_array(input)?;
    ensure!(&found == magic, "not a {} file", String::from_utf8_lossy(magic));
    let format = u32::from_le_bytes(read_array(input)?);
    ensure!(format == FORMAT, "{} format {format}; this build reads {FORMAT}", String::from_utf8_lossy(magic));
    Ok(())
}

/// Microseconds since the epoch, PostgreSQL's `timestamptz` precision.
pub fn micros(time: DateTime<Utc>) -> i64 {
    time.timestamp_micros()
}

pub fn from_micros(micros: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_micros(micros).context("timestamp out of range")
}

/// Writes rows' keys in key order: one section per type, each ended by a
/// zero, with IDs as deltas (plus one, so a row's code is never zero).
struct KeyWriter {
    /// The type of the open section, as an index; 3 once all are closed.
    section: u8,
    previous_id: i64,
    previous_poi_id: i64,
    previous: Option<(ElementKey, i32)>,
    rows: u64,
}

impl KeyWriter {
    fn new() -> Self {
        KeyWriter { section: 0, previous_id: 0, previous_poi_id: 0, previous: None, rows: 0 }
    }

    /// Writes a row's key and its `poi_id`. Rows must come in increasing
    /// (key, version) order, with versions repeating a key only if
    /// `repeats` (`poi_versions` has one row per version; `pois` one per key).
    fn key(&mut self, out: &mut impl Write, key: ElementKey, version: i32, poi_id: i64, repeats: bool) -> Result<()> {
        let ordered = match self.previous {
            None => true,
            Some((k, v)) if k == key => repeats && version > v,
            Some((k, _)) => k < key,
        };
        ensure!(ordered, "rows out of order: {key:?} v{version} after {:?}", self.previous);
        ensure!(key.osm_id > 0 && version > 0 && poi_id > 0, "{key:?} v{version} poi_id {poi_id} isn't positive");
        self.previous = Some((key, version));
        while self.section < key.osm_type as u8 {
            write_varint(out, 0)?;
            self.section += 1;
            self.previous_id = 0;
        }
        write_varint(out, (key.osm_id - self.previous_id) as u64 + 1)?;
        self.previous_id = key.osm_id;
        write_varint(out, u64::from(version.unsigned_abs()))?;
        write_varint(out, zigzag(poi_id - self.previous_poi_id))?;
        self.previous_poi_id = poi_id;
        self.rows += 1;
        Ok(())
    }

    /// Closes the remaining sections and writes the row count.
    fn finish(&mut self, out: &mut impl Write) -> Result<u64> {
        while self.section < 3 {
            write_varint(out, 0)?;
            self.section += 1;
        }
        out.write_all(&self.rows.to_le_bytes())?;
        Ok(self.rows)
    }
}

/// Reads what [`KeyWriter`] wrote.
struct KeyReader {
    section: u8,
    previous_id: i64,
    previous_poi_id: i64,
    rows: u64,
}

impl KeyReader {
    fn new() -> Self {
        KeyReader { section: 0, previous_id: 0, previous_poi_id: 0, rows: 0 }
    }

    /// The next row's key, version and `poi_id`, or `None` at the end, once
    /// the row count is checked.
    fn key(&mut self, input: &mut impl BufRead) -> Result<Option<(ElementKey, i32, i64)>> {
        loop {
            if self.section == 3 {
                return Ok(None);
            }
            let code = read_varint(input)?;
            if code == 0 {
                self.section += 1;
                self.previous_id = 0;
                if self.section == 3 {
                    let rows = read_u64(input)?;
                    ensure!(rows == self.rows, "the table says it has {rows} rows, but has {}", self.rows);
                    return Ok(None);
                }
                continue;
            }
            let osm_type = OsmType::from_index(self.section).context("bad section")?;
            let osm_id = self.previous_id + (code - 1) as i64;
            self.previous_id = osm_id;
            let version = i32::try_from(read_varint(input)?).context("version out of range")?;
            let poi_id = self.previous_poi_id + unzigzag(read_varint(input)?);
            self.previous_poi_id = poi_id;
            self.rows += 1;
            return Ok(Some((ElementKey { osm_type, osm_id }, version, poi_id)));
        }
    }
}

/// Writes `poi_versions` rows, with the sequence that added each if
/// `base_sequence` is given: 0 for the planet's, else the sequence minus the
/// planet's.
pub struct VersionsWriter {
    keys: KeyWriter,
    base_sequence: Option<i64>,
}

impl VersionsWriter {
    pub fn new(base_sequence: Option<i64>) -> Self {
        VersionsWriter { keys: KeyWriter::new(), base_sequence }
    }

    pub fn row(&mut self, out: &mut impl Write, row: &VersionRow, sequence: Option<i64>) -> Result<()> {
        self.keys.key(out, row.key, row.version, row.poi_id, true)?;
        if let Some(base) = self.base_sequence {
            let code = match sequence {
                None => 0,
                Some(s) if s > base => (s - base) as u64,
                Some(s) => bail!("{:?} v{} is from sequence {s}, not after the planet's {base}", row.key, row.version),
            };
            write_varint(out, code)?;
        }
        Ok(())
    }

    pub fn finish(&mut self, out: &mut impl Write) -> Result<u64> {
        self.keys.finish(out)
    }
}

pub struct VersionsReader {
    keys: KeyReader,
    base_sequence: Option<i64>,
}

impl VersionsReader {
    pub fn new(base_sequence: Option<i64>) -> Self {
        VersionsReader { keys: KeyReader::new(), base_sequence }
    }

    /// The next row, and the sequence that added it (`None` for the planet,
    /// or if the file doesn't say).
    pub fn row(&mut self, input: &mut impl BufRead) -> Result<Option<(VersionRow, Option<i64>)>> {
        let Some((key, version, poi_id)) = self.keys.key(input)? else { return Ok(None) };
        let sequence = match self.base_sequence {
            Some(base) => match read_varint(input)? {
                0 => None,
                code => Some(base + code as i64),
            },
            None => None,
        };
        Ok(Some((VersionRow { key, version, poi_id }, sequence)))
    }
}

/// Writes `pois` rows, with each element's previous identity if
/// `with_previous` (as deltas carry).
pub struct PoisWriter {
    keys: KeyWriter,
    with_previous: bool,
}

impl PoisWriter {
    pub fn new(with_previous: bool) -> Self {
        PoisWriter { keys: KeyWriter::new(), with_previous }
    }

    pub fn row(&mut self, out: &mut impl Write, row: &PoiChange) -> Result<()> {
        let state = &row.state;
        self.keys.key(out, row.key, state.version, state.poi_id, false)?;
        out.write_all(&[u8::from(state.active)])?;
        if self.with_previous {
            write_varint(out, row.previous_poi_id.map_or(0, |p| zigzag(p - state.poi_id) + 1))?;
        }
        write_varint(out, state.tags.len() as u64)?;
        for (k, v) in &state.tags {
            write_bytes(out, k.as_bytes())?;
            write_bytes(out, v.as_bytes())?;
        }
        Ok(())
    }

    pub fn finish(&mut self, out: &mut impl Write) -> Result<u64> {
        self.keys.finish(out)
    }
}

pub struct PoisReader {
    keys: KeyReader,
    with_previous: bool,
}

impl PoisReader {
    pub fn new(with_previous: bool) -> Self {
        PoisReader { keys: KeyReader::new(), with_previous }
    }

    pub fn row(&mut self, input: &mut impl BufRead) -> Result<Option<PoiChange>> {
        let Some((key, version, poi_id)) = self.keys.key(input)? else { return Ok(None) };
        let active = match read_array::<1>(input)?[0] {
            0 => false,
            1 => true,
            other => bail!("active flag {other}"),
        };
        let previous_poi_id = match self.with_previous {
            true => match read_varint(input)? {
                0 => None,
                code => Some(poi_id + unzigzag(code - 1)),
            },
            false => None,
        };
        let count = read_varint(input)?;
        let mut tags = Tags::new();
        for _ in 0..count {
            let k = read_string(input)?;
            let v = read_string(input)?;
            ensure!(tags.last_key_value().is_none_or(|(last, _)| *last < k), "tags of {key:?} out of order");
            tags.insert(k, v);
        }
        Ok(Some(PoiChange { key, state: Tracked { version, poi_id, active, tags }, previous_poi_id }))
    }
}

/// One replication diff's changes: everything `db::store` wrote for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    pub previous: Checkpoint,
    pub checkpoint: Checkpoint,
    pub applied_at: DateTime<Utc>,
    /// New POI versions, in key order.
    pub versions: Vec<VersionRow>,
    /// New `pois` rows, in key order.
    pub pois: Vec<PoiChange>,
}

fn write_checkpoint(out: &mut impl Write, checkpoint: &Checkpoint) -> Result<()> {
    ensure!(checkpoint.checksum.len() == 32, "checksum isn't 32 bytes");
    out.write_all(&checkpoint.sequence.to_le_bytes())?;
    out.write_all(&micros(checkpoint.timestamp).to_le_bytes())?;
    out.write_all(&checkpoint.next_poi_id.to_le_bytes())?;
    out.write_all(&checkpoint.checksum)?;
    Ok(())
}

fn read_checkpoint(input: &mut impl Read) -> Result<Checkpoint> {
    Ok(Checkpoint {
        sequence: read_i64_le(input)?,
        timestamp: from_micros(read_i64_le(input)?)?,
        next_poi_id: read_i64_le(input)?,
        checksum: read_array::<32>(input)?.to_vec(),
    })
}

impl Delta {
    pub fn write(&self, out: &mut impl Write) -> Result<()> {
        write_header(out, DELTA_MAGIC)?;
        write_checkpoint(out, &self.previous)?;
        write_checkpoint(out, &self.checkpoint)?;
        out.write_all(&micros(self.applied_at).to_le_bytes())?;
        let mut versions = VersionsWriter::new(None);
        for row in &self.versions {
            versions.row(out, row, None)?;
        }
        versions.finish(out)?;
        let mut pois = PoisWriter::new(true);
        for row in &self.pois {
            pois.row(out, row)?;
        }
        pois.finish(out)?;
        Ok(())
    }

    pub fn read(input: &mut impl BufRead) -> Result<Delta> {
        read_header(input, DELTA_MAGIC)?;
        let previous = read_checkpoint(input)?;
        let checkpoint = read_checkpoint(input)?;
        let applied_at = from_micros(read_i64_le(input)?)?;
        let mut versions = Vec::new();
        let mut reader = VersionsReader::new(None);
        while let Some((row, _)) = reader.row(input)? {
            versions.push(row);
        }
        let mut pois = Vec::new();
        let mut reader = PoisReader::new(true);
        while let Some(row) = reader.row(input)? {
            pois.push(row);
        }
        ensure!(input.fill_buf()?.is_empty(), "data after the delta");
        Ok(Delta { previous, checkpoint, applied_at, versions, pois })
    }

    /// Checks the delta is one diff's changes, consistent with themselves and
    /// with the checksum and identifier counter it claims:
    /// - the checksum is `SHA-256(previous || rows)`;
    /// - its new identities are exactly those from the previous `next_poi_id`
    ///   up to its own;
    /// - every element with new versions has a new `pois` row, whose identity
    ///   is its last version's, and every POI version either continues the
    ///   element's previous identity or starts a new one.
    pub fn verify(&self) -> Result<()> {
        let (previous, checkpoint) = (&self.previous, &self.checkpoint);
        let sequence = checkpoint.sequence;
        ensure!(sequence == previous.sequence + 1, "delta {sequence} doesn't follow {}", previous.sequence);
        let mut checksum = Checksum::new(&previous.checksum);
        for row in &self.versions {
            checksum.add(row);
        }
        ensure!(checksum.finish() == checkpoint.checksum, "delta {sequence}'s rows don't give its checksum");

        let mut new_ids = 0;
        let mut versions = self.versions.iter().peekable();
        for poi in &self.pois {
            let mut last: Option<&VersionRow> = None;
            let mut identity = poi.previous_poi_id;
            while let Some(row) = versions.next_if(|r| r.key <= poi.key) {
                ensure!(row.key == poi.key, "delta {sequence} has versions of {:?} but no pois row", row.key);
                if row.poi_id >= previous.next_poi_id {
                    if identity != Some(row.poi_id) {
                        ensure!(
                            row.poi_id == previous.next_poi_id + new_ids,
                            "delta {sequence}: {row:?} skips identifiers"
                        );
                        new_ids += 1;
                    }
                } else {
                    ensure!(identity == Some(row.poi_id), "delta {sequence}: {row:?} takes another element's identity");
                }
                identity = Some(row.poi_id);
                last = Some(row);
            }
            let state = &poi.state;
            match last {
                Some(last) => {
                    ensure!(
                        last.poi_id == state.poi_id,
                        "delta {sequence}: {:?}'s identity isn't its last version's",
                        poi.key
                    );
                    ensure!(
                        if state.active { last.version == state.version } else { last.version < state.version },
                        "delta {sequence}: {:?} is at v{} but its last POI version is v{}",
                        poi.key,
                        state.version,
                        last.version
                    );
                }
                None => ensure!(
                    !state.active && poi.previous_poi_id == Some(state.poi_id),
                    "delta {sequence}: {:?} changed without a POI version but isn't one that stopped being a POI",
                    poi.key
                ),
            }
        }
        ensure!(versions.next().is_none(), "delta {sequence} has versions out of order or without pois rows");
        ensure!(
            previous.next_poi_id + new_ids == checkpoint.next_poi_id,
            "delta {sequence} has {new_ids} new identities, but its counter moved {}",
            checkpoint.next_poi_id - previous.next_poi_id
        );
        Ok(())
    }
}

/// A POI identity's state, as the status file records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// No identity has this number.
    Unassigned = 0,
    /// The identity's element is a POI in its latest version.
    Active = 1,
    /// Its element was deleted, or its latest version isn't a POI. It can
    /// become active again.
    Inactive = 2,
    /// Its element has a newer identity: it was reused for another place.
    Replaced = 3,
}

pub const STATUS_HEADER: usize = 128;
pub const STATUS_RECORD: usize = 8;
const OSM_ID_BITS: u32 = 40;
const VERSION_BITS: u32 = 20;

/// Packs a status record: bits 0-1 the state, 2-3 the OSM type, 4-23 the
/// last POI version with this identity, and 24-63 the OSM ID.
pub fn pack_status(key: ElementKey, version: i32, state: State) -> Result<u64> {
    ensure!(
        (1..1 << OSM_ID_BITS).contains(&key.osm_id) && (1..1 << VERSION_BITS).contains(&version),
        "{key:?} v{version} doesn't fit a status record; the format needs a new version"
    );
    Ok(state as u64 | (key.osm_type as u64) << 2 | (version as u64) << 4 | (key.osm_id as u64) << 24)
}

pub fn unpack_status(record: u64) -> (State, Option<(ElementKey, i32)>) {
    let state = match record & 3 {
        0 => return (State::Unassigned, None),
        1 => State::Active,
        2 => State::Inactive,
        _ => State::Replaced,
    };
    let osm_type = OsmType::from_index(((record >> 2) & 3) as u8).unwrap_or(OsmType::Relation);
    let version = ((record >> 4) & ((1 << VERSION_BITS) - 1)) as i32;
    let osm_id = (record >> 24) as i64;
    (state, Some((ElementKey { osm_type, osm_id }, version)))
}

fn with_state(record: u64, state: State) -> u64 {
    (record & !3) | state as u64
}

/// Every identity's state, indexed by `poi_id`, built from the tables.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub records: Vec<u64>,
}

impl Status {
    pub fn new() -> Self {
        Status { records: vec![0] }
    }

    fn record(&mut self, poi_id: i64) -> Result<&mut u64> {
        let i = usize::try_from(poi_id).context("negative poi_id")?;
        if i >= self.records.len() {
            ensure!(i < 1 << 36, "poi_id {poi_id} out of range");
            self.records.resize(i + 1, 0);
        }
        Ok(&mut self.records[i])
    }

    /// A POI version. Versions of one identity must come oldest first.
    pub fn version(&mut self, row: &VersionRow) -> Result<()> {
        // Replaced until a `pois` row says the element still has it.
        *self.record(row.poi_id)? = pack_status(row.key, row.version, State::Replaced)?;
        Ok(())
    }

    /// An element's `pois` row, after its versions.
    pub fn poi(&mut self, row: &PoiChange) -> Result<()> {
        if let Some(previous) = row.previous_poi_id.filter(|&p| p != row.state.poi_id) {
            let record = self.record(previous)?;
            *record = with_state(*record, State::Replaced);
        }
        let record = self.record(row.state.poi_id)?;
        let (_, found) = unpack_status(*record);
        ensure!(
            found.is_some_and(|(key, _)| key == row.key),
            "{:?} has poi_id {} with no POI version of it",
            row.key,
            row.state.poi_id
        );
        *record = with_state(*record, if row.state.active { State::Active } else { State::Inactive });
        Ok(())
    }

    pub fn apply(&mut self, delta: &Delta) -> Result<()> {
        for row in &delta.versions {
            self.version(row)?;
        }
        for row in &delta.pois {
            self.poi(row)?;
        }
        Ok(())
    }

    /// Writes the status file, with records up to `checkpoint.next_poi_id`.
    pub fn write(&self, out: &mut impl Write, checkpoint: &Checkpoint) -> Result<()> {
        let count = usize::try_from(checkpoint.next_poi_id)?;
        ensure!(self.records.len() <= count, "identities beyond next_poi_id {count}");
        let mut header = [0u8; STATUS_HEADER];
        header[..8].copy_from_slice(STATUS_MAGIC);
        header[8..12].copy_from_slice(&FORMAT.to_le_bytes());
        header[12..16].copy_from_slice(&(STATUS_RECORD as u32).to_le_bytes());
        header[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
        header[24..32].copy_from_slice(&checkpoint.timestamp.timestamp().to_le_bytes());
        header[32..40].copy_from_slice(&checkpoint.next_poi_id.to_le_bytes());
        header[40..48].copy_from_slice(&(count as u64).to_le_bytes());
        ensure!(checkpoint.checksum.len() == 32, "checksum isn't 32 bytes");
        header[48..80].copy_from_slice(&checkpoint.checksum);
        out.write_all(&header)?;
        let mut block = Vec::with_capacity(1 << 20);
        for i in 0..count {
            block.extend_from_slice(&self.records.get(i).copied().unwrap_or(0).to_le_bytes());
            if block.len() >= 1 << 20 {
                out.write_all(&block)?;
                block.clear();
            }
        }
        out.write_all(&block)?;
        Ok(())
    }

    /// Reads a status file, returning its records and the checkpoint's
    /// sequence, timestamp (to the second) and next_poi_id.
    pub fn read(input: &mut impl Read) -> Result<(Status, Checkpoint)> {
        let header: [u8; STATUS_HEADER] = read_array(input)?;
        ensure!(&header[..8] == STATUS_MAGIC, "not a status file");
        let field = |range: std::ops::Range<usize>| -> [u8; 8] { header[range].try_into().unwrap() };
        let format = u32::from_le_bytes(header[8..12].try_into()?);
        let record = u32::from_le_bytes(header[12..16].try_into()?);
        ensure!(format == FORMAT && record as usize == STATUS_RECORD, "status format {format}, {record}-byte records");
        let checkpoint = Checkpoint {
            sequence: i64::from_le_bytes(field(16..24)),
            timestamp: DateTime::from_timestamp(i64::from_le_bytes(field(24..32)), 0).context("bad timestamp")?,
            next_poi_id: i64::from_le_bytes(field(32..40)),
            checksum: header[48..80].to_vec(),
        };
        let count = u64::from_le_bytes(field(40..48)) as usize;
        let mut bytes = vec![0u8; count * STATUS_RECORD];
        input.read_exact(&mut bytes).context("truncated status file")?;
        let records = bytes.as_chunks::<8>().0.iter().map(|b| u64::from_le_bytes(*b)).collect();
        Ok((Status { records }, checkpoint))
    }
}

impl Default for Status {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::tags;

    fn key(osm_type: OsmType, osm_id: i64) -> ElementKey {
        ElementKey { osm_type, osm_id }
    }

    fn checkpoint(sequence: i64, next_poi_id: i64, checksum: Vec<u8>) -> Checkpoint {
        Checkpoint {
            sequence,
            timestamp: DateTime::from_timestamp(1_790_000_000 + sequence, 0).unwrap(),
            next_poi_id,
            checksum,
        }
    }

    #[test]
    fn varints_and_zigzag_round_trip() {
        let mut buf = Vec::new();
        let values = [0, 1, 127, 128, 300, u64::MAX];
        for v in values {
            write_varint(&mut buf, v).unwrap();
        }
        let mut input = &buf[..];
        for v in values {
            assert_eq!(read_varint(&mut input).unwrap(), v);
        }
        for v in [0, 1, -1, i64::MAX, i64::MIN] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn versions_round_trip_with_sequences() {
        let rows = [
            (VersionRow { key: key(OsmType::Node, 5), version: 1, poi_id: 1 }, None),
            (VersionRow { key: key(OsmType::Node, 5), version: 3, poi_id: 9 }, Some(5137)),
            (VersionRow { key: key(OsmType::Node, 7), version: 2, poi_id: 2 }, None),
            (VersionRow { key: key(OsmType::Relation, 1), version: 1, poi_id: 3 }, Some(5140)),
        ];
        let mut buf = Vec::new();
        let mut writer = VersionsWriter::new(Some(5136));
        for (row, sequence) in &rows {
            writer.row(&mut buf, row, *sequence).unwrap();
        }
        assert_eq!(writer.finish(&mut buf).unwrap(), 4);
        let mut input = &buf[..];
        let mut reader = VersionsReader::new(Some(5136));
        let mut read = Vec::new();
        while let Some(row) = reader.row(&mut input).unwrap() {
            read.push(row);
        }
        assert_eq!(read, rows);
        assert!(input.is_empty());

        // Out of order, or a repeated version, is refused.
        let mut writer = VersionsWriter::new(None);
        writer.row(&mut Vec::new(), &rows[2].0, None).unwrap();
        assert!(writer.row(&mut Vec::new(), &rows[0].0, None).is_err());
        assert!(writer.row(&mut Vec::new(), &rows[2].0, None).is_err());
    }

    fn change(osm_id: i64, version: i32, poi_id: i64, active: bool, previous: Option<i64>) -> PoiChange {
        PoiChange {
            key: key(OsmType::Node, osm_id),
            state: Tracked { version, poi_id, active, tags: tags([("amenity", "cafe"), ("name", "Café ☕")]) },
            previous_poi_id: previous,
        }
    }

    /// A delta in which node 1 continues identity 4, node 2 is replaced by a
    /// new identity, node 3 stops being a POI, and node 9 is new.
    fn delta() -> Delta {
        let previous = checkpoint(5136, 10, vec![7; 32]);
        let versions = vec![
            VersionRow { key: key(OsmType::Node, 1), version: 2, poi_id: 4 },
            VersionRow { key: key(OsmType::Node, 2), version: 6, poi_id: 10 },
            VersionRow { key: key(OsmType::Node, 9), version: 1, poi_id: 11 },
        ];
        let mut checksum = Checksum::new(&previous.checksum);
        for row in &versions {
            checksum.add(row);
        }
        Delta {
            checkpoint: checkpoint(5137, 12, checksum.finish()),
            previous,
            applied_at: DateTime::from_timestamp_micros(1_790_000_000_123_456).unwrap(),
            versions,
            pois: vec![
                change(1, 2, 4, true, Some(4)),
                change(2, 6, 10, true, Some(5)),
                change(3, 8, 6, false, Some(6)),
                change(9, 1, 11, true, None),
            ],
        }
    }

    #[test]
    fn deltas_round_trip_and_verify() {
        let delta = delta();
        delta.verify().unwrap();
        let mut buf = Vec::new();
        delta.write(&mut buf).unwrap();
        assert_eq!(Delta::read(&mut &buf[..]).unwrap(), delta);

        let mut wrong = delta.clone();
        wrong.versions[2].poi_id = 12;
        assert!(wrong.verify().is_err(), "skipped identifier");
        let mut wrong = delta.clone();
        wrong.checkpoint.next_poi_id = 13;
        assert!(wrong.verify().is_err(), "counter moved too far");
        let mut wrong = delta.clone();
        wrong.pois[0].state.poi_id = 5;
        assert!(wrong.verify().is_err(), "identity isn't the last version's");
        let mut wrong = delta.clone();
        wrong.versions[0].poi_id = 5;
        assert!(wrong.verify().is_err(), "took another identity");
        let mut wrong = delta;
        wrong.pois.remove(3);
        assert!(wrong.verify().is_err(), "versions without a pois row");
    }

    #[test]
    fn status_follows_deltas() {
        let mut status = Status::new();
        for (osm_id, version, poi_id) in [(1, 1, 4), (2, 5, 5), (3, 7, 6)] {
            status.version(&VersionRow { key: key(OsmType::Node, osm_id), version, poi_id }).unwrap();
            status.poi(&change(osm_id, version, poi_id, true, None)).unwrap();
        }
        let delta = delta();
        status.apply(&delta).unwrap();
        let state = |poi_id: usize| unpack_status(status.records[poi_id]);
        assert_eq!(state(4), (State::Active, Some((key(OsmType::Node, 1), 2))));
        assert_eq!(state(5), (State::Replaced, Some((key(OsmType::Node, 2), 5))));
        assert_eq!(state(6), (State::Inactive, Some((key(OsmType::Node, 3), 7))));
        assert_eq!(state(10), (State::Active, Some((key(OsmType::Node, 2), 6))));
        assert_eq!(state(11).0, State::Active);
        assert_eq!(state(1).0, State::Unassigned);

        let mut buf = Vec::new();
        status.write(&mut buf, &delta.checkpoint).unwrap();
        assert_eq!(buf.len(), STATUS_HEADER + 12 * STATUS_RECORD);
        let (read, header) = Status::read(&mut &buf[..]).unwrap();
        assert_eq!(read.records, status.records);
        assert_eq!((header.sequence, header.next_poi_id), (5137, 12));
    }

    #[test]
    fn status_records_pack_their_fields() {
        let k = key(OsmType::Way, 13_000_000_000);
        let record = pack_status(k, 70_000, State::Inactive).unwrap();
        assert_eq!(unpack_status(record), (State::Inactive, Some((k, 70_000))));
        assert!(pack_status(k, 1 << 20, State::Active).is_err());
        assert!(pack_status(key(OsmType::Node, 1 << 40), 1, State::Active).is_err());
    }
}
