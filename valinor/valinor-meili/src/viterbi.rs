//! The Viterbi search over the states of a trace (mainline's `ViterbiSearch`):
//! a lazy Dijkstra over the trellis of columns, which finds the winner of each column
//! (the state with the cheapest path to it) column by column, and starts again after a column
//! no path reaches (a breakage).
//!
//! A state's cost so far is a double, the sum of its predecessor's and the (float) transition
//! and emission costs; negative costs mean "no transition" or "removed".
//! Mainline's queue is a Fibonacci heap, whose order among equal costs is its own;
//! here equal costs come out in the order they were pushed.
//! Only exact ties are affected.

use alloc::collections::BinaryHeap;
use core::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::error::MeiliError;
use crate::match_result::StateId;

/// A state reached by the search (mainline's `StateLabel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StateLabel {
    pub(crate) costsofar: f64,
    pub(crate) stateid: StateId,
    pub(crate) predecessor: Option<StateId>,
}

/// The scanned states: what `Predecessor` and `AccumulatedCost` read.
pub(crate) type Scanned = HashMap<StateId, StateLabel>;

/// The emission and transition costs the search asks for (mainline's cost model functions).
///
/// A negative cost means the state or transition isn't usable.
pub(crate) trait CostModel {
    /// The emission cost of a state.
    ///
    /// # Errors
    ///
    /// Fails if the cost can't be computed (a tile can't be read).
    fn emission(&mut self, state: StateId) -> Result<f32, MeiliError>;

    /// The transition cost from `lhs` to `rhs`, given the states scanned so far
    /// (which the route from `lhs` starts from).
    ///
    /// # Errors
    ///
    /// As for [`CostModel::emission`].
    fn transition(
        &mut self,
        scanned: &Scanned,
        lhs: StateId,
        rhs: StateId,
    ) -> Result<f32, MeiliError>;
}

/// A queue entry: cheapest first, then first pushed.
#[derive(Clone, Copy, Debug)]
struct Entry {
    label: StateLabel,
    seq: u64,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Entry {}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed, so that the max-heap pops the cheapest, then the earliest.
        other
            .label
            .costsofar
            .total_cmp(&self.label.costsofar)
            .then(other.seq.cmp(&self.seq))
    }
}

/// A priority queue with one entry per state, whose cost only decreases
/// (mainline's `SPQueue`, a Fibonacci heap with handles).
#[derive(Debug, Default)]
struct Queue {
    heap: BinaryHeap<Entry>,
    /// The sequence number and cost of each state's live entry.
    live: HashMap<StateId, (u64, f64)>,
    seq: u64,
}

impl Queue {
    fn clear(&mut self) {
        self.heap.clear();
        self.live.clear();
    }

    /// Adds a label, or lowers its state's cost if it is lower (`push`).
    /// The replaced entry stays in the heap and is skipped when it comes out.
    fn push(&mut self, label: StateLabel) {
        if let Some(&(_, current)) = self.live.get(&label.stateid)
            && label.costsofar.partial_cmp(&current) != Some(Ordering::Less)
        {
            return;
        }
        self.seq += 1;
        self.live.insert(label.stateid, (self.seq, label.costsofar));
        self.heap.push(Entry {
            label,
            seq: self.seq,
        });
    }

    /// Takes the cheapest label (`top` and `pop`).
    fn pop(&mut self) -> Option<StateLabel> {
        while let Some(entry) = self.heap.pop() {
            if self.live.get(&entry.label.stateid).map(|&(seq, _)| seq) == Some(entry.seq) {
                self.live.remove(&entry.label.stateid);
                return Some(entry.label);
            }
        }
        None
    }
}

/// Mainline's `ViterbiSearch` (with `IViterbiSearch`'s state bookkeeping).
#[derive(Debug, Default)]
pub(crate) struct Viterbi {
    added: HashSet<StateId>,
    states_by_time: Vec<Vec<StateId>>,
    unreached: Vec<Vec<StateId>>,
    scanned: Scanned,
    queue: Queue,
    earliest_time: u32,
    winner_by_time: Vec<Option<StateId>>,
}

