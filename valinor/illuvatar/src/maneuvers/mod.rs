//! Building maneuvers from a trip leg (`odin::ManeuversBuilder`).
//!
//! The builder walks the leg's nodes backwards, starting a new maneuver at every node
//! where the path cannot simply continue the current one ([`builder`]),
//! then repeatedly combines maneuvers that would read as one instruction ([`combine`]),
//! assigns maneuver types ([`maneuver_type`]),
//! and finally runs a series of passes over the list ([`passes`]):
//! roundabouts, "to stay on", turn lanes, guidance views, and so on.
//!
//! Mainline keeps the maneuvers in a `std::list` and walks it with iterators;
//! here they live in a `Vec` and the walks use indices.
//! The comments at each walk explain how its iterators map onto indices.

mod builder;
mod combine;
mod maneuver_type;
mod passes;

pub(crate) use builder::ManeuversBuilder;
