//! Why a stage of the transit build failed.

use std::path::PathBuf;

use thiserror::Error;
use valinor_gtfs::LoadError;
use valinor_router::tiles::OpenError;
use valinor_walk::WalkError;
use valinor_walk::transfer_tile::TileEncodeError;

/// A failure that stops a stage.
///
/// What goes wrong with one feed in the compile stage isn't one of these:
/// it is recorded against that feed, and the rest carry on.
#[derive(Debug, Error)]
pub enum BuildError {
    /// A file couldn't be read.
    #[error("read {path}: {source}")]
    Read {
        /// The path.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// A file couldn't be written.
    #[error("write {path}: {source}")]
    Write {
        /// The path.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// A JSON file couldn't be parsed or produced.
    #[error("use JSON at {path}: {source}")]
    Json {
        /// The path.
        path: PathBuf,
        /// The cause.
        source: serde_json::Error,
    },
    /// Two feeds in the input have one ID.
    #[error("use feed ID {0} twice in one build")]
    DuplicateFeedId(String),
    /// A feed's ID or file can't be used safely as a path.
    #[error("use {0:?} as a feed path, which may escape the build directory")]
    UnsafeFeedPath(String),
    /// The input's feed count, or the build's stop count, doesn't fit its index.
    #[error("index more than 2^32 feeds or stops")]
    TooManyEntries,
    /// The street tiles couldn't be opened.
    #[error("open street tiles at {path}: {source}")]
    StreetTiles {
        /// The path.
        path: PathBuf,
        /// The cause.
        source: Box<OpenError>,
    },
    /// A compiled feed's timetable didn't load, though the compile stage recorded it as compiled.
    #[error("load the timetable of feed {feed}: {source}")]
    Timetable {
        /// The feed.
        feed: String,
        /// The cause.
        source: LoadError,
    },
    /// A street search failed.
    #[error("search the streets: {0}")]
    Walk(#[from] WalkError),
    /// A costing couldn't be made.
    #[error("set up costing for {mode}: {reason}")]
    Costing {
        /// The mode.
        mode: &'static str,
        /// Why.
        reason: String,
    },
    /// A tile couldn't be encoded.
    #[error("encode a tile: {0}")]
    Encode(#[from] TileEncodeError),
    /// A tile couldn't be compressed.
    #[error("compress a tile: {0}")]
    Compress(std::io::Error),
    /// A worker thread panicked.
    #[error("finish the stage: a worker thread panicked")]
    WorkerPanicked,
}
