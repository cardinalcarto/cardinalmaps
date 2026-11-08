//! The front: parsing requests into `Api.options`, as mainline's `ParseApi` does.
//!
//! So far this holds the JSON document that requests are read from.

pub mod json;

pub use json::{JsonNumber, JsonParseError, JsonValue};
