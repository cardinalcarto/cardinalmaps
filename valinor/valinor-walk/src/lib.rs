//! Street search over Valhalla routing tiles, for real-time transit routing.
//!
//! See `docs/transit.md` for the design.
//!
//! - [`WalkSearch`] implements [`valinor_transit::TransferSearch`] directly over routing tiles:
//!   node-based, integer seconds per directed edge, forward and backward,
//!   with multi-source seeds and the "smallest arrival from another seed" contract.
//! - [`StopSnaps`] snaps stops to the street network with Loki, keyed by [`StopId`],
//!   bound to the build of the tiles they were snapped on.
//! - [`StopTables`] computes the access and egress stop tables of a query
//!   with full edge-based Sif.
//! - [`paths`] converts resolved walks and access or egress legs into Thor's [`Paths`],
//!   so [`Thor::build_trip`](valinor_thor::Thor::build_trip) can produce shapes and instructions.
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
mod table;

mod flat_table;
pub mod paths;
pub mod transfer_tile;
mod transfers;

pub use error::WalkError;
pub use flat_table::{FLAT_TABLE_MAGIC, FlatTableError, TableKey};
pub use graph::GraphStats;
pub use search::{ResolvedWalk, SearchStats, WalkHandle, WalkSearch};
pub use snap::{
    SnapConfig, SnapFailure, SnapPoint, SnapQuality, SnapReport, SnappedStop, StopSnaps, snap_stop,
    snap_stops,
};
pub use table::{
    AccessEdge, AccessLeg, AccessSide, StopAttachments, StopTables, TableRequest, TableSeed,
    TableStats,
};
pub use transfers::{
    DEFAULT_TRANSFER_SECONDS, Hop, TableTransfers, TableWalk, TransferTable, sorted_walks_from,
};
