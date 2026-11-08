//! The iteration order of libstdc++'s `std::unordered_set` and `std::unordered_map`.
//!
//! Some of mainline's outputs are written by iterating unordered containers:
//! `/locate` lists the nodes a location snapped to from a `std::unordered_set<uint64_t>`,
//! and Loki lists the edges inside exclude polygons from a `std::unordered_set<GraphId>`.
//! Their order is libstdc++'s (GCC 13), which [`UnorderedSet`] reproduces for unique keys:
//!
//! - The elements form one singly linked list. Each bucket remembers the node before its first
//!   element. A new element goes first in its bucket, or, if its bucket is empty,
//!   at the front of the list (`_M_insert_bucket_begin`).
//! - The table starts with one bucket and grows to 13 on the first insertion; after that it
//!   grows (to the next of libstdc++'s primes at least twice as large) when it would hold more
//!   elements than buckets, re-linking the elements in list order (`_M_rehash_aux`).
//!
//! The bucket of an element is its hash modulo the bucket count;
//! `std::hash` of an integer is the integer itself.

use std::collections::HashMap;

/// Where a bucket's elements start: after the list's head, or after a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Before {
    Head,
    Node(usize),
}

/// The order of a libstdc++ `std::unordered_set` with unique keys (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct UnorderedSet {
    hash: fn(u64) -> u64,
    /// Each node's key and the next node.
    nodes: Vec<(u64, Option<usize>)>,
    head: Option<usize>,
    buckets: Vec<Option<Before>>,
    next_resize: u64,
    index: HashMap<u64, usize>,
}

impl Default for UnorderedSet {
    fn default() -> Self {
        Self::new(identity)
    }
}

/// `std::hash` of an integer.
#[inline]
pub fn identity(key: u64) -> u64 {
    key
}

/// `std::hash<valhalla::baldr::GraphId>`: a murmur3 finalizer.
#[inline]
pub fn graph_id_hash(key: u64) -> u64 {
    let mut v = key;
    v ^= v >> 33;
    v = v.wrapping_mul(0xff51_afd7_ed55_8ccd);
    v ^= v >> 33;
    v = v.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    v ^= v >> 33;
    v
}

impl UnorderedSet {
    /// An empty set with the given hash.
    pub fn new(hash: fn(u64) -> u64) -> Self {
        Self {
            hash,
            nodes: Vec::new(),
            head: None,
            buckets: vec![None],
            next_resize: 0,
            index: HashMap::new(),
        }
    }

    /// The number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether there are no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Whether the set holds `key`.
    #[inline]
    pub fn contains(&self, key: u64) -> bool {
        self.index.contains_key(&key)
    }

    fn bucket(&self, key: u64) -> usize {
        let count = self.buckets.len() as u64;
        // The count is never 0, and the remainder is less than it.
        usize::try_from((self.hash)(key) % count).unwrap_or(0)
    }

    /// libstdc++'s `_Prime_rehash_policy::_M_next_bkt`: the bucket count for at least `n`.
    ///
    /// The small sizes come from its `__fast_bkt` table; larger ones are the entries of its
    /// prime list that a growing set reaches (measured with g++ 13), and beyond those
    /// (more than 20,000 elements) the next prime.
    fn next_bucket_count(&mut self, n: u64) -> u64 {
        const FAST: [u64; 14] = [2, 2, 2, 3, 5, 5, 7, 7, 11, 11, 11, 11, 13, 13];
        const PRIMES: [u64; 10] = [29, 59, 127, 257, 541, 1109, 2357, 5087, 10273, 20753];
        let count =
            match usize::try_from(n).ok().and_then(|i| FAST.get(i)) {
                Some(&count) => count,
                None => PRIMES.iter().copied().find(|&p| p >= n).unwrap_or_else(|| {
                    (n..n.saturating_mul(2)).find(|&c| is_prime(c)).unwrap_or(n)
                }),
            };
        self.next_resize = count;
        count
    }

    /// Inserts `key`; returns whether it was new.
    pub fn insert(&mut self, key: u64) -> bool {
        if self.contains(key) {
            return false;
        }
        // `_M_need_rehash` with a maximum load factor of 1.
        let elements = self.nodes.len() as u64;
        if elements + 1 > self.next_resize {
            let minimum = (elements + 1).max(if self.next_resize == 0 { 11 } else { 0 });
            let buckets = self.buckets.len() as u64;
            if minimum >= buckets {
                let count = self.next_bucket_count((minimum + 1).max(buckets * 2));
                self.rehash(count);
            } else {
                self.next_resize = buckets;
            }
        }
        let node = self.nodes.len();
        self.nodes.push((key, None));
        self.index.insert(key, node);
        self.link(node);
        true
    }

    fn next_of(&self, before: Before) -> Option<usize> {
        match before {
            Before::Head => self.head,
            Before::Node(n) => self.nodes.get(n).and_then(|&(_, next)| next),
        }
    }

    fn set_next(&mut self, before: Before, next: Option<usize>) {
        match before {
            Before::Head => self.head = next,
            Before::Node(n) => {
                if let Some(node) = self.nodes.get_mut(n) {
                    node.1 = next;
                }
            }
        }
    }

    /// `_M_insert_bucket_begin`: links a node first in its bucket.
    fn link(&mut self, node: usize) {
        let bucket = self.bucket(self.nodes[node].0);
        if let Some(before) = self.buckets[bucket] {
            let next = self.next_of(before);
            self.nodes[node].1 = next;
            self.set_next(before, Some(node));
        } else {
            self.nodes[node].1 = self.head;
            self.head = Some(node);
            if let Some(next) = self.nodes[node].1 {
                let next_bucket = self.bucket(self.nodes[next].0);
                self.buckets[next_bucket] = Some(Before::Node(node));
            }
            self.buckets[bucket] = Some(Before::Head);
        }
    }

