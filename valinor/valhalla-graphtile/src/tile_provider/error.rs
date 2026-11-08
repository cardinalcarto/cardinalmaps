//! # Tile provider errors
//!
//! Every fallible tile provider operation returns a [`GraphTileProviderError`].
//! The variants say exactly what went wrong,
//! and [`GraphTileProviderError::kind`] groups them into the broad categories
//! that callers usually branch on.
//!
//! The most important distinction is between a tile that is *not available*
//! and everything else.
//! A regional extract (for example, an offline download on a phone)
//! only contains the tiles that cover the region,
//! but the tiles at its edge still reference neighboring tiles.
//! Following such a reference is not an error in the data:
//! the graph simply ends there,
//! and routing should treat it as a dead end.
//! [`GraphTileProviderError::TileNotAvailable`] is the only variant with that meaning.

use crate::GraphId;
use crate::graph_id::InvalidGraphIdError;
use crate::graph_tile::{GraphTileDecodingError, LookupError};
use crate::tile_provider::tarball::{InvalidIndexBinError, InvalidTarballError};
use thiserror::Error;

/// The broad category of a [`GraphTileProviderError`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GraphTileProviderErrorKind {
    /// The requested tile is not part of the tile set.
    ///
    /// This is expected at the edge of a regional extract,
    /// and callers should usually recover by treating the reference as a dead end.
    NotAvailable,
    /// Reading the underlying storage failed.
    ///
    /// Retrying may succeed (for example, after freeing file handles).
    Io,
    /// The tile data, the tarball, or its index is malformed.
    ///
    /// This usually means a truncated or corrupt download,
    /// or a tile set mixed from incompatible builds.
    /// Retrying will not help; the data needs to be replaced.
    InvalidData,
    /// The caller passed an argument that the operation cannot work with,
    /// such as a non-finite search radius.
    InvalidInput,
    /// An internal invariant was violated (for example, a poisoned lock).
    ///
    /// This indicates a bug, either in Valinor or in the calling code.
    Internal,
}

