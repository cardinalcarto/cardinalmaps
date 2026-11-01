//! The cuisine hierarchy in `dictionaries/cuisine_parents.txt`.

use std::collections::{BTreeSet, HashMap};

lazy_static! {
    static ref PARENTS: HashMap<&'static str, Vec<&'static str>> =
        parse(include_str!("../dictionaries/cuisine_parents.txt"));
}

fn parse(contents: &str) -> HashMap<&str, Vec<&str>> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or_default();
            let mut parts = line.split_whitespace();
            Some((parts.next()?, parts.collect()))
        })
        .collect()
}

/// `cuisine` and every cuisine containing it, e.g. `vietnamese` gives
/// `asian` and `vietnamese`.
#[must_use]
pub fn with_ancestors(cuisine: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![cuisine];
    while let Some(value) = stack.pop() {
        if found.insert(value.to_string()) {
            stack.extend(PARENTS.get(value).into_iter().flatten());
        }
    }
    found
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_ancestors() {
        assert_eq!(
            with_ancestors("cantonese").into_iter().collect::<Vec<_>>(),
            vec!["asian", "cantonese", "chinese"]
        );
        assert_eq!(
            with_ancestors("tex-mex").into_iter().collect::<Vec<_>>(),
            vec!["american", "latin_american", "mexican", "tex-mex"]
        );
        assert_eq!(
            with_ancestors("pizza").into_iter().collect::<Vec<_>>(),
            vec!["pizza"]
        );
    }

    #[test]
    fn test_every_parent_is_a_value_or_root() {
        // A typo in a parent would silently create a new root.
        let roots = [
            "asian",
            "middle_eastern",
            "european",
            "african",
            "latin_american",
            "caribbean",
            "american",
        ];
        for parents in PARENTS.values() {
            for parent in parents {
                assert!(
                    PARENTS.contains_key(parent) || roots.contains(parent),
                    "unknown parent {parent}"
                );
            }
        }
    }
}
