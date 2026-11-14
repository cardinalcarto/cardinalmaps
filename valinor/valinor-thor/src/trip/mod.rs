//! Trip building: the `trip` that Thor sends Odin, from the paths that path search found
//! (mainline's `TripLegBuilder` and the parts of `route_action.cc` that assemble the trip).
//!
//! - [`shape`] is the geometry of the leg's shape (trimming, headings, the polyline).
//! - [`edge`] writes an edge's attributes and the edges that meet the path.
//! - [`leg`] builds a leg (`TripLegBuilder::Build`).
//! - [`build`] puts the legs into routes (`Thor::build_trip`).

pub(crate) mod build;
pub(crate) mod edge;
pub(crate) mod leg;
pub(crate) mod shape;