/// The ways in which a tile provider operation can fail.
///
/// Use [`GraphTileProviderError::kind`] to branch on the broad category,
/// and [`GraphTileProviderError::is_not_available`] to detect the edge of an extract.
#[derive(Debug, Error)]
pub enum GraphTileProviderError {
    /// The tile is not part of this tile set.
    ///
    /// The payload is the base graph ID of the missing tile.
    #[error("load tile {0}: it is not available in this tile set")]
    TileNotAvailable(GraphId),
    /// A graph ID could not be constructed.
    ///
    /// For IDs computed from tile data, this means the data is corrupt.
    #[error("construct a graph ID: {0}")]
    InvalidGraphId(#[from] InvalidGraphIdError),
    /// Reading from the underlying storage failed.
    #[error("read tile data: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes stored for a tile are not a valid graph tile.
    #[error("decode tile {graph_id}: {source}")]
    InvalidTile {
        /// The base graph ID of the tile.
        graph_id: GraphId,
        /// Why decoding failed.
        #[source]
        source: GraphTileDecodingError,
    },
    /// The tile stored for one graph ID says (in its header) that it is a different tile.
    ///
    /// This means that a tarball index or a tile directory is corrupt.
    #[error("load tile {expected}: the stored tile is {actual}")]
    TileIdMismatch {
        /// The base graph ID that was requested.
        expected: GraphId,
        /// The base graph ID in the header of the stored tile.
        actual: GraphId,
    },
    /// A record inside an already loaded tile could not be decoded.
    #[error("decode a tile record: {0}")]
    Decoding(#[from] GraphTileDecodingError),
    /// The shape of an edge could not be decoded.
    #[error("decode the shape of edge {edge_id}: {source}")]
    InvalidShape {
        /// The graph ID of the directed edge.
        edge_id: GraphId,
        /// Why decoding failed.
        #[source]
        source: std::io::Error,
    },
    /// A graph ID does not refer to a feature in the tile that should contain it.
    ///
    /// For IDs computed from tile data, this means the data is corrupt.
    #[error("look up a graph feature: {0}")]
    Lookup(#[from] LookupError),
    /// Recovering the shortcut for an edge walked further than any real shortcut is long.
    ///
    /// This only happens when the tile data is corrupt (for example, edges that form a cycle).
    #[error("find the shortcut for edge {0}: the walk does not terminate")]
    UnterminatedShortcutWalk(GraphId),
    /// The cache lock of the tile provider is poisoned
    /// (another thread panicked while holding it).
    #[error("lock the tile cache: the lock is poisoned")]
    PoisonedCacheLock,
    /// The tarball is malformed.
    #[error("open the tarball: {0}")]
    InvalidTarball(#[from] InvalidTarballError),
    /// The `index.bin` file of the tarball is malformed.
    #[error("parse index.bin: {0}")]
    InvalidIndexBin(#[from] InvalidIndexBinError),
    /// A traffic tile has a version that Valinor does not understand.
    #[error(
        "read traffic tile {graph_id}: version {version} is not supported (expected {expected})"
    )]
    UnsupportedTrafficTileVersion {
        /// The base graph ID of the traffic tile.
        graph_id: GraphId,
        /// The version in the traffic tile header.
        version: u32,
        /// The version that Valinor supports.
        expected: u32,
    },
    /// A traffic tile is too small to hold what its header describes.
    #[error(
        "read traffic tile {graph_id}: {required} bytes are needed, but the tile only has {size}"
    )]
    TruncatedTrafficTile {
        /// The base graph ID of the traffic tile.
        graph_id: GraphId,
        /// The size of the tile in the tarball index.
        size: u32,
        /// The number of bytes needed for the requested read.
        required: u64,
    },
    /// The parameters of a spatial search are not usable.
    #[error("search the graph: {0}")]
    InvalidSearch(#[from] InvalidSearchError),
}

impl GraphTileProviderError {
    /// The broad category of this error.
    #[inline]
    pub const fn kind(&self) -> GraphTileProviderErrorKind {
        match self {
            Self::TileNotAvailable(_) => GraphTileProviderErrorKind::NotAvailable,
            Self::Io(_) => GraphTileProviderErrorKind::Io,
            Self::InvalidGraphId(_)
            | Self::InvalidTile { .. }
            | Self::TileIdMismatch { .. }
            | Self::Decoding(_)
            | Self::InvalidShape { .. }
            | Self::Lookup(_)
            | Self::UnterminatedShortcutWalk(_)
            | Self::InvalidTarball(_)
            | Self::InvalidIndexBin(_)
            | Self::UnsupportedTrafficTileVersion { .. }
            | Self::TruncatedTrafficTile { .. } => GraphTileProviderErrorKind::InvalidData,
            Self::InvalidSearch(_) => GraphTileProviderErrorKind::InvalidInput,
            Self::PoisonedCacheLock => GraphTileProviderErrorKind::Internal,
        }
    }

    /// Is this error caused by a reference to a tile that is not in the tile set?
    ///
    /// See [`GraphTileProviderErrorKind::NotAvailable`].
    #[inline]
    pub const fn is_not_available(&self) -> bool {
        matches!(self, Self::TileNotAvailable(_))
    }
}

/// The reasons why the parameters of a spatial search are not usable.
///
/// Coordinates and distances are reported as `f64`,
/// whatever float type the search was made with.
#[derive(Debug, Error, PartialEq)]
pub enum InvalidSearchError {
    /// The center of the search is not a valid WGS84 coordinate.
    #[error(
        "use ({lon}, {lat}) as the search center: expected a finite longitude in [-180, 180] and latitude in [-90, 90]"
    )]
    InvalidCenter {
        /// The longitude of the center.
        lon: f64,
        /// The latitude of the center.
        lat: f64,
    },
    /// The search radius is negative or not finite.
    #[error("use {0} m as the search radius: expected a finite, non-negative distance")]
    InvalidRadius(f64),
    /// The bounding box of the search is not finite.
    ///
    /// This is not expected to happen for a valid center and radius.
    #[error("compute the bounding box of the search: the result is not finite")]
    NonFiniteBoundingBox,
}

/// Turns a "not available" outcome into `None`.
///
/// Routing code usually treats a reference to a tile outside of the extract as a dead end,
/// while every other error is a real failure.
/// This makes that pattern a one-liner:
///
/// ```
/// # use valhalla_graphtile::GraphId;
/// # use valhalla_graphtile::graph_tile::GraphTile;
/// # use valhalla_graphtile::tile_provider::{GraphTileProvider, GraphTileProviderError, TileAvailability};
/// fn edge_count<P: GraphTileProvider>(
///     provider: &P,
///     tile_id: GraphId,
/// ) -> Result<Option<u32>, GraphTileProviderError> {
///     // `None` at the edge of the extract, an error if the tile is corrupt
///     provider
///         .with_tile_containing(tile_id, |tile| tile.header().directed_edge_count())
///         .if_available()
/// }
/// ```
pub trait TileAvailability<T> {
    /// Returns `Ok(None)` if the error is [`GraphTileProviderError::TileNotAvailable`],
    /// and passes everything else through.
    ///
    /// # Errors
    ///
    /// Returns every error except [`GraphTileProviderError::TileNotAvailable`].
    fn if_available(self) -> Result<Option<T>, GraphTileProviderError>;
}

impl<T> TileAvailability<T> for Result<T, GraphTileProviderError> {
    #[inline]
    fn if_available(self) -> Result<Option<T>, GraphTileProviderError> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(GraphTileProviderError::TileNotAvailable(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }
}
