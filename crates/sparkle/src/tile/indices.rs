//! 16-bit indices in batches, as MapLibre draws them: each batch indexes up to 65,536
//! vertices from its own first vertex on, so a bucket with more vertices than that is
//! drawn in several batches rather than with 32-bit indices, which take twice the memory.

/// One draw of [`batch_indices`]'s indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexBatch {
    /// The vertex index 0 refers to.
    pub first_vertex: u32,
    /// Where the batch's indices start.
    pub first_index: u32,
    pub index_count: u32,
}

/// `indices` (of whole primitives of `primitive` vertices, 3 for triangles and 2 for
/// lines) as 16-bit ones, relative to the first vertex of their batch, and the batches,
/// which draw the primitives in their order. Each batch is the longest run of primitives
/// that spans at most 65,536 vertices.
///
/// `None` when a single primitive spans more than that, or the runs are so short that
/// batches would take more draws than they save memory: draw those with 32-bit indices.
pub fn batch_indices(indices: &[u32], primitive: usize) -> Option<(Vec<u16>, Vec<IndexBatch>)> {
    const SPAN: u32 = 1 << 16;
    /// Fewer indices per batch than this on average aren't worth a draw each.
    const MIN_INDICES_PER_BATCH: usize = 1 << 12;
    debug_assert_eq!(indices.len() % primitive, 0);
    let mut out = Vec::with_capacity(indices.len());
    let mut batches: Vec<IndexBatch> = Vec::new();
    let mut start = 0;
    while start < indices.len() {
        // Extend the run while the vertices it uses stay within one span.
        let (mut min, mut max) = (u32::MAX, 0);
        let mut end = start;
        for p in indices[start..].chunks_exact(primitive) {
            let lo = min.min(*p.iter().min().expect("primitive"));
            let hi = max.max(*p.iter().max().expect("primitive"));
            if hi - lo >= SPAN {
                break;
            }
            (min, max) = (lo, hi);
            end += primitive;
        }
        if end == start {
            return None;
        }
        out.extend(indices[start..end].iter().map(|&i| (i - min) as u16));
        batches.push(IndexBatch {
            first_vertex: min,
            first_index: start as u32,
            index_count: (end - start) as u32,
        });
        if batches.len() > 1 && batches.len() * MIN_INDICES_PER_BATCH > indices.len() {
            return None;
        }
        start = end;
    }
    Some((out, batches))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 32-bit indices `batch_indices` stands for.
    fn expand((indices, batches): &(Vec<u16>, Vec<IndexBatch>)) -> Vec<u32> {
        batches
            .iter()
            .flat_map(|b| {
                let range = b.first_index as usize..(b.first_index + b.index_count) as usize;
                indices[range]
                    .iter()
                    .map(|&i| b.first_vertex + u32::from(i))
            })
            .collect()
    }

    #[test]
    fn small_buckets_take_one_batch() {
        let indices = [0, 1, 2, 2, 1, 65535];
        let batched = batch_indices(&indices, 3).unwrap();
        assert_eq!(batched.0, [0, 1, 2, 2, 1, 65535]);
        assert_eq!(
            batched.1,
            [IndexBatch {
                first_vertex: 0,
                first_index: 0,
                index_count: 6
            }]
        );
        assert_eq!(batch_indices(&[], 3).unwrap(), (vec![], vec![]));
    }

    #[test]
    fn large_buckets_are_split_in_order() {
        // Quads of four vertices each, 50,000 of them: 200,000 vertices.
        let indices: Vec<u32> = (0..50_000u32)
            .flat_map(|q| [0, 2, 1, 1, 2, 3].map(|i| 4 * q + i))
            .collect();
        let batched = batch_indices(&indices, 3).unwrap();
        assert_eq!(expand(&batched), indices);
        let batches = &batched.1;
        assert_eq!(batches.len(), 4);
        for b in batches {
            let used = &batched.0[b.first_index as usize..][..b.index_count as usize];
            assert_eq!(used.iter().min(), Some(&0));
        }
        // Every primitive is whole in its batch.
        assert!(
            batches
                .iter()
                .all(|b| b.first_index % 3 == 0 && b.index_count % 3 == 0)
        );
    }

    #[test]
    fn runs_may_go_back_within_their_span() {
        // Triangles that refer to earlier vertices, as earcut's do.
        let indices = [60_000, 60_001, 59_000, 5, 60_002, 6];
        let batched = batch_indices(&indices, 3).unwrap();
        assert_eq!(batched.1.len(), 1);
        assert_eq!(batched.1[0].first_vertex, 5);
        assert_eq!(expand(&batched), indices);
        let lines = [100_000, 100_001, 40_000, 40_001, 100_002, 100_003];
        let batched = batch_indices(&lines, 2).unwrap();
        assert_eq!(batched.1.len(), 1);
        assert_eq!(expand(&batched), lines);
    }

    #[test]
    fn falls_back_to_32_bits() {
        // A triangle spanning more than a batch can.
        assert_eq!(batch_indices(&[0, 1, 70_000], 3), None);
        // Triangles that jump back and forth would each need a batch.
        let indices: Vec<u32> = (0..10_000u32)
            .flat_map(|t| {
                if t % 2 == 0 {
                    [0, 1, 2]
                } else {
                    [100_000, 100_001, 100_002]
                }
            })
            .collect();
        assert_eq!(batch_indices(&indices, 3), None);
    }
}
