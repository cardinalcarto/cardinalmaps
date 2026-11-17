//! How prominent a place is, as airmail ranks it, and what that takes: the
//! shapes airmail stores, multipolygon assembly, and QRank. A crate of its
//! own so that poi_service, which tags OSM elements with their importance,
//! computes it with exactly the code the indexer uses.

#![forbid(unsafe_code)]

pub mod importance;
pub mod multipolygon;
pub mod osm;
pub mod qrank;
pub mod shape;

pub use geo;
