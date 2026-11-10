//! Access and egress stop tables, and the route filter: the per-profile inputs to a search.

use crate::error::BuildError;
use crate::ids::{Seconds, StopId};
use crate::timetable::{
    INFINITY, RULE_DROP_OFF, RULE_PICKUP, RULE_WHEELCHAIR_SHIFT, RouteAttributes, StopRules,
};

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
    /// The reachable stops, each once. Their costs live in `costs`,
    /// so relaxing a stop's cost repeatedly takes constant time each.
    reachable: Vec<StopId>,
}

impl StopTable {
    /// An empty table for a snapshot of `stop_count` stops.
    ///
    /// Use the snapshot's [`crate::Snapshot::stop_count`], which counts overlay stops.
    pub fn new(stop_count: usize) -> Self {
        Self {
            costs: vec![INFINITY; stop_count],
            reachable: Vec::new(),
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
            self.reachable.push(stop);
        }
        *slot = (*slot).min(cost);
        Ok(())
    }

    /// Whether no stop is reachable.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.reachable.is_empty()
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
    pub(crate) fn entries(&self) -> impl Iterator<Item = (StopId, i32)> + '_ {
        self.reachable
            .iter()
            .map(|&stop| (stop, self.costs[stop.index()]))
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

/// Which routes a query may ride, and which stops a rider may use.
///
/// Wheelchair exclusions are decided when a lane is laid out, by the bits of its rule bytes;
/// a query only picks which bits to read, so a non-wheelchair query pays nothing for this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RouteFilter {
    required: RouteAttributes,
    wheelchair: bool,
}

impl RouteFilter {
    /// Every route.
    pub const ALL: Self = Self {
        required: RouteAttributes::NONE,
        wheelchair: false,
    };

    /// A rider in a wheelchair: routes not marked inaccessible, and stops not marked inaccessible.
    pub const WHEELCHAIR: Self = Self {
        required: RouteAttributes::WHEELCHAIR_ACCESSIBLE,
        wheelchair: true,
    };

    /// Routes that have every one of `required`.
    pub const fn requiring(required: RouteAttributes) -> Self {
        Self {
            required,
            wheelchair: false,
        }
    }

    /// Whether a route with these attributes may be ridden.
    #[inline]
    pub fn allows(self, attributes: RouteAttributes) -> bool {
        attributes.contains(self.required)
    }

    /// The rules a lane's rule byte gives a rider of this query: its wheelchair copy for a
    /// wheelchair query, which a lane clears where a stop or trip is marked inaccessible.
    #[inline]
    pub(crate) fn stop_rules(self, bits: u8) -> StopRules {
        let shift = if self.wheelchair {
            RULE_WHEELCHAIR_SHIFT
        } else {
            0
        };
        StopRules::decode(bits >> shift)
    }

    /// Whether this query uses the wheelchair rule bits.
    #[cfg(test)]
    pub(crate) fn is_wheelchair(self) -> bool {
        self.wheelchair
    }

    /// `(can_alight, can_board)` masks of a lane's rule bytes, in a search's direction.
    #[inline]
    pub(crate) fn rule_masks<const BACK: bool>(self) -> (u8, u8) {
        let shift = if self.wheelchair {
            RULE_WHEELCHAIR_SHIFT
        } else {
            0
        };
        let (alight, board) = if BACK {
            (RULE_PICKUP, RULE_DROP_OFF)
        } else {
            (RULE_DROP_OFF, RULE_PICKUP)
        };
        (alight << shift, board << shift)
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
        assert_eq!(
            table.entries().collect::<Vec<_>>(),
            vec![(StopId::new(1), 20)]
        );
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
