//! Compiles GTFS feeds into the file described in [`crate::format`].
//!
//! The work happens in two passes. [`read`] turns each feed's CSV files into a model,
//! dropping and counting what can't be used, and interpolating missing times.
//! [`assemble`] groups trips into patterns, numbers stops through `valinor-transit`'s builder,
//! and writes the tables.

use nutype::nutype;

use crate::error::CompileError;

mod assemble;
mod interpolate;
mod read;
mod report;
mod seated;
mod shape_match;
mod source;
mod table;
mod time;

pub use report::{DropReason, FeedReport, WARNING_LIMIT};
pub use seated::{CompileOptions, DEFAULT_MAX_SEATED_CHAIN, DEFAULT_SEATED_WAIT_SECONDS};
pub use source::{DirectorySource, GtfsSource, InMemorySource, ZipSource};

/// The namespace for all of a feed's GTFS identifiers.
///
/// Identifiers are unique only within a feed; every lookup is scoped by this.
#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 128),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display)
)]
pub struct FeedId(String);

/// A feed to compile: a namespace and where to read it.
pub struct FeedInput<'a> {
    /// The feed's namespace.
    pub id: FeedId,
    /// The feed's files.
    pub source: Box<dyn GtfsSource + 'a>,
}

impl core::fmt::Debug for FeedInput<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FeedInput")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The result of a compile.
#[derive(Debug)]
pub struct CompiledOutput {
    /// The compiled file.
    pub bytes: Vec<u8>,
    /// What was found in each feed, in the order they were given.
    pub reports: Vec<FeedReport>,
}

/// Compiles feeds into one file.
///
/// Feeds are loaded together: their identifiers are scoped by feed ID,
/// and their trips share one timetable and stop numbering.
///
/// A file holds feeds in one time zone only, because `valinor-transit` has one offset per
/// service date; mixing zones is a [`CompileError::MixedFeedTimeZones`].
///
/// # Errors
///
/// If a feed can't be read, is structurally invalid, or can't be represented.
pub fn compile(feeds: Vec<FeedInput<'_>>) -> Result<CompiledOutput, CompileError> {
    compile_with(feeds, CompileOptions::default())
}

/// As [`compile`], with chosen options.
///
/// # Errors
///
/// As [`compile`].
pub fn compile_with(
    feeds: Vec<FeedInput<'_>>,
    options: CompileOptions,
) -> Result<CompiledOutput, CompileError> {
    let mut pool = assemble::StringPool::new();
    let mut models = Vec::with_capacity(feeds.len());
    for feed in feeds {
        if models
            .iter()
            .any(|model: &read::FeedModel| model.id == *feed.id.as_ref())
        {
            return Err(CompileError::DuplicateFeedId(feed.id.into_inner()));
        }
        models.push(read::read_feed(feed, &mut pool, options)?);
    }
    assemble::assemble(models, pool)
}
