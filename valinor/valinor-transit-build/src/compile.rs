//! The compile stage: every feed to its own timetable file.
//!
//! Each feed is compiled on its own by [`valinor_gtfs::compile`] and writes to
//! `timetables/<id>.timetable`, on as many threads as asked for. A feed that can't be compiled,
//! for any reason, even a panic, is recorded with its error and doesn't stop the others.
//!
//! A run is resumable: `compile-results/<id>.json` records the outcome of the feed's last run,
//! and a feed whose archive is unchanged (by SHA-256) isn't recompiled, as long as its timetable
//! is still on disk. A previous failure on unchanged input is kept too, unless
//! [`CompileOptions::retry_failed`] asks otherwise.

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use valinor_gtfs::{
    CompileOptions as GtfsCompileOptions, CompiledTimetable, FORMAT_VERSION, FeedId, FeedInput,
    ZipSource, civil_date_of, compile_with,
};

use crate::error::BuildError;
use crate::feeds::{FeedEntry, FeedsFile, archive_path, sha256_of_file};
use crate::write_atomic;

/// The compile stage's report, read by the transfer stage.
pub const COMPILE_REPORT: &str = "compile.json";
/// Where timetables go, relative to the output directory.
pub const TIMETABLES_DIR: &str = "timetables";
/// Where each feed's outcome goes as soon as it finishes, for resuming.
pub const RESULTS_DIR: &str = "compile-results";

/// How a feed came out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum FeedOutcome {
    /// The feed compiled.
    Compiled,
    /// The feed couldn't be compiled.
    Failed {
        /// Why not.
        error: String,
    },
}

/// What stay-seated merging did to one feed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatedOutcome {
    /// Combined trips written.
    pub merged: usize,
    /// Trips that only run as part of a combined trip.
    pub absorbed: usize,
    /// Trips with some days merged and some alone.
    pub partial: usize,
    /// Stay-seated links that passed every check.
    pub links_valid: usize,
    /// Chains cut short by the chain limit.
    pub chains_cut: usize,
    /// `transfer_type` 4 or 5 rows that named a trip this feed doesn't have.
    pub rows_unmatched: usize,
    /// Valid `transfer_type=4` rows dropped because their trip already had another row's link.
    #[serde(default)]
    pub rows_conflicting: usize,
    /// Patterns: trips with the same route, stop sequence, and attributes.
    pub patterns: usize,
}

/// One feed's result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeedRecord {
    /// The feed's ID.
    pub id: String,
    /// The feed's Transitous region.
    pub region: String,
    /// The feed's name.
    pub name: String,
    /// The SHA-256 of the archive as read, empty if it couldn't be read.
    pub sha256: String,
    /// The size of the archive as read.
    pub bytes: u64,
    /// The feed's license, verbatim.
    pub license: Box<RawValue>,
    /// Where the feed came from, verbatim.
    pub source: Box<RawValue>,
    /// What happened.
    pub outcome: FeedOutcome,
    /// The compiled file's format version.
    pub format_version: u32,
    /// The timetable, relative to the output directory, if compiled.
    pub timetable: Option<String>,
    /// The timetable's size.
    pub timetable_bytes: u64,
    /// The SHA-256 of the timetable.
    pub timetable_sha256: Option<String>,
    /// Stops in the compiled timetable.
    pub stops: u32,
    /// Routes kept.
    pub routes: usize,
    /// Trips kept, before frequency expansion.
    pub trips: usize,
    /// Stop events, after expansion.
    pub stop_events: usize,
    /// The first date the feed's calendars run on.
    pub first_service_date: Option<String>,
    /// The last date the feed's calendars run on.
    pub last_service_date: Option<String>,
    /// Whether the feed runs on or after the date it was fetched.
    pub has_current_service: bool,
    /// Trips the compiler dropped, by reason.
    pub dropped_trips: BTreeMap<String, usize>,
    /// How many warnings the compiler raised.
    pub warning_count: usize,
    /// Stops where `transfer_type=3` forbids changing at the stop.
    #[serde(default)]
    pub transfers_forbidden_same_stop: usize,
    /// Directed stop pairs whose walk `transfer_type=3` forbids.
    #[serde(default)]
    pub transfers_forbidden_pairs: usize,
    /// `transfer_type=3` rows qualified by route or trip, counted because they can't be honored.
    #[serde(default)]
    pub transfers_forbidden_qualified: usize,
    /// Trip rows after stay-seated merging, and the patterns they fall in.
    #[serde(default)]
    pub seated: Option<SeatedOutcome>,
    /// How long this feed took to compile, in seconds.
    pub seconds: f64,
    /// Whether this run reused a previous result without compiling.
    #[serde(default)]
    pub reused: bool,
    /// Every compile setting the output depends on, so a change in any of them forces a recompile.
    ///
    /// `None` from an earlier version of this tool; such a record is never reused.
    #[serde(default)]
    pub settings: Option<CompileSettings>,
}

