#![forbid(unsafe_code)]
#![warn(clippy::missing_panics_doc)]

#[macro_use]
extern crate lazy_static;

pub mod address_format;
pub mod categories;
pub mod countries;
pub mod cuisine;
pub mod error;
pub mod gazetteer;
pub mod geometry;
pub mod importance;
pub mod index;
pub mod language;
pub mod osm;
pub mod plan;
pub mod poi;
pub mod postcodes;
pub mod spelling;
pub mod substitutions;
pub mod tagger_data;
pub mod text;
pub mod wof;
mod within;
