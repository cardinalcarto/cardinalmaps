//! A leg's `valinor_details`: what the road is like along the leg, edge by edge,
//! and the leg's elevation.
//!
//! This is Valinor's own extension to the OSRM format, which mainline doesn't have.
//! A request asks for it with `valinor_leg_details`,
//! and nothing else in the response changes with it
//! (see `docs/osrm-leg-details.md` for the schema).
//!
//! OSRM's own output says what a road is only per maneuver or per intersection
//! (a step's intersections' `classes`), and has no elevation at all.
//! A client that draws a route's roads and its profile needs both along the route,
//! and the trip leg already has them on its edges, as `trace_attributes` shows them.

use super::json::{Json, Map};
use crate::tyr::trace_attributes::{cycle_lane_name, road_class_name, travel_mode_name, use_name};
use crate::tyr::trip::edge_of;
use crate::tyr::valhalla::elevation;
use valhalla_proto::trip_leg::{Edge, Node};
use valhalla_proto::{Options, TravelMode, TripLeg};

/// The height the graph gives a place it has no elevation for (mainline's `kMinElevation`),
/// in meters: a stored height of zero decodes to it.
const NO_ELEVATION: f32 = -500.0;

/// What a run's edges have in common: everything a run says but how long and how far it is,
/// and the speed the costing gives them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RoadKind {
    travel_mode: i32,
    road_class: i32,
    road_use: i32,
    /// The cycle lane and whether the edge is on a bicycle network, for an edge that is cycled.
    cycling: Option<(i32, bool)>,
    toll: bool,
    tunnel: bool,
    bridge: bool,
    /// The bits of the speed the costing gives the edge, in km/h:
    /// edges at different speeds are different runs, whatever else they share.
    speed: u32,
}

impl RoadKind {
    fn of(edge: &Edge) -> Self {
        Self {
            travel_mode: edge.travel_mode,
            road_class: edge.road_class,
            road_use: edge.r#use,
            cycling: (edge.travel_mode == TravelMode::KBicycle as i32)
                .then_some((edge.cycle_lane, edge.bicycle_network)),
            toll: edge.toll,
            tunnel: edge.tunnel,
            bridge: edge.bridge,
            speed: edge.speed.to_bits(),
        }
    }
}

/// A run: consecutive edges of one kind, with where along the leg it ends.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    kind: RoadKind,
    /// The seconds elapsed at the end of the run, from the start of the leg.
    end_seconds: f64,
    /// The meters travelled at the end of the run, from the start of the leg.
    end_meters: f64,
}

fn elapsed_seconds(node: &Node) -> f64 {
    node.cost
        .unwrap_or_default()
        .elapsed_cost
        .unwrap_or_default()
        .seconds
}

/// The leg's runs, in travel order: its edges, with consecutive edges of one kind joined.
///
/// Edges join only when nothing a client could tell them apart by is lost,
/// so not edges whose speeds differ, however little:
/// a client that draws a speed profile decides for itself which speeds are near enough.
///
/// An edge's time is the elapsed time at its end node less that at its start node,
/// which is how the leg's own duration is counted,
/// so it includes the turn onto the edge, and the runs end when the leg does.
/// An edge's length is the trip's (`length_km`), which is that of the part of it the leg uses.
fn runs(leg: &TripLeg) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut meters = 0.0_f64;
    let start_seconds = leg.node.first().map_or(0.0, elapsed_seconds);
    for pair in leg.node.windows(2) {
        let [node, next] = pair else { continue };
        let edge = edge_of(node);
        let kind = RoadKind::of(edge);
        meters += f64::from(edge.length_km) * 1000.0;
        let end_seconds = elapsed_seconds(next) - start_seconds;
        match runs.last_mut() {
            Some(run) if run.kind == kind => {
                run.end_seconds = end_seconds;
                run.end_meters = meters;
            }
            _ => runs.push(Run {
                kind,
                end_seconds,
                end_meters: meters,
            }),
        }
    }
    runs
}

/// `value` rounded to `places` decimal places.
fn rounded(value: f64, places: i32) -> f64 {
    let scale = 10.0_f64.powi(places);
    (value * scale).round() / scale
}

