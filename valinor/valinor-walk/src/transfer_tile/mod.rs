//! Tiles of precomputed transfers: the on-disk format and how a client loads it.
//!
//! The server build writes one file per travel mode and zoom 12 tile of walks starting at the
//! stops in that tile. Stops are addressed across feeds by `(feed index, stop number)`,
//! where the feed index is the position in the build manifest's sorted feed list,
//! so a tile stays valid for a client that has downloaded any subset of the feeds.
//!
//! [`TransferTile`] is the decoded form; [`assemble_transfer_table`] turns the tiles a client has
//! into a [`TransferTable`](crate::TransferTable) in that client's own stop numbering.
//! Decoding never panics, on any bytes, and is bounded by the limits in [`DecodeLimits`].

mod format;
mod load;

pub use format::{
    DEFAULT_MAX_DECOMPRESSED_BYTES, DecodeLimits, TILE_FORMAT_VERSION, TILE_MAGIC, TILE_ZOOM,
    TileDecodeError, TileEncodeError, TileRows, TileWalk, TileWalks, TransferMode, TransferTile,
    TransferTileRow, decode_tile,
};
pub use load::{
    FeedPlacement, LoadReport, LoadRequest, StopPlacement, TileLoadError, assemble_transfer_table,
};
