//! What a search knows about each directed edge (mainline's `thor::EdgeStatus`).
//!
//! Each edge is unreached, temporarily labeled (in the queue, with the index of its label),
//! permanently labeled (its best path is known), or skipped (a shortcut the search
//! decided not to take).
//! As in mainline, the status of a tile's edges is an array, allocated when the search
//! first reaches the tile; here those arrays are kept for the next search.

use std::collections::HashMap;

use valhalla_graphtile::GraphId;

/// An edge's state in a search (mainline's `EdgeSet`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeSet {
    /// Not reached yet, or reset after a complex restriction (`kUnreachedOrReset`).
    Unreached,
    /// The best path to the edge is known (`kPermanent`).
    Permanent,
    /// The edge is in the queue (`kTemporary`).
    Temporary,
    /// A shortcut the search won't take (`kSkipped`).
    Skipped,
}

/// An edge's state and label index (mainline's `EdgeStatusInfo`), packed in 32 bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EdgeStatusInfo(u32);

/// Mainline keeps the label index in 28 bits.
const INDEX_MASK: u32 = (1 << 28) - 1;

impl EdgeStatusInfo {
    #[inline]
    pub(crate) fn new(set: EdgeSet, index: u32) -> Self {
        let set = match set {
            EdgeSet::Unreached => 0,
            EdgeSet::Permanent => 1,
            EdgeSet::Temporary => 2,
            EdgeSet::Skipped => 3,
        };
        Self((index & INDEX_MASK) | (set << 28))
    }

    #[inline]
    pub(crate) fn set(self) -> EdgeSet {
        match self.0 >> 28 {
            1 => EdgeSet::Permanent,
            2 => EdgeSet::Temporary,
            3 => EdgeSet::Skipped,
            _ => EdgeSet::Unreached,
        }
    }

    #[inline]
    pub(crate) fn index(self) -> u32 {
        self.0 & INDEX_MASK
    }

    #[inline]
    fn with_set(self, set: EdgeSet) -> Self {
        Self::new(set, self.index())
    }
}

/// The status of every edge a search has reached (mainline's `EdgeStatus`).
#[derive(Debug, Default)]
pub(crate) struct EdgeStatus {
    /// Slot by tile (the base graph ID's value).
    tiles: HashMap<u64, usize>,
    /// Each slot holds one tile's edges.
    slots: Vec<Vec<EdgeStatusInfo>>,
    /// How many slots are in use; the rest are kept for later searches.
    used: usize,
    /// The last tile looked up, since searches look at many edges of a tile in a row.
    last: Option<(u64, usize)>,
}

impl EdgeStatus {
    /// Forgets every edge, keeping the storage (`clear`).
    pub(crate) fn clear(&mut self) {
        self.tiles.clear();
        self.used = 0;
        self.last = None;
    }

    #[inline]
    fn slot(&self, tile: u64) -> Option<usize> {
        if let Some((t, s)) = self.last
            && t == tile
        {
            return Some(s);
        }
        self.tiles.get(&tile).copied()
    }

    /// The slot for a tile, made with room for `edge_count` edges if it's new.
    #[inline]
    fn slot_mut(&mut self, tile: u64, edge_count: usize) -> usize {
        if let Some(s) = self.slot(tile) {
            self.last = Some((tile, s));
            return s;
        }
        let s = self.used;
        self.used += 1;
        if s == self.slots.len() {
            self.slots.push(Vec::new());
        }
        if let Some(edges) = self.slots.get_mut(s) {
            edges.clear();
            edges.resize(edge_count, EdgeStatusInfo::default());
        }
        self.tiles.insert(tile, s);
        self.last = Some((tile, s));
        s
    }

    #[inline]
    fn key(edge_id: GraphId) -> (u64, usize) {
        (
            edge_id.tile_base_id().value(),
            usize::try_from(edge_id.feature_index()).unwrap_or(usize::MAX),
        )
    }

    /// An edge's status (`Get`); unreached for edges never seen.
    #[inline]
    pub(crate) fn get(&self, edge_id: GraphId) -> EdgeStatusInfo {
        let (tile, index) = Self::key(edge_id);
        self.slot(tile)
            .and_then(|s| self.slots.get(s))
            .and_then(|edges| edges.get(index))
            .copied()
            .unwrap_or_default()
    }

    /// Sets an edge's status (`Set` or `GetPtr`), given its tile's edge count.
    #[inline]
    pub(crate) fn set(&mut self, edge_id: GraphId, status: EdgeStatusInfo, tile_edge_count: usize) {
        let (tile, index) = Self::key(edge_id);
        let s = self.slot_mut(tile, tile_edge_count);
        if let Some(entry) = self.slots.get_mut(s).and_then(|edges| edges.get_mut(index)) {
            *entry = status;
        }
    }

    /// Changes the state of an edge that has a status, keeping its index (`Update`).
    ///
    /// Mainline throws for an edge whose tile was never seen; nothing calls it so.
    #[inline]
    pub(crate) fn update(&mut self, edge_id: GraphId, set: EdgeSet) {
        let (tile, index) = Self::key(edge_id);
        if let Some(s) = self.slot(tile)
            && let Some(entry) = self.slots.get_mut(s).and_then(|edges| edges.get_mut(index))
        {
            *entry = entry.with_set(set);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_kept_per_tile_and_reused() {
        let a = GraphId::try_from_components(2, 763_927, 5).unwrap();
        let b = GraphId::try_from_components(1, 47_701, 3).unwrap();
        let mut status = EdgeStatus::default();
        assert_eq!(status.get(a).set(), EdgeSet::Unreached);
        status.set(a, EdgeStatusInfo::new(EdgeSet::Temporary, 42), 10);
        status.set(b, EdgeStatusInfo::new(EdgeSet::Skipped, 0), 4);
        assert_eq!(status.get(a).set(), EdgeSet::Temporary);
        assert_eq!(status.get(a).index(), 42);
        status.update(a, EdgeSet::Permanent);
        assert_eq!(status.get(a), EdgeStatusInfo::new(EdgeSet::Permanent, 42));
        assert_eq!(status.get(b).set(), EdgeSet::Skipped);
        status.clear();
        assert_eq!(status.get(a).set(), EdgeSet::Unreached);
        status.set(b, EdgeStatusInfo::new(EdgeSet::Temporary, 1), 4);
        assert_eq!(status.get(a).set(), EdgeSet::Unreached);
        assert_eq!(status.slots.len(), 2);
    }
}
