//! A static 2D point index answering box and radius queries, like
//! [kdbush](https://github.com/mourner/kdbush): points are sorted into a k-d tree in place,
//! with no nodes besides the sorted array.

/// Leaves of this many points or fewer are scanned rather than split further.
const NODE_SIZE: usize = 64;

pub(crate) struct KdBush {
    /// `(index in the input, position)`, in k-d order.
    points: Vec<(u32, [f64; 2])>,
}

impl KdBush {
    pub fn new(points: impl IntoIterator<Item = [f64; 2]>) -> Self {
        let mut points: Vec<_> = points
            .into_iter()
            .enumerate()
            .map(|(i, p)| (i as u32, p))
            .collect();
        sort(&mut points, 0);
        Self { points }
    }

    /// Indices of the points with `min <= position <= max`.
    pub fn range(&self, min: [f64; 2], max: [f64; 2], out: &mut Vec<u32>) {
        self.search(
            |p| p[0] >= min[0] && p[0] <= max[0] && p[1] >= min[1] && p[1] <= max[1],
            |axis, split| (min[axis] <= split, max[axis] >= split),
            out,
        );
    }

    /// Indices of the points within `r` of `center`.
    pub fn within(&self, center: [f64; 2], r: f64, out: &mut Vec<u32>) {
        let r2 = r * r;
        self.search(
            |p| (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) <= r2,
            |axis, split| (center[axis] - r <= split, center[axis] + r >= split),
            out,
        );
    }

    /// Walk the tree: `matches` tests a point, `sides` says whether the query reaches the
    /// lower and upper half of a node split at `split` on `axis`.
    fn search(
        &self,
        matches: impl Fn(&[f64; 2]) -> bool,
        sides: impl Fn(usize, f64) -> (bool, bool),
        out: &mut Vec<u32>,
    ) {
        let mut stack = vec![(0, self.points.len(), 0)];
        while let Some((start, end, axis)) = stack.pop() {
            let node = &self.points[start..end];
            if node.len() <= NODE_SIZE {
                out.extend(node.iter().filter(|(_, p)| matches(p)).map(|(i, _)| *i));
                continue;
            }
            let m = node.len() / 2;
            let (index, p) = node[m];
            if matches(&p) {
                out.push(index);
            }
            let (lower, upper) = sides(axis, p[axis]);
            if lower {
                stack.push((start, start + m, 1 - axis));
            }
            if upper {
                stack.push((start + m + 1, end, 1 - axis));
            }
        }
    }
}

/// Order `points` so the middle one splits the rest on `axis`, and recurse into each half
/// on the other axis.
fn sort(points: &mut [(u32, [f64; 2])], axis: usize) {
    if points.len() <= NODE_SIZE {
        return;
    }
    let m = points.len() / 2;
    points.select_nth_unstable_by(m, |a, b| a.1[axis].total_cmp(&b.1[axis]));
    let (lower, rest) = points.split_at_mut(m);
    sort(lower, 1 - axis);
    sort(&mut rest[1..], 1 - axis);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_brute_force() {
        // A deterministic scatter of points.
        let points: Vec<[f64; 2]> = (0..1000)
            .map(|i| {
                let t = f64::from(i);
                [(t * 0.618_034).fract(), (t * 0.414_214).fract()]
            })
            .collect();
        let index = KdBush::new(points.iter().copied());

        let mut found = Vec::new();
        index.range([0.2, 0.3], [0.5, 0.4], &mut found);
        found.sort_unstable();
        let expected: Vec<u32> = (0..1000)
            .filter(|&i| {
                let p = points[i as usize];
                (0.2..=0.5).contains(&p[0]) && (0.3..=0.4).contains(&p[1])
            })
            .collect();
        assert_eq!(found, expected);

        found.clear();
        index.within([0.5, 0.5], 0.1, &mut found);
        found.sort_unstable();
        let expected: Vec<u32> = (0..1000)
            .filter(|&i| {
                let p = points[i as usize];
                (p[0] - 0.5).powi(2) + (p[1] - 0.5).powi(2) <= 0.01
            })
            .collect();
        assert_eq!(found, expected);
        assert!(!found.is_empty());
    }
}
