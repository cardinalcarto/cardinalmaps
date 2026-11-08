//! Errors raised while building directions.
//!
//! Mainline Odin signals failures by throwing:
//! either a `valhalla_exception_t` with one of the 2xx codes,
//! or a standard library exception (for example from `std::unordered_map::at`
//! when a locale lacks a phrase).
//! `odin_worker_t::narrate` wraps whatever it catches as code 202,
//! and `odin_worker_t::work` wraps *that* as code 299,
//! so a client always sees `299` with both messages chained.
//! [`OdinError`] models the inner failures,
//! and [`OdinError::into_valhalla_error`] reproduces the chain.

use thiserror::Error;
use valhalla_response::{ErrorCode, ValhallaError};

/// A failure while building maneuvers or narrative for a trip leg.
///
/// The display messages are mainline's `what()` strings,
/// because they end up verbatim in the client-visible error message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OdinError {
    /// One of mainline's coded Odin errors (210 through 239).
    #[error("{}", .0.message())]
    Coded(ErrorCode),
    /// The requested language has no narrative dictionary.
    ///
    /// Mainline throws a `std::runtime_error` with this message.
    #[error("Invalid language tag.")]
    InvalidLanguageTag,
    /// A narrative phrase that the builder needs is missing from the dictionary.
    ///
    /// This is `std::unordered_map::at` failing in mainline,
    /// whose libstdc++ message we reproduce.
    #[error("_Map_base::at")]
    MissingPhrase,
    /// A narrative label (an ordinal, a cardinal direction, and so on) is out of range.
    ///
    /// This is `std::vector::at` failing in mainline,
    /// whose libstdc++ message we reproduce.
    #[error("vector::_M_range_check: __n (which is {index}) >= this->size() (which is {len})")]
    LabelOutOfRange {
        /// The requested index.
        index: usize,
        /// The number of labels.
        len: usize,
    },
    /// A pronunciation has no alphabet name, so it cannot be marked up.
    #[error("Missing value in protobuf Pronunciation_Alphabet enum to string")]
    UnknownPronunciationAlphabet,
    /// The trip uses transit, which this port does not narrate.
    ///
    /// Transit routing is out of scope (see `docs/routing.md`).
    #[error("transit directions are not supported")]
    TransitUnsupported,
    /// An embedded locale file failed to parse.
    ///
    /// The tests parse every embedded file, so this means a broken build.
    #[error("locale {language_tag} is corrupt")]
    CorruptLocale {
        /// The locale's language tag.
        language_tag: &'static str,
    },
    /// The trip leg lacks an edge that the builder needs.
    ///
    /// Mainline dereferences a null pointer here (and crashes);
    /// we report it instead, since a stage must never panic on bad input.
    #[error("trip leg has no edge at node {node_index}")]
    MissingEdge {
        /// The node index whose edge was needed.
        node_index: usize,
    },
    /// The trip leg lacks a node that the builder needs.
    ///
    /// Like [`OdinError::MissingEdge`], this would be undefined behavior in mainline.
    #[error("trip leg has no node {node_index}")]
    MissingNode {
        /// The node index that was needed.
        node_index: usize,
    },
}

impl OdinError {
    /// The error a client sees, as mainline's worker reports it.
    ///
    /// The message is `Unknown: Could not build directions for TripLeg: <inner message>`
    /// with code 299, as `odin_worker_t::work` wraps the 202 that `narrate` throws.
    pub fn into_valhalla_error(self) -> ValhallaError {
        let directions =
            ValhallaError::with_detail(ErrorCode::DirectionsBuildingFailed, self.to_string());
        ValhallaError::with_detail(ErrorCode::OdinUnknown, directions.message())
    }
}

impl From<OdinError> for ValhallaError {
    #[inline]
    fn from(error: OdinError) -> Self {
        error.into_valhalla_error()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_chain_like_mainline() {
        let error = OdinError::Coded(ErrorCode::TripPathNoNodes).into_valhalla_error();
        assert_eq!(error.code(), ErrorCode::OdinUnknown);
        assert_eq!(
            error.message(),
            "Unknown: Could not build directions for TripLeg: Trip path does not have any nodes"
        );
    }

    #[test]
    fn standard_library_messages_match_libstdcxx() {
        assert_eq!(OdinError::MissingPhrase.to_string(), "_Map_base::at");
        assert_eq!(
            OdinError::LabelOutOfRange { index: 7, len: 3 }.to_string(),
            "vector::_M_range_check: __n (which is 7) >= this->size() (which is 3)"
        );
    }
}
