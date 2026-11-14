//! The measurements and their candidates, with the routes found from each candidate
//! (mainline's `StateContainer` and `State`).

use crate::candidate_search::Candidate;
use crate::match_result::StateId;
use crate::measurement::Measurement;
use crate::routing::Label;

/// The routes from a state to the states of the next column (mainline's `State::SetRoute`):
/// the search's labels, and the label that reached each state of the next column.
#[derive(Debug)]
pub(crate) struct Route {
    pub(crate) labels: Vec<Label>,
    pub(crate) dests: Vec<Option<u32>>,
}

/// A candidate of a measurement, and its routes once found (mainline's `State`).
#[derive(Debug)]
pub(crate) struct State {
    pub(crate) candidate: Candidate,
    pub(crate) route: Option<Route>,
}

impl State {
    /// Whether the routes from this state have been searched (`routed`).
    #[inline]
    pub(crate) const fn routed(&self) -> bool {
        self.route.is_some()
    }
}

/// The trace's columns: per (non-interpolated) measurement, its candidates
/// (mainline's `StateContainer`).
#[derive(Debug, Default)]
pub(crate) struct Container {
    measurements: Vec<Measurement>,
    leave_times: Vec<f64>,
    columns: Vec<Vec<State>>,
    /// Label storage of earlier routes, kept for reuse.
    spare_labels: Vec<Vec<Label>>,
}

impl Container {
    /// Empties the container, keeping the routes' storage (`Clear`).
    pub(crate) fn clear(&mut self) {
        self.measurements.clear();
        self.leave_times.clear();
        for column in self.columns.drain(..) {
            for state in column {
                if let Some(route) = state.route {
                    self.spare_labels.push(route.labels);
                }
            }
        }
    }

    /// Label storage for a new route.
    pub(crate) fn take_labels(&mut self) -> Vec<Label> {
        let mut labels = self.spare_labels.pop().unwrap_or_default();
        labels.clear();
        labels
    }

    /// The number of columns (`size`).
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.columns.len()
    }

    /// A state (`state`); `None` for one that doesn't exist (mainline reads out of bounds).
    #[inline]
    pub(crate) fn state(&self, id: StateId) -> Option<&State> {
        self.columns.get(id.time as usize)?.get(id.id as usize)
    }

    #[inline]
    pub(crate) fn state_mut(&mut self, id: StateId) -> Option<&mut State> {
        self.columns
            .get_mut(id.time as usize)?
            .get_mut(id.id as usize)
    }

    /// A column's states (`column`).
    #[inline]
    pub(crate) fn column(&self, time: u32) -> &[State] {
        self.columns.get(time as usize).map_or(&[], Vec::as_slice)
    }

    /// A column's measurement (`measurement`).
    #[inline]
    pub(crate) fn measurement(&self, time: u32) -> Option<&Measurement> {
        self.measurements.get(time as usize)
    }

    /// When the trace left a column's measurement (`leave_time`): its time, or the time of
    /// the last measurement interpolated after it when the trace lingered there.
    #[inline]
    pub(crate) fn leave_time(&self, time: u32) -> Option<f64> {
        self.leave_times.get(time as usize).copied()
    }

    /// `SetMeasurementLeaveTime`.
    pub(crate) fn set_leave_time(&mut self, time: u32, leave_time: f64) {
        if let Some(t) = self.leave_times.get_mut(time as usize) {
            *t = leave_time;
        }
    }

    /// Adds a column for a measurement (`AppendMeasurement`), returning its time.
    pub(crate) fn append_measurement(&mut self, measurement: Measurement) -> u32 {
        let time = u32::try_from(self.measurements.len()).unwrap_or(u32::MAX);
        self.measurements.push(measurement);
        self.leave_times.push(measurement.epoch_time());
        self.columns.push(Vec::new());
        time
    }

    /// Adds a candidate to the last column (`AppendCandidate`), returning its state ID.
    pub(crate) fn append_candidate(&mut self, candidate: Candidate) -> Option<StateId> {
        let time = u32::try_from(self.columns.len().checked_sub(1)?).ok()?;
        let column = self.columns.last_mut()?;
        let id = StateId {
            time,
            id: u32::try_from(column.len()).ok()?,
        };
        column.push(State {
            candidate,
            route: None,
        });
        Some(id)
    }

    /// Whether there are at least two columns and a candidate (`HasMinimumCandidates`).
    pub(crate) fn has_minimum_candidates(&self) -> bool {
        self.columns.len() >= 2 && self.columns.iter().any(|c| !c.is_empty())
    }

    /// The label of the route from `from` that reached `to` (`last_label`), if any:
    /// only states of the next column are reached.
    pub(crate) fn last_label(&self, from: StateId, to: StateId) -> Option<(&[Label], u32)> {
        if to.time != from.time.checked_add(1)? {
            return None;
        }
        let route = self.state(from)?.route.as_ref()?;
        let idx = (*route.dests.get(to.id as usize)?)?;
        Some((&route.labels, idx))
    }

    /// The labels of the route from `from` to `to`, from the last back to the origin label
    /// (`RouteBegin` to `RouteEnd`); empty if there is none.
    pub(crate) fn route(&self, from: StateId, to: StateId) -> RouteIter<'_> {
        match self.last_label(from, to) {
            Some((labels, idx)) => RouteIter {
                labels,
                next: Some(idx),
            },
            None => RouteIter {
                labels: &[],
                next: None,
            },
        }
    }
}

/// Labels from a route's end back to its origin (mainline's `RoutePathIterator`).
#[derive(Clone, Debug)]
pub(crate) struct RouteIter<'a> {
    labels: &'a [Label],
    next: Option<u32>,
}

impl<'a> Iterator for RouteIter<'a> {
    type Item = &'a Label;

    fn next(&mut self) -> Option<&'a Label> {
        let label = self.labels.get(self.next? as usize)?;
        self.next = label.predecessor();
        Some(label)
    }
}
