//! `*-sort-key`: drawing a bucket's features in key order rather than tile order.
//!
//! Vertices and their paint values stay where they are; only index runs are reordered,
//! so the vertex data of every layer type can be built in one pass.

use std::ops::Range;

use crate::style::PropertyValue;

/// Each feature's sort key and index runs, collected while building a bucket.
#[derive(Debug, Default)]
pub(crate) struct SortKeys {
    /// Per feature: the key and its runs in up to two index lists.
    features: Vec<(f32, [Range<usize>; 2])>,
}

impl SortKeys {
    /// Collect keys for `property`, or nothing (keep tile order) when it is unset or the
    /// same for every feature.
    pub fn new(property: &PropertyValue<Option<f32>>) -> Option<Self> {
        property.is_feature_dependent().then(Self::default)
    }

    /// Record a feature's key and the index runs it added. A missing key sorts as 0.
    pub fn push(&mut self, key: Option<f32>, runs: [Range<usize>; 2]) {
        let key = key.filter(|k| k.is_finite()).unwrap_or(0.0);
        self.features.push((key, runs));
    }

    /// Reorder the runs of each index list by key, keeping tile order between equal keys.
    pub fn apply(mut self, lists: [&mut Vec<u32>; 2]) {
        if self.features.is_sorted_by(|a, b| a.0 <= b.0) {
            return;
        }
        self.features.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (i, list) in lists.into_iter().enumerate() {
            let mut out = Vec::with_capacity(list.len());
            for (_, runs) in &self.features {
                out.extend_from_slice(&list[runs[i].clone()]);
            }
            // Indices outside every run (there shouldn't be any) keep their place at the end.
            if out.len() == list.len() {
                *list = out;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn runs_follow_keys() {
        let key = PropertyValue::from_json("line-sort-key", &json!(["get", "k"]), None).unwrap();
        let mut keys = SortKeys::new(&key).unwrap();
        let mut a = vec![0, 1, 2, 3, 4, 5];
        let mut b = vec![10, 11, 12];
        keys.push(Some(2.0), [0..3, 0..1]);
        keys.push(None, [3..4, 1..2]);
        keys.push(Some(-1.0), [4..6, 2..3]);
        keys.apply([&mut a, &mut b]);
        assert_eq!(a, [4, 5, 3, 0, 1, 2]);
        assert_eq!(b, [12, 11, 10]);

        let constant = PropertyValue::from_json("line-sort-key", &json!(3), None).unwrap();
        assert!(SortKeys::new(&constant).is_none());
    }
}
