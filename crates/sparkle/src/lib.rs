//! CPU-side tile processing for the sparkle map renderer.
//!
//! This crate turns [MapLibre Tiles](https://maplibre.org/maplibre-tile-spec/) and a
//! [MapLibre style](https://maplibre.org/maplibre-style-spec/) into geometry buffers that a
//! GPU can draw directly. It deliberately knows nothing about windows, cameras or graphics
//! APIs, so the same output can feed any renderer.
//!
//! ```no_run
//! # fn main() -> Result<(), sparkle::Error> {
//! # let (style_json, tile_bytes) = ("", &[][..]);
//! use sparkle::{Style, TileId, TileProcessor};
//!
//! let style: Style = style_json.parse()?;
//! let mut processor = TileProcessor::new();
//! let tile = processor.process(&style, "openmaptiles", TileId::new(14, 8185, 5449), tile_bytes)?;
//! for bucket in &tile.buckets {
//!     let layer = &style.layers[bucket.style_layer];
//!     // upload bucket.geometry's buffers and draw them with `layer`'s paint properties
//! }
//! # Ok(())
//! # }
//! ```

pub mod dem;
mod error;
pub mod geojson;
pub mod glyphs;
pub mod light;
pub mod paint;
pub mod pattern;
pub mod projection;
pub mod query;
pub mod sky;
pub mod sprite;
pub mod state;
pub mod style;
pub mod symbol;
pub mod terrain;
pub mod tile;
pub mod transition;
pub mod video;

pub use error::Error;
pub use style::Style;
pub use tile::{Bucket, BucketGeometry, ProcessedTile, TileId, TileProcessor, TileResources};
