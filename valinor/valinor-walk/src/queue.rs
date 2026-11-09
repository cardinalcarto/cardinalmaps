//! A double bucket queue over integer seconds: a ring of buckets indexed by clock.
//!
//! The search pops labels in clock order, and each edge costs at most [`crate::graph::MAX_SECS`]
//! seconds, so a ring wider than that plus the spread of the seeds never collides.
//! Buckets are intrusive lists through one flat pool of entries,
//! so pushing allocates nothing once the pool is warm.
//! Which buckets are occupied is a bitset, so jumping across quiet stretches of the clock
//! is a few `trailing_zeros` calls.

const NONE: u32 = u32::MAX;

/// A ring of buckets, in seconds.
#[derive(Debug, Default)]
pub(crate) struct DialQueue {
    /// The first entry of each bucket, or [`NONE`]. The length is a power of two.
    heads: Vec<u32>,
    /// Which buckets hold entries.
    occupied: Vec<u64>,
    /// `(node, next entry of the same bucket)`.
    entries: Vec<(u32, u32)>,
    /// `heads.len() - 1`.
    mask: usize,
    /// The clock at offset zero.
    base: i32,
    /// The offset of the current bucket from `base`.
    current: u64,
    /// How many entries are in the queue.
    pending: usize,
}

impl DialQueue {
    /// Prepares the queue for a search whose clocks stay within `[base, base + span)`.
    ///
    /// # Performance
    ///
    /// Allocates only when `span` is larger than any seen so far.
    pub(crate) fn reset(&mut self, base: i32, span: u64) {
        let needed = usize::try_from(span.max(1 << 17))
            .unwrap_or(usize::MAX)
            .next_power_of_two();
        if self.heads.len() < needed {
            self.heads.clear();
            self.heads.resize(needed, NONE);
            self.occupied.clear();
            self.occupied.resize(needed.div_ceil(64), 0);
        }
        self.mask = self.heads.len() - 1;
        self.base = base;
        self.current = 0;
        debug_assert_eq!(self.pending, 0, "a search must drain or clear its queue");
        self.entries.clear();
        self.pending = 0;
    }

    /// Discards everything in the queue, leaving it ready to be reset.
    pub(crate) fn clear(&mut self) {
        for slot in &mut self.heads {
            *slot = NONE;
        }
        for word in &mut self.occupied {
            *word = 0;
        }
        self.entries.clear();
        self.pending = 0;
    }

    /// Pushes `node` at `clock`, which must not be before the last clock popped.
    #[inline]
    pub(crate) fn push(&mut self, clock: i32, node: u32) {
        let offset = (i64::from(clock) - i64::from(self.base)) as u64;
        let bucket = offset as usize & self.mask;
        let entry = u32::try_from(self.entries.len()).unwrap_or(u32::MAX);
        self.entries.push((node, self.heads[bucket]));
        self.heads[bucket] = entry;
        self.occupied[bucket >> 6] |= 1 << (bucket & 63);
        self.pending += 1;
    }

    /// Pops the entry with the smallest clock, if any.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<(i32, u32)> {
        if self.pending == 0 {
            return None;
        }
        let bucket = self.advance();
        let entry = self.heads[bucket];
        let (node, next) = self.entries[entry as usize];
        self.heads[bucket] = next;
        if next == NONE {
            self.occupied[bucket >> 6] &= !(1 << (bucket & 63));
        }
        self.pending -= 1;
        // The ring is wider than the live span of clocks, so `current` is correct as is.
        let clock = i64::from(self.base) + self.current as i64;
        // The clock fits, as the queue is only given clocks that do.
        Some((i32::try_from(clock).unwrap_or(i32::MAX), node))
    }

    /// Moves `current` to the next occupied bucket, returning its ring position.
    ///
    /// There is at least one.
    #[inline]
    fn advance(&mut self) -> usize {
        let bucket = self.current as usize & self.mask;
        let mut word = bucket >> 6;
        let bits = self.occupied[word] & (!0u64 << (bucket & 63));
        if bits != 0 {
            let found = (word << 6) | bits.trailing_zeros() as usize;
            self.current += (found - bucket) as u64;
            return found;
        }
        // The ring can wrap around, so look at each word once, starting after this one.
        let words = self.occupied.len();
        let mut scanned = 1u64;
        loop {
            word = (word + 1) % words;
            if self.occupied[word] != 0 {
                let found = (word << 6) | self.occupied[word].trailing_zeros() as usize;
                let gap = (found + self.heads.len() - bucket) & self.mask;
                self.current += gap as u64;
                return found;
            }
            scanned += 1;
            if scanned > words as u64 + 1 {
                // Unreachable: `pending > 0` means some bucket is occupied.
                return bucket;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drained(queue: &mut DialQueue) -> Vec<(i32, u32)> {
        let mut out = Vec::new();
        while let Some(e) = queue.pop() {
            out.push(e);
        }
        out
    }

    #[test]
    fn pops_in_clock_order_with_a_gap() {
        let mut q = DialQueue::default();
        q.reset(1000, 1 << 16);
        q.push(1010, 1);
        q.push(1000, 2);
        q.push(60000, 3);
        assert_eq!(q.pop(), Some((1000, 2)));
        q.push(1005, 4);
        assert_eq!(drained(&mut q), vec![(1005, 4), (1010, 1), (60000, 3)]);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn a_zero_step_comes_out_of_the_same_bucket() {
        let mut q = DialQueue::default();
        q.reset(0, 1 << 16);
        q.push(5, 1);
        assert_eq!(q.pop(), Some((5, 1)));
        q.push(5, 2);
        assert_eq!(q.pop(), Some((5, 2)));
    }

    #[test]
    fn negative_clocks_order_correctly() {
        let mut q = DialQueue::default();
        q.reset(-500, 1 << 16);
        q.push(-20, 1);
        q.push(-500, 2);
        q.push(-499, 3);
        assert_eq!(drained(&mut q), vec![(-500, 2), (-499, 3), (-20, 1)]);
    }

    #[test]
    fn a_reused_queue_is_clean_after_clear() {
        let mut q = DialQueue::default();
        q.reset(0, 1 << 16);
        q.push(5, 1);
        q.clear();
        q.reset(0, 1 << 16);
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn wraps_around_the_ring_across_many_pushes() {
        let mut q = DialQueue::default();
        q.reset(0, 1 << 16);
        let mut clock = 0;
        for i in 0..200_000u32 {
            q.push(clock + 5, i);
            let popped = q.pop().expect("one entry");
            assert_eq!(popped, (clock + 5, i));
            clock += 5;
        }
    }
}
