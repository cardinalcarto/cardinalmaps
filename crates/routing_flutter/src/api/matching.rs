//! Map matching for navigation: where on the roads the latest fix was, from a window of
//! recent fixes, through the offline router's `trace_attributes`.
//!
//! A single fix can't say which of two roads 15 m apart you're on; the fixes before it can,
//! since only one of the roads goes where they went. So each fix is matched together with
//! the recent ones before it, and only the last fix's match is used: the matched point, and
//! the heading of the road there. [`NavigationSession::update_location`] then decides
//! whether to trust it.
//!
//! The request is built and the answer read here, so Dart makes one call per fix
//! ([`OfflineRouter::match_position`]).
//!
//! [`NavigationSession::update_location`]: super::navigation::NavigationSession::update_location
//! [`OfflineRouter::match_position`]: super::router::OfflineRouter::match_position

use super::navigation::{NavLocation, NavigationMode};
use serde_json::{Value, json};

/// How a fix was matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchKind {
    /// Matched to a road on its own merits.
    Matched,
    /// Too close to the fix before to be matched on its own, so placed along the path
    /// between its neighbors' matches.
    Interpolated,
}

/// Where on the roads the last fix of a window was matched.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MatchedPosition {
    pub lat: f64,
    pub lng: f64,
    pub kind: MatchKind,
    /// The direction the matched road goes there, the way the trace traveled it, in degrees
    /// clockwise from true north; none when the matched path ends without length to take a
    /// heading from (matched right at its first node).
    pub heading: Option<f64>,
}

impl NavigationMode {
    /// The router's costing for matching fixes of a trip traveling this way, so a car isn't
    /// matched onto a footpath or a walker onto a motorway.
    pub(crate) fn costing(self) -> &'static str {
        match self {
            NavigationMode::Driving => "auto",
            NavigationMode::Cycling => "bicycle",
            NavigationMode::Walking => "pedestrian",
        }
    }
}

/// What the answer is asked to include: only what [`parse_match`] reads, so the router
/// serializes no more than it must.
const ATTRIBUTES: [&str; 4] = [
    "edge.end_heading",
    "matched.point",
    "matched.type",
    "matched.edge_index",
];

/// The `trace_attributes` request body matching `fixes` (oldest first, the fix to place
/// last), traveling by `mode`.
///
/// `map_snap`, since fixes are a noisy trace of positions, not a route's own shape for
/// `edge_walk`. Each fix carries its time and its own accuracy, so a poor fix weighs less
/// in the match than a good one; the search radius stays the router's default (50 m),
/// past which a fix is too far from any road to be placed on one anyway.
pub(crate) fn match_body(mode: NavigationMode, fixes: &[NavLocation]) -> Vec<u8> {
    let shape: Vec<Value> = fixes
        .iter()
        .map(|fix| {
            json!({
                "lat": fix.lat,
                "lon": fix.lng,
                "time": fix.timestamp_ms as f64 / 1000.0,
                // Whole meters, as Valhalla takes them; at least 1, since 0 would say the
                // fix is exact.
                "accuracy": fix.horizontal_accuracy.ceil().clamp(1.0, f64::from(u16::MAX)) as u32,
            })
        })
        .collect();
    serde_json::to_vec(&json!({
        "costing": mode.costing(),
        "shape_match": "map_snap",
        "shape": shape,
        "filters": {"attributes": ATTRIBUTES, "action": "include"},
    }))
    .unwrap_or_default()
}