/// The decimal places of a run's `duration`.
const DURATION_PLACES: i32 = 3;
/// The decimal places of a run's `distance`.
const DISTANCE_PLACES: i32 = 1;

/// The runs as JSON.
///
/// Each duration and distance is the difference of the rounded totals at the run's two ends,
/// so the rounding doesn't add up along the leg:
/// the runs' sum is the rounded total at the end of the last.
fn runs_json(runs: &[Run]) -> Vec<Json> {
    let (mut seconds_so_far, mut meters_so_far) = (0.0_f64, 0.0_f64);
    runs.iter()
        .map(|run| {
            let end_seconds = rounded(run.end_seconds, DURATION_PLACES);
            let end_meters = rounded(run.end_meters, DISTANCE_PLACES);
            let duration = end_seconds - seconds_so_far;
            let distance = end_meters - meters_so_far;
            (seconds_so_far, meters_so_far) = (end_seconds, end_meters);

            let mut out = Map::new();
            out.emplace("duration", Json::Fixed(duration, 3));
            out.emplace("distance", Json::Fixed(distance, 1));
            let speed = if duration > 0.0 {
                distance / duration
            } else {
                0.0
            };
            out.emplace("speed", Json::Fixed(speed, 2));
            let kind = run.kind;
            out.emplace("travel_mode", travel_mode_name(kind.travel_mode));
            out.emplace("road_class", road_class_name(kind.road_class));
            out.emplace("use", use_name(kind.road_use));
            if let Some((cycle_lane, bicycle_network)) = kind.cycling {
                out.emplace("cycle_lane", cycle_lane_name(cycle_lane));
                out.emplace("bicycle_network", bicycle_network);
            }
            for (key, value) in [
                ("toll", kind.toll),
                ("tunnel", kind.tunnel),
                ("bridge", kind.bridge),
            ] {
                if value {
                    out.emplace(key, true);
                }
            }
            out.into()
        })
        .collect()
}

/// The leg's elevation every `interval` meters, as the Valhalla format gives it
/// ([`elevation`]), or none if the graph doesn't have the elevation of all of the leg.
///
/// The Valhalla format fails the request when the first edge has no samples,
/// and gives −500 m wherever a tile was built without elevation.
/// Neither is any use to a client drawing a profile, and a route is worth having without one,
/// so here a leg with an edge that has no samples, or a sample that is the graph's
/// "no elevation" height, has no elevation.
fn leg_elevation(leg: &TripLeg, interval: f32) -> Option<Vec<f32>> {
    let (_, edges) = leg.node.split_last()?;
    let known = !edges.is_empty()
        && edges.iter().map(edge_of).all(|edge| {
            !edge.elevation.is_empty() && edge.elevation.iter().all(|&h| h > NO_ELEVATION)
        });
    if !known {
        return None;
    }
    elevation(leg, interval).ok()
}

