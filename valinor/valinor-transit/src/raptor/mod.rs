//! Multimodal RAPTOR: labels, the route scan, and the search driver.

mod labels;
mod router;
mod scan;
mod search;

pub(crate) use labels::{LabelKind, Labels};
pub(crate) use router::round_results;
pub use router::{RoundResult, Router, SearchRun, TransitQuery};
pub use search::MAX_TRIPS;

/// Converts between real time and a search's clock.
///
/// A forward search's clock is real time. A backward search's is real time negated,
/// so both directions minimize their clock and run the same code.
#[inline]
pub(crate) const fn clock_of<const BACK: bool>(real: i32) -> i32 {
    if BACK { -real } else { real }
}
