//! Forming instructions for maneuvers (`odin::NarrativeBuilder`).
//!
//! [`builder`] decides which instructions each maneuver gets and forms the verbal multi-cues;
//! [`instructions`] forms each kind of instruction from the locale's phrases ([`dictionary`]),
//! with lengths written per locale ([`number`]).

mod builder;
mod dictionary;
mod instructions;
mod number;

pub(crate) use builder::NarrativeBuilder;
pub use builder::verbal_alert_approach_instruction;
pub use dictionary::is_supported_language;