#[inline]
fn is_invalid_cost(cost: f64) -> bool {
    cost < 0.0
}

impl Viterbi {
    /// Forgets every state and search (`Clear`).
    pub(crate) fn clear(&mut self) {
        self.added.clear();
        self.states_by_time.clear();
        self.clear_search();
    }

    /// Forgets the search, keeping the states (`ClearSearch`).
    pub(crate) fn clear_search(&mut self) {
        self.earliest_time = 0;
        self.queue.clear();
        self.scanned.clear();
        self.winner_by_time.clear();
        self.unreached.clone_from(&self.states_by_time);
    }

    /// Adds a state (`AddStateId`); false if it was already there.
    pub(crate) fn add_state(&mut self, id: StateId) -> bool {
        if !self.added.insert(id) {
            return false;
        }
        let time = id.time as usize;
        if self.states_by_time.len() <= time {
            self.states_by_time.resize_with(time + 1, Vec::new);
        }
        self.states_by_time[time].push(id);
        if self.unreached.len() <= time {
            self.unreached.resize_with(time + 1, Vec::new);
        }
        self.unreached[time].push(id);
        true
    }

    /// Removes a state (`RemoveStateId`); the search must be cleared before the next one.
    ///
    /// # Errors
    ///
    /// Fails if the state was added but isn't in its column (which can't happen).
    pub(crate) fn remove_state(&mut self, id: StateId) -> Result<bool, MeiliError> {
        if !self.added.remove(&id) {
            return Ok(false);
        }
        let column = self
            .states_by_time
            .get_mut(id.time as usize)
            .ok_or(MeiliError::Logic("the state must exist in the column"))?;
        let at = column
            .iter()
            .position(|&s| s == id)
            .ok_or(MeiliError::Logic("the state must exist in the column"))?;
        column.remove(at);
        Ok(true)
    }

    /// The scanned states.
    #[inline]
    pub(crate) const fn scanned(&self) -> &Scanned {
        &self.scanned
    }

    /// The predecessor of a scanned state (`Predecessor`).
    #[inline]
    pub(crate) fn predecessor(&self, id: StateId) -> Option<StateId> {
        self.scanned.get(&id).and_then(|l| l.predecessor)
    }

    /// A scanned state's cost so far, or -1 (`AccumulatedCost`).
    #[inline]
    pub(crate) fn accumulated_cost(&self, id: StateId) -> f64 {
        self.scanned.get(&id).map_or(-1.0, |l| l.costsofar)
    }

    /// The winner at a time (`SearchWinner`), searching as far as needed.
    ///
    /// # Errors
    ///
    /// Fails if a cost can't be computed or a consistency check fails.
    pub(crate) fn search_winner(
        &mut self,
        time: u32,
        model: &mut impl CostModel,
    ) -> Result<Option<StateId>, MeiliError> {
        if let Some(&winner) = self.winner_by_time.get(time as usize) {
            return Ok(winner);
        }
        let Some(max_allowed) = self.unreached.len().checked_sub(1) else {
            return Ok(None);
        };
        let target = (time as usize).min(max_allowed);
        let mut searched = self.iterative_search(target, false, model)?;
        while searched < target {
            searched = self.iterative_search(target, true, model)?;
        }
        Ok(self.winner_by_time.get(time as usize).copied().flatten())
    }

    /// The most probable path ending at `time`, from there back to where it starts:
    /// the state at the start has no predecessor, or the winner at `time` is missing
    /// (mainline's `SearchPathVS(time, false)` iterated to `PathEnd`).
    ///
    /// # Errors
    ///
    /// As for [`Viterbi::search_winner`].
    pub(crate) fn search_path(
        &mut self,
        time: u32,
        model: &mut impl CostModel,
        out: &mut Vec<Option<StateId>>,
    ) -> Result<(), MeiliError> {
        let mut time = time;
        let mut state = self.search_winner(time, model)?;
        loop {
            out.push(state);
            if time == 0 {
                return Ok(());
            }
            if let Some(id) = state {
                match self.predecessor(id) {
                    Some(predecessor) => state = Some(predecessor),
                    // Without breaks, the path ends at a state without a predecessor.
                    None => return Ok(()),
                }
                time -= 1;
            } else {
                time -= 1;
                state = self.search_winner(time, model)?;
            }
        }
    }

