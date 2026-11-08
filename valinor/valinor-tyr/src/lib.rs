//! Valhalla-compatible request parsing and response serialization.
//!
//! This crate holds the two ends of the stage chain described in `docs/routing.md`:
//!
//! - The **front** parses an HTTP request (an action plus a JSON body or `?json=` parameter)
//!   into `Api.options`, exactly as mainline's `ParseApi` does,
//!   including the defaults it fills in and the errors it raises.
//! - **Tyr** serializes a finished request (an `Api` with `trip` and `directions`)
//!   as Valhalla JSON, OSRM-format JSON, or PBF, as mainline's `tyr` serializers do.

extern crate alloc;

pub mod costing_options;
pub mod front;
pub mod tyr;

#[cfg(test)]
mod test_support;
