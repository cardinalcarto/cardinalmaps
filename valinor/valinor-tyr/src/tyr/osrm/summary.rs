//! Leg summaries: the fewest longest street names that tell a route's leg apart
//! from the same leg of the other routes
//! (mainline's `route_summary_cache` and `summarize_route_legs`).

use valhalla_proto::DirectionsRoute;

/// Mainline's `MAX_USED_SEGMENTS`: summaries name at least this many streets when they can.
const MAX_USED_SEGMENTS: usize = 2;

/// A street and how much of a leg uses it.
#[derive(Debug, Clone)]
struct NamedSegment {
    name: String,
    /// The first maneuver on this street.
    index: usize,
    /// The total length of the maneuvers on it, summed in `float` as mainline does.
    distance: f32,
}

/// The named segments of each route's legs, longest first.
fn segments_by_distance(routes: &[DirectionsRoute]) -> Vec<Vec<Vec<NamedSegment>>> {
    routes
        .iter()
        .map(|route| {
            route
                .legs
                .iter()
                .map(|leg| {
                    let mut segments: Vec<NamedSegment> = Vec::new();
                    for (index, maneuver) in leg.maneuver.iter().enumerate() {
                        let Some(name) = maneuver.street_name.first() else {
                            continue;
                        };
                        match segments.iter_mut().find(|s| s.name == name.value) {
                            Some(segment) => segment.distance += maneuver.length,
                            None => segments.push(NamedSegment {
                                name: name.value.clone(),
                                index,
                                distance: maneuver.length,
                            }),
                        }
                    }
                    // Mainline sorts a hash map's entries, so the order of equal lengths
                    // is unspecified there; we keep the streets' order along the leg.
                    segments.sort_by(|a, b| b.distance.total_cmp(&a.distance));
                    segments
                })
                .collect()
        })
        .collect()
}

/// `route_summary_cache::get_n_segment_summary`: the `n` longest streets in route order.
fn n_segment_summary(segments: &[NamedSegment], n: usize) -> String {
    if n == 0 || n > segments.len() {
        return String::new();
    }
    let mut by_index: Vec<&NamedSegment> = segments[..n].iter().collect();
    by_index.sort_by_key(|segment| segment.index);
    let names: Vec<&str> = by_index
        .iter()
        .map(|segment| segment.name.as_str())
        .collect();
    names.join(", ")
}

/// `summarize_route_legs`: each route's leg summaries.
pub(super) fn summarize_route_legs(routes: &[DirectionsRoute]) -> Vec<Vec<String>> {
    let segments = segments_by_distance(routes);
    let count = |route: usize, leg: usize| {
        segments
            .get(route)
            .and_then(|r| r.get(leg))
            .map_or(0, Vec::len)
    };
    let summary = |route: usize, leg: usize, n: usize| {
        segments
            .get(route)
            .and_then(|r| r.get(leg))
            .map_or_else(String::new, |s| n_segment_summary(s, n))
    };

    (0..routes.len())
        .map(|route_i| {
            (0..routes[route_i].legs.len())
                .map(|leg| {
                    let num_named_segments_i = count(route_i, leg);
                    let mut needed = MAX_USED_SEGMENTS.min(num_named_segments_i);
                    for (route_j, other) in routes.iter().enumerate() {
                        if route_i == route_j || leg >= other.legs.len() {
                            continue;
                        }
                        let comparable = num_named_segments_i.min(count(route_j, leg));
                        let mut k = needed.min(comparable);
                        while k < comparable {
                            if summary(route_i, leg, k) != summary(route_j, leg, k) {
                                break;
                            }
                            k += 1;
                        }
                        needed = needed.max(k);
                    }
                    summary(route_i, leg, needed)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::directions_leg::Maneuver;
    use valhalla_proto::{DirectionsLeg, StreetName};

    fn maneuver(name: Option<&str>, length: f32) -> Maneuver {
        Maneuver {
            street_name: name
                .map(|value| StreetName {
                    value: value.to_string(),
                    ..StreetName::default()
                })
                .into_iter()
                .collect(),
            length,
            ..Maneuver::default()
        }
    }

    fn route(maneuvers: Vec<Maneuver>) -> DirectionsRoute {
        DirectionsRoute {
            legs: vec![DirectionsLeg {
                maneuver: maneuvers,
                ..DirectionsLeg::default()
            }],
        }
    }

    #[test]
    fn two_longest_names_in_route_order() {
        let r = route(vec![
            maneuver(Some("A"), 1.0),
            maneuver(Some("B"), 5.0),
            maneuver(None, 9.0),
            maneuver(Some("C"), 3.0),
            maneuver(Some("A"), 3.0),
        ]);
        assert_eq!(summarize_route_legs(&[r]), vec![vec!["A, B".to_string()]]);
    }

    #[test]
    fn single_name_and_none() {
        let one = route(vec![maneuver(Some("A"), 1.0)]);
        let none = route(vec![maneuver(None, 1.0)]);
        assert_eq!(
            summarize_route_legs(&[one, none]),
            vec![vec!["A".to_string()], vec![String::new()]]
        );
    }

    #[test]
    fn alternates_get_longer_summaries_until_unique() {
        let a = route(vec![
            maneuver(Some("X"), 10.0),
            maneuver(Some("Y"), 5.0),
            maneuver(Some("Z"), 1.0),
        ]);
        let b = route(vec![
            maneuver(Some("X"), 10.0),
            maneuver(Some("Y"), 5.0),
            maneuver(Some("W"), 1.0),
        ]);
        assert_eq!(
            summarize_route_legs(&[a, b]),
            vec![vec!["X, Y, Z".to_string()], vec!["X, Y, W".to_string()]]
        );
    }
}