    /// `InitQueue`.
    fn init_queue(&mut self, source: usize, model: &mut impl CostModel) -> Result<(), MeiliError> {
        self.queue.clear();
        let column = self.unreached.get(source).map_or(&[][..], Vec::as_slice);
        for &id in column {
            let emission = f64::from(model.emission(id)?);
            if is_invalid_cost(emission) {
                continue;
            }
            self.queue.push(StateLabel {
                costsofar: emission,
                stateid: id,
                predecessor: None,
            });
        }
        Ok(())
    }

    /// `AddSuccessorsToQueue`.
    fn add_successors(
        &mut self,
        id: StateId,
        model: &mut impl CostModel,
    ) -> Result<(), MeiliError> {
        let next_time = id.time as usize + 1;
        if next_time >= self.unreached.len() {
            return Err(MeiliError::Logic(
                "the state is impossible to have successors",
            ));
        }
        let costsofar = self
            .scanned
            .get(&id)
            .ok_or(MeiliError::Logic("the state must be scanned"))?
            .costsofar;
        if is_invalid_cost(costsofar) {
            return Err(MeiliError::Logic(
                "impossible to get invalid cost from scanned labels",
            ));
        }
        let Self {
            unreached,
            scanned,
            queue,
            ..
        } = self;
        for &next in &unreached[next_time] {
            let emission = model.emission(next)?;
            if is_invalid_cost(f64::from(emission)) {
                continue;
            }
            let transition = model.transition(scanned, id, next)?;
            if is_invalid_cost(f64::from(transition)) {
                continue;
            }
            // `prev + transition + emission`, in double precision.
            let next_costsofar = costsofar + f64::from(transition) + f64::from(emission);
            if is_invalid_cost(next_costsofar) {
                continue;
            }
            queue.push(StateLabel {
                costsofar: next_costsofar,
                stateid: next,
                predecessor: Some(id),
            });
        }
        Ok(())
    }

    /// `IterativeSearch`: continues the last search (or starts a new one) until the winner at
    /// `target` is found or no state can be reached, returning the last time searched.
    fn iterative_search(
        &mut self,
        target: usize,
        request_new_start: bool,
        model: &mut impl CostModel,
    ) -> Result<usize, MeiliError> {
        if self.unreached.len() <= target {
            return Err(MeiliError::Logic(
                "the target time is beyond the maximum allowed time",
            ));
        }
        if target < self.winner_by_time.len() {
            return Ok(target);
        }
        let source = match self.winner_by_time.last() {
            Some(&Some(winner)) if !request_new_start => {
                self.add_successors(winner, model)?;
                self.winner_by_time.len() - 1
            }
            _ => {
                let source = self.winner_by_time.len();
                self.init_queue(source, model)?;
                source
            }
        };
        let mut searched = source;
        while let Some(label) = self.queue.pop() {
            let id = label.stateid;
            if id.time < self.earliest_time {
                continue;
            }
            if self.scanned.insert(id, label).is_some() {
                return Err(MeiliError::Logic(
                    "the principle of optimality is violated in the viterbi search",
                ));
            }
            let time = id.time as usize;
            let column = self
                .unreached
                .get_mut(time)
                .ok_or(MeiliError::Logic("the state must exist in the column"))?;
            let at = column
                .iter()
                .position(|&s| s == id)
                .ok_or(MeiliError::Logic("the state must exist in the column"))?;
            column.remove(at);
            if column.is_empty() {
                self.earliest_time = id.time + 1;
            }
            if self.winner_by_time.len() <= time {
                if time != self.winner_by_time.len() {
                    return Err(MeiliError::Logic("found a state from the future time"));
                }
                self.winner_by_time.push(Some(id));
            }
            searched = searched.max(time);
            if target <= searched {
                break;
            }
            self.add_successors(id, model)?;
        }
        if self.winner_by_time.len() <= searched {
            self.winner_by_time.resize(searched + 1, None);
        }
        Ok(searched)
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "the costs are exact")]
mod tests {
    use super::*;