/// The compile settings a timetable depends on, for deciding whether an old one can be reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompileSettings {
    /// See [`valinor_gtfs::CompileOptions::seated_wait_seconds`].
    pub seated_wait_seconds: i32,
    /// See [`valinor_gtfs::CompileOptions::max_seated_chain`].
    pub max_seated_chain: usize,
    /// See [`valinor_gtfs::CompileOptions::merge_seated_trips`].
    pub merge_seated_trips: bool,
}

impl From<GtfsCompileOptions> for CompileSettings {
    fn from(options: GtfsCompileOptions) -> Self {
        Self {
            seated_wait_seconds: options.seated_wait_seconds,
            max_seated_chain: options.max_seated_chain,
            merge_seated_trips: options.merge_seated_trips,
        }
    }
}

/// The whole stage's report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompileReport {
    /// The Transitous commit the feeds came from.
    pub transitous_commit: String,
    /// When they were fetched.
    pub fetched_at: String,
    /// The compiled file's format version.
    pub format_version: u32,
    /// Every feed, sorted by ID.
    pub feeds: Vec<FeedRecord>,
    /// Feeds the fetch stage skipped, as it wrote them.
    pub skipped: Vec<Box<RawValue>>,
    /// Realtime endpoints, as the fetch stage wrote them.
    pub realtime: Vec<Box<RawValue>>,
}

impl CompileReport {
    /// Reads the report in an output directory.
    ///
    /// # Errors
    ///
    /// If it can't be read or parsed.
    pub fn read(dir: &Path) -> Result<Self, BuildError> {
        let path = dir.join(COMPILE_REPORT);
        let bytes = fs::read(&path).map_err(|source| BuildError::Read {
            path: path.clone(),
            source,
        })?;
        serde_json::from_slice(&bytes).map_err(|source| BuildError::Json { path, source })
    }
}

/// How to run the compile stage.
#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// The fetch stage's output: `feeds.json` and `feeds/`.
    pub input: PathBuf,
    /// Where to write timetables, results, and the report.
    pub output: PathBuf,
    /// How many feeds to compile at once.
    pub threads: NonZeroUsize,
    /// Recompile a feed that failed last time, even if its archive is unchanged.
    pub retry_failed: bool,
    /// The longest wait two trips of one `block_id` may be merged across, in seconds.
    pub seated_wait_seconds: i32,
}

