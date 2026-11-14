//! Finding the k best paths (mainline's `TopKSearch` and `EnlargedViterbiSearch`).
//!
//! After each path is found, its states are cloned into a new layer of the trellis,
//! with costs arranged so that the next search can use any of the path's states but not the
//! whole path: the clones continue only into clones (or into other states, but never back),
//! the first original state of the path is unusable, and so is the last clone.
//! Each layer wraps the previous layer's costs, as mainline's enlarged cost models wrap the
//! models they replace; a cost resolves down through the layers to the original states.
//!
//! Mainline caches each layer's costs by state.
//! Transition costs never change once computed (a state's routes are searched once),
//! so they are computed again here instead.
//! Emission costs are cached as mainline caches them, because the caches decide results:
//! a layer checks whether its own clones' states were removed before it looks in its cache,
//! but a cost an upper layer cached before a removal stays, so a removed state's clones
//! can still be used through the layers that already cached them.

use std::collections::{HashMap, HashSet};

use crate::error::MeiliError;
use crate::match_result::StateId;

/// One cloned path (mainline's `EnlargedViterbiSearch`).
#[derive(Debug, Default)]
struct Layer {
    /// Clone to the state it clones (which may be a clone of an earlier layer).
    origin: HashMap<StateId, StateId>,
    /// State to its clone in this layer.
    clone: HashMap<StateId, StateId>,
    clone_start_time: Option<u32>,
    clone_end_time: Option<u32>,
    /// The emission costs this layer has computed (`EnlargedEmissionCostModel::cached_costs_`).
    emission_cache: HashMap<StateId, f32>,
}

/// The layers of cloned paths (mainline's `TopKSearch`).
#[derive(Debug, Default)]
pub(crate) struct TopK {
    layers: Vec<Layer>,
    /// The next clone ID per time, counting down from `u32::MAX`.
    last_claimed: HashMap<u32, u32>,
    /// Clone to the original state it stands for.
    initial_origins: HashMap<StateId, StateId>,
    /// Original states that were removed as redundant.
    removed: HashSet<StateId>,
}

/// What a cost resolves to.
pub(crate) enum Resolved<T> {
    /// The cost is the original model's for these states.
    Original(T),
    /// The state or transition isn't usable (mainline's cost of -1).
    Invalid,
}

impl TopK {
    /// Forgets every layer (`Clear`).
    pub(crate) fn clear(&mut self) {
        self.layers.clear();
        self.last_claimed.clear();
        self.initial_origins.clear();
        self.removed.clear();
    }

    /// The original state a state stands for, or `None` for an original (`GetOrigin`).
    #[inline]
    pub(crate) fn origin(&self, id: StateId) -> Option<StateId> {
        self.initial_origins.get(&id).copied()
    }

    /// Marks an original state removed (`RemoveStateId`).
    #[inline]
    pub(crate) fn remove(&mut self, id: StateId) {
        self.removed.insert(id);
    }

    /// Whether an original state was removed (`IsRemoved`).
    #[inline]
    pub(crate) fn is_removed(&self, id: StateId) -> bool {
        self.removed.contains(&id)
    }

    /// Clones a path into a new layer (`RemovePath` and `ClonePath`), returning the clones
    /// (in increasing time) for the search to add.
    ///
    /// # Errors
    ///
    /// Fails if a time runs out of clone IDs.
    pub(crate) fn remove_path(
        &mut self,
        path: &[Option<StateId>],
    ) -> Result<Vec<StateId>, MeiliError> {
        let mut layer = Layer::default();
        for &origin in path.iter().flatten() {
            let id = if let Some(id) = self.last_claimed.get_mut(&origin.time) {
                *id = id.checked_sub(1).ok_or(MeiliError::Logic(
                    "generated clone state IDs must be unique",
                ))?;
                *id
            } else {
                self.last_claimed.insert(origin.time, u32::MAX);
                u32::MAX
            };
            let clone = StateId {
                time: origin.time,
                id,
            };
            layer.clone.insert(origin, clone);
            layer.origin.insert(clone, origin);
            let root = self.initial_origins.get(&origin).copied().unwrap_or(origin);
            self.initial_origins.insert(clone, root);
            if layer.clone_start_time.is_none_or(|t| origin.time < t) {
                layer.clone_start_time = Some(origin.time);
            }
            if layer.clone_end_time.is_none_or(|t| t < origin.time) {
                layer.clone_end_time = Some(origin.time);
            }
        }
        let mut clones: Vec<StateId> = layer.clone.values().copied().collect();
        clones.sort_unstable();
        self.layers.push(layer);
        Ok(clones)
    }

    /// The emission cost of a state through every layer (`EnlargedEmissionCostModel`),
    /// with `base` the original model's.
    ///
    /// # Errors
    ///
    /// As `base` fails.
    pub(crate) fn emission(
        &mut self,
        id: StateId,
        base: &mut impl FnMut(StateId) -> Result<f32, MeiliError>,
    ) -> Result<f32, MeiliError> {
        self.emission_at(self.layers.len(), id, base)
    }

