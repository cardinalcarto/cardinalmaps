//! libstdc++'s `std::sort` (GCC 13), step for step.
//!
//! `std::sort` isn't stable, and Loki's results depend on the order it leaves equal elements in:
//! candidates at the same node have the same distance, and the first one to add an edge
//! decides its bounding circle.
//! So we sort exactly as mainline's standard library does: an introsort with a median of three
//! pivot, falling back to a heap sort when it recurses too deep, and finishing with an
//! insertion sort once the ranges are at most 16 long.

/// Sorts `v` with `less` as libstdc++'s `std::sort` does.
pub(crate) fn sort<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    if v.len() < 2 {
        return;
    }
    let depth_limit = 2 * v.len().ilog2();
    introsort_loop(v, depth_limit, less);
    final_insertion_sort(v, less);
}

/// `_S_threshold`.
const THRESHOLD: usize = 16;

fn introsort_loop<T: Clone>(v: &mut [T], mut depth_limit: u32, less: &impl Fn(&T, &T) -> bool) {
    let mut last = v.len();
    while last > THRESHOLD {
        if depth_limit == 0 {
            heap_sort(&mut v[..last], less);
            return;
        }
        depth_limit -= 1;
        let cut = unguarded_partition_pivot(&mut v[..last], less);
        introsort_loop(&mut v[cut..last], depth_limit, less);
        last = cut;
    }
}

/// Moves the median of the second, middle, and last elements to the front, then partitions
/// the rest around it (`__unguarded_partition_pivot`).
fn unguarded_partition_pivot<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) -> usize {
    let last = v.len();
    let mid = last / 2;
    move_median_to_first(v, 0, 1, mid, last - 1, less);
    unguarded_partition(v, 1, last, 0, less)
}

fn move_median_to_first<T>(
    v: &mut [T],
    result: usize,
    a: usize,
    b: usize,
    c: usize,
    less: &impl Fn(&T, &T) -> bool,
) {
    let swap_with = if less(&v[a], &v[b]) {
        if less(&v[b], &v[c]) {
            b
        } else if less(&v[a], &v[c]) {
            c
        } else {
            a
        }
    } else if less(&v[a], &v[c]) {
        a
    } else if less(&v[b], &v[c]) {
        c
    } else {
        b
    };
    v.swap(result, swap_with);
}

/// `__unguarded_partition`: the pivot is at `pivot`, outside `[first, last)`.
fn unguarded_partition<T>(
    v: &mut [T],
    mut first: usize,
    mut last: usize,
    pivot: usize,
    less: &impl Fn(&T, &T) -> bool,
) -> usize {
    loop {
        while first < v.len() && less(&v[first], &v[pivot]) {
            first += 1;
        }
        last -= 1;
        while last > 0 && less(&v[pivot], &v[last]) {
            last -= 1;
        }
        if first >= last {
            return first;
        }
        v.swap(first, last);
        first += 1;
    }
}

fn final_insertion_sort<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    if v.len() > THRESHOLD {
        insertion_sort(&mut v[..THRESHOLD], less);
        for i in THRESHOLD..v.len() {
            unguarded_linear_insert(v, i, less);
        }
    } else {
        insertion_sort(v, less);
    }
}

fn insertion_sort<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    for i in 1..v.len() {
        if less(&v[i], &v[0]) {
            // Rotating moves the element to the front and everything before it up one.
            v[..=i].rotate_right(1);
        } else {
            unguarded_linear_insert(v, i, less);
        }
    }
}

/// Moves `v[last]` down past every element it is less than (`__unguarded_linear_insert`).
fn unguarded_linear_insert<T: Clone>(v: &mut [T], mut last: usize, less: &impl Fn(&T, &T) -> bool) {
    let value = v[last].clone();
    while last > 0 && less(&value, &v[last - 1]) {
        v[last] = v[last - 1].clone();
        last -= 1;
    }
    v[last] = value;
}

