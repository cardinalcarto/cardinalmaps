//! Access and egress stop tables, and the route filter: the per-profile inputs to a search.

use crate::error::BuildError;
use crate::ids::{Seconds, StopId};
use crate::timetable::{INFINITY, RouteAttributes};

/// Time-independent costs between one end of a journey and every reachable stop.
///
/// A table is computed once per profile and shared by its forward and backward searches.
/// Access is seconds from the origin to each stop; egress is seconds from each stop to the target.
///
/// A table must cover every stop riders could use:
/// the search does not walk between the access table and the first boarding,
/// or between the last alighting and the egress table.
#[derive(Debug, Clone)]
pub struct StopTable {
    /// Dense by stop; [`INFINITY`] where the stop isn't reachable.
    costs: Vec<i32>,
    entries: Vec<(StopId, i32)>,
}

impl StopTable {
    /// An empty table for a snapshot of `stop_count` stops.
    ///
    /// Use the snapshot's [`crate::Snapshot::stop_count`], which counts overlay stops.
    pub fn new(stop_count: usize) -> Self {
        Self {
            costs: vec![INFINITY; stop_count],
            entries: Vec::new(),
        }
    }

    /// Sets a stop's cost, keeping the smaller if it is set twice.
    ///
    /// # Errors
    ///
    /// If the stop isn't in this table.
    pub fn set(&mut self, stop: StopId, cost: Seconds) -> Result<(), BuildError> {
        let index = stop.index();
        let slot = self
            .costs
            .get_mut(index)
            .ok_or(BuildError::UnknownStopId(stop))?;
        let cost = cost.as_i32();
        if *slot == INFINITY {
            self.entries.push((stop, cost));
            *slot = cost;
        } else if cost < *slot {
            *slot = cost;
            for entry in &mut self.entries {
                if entry.0 == stop {
                    entry.1 = cost;
                }
            }
        }
        Ok(())
    }

    /// How many stops the table has room for.
    #[inline]
    pub fn stop_count(&self) -> usize {
        self.costs.len()
    }

    /// The cost at each stop; unreachable stops are above every real cost.
    #[inline]
    pub(crate) fn costs(&self) -> &[i32] {
        &self.costs
    }

    /// The reachable stops and their costs.
    #[inline]
    pub(crate) fn entries(&self) -> &[(StopId, i32)] {
        &self.entries
    }

    /// The cost at a stop, if it is reachable.
    pub fn cost(&self, stop: StopId) -> Option<Seconds> {
        let cost = *self.costs.get(stop.index())?;
        if cost == INFINITY {
            return None;
        }
        Seconds::try_new(cost.cast_unsigned()).ok()
    }
}

/// Which routes a query may ride.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteFilter {
    required: RouteAttributes,
}

impl RouteFilter {
    /// Every route.
    pub const ALL: Self = Self {
        required: RouteAttributes::NONE,
    };

    /// Routes that have every one of `required`.
    pub const fn requiring(required: RouteAttributes) -> Self {
        Self { required }
    }

    /// Whether a route with these attributes may be ridden.
    #[inline]
    pub(crate) fn allows(self, attributes: RouteAttributes) -> bool {
        attributes.contains(self.required)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u32) -> Seconds {
        Seconds::try_new(s).expect("in range")
    }

    #[test]
    fn smaller_cost_wins_and_entries_stay_unique() {
        let mut table = StopTable::new(3);
        table.set(StopId::new(1), secs(50)).expect("set");
        table.set(StopId::new(1), secs(20)).expect("set");
        table.set(StopId::new(1), secs(90)).expect("set");
        assert_eq!(table.cost(StopId::new(1)), Some(secs(20)));
        assert_eq!(table.cost(StopId::new(0)), None);
        assert_eq!(table.entries(), &[(StopId::new(1), 20)]);
    }

    #[test]
    fn out_of_range_stop_is_an_error() {
        let mut table = StopTable::new(1);
        assert_eq!(
            table.set(StopId::new(1), secs(0)),
            Err(BuildError::UnknownStopId(StopId::new(1))),
        );
    }
}
