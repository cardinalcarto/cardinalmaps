//! Odin: turns a Valhalla trip into directions.
//!
//! This crate is the Odin stage of the router described in `docs/routing.md`.
//! Given an [`Api`] message with a `trip` (Thor's output),
//! it builds the `directions`: the maneuvers of each leg with their textual,
//! verbal, and succinct instructions, in the requested language and units.
//! The result matches mainline Valhalla 3.9.1 given the same trip.
//!
//! The structure follows mainline's `src/odin`:
//!
//! - `trip` provides the enhanced trip leg predicates (`enhancedtrippath.cc`).
//! - `maneuvers` builds and combines maneuvers (`maneuversbuilder.cc`).
//! - `narrative` forms instructions from the locale dictionaries (`narrativebuilder.cc`).
//! - `directions` drives both and fills in the directions message (`directionsbuilder.cc`).
//!
//! Like mainline, the stage also updates the trip in place:
//! it fills in the headings of very short edges
//! and marks the turn lanes that maneuvers use,
//! which later stages (Tyr's OSRM serializer in particular) read.
//!
//! # Limitations
//!
//! Transit (and multimodal) trips are refused with an error,
//! since transit routing is out of scope for Valinor.

// Mainline compares floats exactly (rounded distances, level ranges, and so on),
// and matching its output means making the same comparisons.
#![expect(clippy::float_cmp, reason = "mainline compares floats exactly")]

mod directions;
mod error;
#[cfg(test)]
mod mainline_tests;
mod maneuver;
mod maneuvers;
mod markup_formatter;
mod narrative;
mod signs;
mod street_names;
mod trip;
mod turn;
mod verbal_text_formatter;

pub use error::OdinError;
pub use markup_formatter::{DEFAULT_PHONEME_FORMAT, MarkupFormatter};
pub use narrative::{is_supported_language, verbal_alert_approach_instruction};
use valhalla_proto::Api;
use valhalla_response::ValhallaError;

/// The Odin stage, with its configuration (`odin_worker_t`).
#[derive(Debug, Clone, Default)]
pub struct Odin {
    markup_formatter: MarkupFormatter,
}

impl Odin {
    /// An Odin stage with the given phoneme markup configuration.
    #[inline]
    pub fn new(markup_formatter: MarkupFormatter) -> Self {
        Self { markup_formatter }
    }

    /// Builds the directions for the request's trip (`odin_worker_t::narrate`).
    ///
    /// The directions are appended to `request.directions`,
    /// and the trip legs are updated in place as mainline updates them.
    ///
    /// # Errors
    ///
    /// The error mainline's worker reports (code 299, wrapping 202 and the cause)
    /// when the trip cannot be narrated:
    /// for example a leg with fewer than two nodes or locations,
    /// a language without a dictionary, or a transit trip.
    ///
    /// # Performance
    ///
    /// The first request in each language parses that language's dictionary.
    pub fn narrate(&self, request: &mut Api) -> Result<(), ValhallaError> {
        directions::build(request, &self.markup_formatter).map_err(OdinError::into_valhalla_error)
    }
}

/// The Odin stage with mainline's default configuration (phoneme markup disabled).
///
/// # Errors
///
/// See [`Odin::narrate`].
#[inline]
pub fn narrate(request: &mut Api) -> Result<(), ValhallaError> {
    Odin::default().narrate(request)
}