    /// The emission cost through the first `n` layers.
    fn emission_at(
        &mut self,
        n: usize,
        id: StateId,
        base: &mut impl FnMut(StateId) -> Result<f32, MeiliError>,
    ) -> Result<f32, MeiliError> {
        let Some(index) = n.checked_sub(1) else {
            return base(id);
        };
        let layer = &self.layers[index];
        let origin = layer.origin.get(&id).copied();
        // Only this layer's clones are checked for removed states, before the cache.
        if origin.is_some_and(|o| self.removed.contains(&o)) {
            return Ok(-1.0);
        }
        if let Some(&cost) = layer.emission_cache.get(&id) {
            return Ok(cost);
        }
        let cost = match origin {
            // The last clone is unusable.
            Some(_) if Some(id.time) == layer.clone_end_time => -1.0,
            Some(origin) => self.emission_at(index, origin, base)?,
            // The first cloned original is unusable.
            None if Some(id.time) == layer.clone_start_time && layer.clone.contains_key(&id) => {
                -1.0
            }
            None => self.emission_at(index, id, base)?,
        };
        self.layers[index].emission_cache.insert(id, cost);
        Ok(cost)
    }

    /// The states whose original transition cost a transition has, through every layer
    /// (`EnlargedTransitionCostModel`).
    pub(crate) fn resolve_transition(
        &self,
        lhs: StateId,
        rhs: StateId,
    ) -> Resolved<(StateId, StateId)> {
        let (mut lhs, mut rhs) = (lhs, rhs);
        for layer in self.layers.iter().rev() {
            let origin_lhs = layer.origin.get(&lhs).copied();
            let origin_rhs = layer.origin.get(&rhs).copied();
            match (origin_lhs, origin_rhs) {
                (Some(l), Some(r)) => (lhs, rhs) = (l, r),
                // A clone may continue into an original, unless the original has a clone.
                (Some(l), None) => {
                    if layer.clone.contains_key(&rhs) {
                        return Resolved::Invalid;
                    }
                    lhs = l;
                }
                // An original never continues into a clone.
                (None, Some(_)) => return Resolved::Invalid,
                (None, None) => {}
            }
        }
        Resolved::Original((lhs, rhs))
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "the costs are exact")]
mod tests {
    use super::*;

    const fn s(time: u32, id: u32) -> StateId {
        StateId { time, id }
    }

    #[test]
    fn clones_a_path_and_blocks_it() {
        let mut topk = TopK::default();
        let path = [Some(s(2, 0)), Some(s(1, 1)), Some(s(0, 0))];
        let clones = topk.remove_path(&path).unwrap();
        assert_eq!(clones, [s(0, u32::MAX), s(1, u32::MAX), s(2, u32::MAX)]);
        assert_eq!(topk.origin(s(1, u32::MAX)), Some(s(1, 1)));
        assert_eq!(topk.origin(s(1, 1)), None);
        // The first original and the last clone are unusable; the rest resolve.
        // (The base cost here is the state's ID.)
        let mut base = |id: StateId| Ok(f32::from(u8::try_from(id.id).unwrap_or(99)));
        assert_eq!(topk.emission(s(0, 0), &mut base).unwrap(), -1.0);
        assert_eq!(topk.emission(s(2, u32::MAX), &mut base).unwrap(), -1.0);
        assert_eq!(topk.emission(s(1, u32::MAX), &mut base).unwrap(), 1.0);
        assert_eq!(topk.emission(s(2, 0), &mut base).unwrap(), 0.0);
        // Clones continue into clones and into originals without clones, never the reverse.
        assert!(matches!(
            topk.resolve_transition(s(0, u32::MAX), s(1, u32::MAX)),
            Resolved::Original(p) if p == (s(0, 0), s(1, 1))
        ));
        assert!(matches!(
            topk.resolve_transition(s(0, u32::MAX), s(1, 1)),
            Resolved::Invalid
        ));
        assert!(matches!(
            topk.resolve_transition(s(0, u32::MAX), s(1, 2)),
            Resolved::Original(p) if p == (s(0, 0), s(1, 2))
        ));
        assert!(matches!(
            topk.resolve_transition(s(0, 1), s(1, u32::MAX)),
            Resolved::Invalid
        ));
        // A second layer clones clones, and its IDs count down.
        let clones = topk
            .remove_path(&[Some(s(1, u32::MAX)), None, Some(s(0, 3))])
            .unwrap();
        assert_eq!(clones, [s(0, u32::MAX - 1), s(1, u32::MAX - 1)]);
        assert_eq!(topk.origin(s(1, u32::MAX - 1)), Some(s(1, 1)));
        // A removed original makes its clones unusable, but not where an upper layer has
        // already cached a clone's cost, as in mainline.
        let mut base = |id: StateId| Ok(f32::from(u8::try_from(id.id).unwrap_or(99)));
        assert_eq!(topk.emission(s(1, u32::MAX), &mut base).unwrap(), 1.0);
        topk.remove(s(1, 1));
        assert_eq!(topk.emission(s(1, u32::MAX), &mut base).unwrap(), 1.0);
        assert_eq!(
            topk.emission_at(1, s(1, u32::MAX), &mut base).unwrap(),
            -1.0
        );
    }
}