/// The last fix's match in a `trace_attributes` answer, or none if it has none: an error
/// (any status but 200), an unmatched last point, or an answer without points.
///
/// The heading is the `end_heading` of the last fix's edge when that edge is the last of the
/// path: Thor trims the path's last edge at the last matched point, so its end heading is
/// the road's heading coming into that point, taken over the chord from 15 m back along the
/// edge (24 m on motorways, 7.5 m on paths, less where the edge is shorter).
/// `end_heading_is_the_roads_heading_at_the_last_fix` checks this on real tiles: on a curve
/// the chord lags the tangent at the point by up to a few tens of degrees, which still
/// points a new route the right way along the road.
/// Were the last fix on an earlier edge, that edge's end would be somewhere ahead of it, so
/// there's no heading.
pub(crate) fn parse_match(status: u16, body: &[u8]) -> Option<MatchedPosition> {
    if status != 200 {
        return None;
    }
    let answer: Value = serde_json::from_slice(body).ok()?;
    let last = answer["matched_points"].as_array()?.last()?;
    let kind = match last["type"].as_str()? {
        "matched" => MatchKind::Matched,
        "interpolated" => MatchKind::Interpolated,
        _ => return None,
    };
    let edges = answer["edges"].as_array();
    let heading = last["edge_index"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
        .filter(|&i| edges.is_some_and(|edges| i + 1 == edges.len()))
        .and_then(|i| edges?[i]["end_heading"].as_f64());
    Some(MatchedPosition {
        lat: last["lat"].as_f64()?,
        lng: last["lon"].as_f64()?,
        kind,
        heading,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(lat: f64, lng: f64, accuracy: f64, timestamp_ms: i64) -> NavLocation {
        NavLocation {
            lat,
            lng,
            horizontal_accuracy: accuracy,
            course: None,
            course_accuracy: None,
            speed: None,
            speed_accuracy: None,
            timestamp_ms,
        }
    }

    #[test]
    fn the_body_has_each_fix_with_its_time_and_accuracy() {
        let body: Value = serde_json::from_slice(&match_body(
            NavigationMode::Cycling,
            &[
                fix(47.6, -122.3, 4.2, 1_000_500),
                fix(47.61, -122.3, 0.0, 1_001_500),
            ],
        ))
        .unwrap();
        assert_eq!(body["costing"], "bicycle");
        assert_eq!(body["shape_match"], "map_snap");
        let shape = body["shape"].as_array().unwrap();
        assert_eq!(shape.len(), 2);
        assert_eq!(shape[0]["lat"], 47.6);
        assert_eq!(shape[0]["lon"], -122.3);
        assert_eq!(shape[0]["time"], 1000.5);
        assert_eq!(shape[0]["accuracy"], 5);
        assert_eq!(shape[1]["accuracy"], 1);
        assert_eq!(body["filters"]["action"], "include");
        assert_eq!(NavigationMode::Driving.costing(), "auto");
        assert_eq!(NavigationMode::Walking.costing(), "pedestrian");
    }

    #[test]
    fn the_last_points_match_and_its_edges_end_heading() {
        let answer = br#"{"edges": [{"end_heading": 10}, {"end_heading": 92.5}],
            "matched_points": [
                {"lat": 1, "lon": 2, "type": "matched", "edge_index": 0},
                {"lat": 3, "lon": 4, "type": "interpolated", "edge_index": 1}]}"#;
        assert_eq!(
            parse_match(200, answer),
            Some(MatchedPosition {
                lat: 3.0,
                lng: 4.0,
                kind: MatchKind::Interpolated,
                heading: Some(92.5),
            })
        );
        // On an edge before the last, the edge's end is ahead of the point.
        let earlier = br#"{"edges": [{"end_heading": 10}, {"end_heading": 92.5}],
            "matched_points": [{"lat": 3, "lon": 4, "type": "matched", "edge_index": 0}]}"#;
        assert_eq!(parse_match(200, earlier).unwrap().heading, None);
    }

    #[test]
    fn errors_unmatched_points_and_nothing_are_no_match() {
        assert_eq!(parse_match(400, br#"{"error_code": 444}"#), None);
        assert_eq!(parse_match(200, b"not json"), None);
        assert_eq!(parse_match(200, br#"{"matched_points": []}"#), None);
        let unmatched = br#"{"edges": [],
            "matched_points": [{"lat": 3, "lon": 4, "type": "unmatched",
                "edge_index": 18446744073709551615}]}"#;
        assert_eq!(parse_match(200, unmatched), None);
    }
    /// Over Seattle's tiles, from `CARDINAL_SEATTLE_TILES` (real data, so not in the
    /// repository): simulated trips with noisy fixes a second apart, matched as navigation
    /// matches them.
    mod seattle {
        use super::super::*;
        use crate::api::directions::{DirectionsAdapter, DirectionsPoint, DistanceUnits};
        use crate::api::navigation::{LocationSimulation, SimulationBias};
        use crate::api::router::OfflineRouter;
        use std::time::Instant;

        fn router() -> OfflineRouter {
            let tiles = std::env::var("CARDINAL_SEATTLE_TILES").expect("CARDINAL_SEATTLE_TILES");
            OfflineRouter::open(vec![tiles], None).ok().unwrap()
        }

        fn point(lat: f64, lng: f64) -> DirectionsPoint {
            DirectionsPoint { lat, lng }
        }

        /// Trips for each way of traveling, and how fast each goes, in meters per second.
        fn trips() -> Vec<(
            &'static str,
            NavigationMode,
            f64,
            DirectionsPoint,
            DirectionsPoint,
        )> {
            vec![
                // Northgate to Sea-Tac: city streets, then I-5.
                (
                    "auto north-south",
                    NavigationMode::Driving,
                    15.0,
                    point(47.7062, -122.3255),
                    point(47.4502, -122.3088),
                ),
                // Westlake to Cal Anderson Park: downtown's grid, where the most roads are.
                (
                    "auto downtown",
                    NavigationMode::Driving,
                    10.0,
                    point(47.6114, -122.3375),
                    point(47.6170378, -122.3191692),
                ),
                (
                    "bicycle",
                    NavigationMode::Cycling,
                    5.0,
                    point(47.6510, -122.3500),
                    point(47.6553, -122.3035),
                ),
                (
                    "pedestrian",
                    NavigationMode::Walking,
                    1.4,
                    point(47.6094, -122.3422),
                    point(47.6205, -122.3493),
                ),
            ]
        }

        /// A small deterministic generator, for the fixes' noise.
        struct Noise(u64);

        impl Noise {
            /// Uniform in [-1, 1).
            fn next(&mut self) -> f64 {
                self.0 = self
                    .0
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
            }
        }

        /// Fixes a second apart along the route from `from` to `to`, set 3 m to the left
        /// (Ferrostar's simulation bias) and thrown up to 3 m more each way at random, as a
        /// phone's are, each claiming 5 m of accuracy.
        fn fixes(
            router: &OfflineRouter,
            mode: NavigationMode,
            speed: f64,
            from: DirectionsPoint,
            to: DirectionsPoint,
        ) -> Vec<NavLocation> {
            let adapter = DirectionsAdapter::valhalla(
                "offline:route".into(),
                mode.costing().into(),
                0,
                "en-US".into(),
                DistanceUnits::Kilometers,
                false,
            )
            .unwrap();
            let request = adapter.request(from, to).unwrap();
            let response = router.route(request.body);
            assert_eq!(response.status, 200);
            let route = adapter
                .parse(response.body)
                .unwrap()
                .swap_remove(0)
                .navigable;
            let mut simulation =
                LocationSimulation::new(&route, Some(speed), SimulationBias::Left, 3.0).unwrap();
            let mut noise = Noise(7);
            let mut fixes = Vec::new();
            let start = 1_763_000_000_000;
            while !simulation.is_finished() {
                let mut fix = simulation.advance();
                let cos = fix.lat.to_radians().cos();
                fix.lat += 3.0 * noise.next() / 111_195.0;
                fix.lng += 3.0 * noise.next() / (111_195.0 * cos);
                fix.horizontal_accuracy = 5.0;
                fix.timestamp_ms = start + 1000 * fixes.len() as i64;
                fixes.push(fix);
            }
            fixes
        }

        /// The degrees between two headings, 0 to 180.
        fn angle(a: f64, b: f64) -> f64 {
            let d = (a - b).rem_euclid(360.0);
            d.min(360.0 - d)
        }

        fn bearing(a: (f64, f64), b: (f64, f64)) -> f64 {
            let (la, lb) = (a.0.to_radians(), b.0.to_radians());
            let dl = (b.1 - a.1).to_radians();
            let y = dl.sin() * lb.cos();
            let x = la.cos() * lb.sin() - la.sin() * lb.cos() * dl.cos();
            y.atan2(x).to_degrees().rem_euclid(360.0)
        }

        fn meters(a: (f64, f64), b: (f64, f64)) -> f64 {
            let cos = a.0.to_radians().cos();
            ((a.0 - b.0) * 111_195.0).hypot((a.1 - b.1) * 111_195.0 * cos)
        }

        /// A Valhalla polyline (six decimal places) as latitudes and longitudes.
        fn decode(polyline: &str) -> Vec<(f64, f64)> {
            let (mut points, mut lat, mut lng) = (Vec::new(), 0i64, 0i64);
            let mut bytes = polyline.bytes().peekable();
            let mut next = || -> Option<i64> {
                let (mut result, mut shift) = (0i64, 0);
                loop {
                    let b = i64::from(bytes.next()?) - 63;
                    result |= (b & 0x1f) << shift;
                    shift += 5;
                    if b < 0x20 {
                        break;
                    }
                }
                Some(if result & 1 == 1 {
                    !(result >> 1)
                } else {
                    result >> 1
                })
            };
            while let (Some(dlat), Some(dlng)) = (next(), next()) {
                lat += dlat;
                lng += dlng;
                points.push((lat as f64 / 1e6, lng as f64 / 1e6));
            }
            points
        }

        /// The inference [`parse_match`] relies on: the path's last edge ends at the last
        /// matched point (Thor trims it there), so its `end_heading` is the road's heading
        /// at the last fix.
        ///
        /// For windows ending all along each trip, the answer's shape ends where the last
        /// point matched, that point is on the last edge, and the last edge's `end_heading`
        /// is the heading of the chord over that edge's last few meters, as mainline takes
        /// it. On a curve the chord lags the road's tangent at the point: it prints by how
        /// much at worst.
        #[test]
        #[ignore = "needs CARDINAL_SEATTLE_TILES"]
        fn end_heading_is_the_roads_heading_at_the_last_fix() {
            let router = router();
            for (name, mode, speed, from, to) in trips() {
                let fixes = fixes(&router, mode, speed, from, to);
                let (mut checked, mut no_heading, mut past_15) = (0, 0, 0);
                let (mut worst_end, mut worst_heading, mut worst_tangent) =
                    (0.0f64, 0.0f64, 0.0f64);
                for end in (30..fixes.len()).step_by(7) {
                    let window = &fixes[end - 30..end];
                    let mut body: Value =
                        serde_json::from_slice(&match_body(mode, window)).unwrap();
                    body["filters"]["attributes"]
                        .as_array_mut()
                        .unwrap()
                        .extend(["shape".into(), "edge.begin_shape_index".into()]);
                    let response = router.trace_attributes(serde_json::to_vec(&body).unwrap());
                    assert_eq!(response.status, 200, "{name} {end}");
                    let answer: Value = serde_json::from_slice(&response.body).unwrap();
                    let last = answer["matched_points"].as_array().unwrap().last().unwrap();
                    if last["type"] == "unmatched" {
                        continue;
                    }
                    let edges = answer["edges"].as_array().unwrap();
                    assert_eq!(
                        last["edge_index"].as_u64(),
                        Some(edges.len() as u64 - 1),
                        "{name} {end}: the last point is on the last edge"
                    );
                    let shape = decode(answer["shape"].as_str().unwrap());
                    let matched = (last["lat"].as_f64().unwrap(), last["lon"].as_f64().unwrap());
                    let tail = *shape.last().unwrap();
                    worst_end = worst_end.max(meters(tail, matched));
                    // The heading over the shape's last 2 m, as near the tangent there as
                    // the shape says.
                    let before = shape
                        .iter()
                        .rev()
                        .find(|p| meters(**p, tail) >= 2.0)
                        .copied();
                    // Mainline takes an edge's end heading from the point 15 m back along
                    // the edge (21 or 24 m on major roads, 7.5 m on paths: `heading_offset`)
                    // to its end, within the edge's own part of the shape; that point, for
                    // each offset.
                    let first =
                        edges.last().unwrap()["begin_shape_index"].as_u64().unwrap() as usize;
                    let chords: Vec<(f64, f64)> = [7.5, 15.0, 21.0, 24.0]
                        .iter()
                        .map(|&offset| {
                            let mut walked = 0.0;
                            for i in (first + 1..shape.len()).rev() {
                                let step = meters(shape[i - 1], shape[i]);
                                if walked + step > offset {
                                    let t = (offset - walked) / step;
                                    let (p0, p1) = (shape[i - 1], shape[i]);
                                    return (p1.0 + (p0.0 - p1.0) * t, p1.1 + (p0.1 - p1.1) * t);
                                }
                                walked += step;
                            }
                            shape[first]
                        })
                        .collect();
                    let heading = parse_match(200, &response.body).unwrap().heading;
                    match (heading, before) {
                        (Some(heading), Some(before)) => {
                            let tangent = angle(heading, bearing(before, tail));
                            worst_tangent = worst_tangent.max(tangent);
                            past_15 += usize::from(tangent > 15.0);
                            let off = chords
                                .iter()
                                .map(|&chord| angle(heading, bearing(chord, tail)))
                                .fold(f64::MAX, f64::min);
                            worst_heading = worst_heading.max(off);
                        }
                        _ => no_heading += 1,
                    }
                    checked += 1;
                }
                println!(
                    "{name}: {checked} windows; the shape ends at most {worst_end:.2} m from the \
                     last match; end_heading is at most {worst_heading:.1}° from the last edge's \
                     chord and {worst_tangent:.1}° from its last 2 m ({past_15} over 15°); \
                     {no_heading} without a heading"
                );
                assert!(checked > 0, "{name}");
                assert!(worst_end < 0.5, "{name}: {worst_end} m");
                // Whole degrees, and the shape's six decimal places over a short chord.
                assert!(worst_heading < 3.0, "{name}: {worst_heading}°");
            }
        }

        /// How long [`OfflineRouter::match_position`] takes per fix, body and parsing
        /// included, over 30 s and 60 s windows of each trip. Run it in a release build:
        ///
        /// ```sh
        /// CARDINAL_SEATTLE_TILES=... cargo test --release -p routing_flutter match_latency \
        ///     -- --ignored --nocapture
        /// ```
        #[test]
        #[ignore = "needs CARDINAL_SEATTLE_TILES; a benchmark"]
        fn match_latency() {
            let router = router();
            println!("| trip | window | fixes matched | unmatched | median ms | p95 ms | max ms |");
            println!("|---|---|---|---|---|---|---|");
            for (name, mode, speed, from, to) in trips() {
                let fixes = fixes(&router, mode, speed, from, to);
                for window in [30, 60] {
                    // Warm the candidate grids along the way first, as a trip does as it goes.
                    let ends: Vec<usize> = (window..=fixes.len())
                        .step_by((fixes.len() / 300).max(1))
                        .collect();
                    for &end in ends.iter().take(3) {
                        router.match_position(mode, fixes[end - window..end].to_vec());
                    }
                    let mut times = Vec::new();
                    let mut unmatched = 0;
                    for &end in &ends {
                        let window = fixes[end - window..end].to_vec();
                        let started = Instant::now();
                        let matched = router.match_position(mode, window);
                        times.push(started.elapsed().as_secs_f64() * 1000.0);
                        unmatched += usize::from(matched.is_none());
                    }
                    times.sort_by(f64::total_cmp);
                    let at = |q: f64| times[((times.len() - 1) as f64 * q).round() as usize];
                    println!(
                        "| {name} | {window} s | {} | {unmatched} | {:.2} | {:.2} | {:.2} |",
                        times.len(),
                        at(0.5),
                        at(0.95),
                        times.last().unwrap()
                    );
                }
            }
        }
    }
}
