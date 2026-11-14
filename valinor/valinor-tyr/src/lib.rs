//! Valhalla-compatible request parsing and response serialization.
//!
//! This crate holds the two ends of the stage chain described in `docs/routing.md`:
//!
//! - The **front** parses an HTTP request (an action plus a JSON body or `?json=` parameter)
//!   into `Api.options`, exactly as mainline's `ParseApi` does,
//!   including the defaults it fills in and the errors it raises.
//! - **Tyr** serializes a finished request (an `Api` with `trip` and `directions`)
//!   as Valhalla JSON, OSRM-format JSON, or PBF, as mainline's `tyr` serializers do.
//! - [`AttributesController`] is mainline's `baldr::AttributesController`:
//!   which attributes a request's filters ask for,
//!   shared by Thor's trip building and Tyr's serializers.

extern crate alloc;

pub use valinor_sif::costing_options;
pub mod attributes;
pub mod front;
pub mod tyr;

pub use attributes::{Attribute, AttributesController, Category};

#[cfg(test)]
mod test_support;