/// Runs the compile stage.
///
/// # Errors
///
/// Only for failures that aren't about one feed: an unreadable `feeds.json`, or an output
/// directory that can't be written. Everything specific to a feed is recorded on the feed.
pub fn run(options: &CompileOptions) -> Result<CompileReport, BuildError> {
    let mut input = FeedsFile::read(&options.input)?;
    input.feeds.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    for dir in [TIMETABLES_DIR, RESULTS_DIR] {
        let path = options.output.join(dir);
        fs::create_dir_all(&path).map_err(|source| BuildError::Write { path, source })?;
    }
    let next = AtomicUsize::new(0);
    let slots: Mutex<Vec<Option<FeedRecord>>> = Mutex::new(vec![None; input.feeds.len()]);
    let first_error: Mutex<Option<BuildError>> = Mutex::new(None);
    let fetched_date = input
        .fetched_at
        .get(..10)
        .unwrap_or("0000-00-00")
        .to_owned();
    std::thread::scope(|scope| {
        for _ in 0..options.threads.get().min(input.feeds.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(feed) = input.feeds.get(index) else {
                        break;
                    };
                    match compile_feed(options, feed, &fetched_date) {
                        Ok(record) => {
                            if let Ok(mut slots) = slots.lock() {
                                slots[index] = Some(record);
                            }
                        }
                        Err(error) => {
                            if let Ok(mut first) = first_error.lock() {
                                first.get_or_insert(error);
                            }
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = first_error.into_inner().ok().flatten() {
        return Err(error);
    }
    let feeds: Vec<FeedRecord> = slots
        .into_inner()
        .map_err(|_| BuildError::WorkerPanicked)?
        .into_iter()
        .collect::<Option<_>>()
        .ok_or(BuildError::WorkerPanicked)?;
    let report = CompileReport {
        transitous_commit: input.transitous_commit,
        fetched_at: input.fetched_at,
        format_version: FORMAT_VERSION,
        feeds,
        skipped: input.skipped,
        realtime: input.realtime,
    };
    let path = options.output.join(COMPILE_REPORT);
    let bytes = serde_json::to_vec_pretty(&report).map_err(|source| BuildError::Json {
        path: path.clone(),
        source,
    })?;
    write_atomic(&path, &bytes)?;
    Ok(report)
}

fn compile_feed(
    options: &CompileOptions,
    feed: &FeedEntry,
    fetched_date: &str,
) -> Result<FeedRecord, BuildError> {
    let started = Instant::now();
    let archive = archive_path(&options.input, feed)?;
    let (sha256, bytes) = match sha256_of_file(&archive) {
        Ok(value) => value,
        Err(error) => return finish_failed(options, feed, error.to_string(), None, started),
    };
    if sha256 != feed.sha256 {
        let error = format!(
            "verify the archive: feeds.json says sha256 {}, the file is {sha256}",
            feed.sha256
        );
        return finish_failed(options, feed, error, Some((sha256, bytes)), started);
    }
    if let Some(mut record) = reusable(options, feed, &sha256, fetched_date) {
        record.reused = true;
        return Ok(record);
    }
    match compile_archive(options, feed, &archive, bytes, fetched_date, started) {
        Ok(record) => {
            write_sidecar(options, &record)?;
            Ok(record)
        }
        Err(Failed::Feed(error)) => {
            finish_failed(options, feed, error, Some((sha256, bytes)), started)
        }
        Err(Failed::Stage(error)) => Err(error),
    }
}

/// The GTFS compiler's options for this run; everything not set here is the compiler's default.
fn gtfs_options_of(options: &CompileOptions) -> GtfsCompileOptions {
    GtfsCompileOptions {
        seated_wait_seconds: options.seated_wait_seconds,
        ..GtfsCompileOptions::default()
    }
}

/// What this run's output depends on besides the archive and the compiler itself.
fn settings_of(options: &CompileOptions) -> CompileSettings {
    gtfs_options_of(options).into()
}

/// What a previous run left for this feed, if it can stand in for compiling.
fn reusable(
    options: &CompileOptions,
    feed: &FeedEntry,
    sha256: &str,
    fetched_date: &str,
) -> Option<FeedRecord> {
    let bytes = fs::read(sidecar_path(options, &feed.id)).ok()?;
    let mut record: FeedRecord = serde_json::from_slice(&bytes).ok()?;
    if record.sha256 != sha256
        || record.format_version != FORMAT_VERSION
        || record.settings != Some(settings_of(options))
    {
        return None;
    }
    match &record.outcome {
        FeedOutcome::Failed { .. } => (!options.retry_failed).then_some(())?,
        FeedOutcome::Compiled => {
            // A timetable that was deleted or truncated can't be reused.
            let path = options.output.join(record.timetable.as_ref()?);
            let (timetable_sha, timetable_bytes) = sha256_of_file(&path).ok()?;
            if Some(timetable_sha) != record.timetable_sha256
                || timetable_bytes != record.timetable_bytes
            {
                return None;
            }
        }
    }
    // Unchanged compiled output, with everything else about it from this run's listing.
    record.region.clone_from(&feed.region);
    record.name.clone_from(&feed.name);
    record.license = feed.license.clone();
    record.source = feed.source.clone();
    record.has_current_service = has_service(record.last_service_date.as_deref(), fetched_date);
    Some(record)
}

fn has_service(last_service_date: Option<&str>, fetched_date: &str) -> bool {
    last_service_date.is_some_and(|last| last >= fetched_date)
}

enum Failed {
    Feed(String),
    Stage(BuildError),
}

fn compile_archive(
    options: &CompileOptions,
    feed: &FeedEntry,
    archive: &Path,
    bytes: u64,
    fetched_date: &str,
    started: Instant,
) -> Result<FeedRecord, Failed> {
    let feed_id = FeedId::try_new(feed.id.clone())
        .map_err(|error| Failed::Feed(format!("use the feed ID: {error}")))?;
    let source = ZipSource::from_path(archive).map_err(|e| Failed::Feed(e.to_string()))?;
    let input = vec![FeedInput {
        id: feed_id,
        source: Box::new(source),
    }];
    let gtfs_options = gtfs_options_of(options);
    let compiled = panic::catch_unwind(AssertUnwindSafe(|| compile_with(input, gtfs_options)))
        .map_err(|payload| {
            Failed::Feed(format!(
                "compile the feed: panicked: {}",
                panic_text(&payload)
            ))
        })?
        .map_err(|e| Failed::Feed(e.to_string()))?;
    let timetable = CompiledTimetable::parse(&compiled.bytes)
        .map_err(|e| Failed::Feed(format!("reload the compiled file: {e}")))?;
    let stops = u32::try_from(timetable.stop_count())
        .map_err(|_| Failed::Feed("hold more stops than the build can number".to_owned()))?;
    let (first, last) = timetable
        .service_date_range()
        .map(|(first, last)| (civil_date_of(first), civil_date_of(last)))
        .map_or((None, None), |(first, last)| {
            (
                first.ok().map(|d| d.to_string()),
                last.ok().map(|d| d.to_string()),
            )
        });
    let report = compiled.reports.first();
    let relative = format!("{TIMETABLES_DIR}/{}.timetable", feed.id);
    let path = options.output.join(&relative);
    write_atomic(&path, &compiled.bytes).map_err(Failed::Stage)?;
    let (timetable_sha, timetable_bytes) = sha256_of_file(&path).map_err(Failed::Stage)?;
    let dropped_trips = report
        .map(|report| {
            report
                .dropped_trips
                .iter()
                .map(|(reason, count)| (format!("{reason:?}"), *count))
                .collect()
        })
        .unwrap_or_default();
    Ok(FeedRecord {
        id: feed.id.clone(),
        region: feed.region.clone(),
        name: feed.name.clone(),
        sha256: feed.sha256.clone(),
        bytes,
        license: feed.license.clone(),
        source: feed.source.clone(),
        outcome: FeedOutcome::Compiled,
        format_version: FORMAT_VERSION,
        timetable: Some(relative),
        timetable_bytes,
        timetable_sha256: Some(timetable_sha),
        stops,
        routes: report.map_or(0, |r| r.routes),
        trips: report.map_or(0, |r| r.trips_kept),
        stop_events: report.map_or(0, |r| r.stop_events),
        has_current_service: has_service(last.as_deref(), fetched_date),
        first_service_date: first,
        last_service_date: last,
        dropped_trips,
        warning_count: report.map_or(0, |r| r.warning_count),
        transfers_forbidden_same_stop: report.map_or(0, |r| r.transfers_forbidden_same_stop),
        transfers_forbidden_pairs: report.map_or(0, |r| r.transfers_forbidden_pairs),
        transfers_forbidden_qualified: report.map_or(0, |r| r.transfers_forbidden_qualified),
        seated: report.map(|r| SeatedOutcome {
            merged: r.seated_trips_merged,
            absorbed: r.seated_trips_absorbed,
            partial: r.seated_trips_partial,
            links_valid: r.seated_links_valid,
            chains_cut: r.seated_chains_cut,
            rows_unmatched: r.seated_rows_unmatched,
            rows_conflicting: r.seated_rows_conflicting,
            patterns: r.patterns,
        }),
        seconds: started.elapsed().as_secs_f64(),
        reused: false,
        settings: Some(settings_of(options)),
    })
}

fn panic_text(payload: &(dyn core::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "without a message".to_owned())
}

/// Records a feed's failure and removes a stale timetable.
///
/// `read` is the archive's actual SHA-256 and size, if it could be read. When the archive matched
/// its listing, the result is kept for the next run: unchanged input would fail the same way.
/// Otherwise the failure is about this input, not the feed, so nothing is kept to reuse.
fn finish_failed(
    options: &CompileOptions,
    feed: &FeedEntry,
    error: String,
    read: Option<(String, u64)>,
    started: Instant,
) -> Result<FeedRecord, BuildError> {
    let path = options
        .output
        .join(TIMETABLES_DIR)
        .join(format!("{}.timetable", feed.id));
    match fs::remove_file(&path) {
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
            return Err(BuildError::Write { path, source });
        }
        _ => {}
    }
    tracing::warn!(feed = %feed.id, %error, "feed failed to compile");
    let matches_listing = read.as_ref().is_some_and(|(sha, _)| *sha == feed.sha256);
    let (sha256, bytes) = read.unwrap_or_default();
    let record = FeedRecord {
        id: feed.id.clone(),
        region: feed.region.clone(),
        name: feed.name.clone(),
        sha256,
        bytes,
        license: feed.license.clone(),
        source: feed.source.clone(),
        outcome: FeedOutcome::Failed { error },
        format_version: FORMAT_VERSION,
        timetable: None,
        timetable_bytes: 0,
        timetable_sha256: None,
        stops: 0,
        routes: 0,
        trips: 0,
        stop_events: 0,
        first_service_date: None,
        last_service_date: None,
        has_current_service: false,
        dropped_trips: BTreeMap::new(),
        warning_count: 0,
        transfers_forbidden_same_stop: 0,
        transfers_forbidden_pairs: 0,
        transfers_forbidden_qualified: 0,
        seated: None,
        seconds: started.elapsed().as_secs_f64(),
        reused: false,
        settings: Some(settings_of(options)),
    };
    if matches_listing {
        write_sidecar(options, &record)?;
    } else {
        remove_sidecar(options, &feed.id)?;
    }
    Ok(record)
}

fn remove_sidecar(options: &CompileOptions, id: &str) -> Result<(), BuildError> {
    let path = sidecar_path(options, id);
    match fs::remove_file(&path) {
        Err(source) if source.kind() != std::io::ErrorKind::NotFound => {
            Err(BuildError::Write { path, source })
        }
        _ => Ok(()),
    }
}

fn sidecar_path(options: &CompileOptions, id: &str) -> PathBuf {
    options.output.join(RESULTS_DIR).join(format!("{id}.json"))
}

fn write_sidecar(options: &CompileOptions, record: &FeedRecord) -> Result<(), BuildError> {
    let path = sidecar_path(options, &record.id);
    let bytes = serde_json::to_vec_pretty(record).map_err(|source| BuildError::Json {
        path: path.clone(),
        source,
    })?;
    write_atomic(&path, &bytes)
}
