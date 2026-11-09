//! Street search over Valhalla routing tiles, for real-time transit routing.
//!
//! See `docs/transit.md` for the design.
//!
//! - [`WalkSearch`] implements [`valinor_transit::TransferSearch`] directly over routing tiles:
//!   node-based, integer seconds per directed edge, forward and backward,
//!   with multi-source seeds and the "smallest arrival from another seed" contract.
//! - [`StopSnaps`] snaps stops to the street network with Loki, keyed by [`StopId`],
//!   bound to the build of the tiles they were snapped on.
//!
//! There is no contraction and no preprocessing that depends on stops:
//! real-time feeds can add stops at any time.
//!
//! [`StopId`]: valinor_transit::StopId
//! [`Paths`]: valinor_thor::Paths

mod error;
mod graph;
mod queue;
mod search;
mod snap;

pub use error::WalkError;
pub use graph::GraphStats;
pub use search::{ResolvedWalk, SearchStats, WalkHandle, WalkSearch};
pub use snap::{
    SnapConfig, SnapFailure, SnapPoint, SnapQuality, SnapReport, SnappedStop, StopSnaps, snap_stop,
    snap_stops,
};