/// `__partial_sort(first, last, last)`: a heap sort.
fn heap_sort<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    make_heap(v, less);
    let mut last = v.len();
    while last > 1 {
        last -= 1;
        // `__pop_heap(first, last, last)`
        let value = v[last].clone();
        v[last] = v[0].clone();
        adjust_heap(&mut v[..last], 0, value, less);
    }
}

fn make_heap<T: Clone>(v: &mut [T], less: &impl Fn(&T, &T) -> bool) {
    let len = v.len();
    if len < 2 {
        return;
    }
    let mut parent = (len - 2) / 2;
    loop {
        let value = v[parent].clone();
        adjust_heap(v, parent, value, less);
        if parent == 0 {
            return;
        }
        parent -= 1;
    }
}

fn adjust_heap<T: Clone>(v: &mut [T], mut hole: usize, value: T, less: &impl Fn(&T, &T) -> bool) {
    let len = v.len();
    let top = hole;
    let mut second = hole;
    while second < (len.saturating_sub(1)) / 2 {
        second = 2 * (second + 1);
        if less(&v[second], &v[second - 1]) {
            second -= 1;
        }
        v[hole] = v[second].clone();
        hole = second;
    }
    if len & 1 == 0 && len >= 2 && second == (len - 2) / 2 {
        second = 2 * (second + 1);
        v[hole] = v[second - 1].clone();
        hole = second - 1;
    }
    // `__push_heap`
    while hole > top {
        let parent = (hole - 1) / 2;
        if !less(&v[parent], &value) {
            break;
        }
        v[hole] = v[parent].clone();
        hole = parent;
    }
    v[hole] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic generator, so the test needs no dependency.
    fn values(n: usize, seed: u64, modulo: u64) -> Vec<(u64, usize)> {
        let mut state = seed;
        (0..n)
            .map(|i| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 33) % modulo, i)
            })
            .collect()
    }

    #[test]
    fn sorts_by_key() {
        for n in [0, 1, 2, 5, 16, 17, 40, 200, 1000] {
            for modulo in [3, 1000] {
                let mut v = values(n, n as u64 + modulo, modulo);
                sort(&mut v, &|a: &(u64, usize), b: &(u64, usize)| a.0 < b.0);
                assert!(v.windows(2).all(|w| w[0].0 <= w[1].0), "{n} {modulo}");
                let mut indexes: Vec<usize> = v.iter().map(|x| x.1).collect();
                indexes.sort_unstable();
                assert_eq!(indexes, (0..n).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn small_ranges_are_stable() {
        let mut v = values(16, 7, 3);
        let mut expected = v.clone();
        expected.sort_by_key(|x| x.0);
        sort(&mut v, &|a: &(u64, usize), b: &(u64, usize)| a.0 < b.0);
        assert_eq!(v, expected);
    }

    #[test]
    fn matches_libstdcxx_on_ties() {
        // From g++ 13: std::sort of these keys (with their indexes as payload) by key only.
        let keys = [3, 1, 2, 3, 1, 2, 3, 1, 2, 0, 0, 3, 1, 2, 3, 1, 2, 3, 1, 2];
        let mut v: Vec<(u32, usize)> = keys.iter().copied().zip(0..).collect();
        sort(&mut v, &|a: &(u32, usize), b: &(u32, usize)| a.0 < b.0);
        let order: Vec<usize> = v.iter().map(|x| x.1).collect();
        assert_eq!(
            order,
            vec![
                10, 9, 1, 18, 15, 12, 7, 4, 8, 5, 13, 2, 16, 19, 6, 11, 3, 14, 17, 0
            ]
        );

        // 300 elements with 7 keys (deep enough to partition several times), hashed by g++.
        let mut state: u64 = 99;
        let mut w: Vec<(u64, u64)> = (0..300)
            .map(|i| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                ((state >> 33) % 7, i)
            })
            .collect();
        sort(&mut w, &|a: &(u64, u64), b: &(u64, u64)| a.0 < b.0);
        let hash = w
            .iter()
            .fold(0_u64, |h, x| h.wrapping_mul(31).wrapping_add(x.1));
        assert_eq!(hash, 14_169_721_088_051_255_294);
    }
}
