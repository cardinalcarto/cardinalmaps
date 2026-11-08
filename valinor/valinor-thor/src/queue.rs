//! Mainline's approximate priority queue (`baldr::DoubleBucketQueue`).
//!
//! Labels are kept in buckets of [`unit size`](BucketQueue::reuse) cost each,
//! over a range of costs; labels beyond the range wait in an overflow bucket.
//! The queue pops the current bucket's labels last in, first out,
//! and moves to the next bucket when the current one is empty.
//!
//! This is not an exact priority queue: within a bucket, a label with a higher cost
//! can come out before one with a lower cost.
//! Path search settles an edge when it pops it, so that order decides
//! which of two nearly equal paths wins, and with it the edges of the route.
//! So this follows mainline's arithmetic (including where it rounds to `float`)
//! and its order within buckets exactly.

/// What the queue needs to know about a label: its sort cost.
pub(crate) trait SortCost {
    fn sortcost(&self) -> f32;
}

/// Label indexes sorted by sort cost (mainline's `DoubleBucketQueue`).
#[derive(Debug, Default)]
pub(crate) struct BucketQueue {
    bucketrange: f32,
    bucketsize: f32,
    inv: f32,
    mincost: f64,
    maxcost: f32,
    currentcost: f32,
    buckets: Vec<Vec<u32>>,
    current: usize,
    overflow: Vec<u32>,
}

/// Which bucket a cost belongs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bucket {
    Low(usize),
    Overflow,
}