    /// `_M_rehash_aux` for unique keys.
    fn rehash(&mut self, count: u64) {
        let count = usize::try_from(count).unwrap_or(usize::MAX).max(1);
        self.buckets = vec![None; count];
        let mut p = self.head.take();
        let mut begin_bucket = 0;
        while let Some(node) = p {
            let next = self.nodes[node].1;
            let bucket = self.bucket(self.nodes[node].0);
            if let Some(before) = self.buckets[bucket] {
                let after = self.next_of(before);
                self.nodes[node].1 = after;
                self.set_next(before, Some(node));
            } else {
                self.nodes[node].1 = self.head;
                self.head = Some(node);
                self.buckets[bucket] = Some(Before::Head);
                if self.nodes[node].1.is_some() {
                    self.buckets[begin_bucket] = Some(Before::Node(node));
                }
                begin_bucket = bucket;
            }
            p = next;
        }
    }

    /// The elements in iteration order.
    pub fn iter(&self) -> impl Iterator<Item = u64> + '_ {
        let mut p = self.head;
        core::iter::from_fn(move || {
            let node = p?;
            let (key, next) = self.nodes.get(node)?;
            p = *next;
            Some(*key)
        })
    }
}

fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    let mut d = 2;
    while d * d <= n {
        if n.is_multiple_of(d) {
            return false;
        }
        d += 1;
    }
    true
}

/// The order of a libstdc++ `std::unordered_map` with integer keys, with its values.
#[derive(Clone, Debug)]
pub struct UnorderedMap<V> {
    keys: UnorderedSet,
    values: HashMap<u64, V>,
}

impl<V> Default for UnorderedMap<V> {
    fn default() -> Self {
        Self {
            keys: UnorderedSet::default(),
            values: HashMap::new(),
        }
    }
}

impl<V: Default> UnorderedMap<V> {
    /// The value for `key`, inserting a default one first if there is none (`operator[]`).
    pub fn entry(&mut self, key: u64) -> &mut V {
        self.keys.insert(key);
        self.values.entry(key).or_default()
    }
}

impl<V> UnorderedMap<V> {
    /// The value for `key`, if any.
    #[inline]
    pub fn get(&self, key: u64) -> Option<&V> {
        self.values.get(&key)
    }

    /// The number of keys.
    #[inline]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether there are no keys.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The keys and values in iteration order.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &V)> + '_ {
        self.keys
            .iter()
            .filter_map(|k| self.values.get(&k).map(|v| (k, v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_sets_iterate_like_libstdcxx() {
        // 13 buckets: values in empty buckets go to the front.
        let mut set = UnorderedSet::default();
        for v in [1, 2, 3] {
            set.insert(v);
        }
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![3, 2, 1]);
        // 14 is in bucket 1, so it goes before 1.
        set.insert(14);
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![3, 2, 14, 1]);
        assert!(!set.insert(2));
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn growing_sets_iterate_like_libstdcxx() {
        // From g++ 13, which grows the table from 13 to 29 buckets at the 14th element.
        let mut set = UnorderedSet::default();
        let values = [
            8_456_098_473,
            5161,
            99999,
            12,
            25,
            38,
            7_281_335_864,
            1,
            14,
            27,
            40,
            53,
            66,
            79,
            92,
            105,
            118,
        ];
        for v in values {
            set.insert(v);
        }
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            vec![
                118,
                105,
                92,
                79,
                8_456_098_473,
                5161,
                99999,
                12,
                25,
                38,
                7_281_335_864,
                1,
                14,
                27,
                40,
                53,
                66
            ]
        );
    }

    #[test]
    fn large_sets_keep_every_element() {
        let mut set = UnorderedSet::new(graph_id_hash);
        for v in 0..5000 {
            assert!(set.insert(v * 7919));
        }
        let mut values: Vec<u64> = set.iter().collect();
        assert_eq!(values.len(), 5000);
        values.sort_unstable();
        assert_eq!(values, (0..5000).map(|v| v * 7919).collect::<Vec<_>>());
    }

    #[test]
    fn large_sets_iterate_like_libstdcxx() {
        // Hashes of the iteration order from g++ 13, with GraphId's hash and the identity.
        let mut by_graph_id = UnorderedSet::new(graph_id_hash);
        let mut by_identity = UnorderedSet::default();
        for i in 0..3000 {
            by_graph_id.insert(i * 7919 + 13);
            by_identity.insert(i * 104_729 + 7);
        }
        let digest = |set: &UnorderedSet| {
            set.iter()
                .fold(0_u64, |h, v| h.wrapping_mul(31).wrapping_add(v))
        };
        assert_eq!(digest(&by_graph_id), 17_614_831_906_851_448_418);
        assert_eq!(digest(&by_identity), 6_237_109_928_388_540_292);
    }

    #[test]
    fn maps_keep_values() {
        let mut map: UnorderedMap<Vec<u32>> = UnorderedMap::default();
        map.entry(5).push(1);
        map.entry(18).push(2);
        map.entry(5).push(3);
        assert_eq!(map.get(5), Some(&vec![1, 3]));
        assert_eq!(map.iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![18, 5]);
        assert_eq!(map.len(), 2);
    }
}
