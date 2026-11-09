//! Rules for walking between two particular stops, from `transfers.txt`.
//!
//! The rows are few compared with stops, so the transfer search reads them only for the seeds
//! that have some: a stop with no row costs one comparison per walk, nothing more.

use crate::error::BuildError;
use crate::ids::{Direction, Seconds, StopId};
use crate::timetable::arena::offset_of;

/// A rule for walking from one stop to another, by stop numbers of its timetable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferPair {
    /// The stop the walk starts at.
    pub from: StopId,
    /// The stop the walk ends at; never the same as `from`.
    pub to: StopId,
    /// The walk's cost is at least this.
    pub floor: Seconds,
}

/// One direction of [`TransferPairs`], as flat columns keyed by stop.
#[derive(Debug, Clone, Default)]
struct PairRows {
    /// `offsets[s]..offsets[s + 1]` is stop `s`'s run of the other columns.
    offsets: Vec<u32>,
    stops: Vec<u32>,
    floors: Vec<i32>,
}

impl PairRows {
    fn build(stop_count: usize, rows: &mut [(u32, u32, i32)]) -> Result<Self, BuildError> {
        rows.sort_unstable();
        let mut offsets = vec![0_u32; stop_count + 1];
        for &(key, ..) in rows.iter() {
            if key as usize >= stop_count {
                return Err(BuildError::UnknownStopId(StopId::new(key)));
            }
            offsets[key as usize + 1] += 1;
        }
        for index in 1..offsets.len() {
            offsets[index] += offsets[index - 1];
        }
        Ok(Self {
            offsets,
            stops: rows.iter().map(|row| row.1).collect(),
            floors: rows.iter().map(|row| row.2).collect(),
        })
    }

    #[inline]
    fn row(&self, stop: usize) -> (&[u32], &[i32]) {
        let (Some(&start), Some(&end)) = (self.offsets.get(stop), self.offsets.get(stop + 1))
        else {
            return (&[], &[]);
        };
        let range = start as usize..end as usize;
        (&self.stops[range.clone()], &self.floors[range])
    }
}

/// Every [`TransferPair`] of a window, indexed by either stop of the pair.
///
/// Built once when a window is materialized; a snapshot's overlay stops have no rows.
#[derive(Debug, Clone, Default)]
pub struct TransferPairs {
    outgoing: PairRows,
    incoming: PairRows,
}

impl TransferPairs {
    /// Indexes `rows`, one per directed pair of different stops.
    ///
    /// # Errors
    ///
    /// If a row names a stop past `stop_count`, joins a stop to itself, or there are too many.
    pub fn new(stop_count: usize, rows: &[TransferPair]) -> Result<Self, BuildError> {
        let mut outgoing = Vec::with_capacity(rows.len());
        let mut incoming = Vec::with_capacity(rows.len());
        for row in rows {
            if row.from == row.to {
                return Err(BuildError::TransferToSameStop(row.from));
            }
            let floor = row.floor.as_i32();
            outgoing.push((row.from.into_inner(), row.to.into_inner(), floor));
            incoming.push((row.to.into_inner(), row.from.into_inner(), floor));
        }
        offset_of(rows.len())?;
        Ok(Self {
            outgoing: PairRows::build(stop_count, &mut outgoing)?,
            incoming: PairRows::build(stop_count, &mut incoming)?,
        })
    }

    /// Whether any pair of stops has a rule.
    #[cfg(test)]
    pub(crate) fn has_rows(&self) -> bool {
        !self.outgoing.stops.is_empty()
    }

    /// The rules seen from a seed: walks leaving it when searching forward,
    /// and walks into it when searching backward.
    #[inline]
    pub(crate) fn of_seed(&self, seed: StopId, direction: Direction) -> (&[u32], &[i32]) {
        match direction {
            Direction::Forward => self.outgoing.row(seed.index()),
            Direction::Backward => self.incoming.row(seed.index()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Seconds;

    fn pair(from: u32, to: u32, floor: u32) -> TransferPair {
        TransferPair {
            from: StopId::new(from),
            to: StopId::new(to),
            floor: Seconds::try_new(floor).expect("in range"),
        }
    }

    #[test]
    fn each_seed_sees_its_own_rows_in_both_directions() {
        let pairs =
            TransferPairs::new(4, &[pair(2, 1, 30), pair(0, 3, 90), pair(2, 3, 60)]).expect("ok");
        assert_eq!(
            pairs.of_seed(StopId::new(2), Direction::Forward),
            (&[1, 3][..], &[30, 60][..])
        );
        assert_eq!(
            pairs.of_seed(StopId::new(3), Direction::Backward),
            (&[0, 2][..], &[90, 60][..])
        );
        assert_eq!(
            pairs.of_seed(StopId::new(1), Direction::Forward),
            (&[][..], &[][..])
        );
        // A stop past the table has no rows.
        assert_eq!(
            pairs.of_seed(StopId::new(9), Direction::Forward),
            (&[][..], &[][..])
        );
    }

    #[test]
    fn a_pair_of_one_stop_is_refused() {
        assert_eq!(
            TransferPairs::new(2, &[pair(1, 1, 30)]).err(),
            Some(BuildError::TransferToSameStop(StopId::new(1)))
        );
    }

    #[test]
    fn a_stop_past_the_window_is_refused() {
        assert_eq!(
            TransferPairs::new(2, &[pair(0, 5, 30)]).err(),
            Some(BuildError::UnknownStopId(StopId::new(5)))
        );
    }
}