impl BucketQueue {
    /// Empties the queue and sets its cost range (`reuse`): `bucket_count` buckets
    /// of `bucketsize` from `mincost` (rounded down to a multiple of the bucket size).
    ///
    /// The buckets' storage is kept for the next search.
    pub(crate) fn reuse(&mut self, mincost: f32, range: f32, bucketsize: u32) {
        let bucketsize = bucketsize.max(1);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline's static_cast<uint32_t> of a non-negative cost"
        )]
        let c = mincost as u32;
        #[expect(clippy::cast_precision_loss, reason = "mainline stores it in a float")]
        let currentcost = (c - c % bucketsize) as f32;
        self.currentcost = currentcost;
        self.mincost = f64::from(currentcost);
        self.bucketrange = range;
        #[expect(clippy::cast_precision_loss, reason = "mainline stores it in a float")]
        let size = bucketsize as f32;
        self.bucketsize = size;
        self.inv = 1.0 / size;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double sum in a float"
        )]
        let maxcost = (self.mincost + f64::from(range)) as f32;
        self.maxcost = maxcost;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "mainline converts the float count to size_t"
        )]
        let count = (range / size) as usize + 1;
        self.buckets.iter_mut().for_each(Vec::clear);
        self.buckets.resize_with(count, Vec::new);
        self.current = 0;
        self.overflow.clear();
    }

    /// Where a label with this sort cost goes (`get_bucket`).
    #[inline]
    fn bucket_of(&self, cost: f32) -> Bucket {
        if cost < self.currentcost {
            Bucket::Low(self.current.min(self.buckets.len().saturating_sub(1)))
        } else if cost < self.maxcost {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mainline casts to uint32"
            )]
            let index = ((f64::from(cost) - self.mincost) * f64::from(self.inv)) as u32;
            let index = index as usize;
            // Always in range for a cost below `maxcost`;
            // anything else waits in the overflow bucket rather than panicking.
            if index < self.buckets.len() {
                Bucket::Low(index)
            } else {
                Bucket::Overflow
            }
        } else {
            Bucket::Overflow
        }
    }

    #[inline]
    fn bucket_mut(&mut self, bucket: Bucket) -> &mut Vec<u32> {
        match bucket {
            Bucket::Low(i) => match self.buckets.get_mut(i) {
                Some(b) => b,
                None => &mut self.overflow,
            },
            Bucket::Overflow => &mut self.overflow,
        }
    }

    /// Adds a label, by its current sort cost (`add`).
    #[inline]
    pub(crate) fn add(&mut self, label: u32, sortcost: f32) {
        let bucket = self.bucket_of(sortcost);
        self.bucket_mut(bucket).push(label);
    }

    /// Moves a label from the bucket of its old sort cost to that of its new one (`decrease`).
    ///
    /// Call this before updating the label's sort cost.
    #[inline]
    pub(crate) fn decrease(&mut self, label: u32, oldcost: f32, newcost: f32) {
        let old = self.bucket_of(oldcost);
        let new = self.bucket_of(newcost);
        if old != new {
            self.bucket_mut(new).push(label);
            let previous = self.bucket_mut(old);
            if let Some(position) = previous.iter().position(|&l| l == label) {
                previous.remove(position);
            }
        }
    }

    /// Whether every bucket from the current one on is empty, advancing past empty ones.
    fn empty(&mut self) -> bool {
        while self.buckets.get(self.current).is_some_and(Vec::is_empty) {
            self.current += 1;
            self.currentcost += self.bucketsize;
        }
        self.current >= self.buckets.len()
    }

    /// Takes the next label (`pop`), or `None` if the queue is empty.
    pub(crate) fn pop<L: SortCost>(&mut self, labels: &[L]) -> Option<u32> {
        if self.empty() {
            if self.overflow.is_empty() {
                // Mainline steps back to the last bucket (`--currentbucket_`),
                // where later labels below the current cost go.
                self.current = self.buckets.len().saturating_sub(1);
                return None;
            }
            self.empty_overflow(labels);
            if self.empty() {
                return None;
            }
        }
        self.buckets.get_mut(self.current)?.pop()
    }

    /// Refills the buckets from the overflow bucket, from its cheapest label on
    /// (`empty_overflow`).
    fn empty_overflow<L: SortCost>(&mut self, labels: &[L]) {
        let cost_of = |label: u32| {
            usize::try_from(label)
                .ok()
                .and_then(|i| labels.get(i))
                .map_or(f32::MAX, SortCost::sortcost)
        };
        // The first label with the smallest cost (`std::min_element`).
        let mut min: Option<f32> = None;
        for &label in &self.overflow {
            let cost = cost_of(label);
            if min.is_none_or(|m| cost < m) {
                min = Some(cost);
            }
        }
        if let Some(min) = min {
            let min = f64::from(min);
            let range = f64::from(self.bucketrange);
            self.mincost += ((min - self.mincost) / range).floor() * range;
            if self.mincost > min {
                self.mincost -= range;
            } else if self.mincost + range < min {
                self.mincost += range;
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "mainline stores the double sum in a float"
            )]
            let maxcost = (self.mincost + range) as f32;
            self.maxcost = maxcost;
            let mut overflow = core::mem::take(&mut self.overflow);
            overflow.retain(|&label| {
                let cost = cost_of(label);
                if cost < self.maxcost {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "mainline casts to uint32"
                    )]
                    let index =
                        ((f64::from(cost) - self.mincost) * f64::from(self.inv)) as u32 as usize;
                    if let Some(bucket) = self.buckets.get_mut(index) {
                        bucket.push(label);
                        return false;
                    }
                }
                true
            });
            self.overflow = overflow;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "mainline stores the double in a float"
        )]
        let currentcost = self.mincost as f32;
        self.currentcost = currentcost;
        self.current = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cost(f32);
    impl SortCost for Cost {
        fn sortcost(&self) -> f32 {
            self.0
        }
    }

    #[test]
    fn pops_by_bucket_and_lifo_within_a_bucket() {
        let labels = [Cost(5.5), Cost(5.2), Cost(1.0), Cost(30.0), Cost(5.9)];
        let mut q = BucketQueue::default();
        q.reuse(0.0, 10.0, 1);
        for (i, l) in labels.iter().enumerate() {
            q.add(u32::try_from(i).unwrap(), l.0);
        }
        let mut order = Vec::new();
        while let Some(l) = q.pop(&labels) {
            order.push(l);
        }
        // 30 is in the overflow bucket until the low buckets are empty.
        assert_eq!(order, [2, 4, 1, 0, 3]);
    }

    #[test]
    fn decrease_moves_between_buckets() {
        let mut labels = vec![Cost(8.0), Cost(9.0)];
        let mut q = BucketQueue::default();
        q.reuse(3.7, 20.0, 1);
        q.add(0, 8.0);
        q.add(1, 9.0);
        q.decrease(1, 9.0, 4.5);
        labels[1].0 = 4.5;
        assert_eq!(q.pop(&labels), Some(1));
        assert_eq!(q.pop(&labels), Some(0));
        assert_eq!(q.pop(&labels), None);
        // After running dry, cheap labels go into the last bucket.
        q.add(0, 1.0);
        assert_eq!(q.pop(&labels), Some(0));
    }

    #[test]
    fn overflow_refills_at_the_cheapest_label() {
        let labels = [Cost(150.0), Cost(125.0), Cost(125.5), Cost(400.0)];
        let mut q = BucketQueue::default();
        q.reuse(0.0, 100.0, 1);
        for i in 0..4 {
            q.add(i, labels[i as usize].0);
        }
        assert_eq!(q.pop(&labels), Some(2));
        assert_eq!(q.pop(&labels), Some(1));
        assert_eq!(q.pop(&labels), Some(0));
        assert_eq!(q.pop(&labels), Some(3));
        assert_eq!(q.pop(&labels), None);
    }
}