/// The leg's `valinor_details`: its `runs`, and with a positive `elevation_interval`
/// and elevation in the graph, the interval and the `elevation` sampled at it, in meters
/// whatever the request's units (as the rest of the OSRM format is).
pub(super) fn leg_details(leg: &TripLeg, options: &Options) -> Map {
    let mut details = Map::new();
    details.emplace("runs", runs_json(&runs(leg)));
    let interval = options.elevation_interval;
    if interval > 0.0
        && let Some(heights) = leg_elevation(leg, interval)
    {
        details.emplace("elevation_interval", Json::Fixed(f64::from(interval), 1));
        let heights: Vec<Json> = heights
            .into_iter()
            .map(|height| Json::Fixed(f64::from(height), 1))
            .collect();
        details.emplace("elevation", heights);
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::captures;
    use crate::tyr::serialize_directions;
    use serde_json::{Value, json};
    use std::path::Path;
    use valhalla_proto::trip_leg::{Cost, CycleLane, PathCost, Use};
    use valhalla_proto::{Api, RoadClass};

    fn node(seconds: f64, edge: Option<Edge>) -> Node {
        Node {
            edge,
            cost: Some(PathCost {
                elapsed_cost: Some(Cost { seconds, cost: 0.0 }),
                transition_cost: None,
            }),
            ..Node::default()
        }
    }

    fn street(length_km: f32) -> Edge {
        Edge {
            length_km,
            road_class: RoadClass::KResidential as i32,
            travel_mode: TravelMode::KDrive as i32,
            ..Edge::default()
        }
    }

    /// A leg of `edges`, each ending at the elapsed seconds given with it.
    fn leg(edges: Vec<(Edge, f64)>) -> TripLeg {
        let mut nodes = Vec::new();
        let mut elapsed = 0.0;
        for (edge, end) in edges {
            nodes.push(node(elapsed, Some(edge)));
            elapsed = end;
        }
        nodes.push(node(elapsed, None));
        TripLeg {
            node: nodes,
            ..TripLeg::default()
        }
    }

    fn details(leg: &TripLeg, elevation_interval: f32) -> Value {
        let options = Options {
            elevation_interval,
            ..Options::default()
        };
        serde_json::from_str(&leg_details(leg, &options).to_json()).expect("valid JSON")
    }

    #[test]
    fn consecutive_edges_of_one_kind_are_one_run() {
        let motorway = |length_km| Edge {
            road_class: RoadClass::KMotorway as i32,
            ..street(length_km)
        };
        let tolled = |length_km| Edge {
            toll: true,
            tunnel: true,
            ..motorway(length_km)
        };
        let leg = leg(vec![
            (street(0.1), 20.0),
            (street(0.2), 50.0),
            (motorway(1.0), 90.0),
            (tolled(2.0), 170.0),
            (tolled(0.5), 190.0),
            (motorway(0.25), 200.0),
            (
                Edge {
                    r#use: Use::KFerryUse as i32,
                    bridge: true,
                    ..street(3.0)
                },
                800.0,
            ),
        ]);
        assert_eq!(
            details(&leg, 0.0),
            json!({"runs": [
                {"duration": 50.0, "distance": 300.0, "speed": 6.0, "travel_mode": "drive",
                 "road_class": "residential", "use": "road"},
                {"duration": 40.0, "distance": 1000.0, "speed": 25.0, "travel_mode": "drive",
                 "road_class": "motorway", "use": "road"},
                {"duration": 100.0, "distance": 2500.0, "speed": 25.0, "travel_mode": "drive",
                 "road_class": "motorway", "use": "road", "toll": true, "tunnel": true},
                {"duration": 10.0, "distance": 250.0, "speed": 25.0, "travel_mode": "drive",
                 "road_class": "motorway", "use": "road"},
                {"duration": 600.0, "distance": 3000.0, "speed": 5.0, "travel_mode": "drive",
                 "road_class": "residential", "use": "ferry", "bridge": true},
            ]})
        );
    }

    #[test]
    fn edges_at_different_speeds_are_different_runs() {
        let at = |speed: f32, length_km| Edge {
            speed,
            ..street(length_km)
        };
        // The speed is the costing's for the edge, not what its time and length come to:
        // the second edge has a slow turn onto it and still joins the first.
        let leg = leg(vec![
            (at(30.0, 0.1), 12.0),
            (at(30.0, 0.1), 32.0),
            (at(30.5, 0.1), 44.0),
            (at(50.0, 0.5), 80.0),
        ]);
        let details = details(&leg, 0.0);
        let runs = details["runs"].as_array().expect("runs");
        let said: Vec<(&Value, &Value, &Value)> = runs
            .iter()
            .map(|run| (&run["duration"], &run["distance"], &run["speed"]))
            .collect();
        assert_eq!(
            said,
            [
                (&json!(32.0), &json!(200.0), &json!(6.25)),
                (&json!(12.0), &json!(100.0), &json!(8.33)),
                (&json!(36.0), &json!(500.0), &json!(13.89)),
            ]
        );
    }

    #[test]
    fn only_cycled_edges_say_what_cycling_them_is_like() {
        let cycled = |cycle_lane: CycleLane, bicycle_network| Edge {
            travel_mode: TravelMode::KBicycle as i32,
            cycle_lane: cycle_lane as i32,
            bicycle_network,
            ..street(0.1)
        };
        let leg = leg(vec![
            (cycled(CycleLane::KNoCycleLane, false), 10.0),
            (cycled(CycleLane::KDedicated, false), 20.0),
            (cycled(CycleLane::KDedicated, true), 30.0),
            (
                Edge {
                    r#use: Use::KCyclewayUse as i32,
                    ..cycled(CycleLane::KSeparated, true)
                },
                40.0,
            ),
            // A walked edge's cycle lane is nothing to the run, and doesn't split it.
            (
                Edge {
                    travel_mode: TravelMode::KPedestrian as i32,
                    r#use: Use::KStepsUse as i32,
                    ..street(0.1)
                },
                50.0,
            ),
            (
                Edge {
                    travel_mode: TravelMode::KPedestrian as i32,
                    r#use: Use::KStepsUse as i32,
                    cycle_lane: CycleLane::KShared as i32,
                    ..street(0.1)
                },
                60.0,
            ),
        ]);
        let details = details(&leg, 0.0);
        let runs = details["runs"].as_array().expect("runs");
        let said: Vec<(&Value, &Value, &Value, &Value)> = runs
            .iter()
            .map(|run| {
                (
                    &run["travel_mode"],
                    &run["use"],
                    &run["cycle_lane"],
                    &run["bicycle_network"],
                )
            })
            .collect();
        assert_eq!(
            said,
            [
                (
                    &json!("bicycle"),
                    &json!("road"),
                    &json!("none"),
                    &json!(false)
                ),
                (
                    &json!("bicycle"),
                    &json!("road"),
                    &json!("dedicated"),
                    &json!(false)
                ),
                (
                    &json!("bicycle"),
                    &json!("road"),
                    &json!("dedicated"),
                    &json!(true)
                ),
                (
                    &json!("bicycle"),
                    &json!("cycleway"),
                    &json!("separated"),
                    &json!(true)
                ),
                (
                    &json!("pedestrian"),
                    &json!("steps"),
                    &Value::Null,
                    &Value::Null
                ),
            ]
        );
        assert_eq!(runs[4]["distance"], json!(200.0));
    }

    #[test]
    fn rounding_does_not_add_up_along_the_leg() {
        // A thousand edges of alternating kinds, each 0.0004 s and 0.04 m longer than a
        // round figure: rounded one by one they would lose 0.4 s and 40 m.
        let edges = (1..=1000)
            .map(|i| {
                let mut edge = street(0.010_04);
                edge.tunnel = i % 2 == 0;
                (edge, f64::from(i) * 1.0004)
            })
            .collect();
        let details = details(&leg(edges), 0.0);
        let runs = details["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 1000);
        let sum = |key: &str| -> f64 { runs.iter().map(|run| run[key].as_f64().unwrap()).sum() };
        assert!(
            (sum("duration") - 1000.4).abs() < 1e-6,
            "{}",
            sum("duration")
        );
        assert!(
            (sum("distance") - 10_040.0).abs() < 0.06,
            "{}",
            sum("distance")
        );
    }

    #[test]
    fn a_leg_without_edges_has_no_runs() {
        assert_eq!(details(&TripLeg::default(), 30.0), json!({"runs": []}));
        assert_eq!(details(&leg(Vec::new()), 30.0), json!({"runs": []}));
    }

    fn with_heights(heights: &[f32]) -> Edge {
        Edge {
            elevation: heights.to_vec(),
            elevation_sampling_interval: 30.0,
            ..street(0.06)
        }
    }

    #[test]
    fn elevation_is_sampled_at_the_requests_interval_in_meters() {
        let leg = leg(vec![
            (with_heights(&[100.0, 110.0, 120.0]), 10.0),
            (with_heights(&[120.0, 105.0, 90.0]), 20.0),
        ]);
        let details = details(&leg, 20.0);
        assert_eq!(details["elevation_interval"], json!(20.0));
        // Every 20 m along 120 m, and the end.
        assert_eq!(
            details["elevation"],
            json!([100.0, 106.7, 113.3, 120.0, 110.0, 100.0, 90.0])
        );
    }

    #[test]
    fn elevation_is_left_out_unless_asked_for_and_known_all_along() {
        let known = with_heights(&[100.0, 110.0, 120.0]);
        let has_elevation = |edges: Vec<Edge>, interval: f32| {
            let leg = leg(edges.into_iter().map(|edge| (edge, 0.0)).collect());
            let details = details(&leg, interval);
            assert_eq!(
                details.get("elevation").is_some(),
                details.get("elevation_interval").is_some()
            );
            assert!(details["runs"].is_array());
            details.get("elevation").is_some()
        };
        assert!(has_elevation(vec![known.clone()], 30.0));
        // No interval: none is asked for.
        assert!(!has_elevation(vec![known.clone()], 0.0));
        // The trip was built without elevation: the first edge has no samples,
        // which fails a request in the Valhalla format.
        assert!(!has_elevation(vec![street(0.06), known.clone()], 30.0));
        assert!(!has_elevation(vec![known.clone(), street(0.06)], 30.0));
        // A tile built without elevation gives its nodes the "no elevation" height.
        assert!(!has_elevation(
            vec![known.clone(), with_heights(&[120.0, -500.0])],
            30.0
        ));
        assert!(!has_elevation(vec![with_heights(&[-500.0, -500.0])], 30.0));
        // Below sea level is still elevation.
        assert!(has_elevation(vec![with_heights(&[-10.0, -430.0])], 30.0));
    }

    /// The captured OSRM-format cases with what Odin handed Tyr for each.
    fn osrm_cases() -> Vec<(String, Api)> {
        captures("osrm")
            .into_iter()
            .filter_map(|capture| Some((capture.name(), capture.odin_output()?)))
            .collect()
    }

    fn serialized(api: &Api, leg_details: bool, elevation_interval: Option<f32>) -> Value {
        let mut api = api.clone();
        let options = api.options.as_mut().expect("options");
        options.valinor_leg_details = leg_details;
        if let Some(interval) = elevation_interval {
            options.elevation_interval = interval;
        }
        let response = serialize_directions(&mut api).expect("serializes");
        serde_json::from_slice(&response.body).expect("valid JSON")
    }

    fn legs_mut(response: &mut Value) -> impl Iterator<Item = &mut Value> {
        let key = if response.get("matchings").is_some() {
            "matchings"
        } else {
            "routes"
        };
        response[key]
            .as_array_mut()
            .expect("routes")
            .iter_mut()
            .flat_map(|route| route["legs"].as_array_mut().expect("legs").iter_mut())
    }

    /// Asking for the details adds `valinor_details` to each leg of every captured case and
    /// changes nothing else: with the details taken out again, the response is the one
    /// without them (which `osrm_responses_match_mainline` holds to mainline's).
    /// An elevation interval alone changes nothing at all.
    #[test]
    fn the_details_are_all_that_asking_for_them_changes() {
        let cases = osrm_cases();
        assert!(cases.len() >= 40, "only {} captures", cases.len());
        for (name, api) in &cases {
            let plain = serialized(api, false, None);
            assert_eq!(serialized(api, false, Some(30.0)), plain, "{name}");
            for interval in [None, Some(30.0)] {
                let mut detailed = serialized(api, true, interval);
                for leg in legs_mut(&mut detailed) {
                    let details = leg
                        .as_object_mut()
                        .expect("a leg")
                        .remove("valinor_details");
                    assert!(details.is_some_and(|d| d["runs"].is_array()), "{name}");
                }
                assert_eq!(detailed, plain, "{name}");
            }
        }
    }

    /// On every captured route (Andorra and Seattle, which mainline built),
    /// a leg's runs take the leg's `duration` to the millisecond it is written to,
    /// and go its `distance` to within a meter and a part in 100,000:
    /// the leg's own distance is Odin's sum of its maneuvers' lengths in single precision,
    /// in the request's units.
    #[test]
    fn a_legs_runs_add_up_to_the_leg() {
        let mut legs = 0;
        let (mut worst_seconds, mut worst_meters) = (0.0_f64, 0.0_f64);
        for (name, api) in &osrm_cases() {
            let mut detailed = serialized(api, true, None);
            for leg in legs_mut(&mut detailed) {
                let runs = leg["valinor_details"]["runs"].as_array().expect("runs");
                let sum =
                    |key: &str| -> f64 { runs.iter().map(|run| run[key].as_f64().unwrap()).sum() };
                let duration = leg["duration"].as_f64().expect("duration");
                let distance = leg["distance"].as_f64().expect("distance");
                let seconds_off = (sum("duration") - duration).abs();
                let meters_off = (sum("distance") - distance).abs();
                assert!(
                    seconds_off < 0.0015,
                    "{name}: {seconds_off} s of {duration}"
                );
                assert!(
                    meters_off < 1.0 + distance * 1e-5,
                    "{name}: {meters_off} m of {distance}"
                );
                worst_seconds = worst_seconds.max(seconds_off);
                worst_meters = worst_meters.max(meters_off);
                legs += 1;
            }
        }
        assert!(legs >= 40, "only {legs} legs");
        println!("{legs} legs: at most {worst_seconds:.4} s and {worst_meters:.3} m off");
    }

    /// A toll is on the runs where it is on the edges.
    ///
    /// The third route of `seattle/auto_alternates` takes SR 99 through its tolled tunnel
    /// under downtown Seattle. Its step on SR 99 is 7.8 km,
    /// and mainline marks every intersection of the step `toll`,
    /// since a maneuver any part of which is tolled is tolled (`portions_toll`).
    /// The runs have the toll on the 3.8 km that is: the ramp into the tunnel, the tunnel,
    /// and the road out of it as far as the first exit.
    #[test]
    fn a_toll_is_where_the_edges_have_it() {
        let cases = osrm_cases();
        let (_, api) = cases
            .iter()
            .find(|(name, _)| name == "seattle/auto_alternates/osrm")
            .expect("the case");
        let detailed = serialized(api, true, None);
        let leg = &detailed["routes"][2]["legs"][0];

        let tolled_steps: Vec<(f64, usize, usize)> = leg["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .filter_map(|step| {
                let intersections = step["intersections"].as_array().expect("intersections");
                let tolled = intersections
                    .iter()
                    .filter(|intersection| {
                        intersection["classes"]
                            .as_array()
                            .is_some_and(|classes| classes.contains(&json!("toll")))
                    })
                    .count();
                (tolled > 0).then(|| {
                    assert_eq!(step["ref"], "SR 99");
                    (
                        step["distance"].as_f64().unwrap(),
                        tolled,
                        intersections.len(),
                    )
                })
            })
            .collect();
        assert_eq!(tolled_steps, [(7803.0, 59, 59)]);

        let runs = leg["valinor_details"]["runs"].as_array().expect("runs");
        assert_eq!(runs.len(), 49);
        // Where the tolled runs start and end, to the meter, and how much of them is tunnel.
        let (mut at, mut tunnel) = (0.0_f64, 0.0_f64);
        let mut tolled: Vec<(f64, f64)> = Vec::new();
        for run in runs {
            let distance = run["distance"].as_f64().unwrap();
            let end = at + distance;
            if run.get("toll").is_some() {
                assert_eq!(run["toll"], true);
                if run.get("tunnel").is_some() {
                    tunnel += distance;
                }
                match tolled.last_mut() {
                    Some(stretch) if (stretch.1 - at).abs() < 0.5 => stretch.1 = end.round(),
                    _ => tolled.push((at.round(), end.round())),
                }
            }
            at = end;
        }
        // One stretch, 3,757 m long, 3,344 m of it underground.
        assert_eq!(tolled, [(7379.0, 11136.0)]);
        assert!((tunnel - 3344.0).abs() < 1e-6, "{tunnel}");
    }

    /// Mainline's own trip with elevation (`andorra/pedestrian_elevation`, asked for every 30 m)
    /// gives the leg, in the OSRM format, the heights mainline gives it in the Valhalla format.
    /// The other captured trips were built without an elevation interval,
    /// so their edges have no samples:
    /// asking for elevation of them gives legs without it, not an error.
    #[test]
    fn elevation_is_mainlines_where_the_trip_has_it() {
        let mut with_elevation = Vec::new();
        for (name, api) in &osrm_cases() {
            let mut detailed = serialized(api, true, Some(30.0));
            for leg in legs_mut(&mut detailed) {
                let details = &leg["valinor_details"];
                assert_eq!(
                    details.get("elevation").is_some(),
                    details.get("elevation_interval").is_some(),
                    "{name}"
                );
                if details.get("elevation").is_some() {
                    with_elevation.push((name.clone(), details.clone()));
                }
            }
        }
        let [(name, details)] = with_elevation.as_slice() else {
            panic!("legs with elevation: {with_elevation:?}");
        };
        assert_eq!(name, "andorra/pedestrian_elevation/osrm");
        let valhalla = captures("json")
            .into_iter()
            .find(|capture| capture.case == "andorra/pedestrian_elevation")
            .expect("the case in the Valhalla format")
            .json_response()
            .1;
        let leg = &valhalla["trip"]["legs"][0];
        assert_eq!(details["elevation_interval"], leg["elevation_interval"]);
        // The Valhalla format cuts a height off after its first decimal place
        // (rapidjson's `SetMaxDecimalPlaces`), and this one rounds to it, as OSRM numbers are.
        let heights = |heights: &Value| -> Vec<f64> {
            let heights = heights.as_array().expect("heights");
            heights.iter().map(|h| h.as_f64().unwrap()).collect()
        };
        let (ours, mainlines) = (heights(&details["elevation"]), heights(&leg["elevation"]));
        assert_eq!(ours.len(), 23);
        assert_eq!(ours.len(), mainlines.len());
        for (ours, mainlines) in ours.iter().zip(&mainlines) {
            assert!(
                (0.0..0.1001).contains(&(ours - mainlines)),
                "{ours} {mainlines}"
            );
        }
    }

    /// The Seattle cases Cardinal's bridge reads in its tests (`crates/routing_flutter`):
    /// a drive with the tolled tunnel among its alternates, two bicycle rides, and two walks.
    const BRIDGE_FIXTURES: [&str; 5] = [
        "seattle/auto_alternates",
        "seattle/bicycle_capitol_hill_to_columbia_city",
        "seattle/bicycle_fremont_to_uw",
        "seattle/pedestrian_pike_place_to_space_needle",
        "seattle/pedestrian_westlake_to_capitol_hill",
    ];

    /// Writes [`BRIDGE_FIXTURES`] with their legs' details, as the server would answer them,
    /// to the directory `VALINOR_TYR_LEG_DETAILS_FIXTURES` names,
    /// each as `<case>_details.json` (`seattle_auto_alternates_details.json`).
    /// The trips are mainline's, built without elevation, so the legs have none.
    ///
    /// ```shell
    /// VALINOR_TYR_LEG_DETAILS_FIXTURES=$PWD/../crates/routing_flutter/fixtures \
    ///   cargo test -p valinor-tyr write_leg_details_fixtures -- --ignored
    /// ```
    #[test]
    #[ignore = "writes fixtures (VALINOR_TYR_LEG_DETAILS_FIXTURES)"]
    fn write_leg_details_fixtures() {
        let dir = std::env::var("VALINOR_TYR_LEG_DETAILS_FIXTURES")
            .expect("VALINOR_TYR_LEG_DETAILS_FIXTURES");
        let cases = osrm_cases();
        for case in BRIDGE_FIXTURES {
            let (_, api) = cases
                .iter()
                .find(|(name, _)| name.strip_suffix("/osrm") == Some(case))
                .unwrap_or_else(|| panic!("{case}: no capture"));
            let mut api = api.clone();
            api.options.as_mut().expect("options").valinor_leg_details = true;
            let response = serialize_directions(&mut api).expect("serializes");
            let path = Path::new(&dir).join(format!("{}_details.json", case.replace('/', "_")));
            std::fs::write(&path, response.body)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}