    /// Emission is the state's ID; transitions cost the difference of IDs,
    /// except that `broken` transitions don't exist.
    struct Model {
        broken: Vec<(StateId, StateId)>,
    }

    impl CostModel for Model {
        fn emission(&mut self, state: StateId) -> Result<f32, MeiliError> {
            Ok(f32::from(u8::try_from(state.id).unwrap()))
        }
        fn transition(
            &mut self,
            _: &Scanned,
            lhs: StateId,
            rhs: StateId,
        ) -> Result<f32, MeiliError> {
            if self.broken.contains(&(lhs, rhs)) {
                return Ok(-1.0);
            }
            Ok(f32::from(u8::try_from(lhs.id.abs_diff(rhs.id)).unwrap()))
        }
    }

    const fn s(time: u32, id: u32) -> StateId {
        StateId { time, id }
    }

    #[test]
    fn finds_the_cheapest_path() {
        let mut v = Viterbi::default();
        for t in 0..3 {
            for i in 0..3 {
                assert!(v.add_state(s(t, i)));
            }
        }
        assert!(!v.add_state(s(0, 0)));
        let mut model = Model { broken: vec![] };
        let mut path = Vec::new();
        v.search_path(2, &mut model, &mut path).unwrap();
        assert_eq!(path, [Some(s(2, 0)), Some(s(1, 0)), Some(s(0, 0))]);
        assert_eq!(v.accumulated_cost(s(2, 0)), 0.0);
    }

    #[test]
    fn restarts_after_a_breakage() {
        let mut v = Viterbi::default();
        for t in 0..3 {
            v.add_state(s(t, 0));
        }
        let mut model = Model {
            broken: vec![(s(0, 0), s(1, 0))],
        };
        let mut path = Vec::new();
        v.search_path(2, &mut model, &mut path).unwrap();
        // Time 1 starts a new search, so the path stops there.
        assert_eq!(path, [Some(s(2, 0)), Some(s(1, 0))]);
        assert_eq!(v.predecessor(s(1, 0)), None);
        path.clear();
        v.search_path(0, &mut model, &mut path).unwrap();
        assert_eq!(path, [Some(s(0, 0))]);
    }

    #[test]
    fn a_column_without_states_has_no_winner() {
        let mut v = Viterbi::default();
        v.add_state(s(0, 0));
        v.add_state(s(2, 0));
        let mut model = Model { broken: vec![] };
        let mut path = Vec::new();
        v.search_path(2, &mut model, &mut path).unwrap();
        assert_eq!(path, [Some(s(2, 0))]);
        path.clear();
        v.search_path(1, &mut model, &mut path).unwrap();
        assert_eq!(path, [None, Some(s(0, 0))]);
        // Past the last column with states, nothing.
        assert_eq!(v.search_winner(5, &mut model).unwrap(), None);
    }

    #[test]
    fn queue_keeps_the_lowest_cost_per_state() {
        let mut q = Queue::default();
        let label = |cost, id| StateLabel {
            costsofar: cost,
            stateid: s(0, id),
            predecessor: None,
        };
        q.push(label(5.0, 1));
        q.push(label(7.0, 1));
        q.push(label(3.0, 2));
        q.push(label(2.0, 1));
        assert_eq!(q.pop(), Some(label(2.0, 1)));
        assert_eq!(q.pop(), Some(label(3.0, 2)));
        assert_eq!(q.pop(), None);
        // A popped state can be pushed again.
        q.push(label(9.0, 1));
        assert_eq!(q.pop(), Some(label(9.0, 1)));
    }
}
