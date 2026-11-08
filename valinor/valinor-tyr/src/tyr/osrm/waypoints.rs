//! OSRM `waypoints` and `tracepoints` (`osrm::waypoint` and `osrm::waypoints`
//! in mainline's `serializers.cc`, and the optimized route `waypoints`
//! in `route_serializer_osrm.cc`).

use super::json::{Json, Map};
use crate::tyr::openlr::distance;
use crate::tyr::polyline::LngLat;
use crate::tyr::unknown;
use valhalla_proto::{LatLng, Location, Trip, lat_lng};
use valhalla_response::ValhallaError;

/// `to_ll`: unset coordinates read as 0.
pub(super) fn to_ll(ll: Option<&LatLng>) -> LngLat {
    let lat = match ll.and_then(|ll| ll.has_lat.as_ref()) {
        Some(lat_lng::HasLat::Lat(lat)) => *lat,
        None => 0.0,
    };
    let lng = match ll.and_then(|ll| ll.has_lng.as_ref()) {
        Some(lat_lng::HasLng::Lng(lng)) => *lng,
        None => 0.0,
    };
    LngLat { lng, lat }
}

/// What kind of waypoint to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WaypointKind {
    /// A route's waypoint.
    Route,
    /// A map matching tracepoint, with its matching indexes.
    Tracepoint,
    /// An optimized route's waypoint, with its index in the optimized order.
    Optimized,
}

/// `osrm::waypoint`: the location snapped to the road, its street name,
/// and the distance it was snapped.
fn waypoint(location: &Location, kind: WaypointKind) -> Result<Json, ValhallaError> {
    let correlation = location.correlation.clone().unwrap_or_default();
    let edge = correlation
        .edges
        .first()
        .ok_or_else(|| unknown("waypoint without a correlated edge"))?;
    let snapped = to_ll(edge.ll.as_ref());
    let mut out = Map::new();
    out.emplace(
        "location",
        vec![Json::Fixed(snapped.lng, 6), Json::Fixed(snapped.lat, 6)],
    );
    out.emplace("name", edge.names.first().cloned().unwrap_or_default());
    out.emplace(
        "distance",
        Json::Fixed(distance(to_ll(location.ll.as_ref()), snapped), 3),
    );
    match kind {
        WaypointKind::Route => {}
        WaypointKind::Tracepoint => {
            out.emplace("alternatives_count", (correlation.edges.len() - 1) as u64);
            if correlation.waypoint_index == u32::MAX {
                // Neither a break nor a leg's end.
                out.emplace("waypoint_index", Json::Null);
            } else {
                out.emplace("waypoint_index", correlation.waypoint_index);
            }
            out.emplace("matchings_index", correlation.route_index);
        }
        WaypointKind::Optimized => {
            // TODO (from mainline): trips_index is always 0.
            out.emplace("trips_index", 0_u64);
            out.emplace("waypoint_index", correlation.waypoint_index);
        }
    }
    Ok(out.into())
}

/// `osrm::waypoints(trip)`: every leg's locations, without repeating where legs meet.
///
/// All routes share the first route's waypoints.
pub(super) fn route_waypoints(trip: &Trip) -> Result<Vec<Json>, ValhallaError> {
    let route = trip
        .routes
        .first()
        .ok_or_else(|| unknown("the trip has no routes"))?;
    let mut out = Vec::new();
    for leg in &route.legs {
        for (i, location) in leg.location.iter().enumerate() {
            if i == 0 && !out.is_empty() {
                continue;
            }
            out.push(waypoint(location, WaypointKind::Route)?);
        }
    }
    Ok(out)
}

/// `osrm::waypoints(locations, is_tracepoint)`: `null` for locations that matched nothing.
pub(super) fn tracepoints(shape: &[Location]) -> Result<Vec<Json>, ValhallaError> {
    shape
        .iter()
        .map(|location| {
            if location
                .correlation
                .as_ref()
                .is_none_or(|correlation| correlation.edges.is_empty())
            {
                Ok(Json::Null)
            } else {
                waypoint(location, WaypointKind::Tracepoint)
            }
        })
        .collect()
}

/// The optimized route `waypoints`: in the original order,
/// each with its index in the optimized order (which mainline also stores in the request).
pub(super) fn optimized_waypoints(locations: &mut [Location]) -> Result<Vec<Json>, ValhallaError> {
    let original_index = |location: &Location| {
        location
            .correlation
            .as_ref()
            .map_or(0, |correlation| correlation.original_index)
    };
    let mut indexes: Vec<usize> = (0..locations.len()).collect();
    // Original indexes are unique, so the sort order is fully determined.
    indexes.sort_by_key(|&i| original_index(&locations[i]));
    let mut out = Vec::with_capacity(indexes.len());
    for index in indexes {
        let location = &mut locations[index];
        location
            .correlation
            .get_or_insert_with(Default::default)
            .waypoint_index = u32::try_from(index).unwrap_or(u32::MAX);
        out.push(waypoint(location, WaypointKind::Optimized)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::{Correlation, PathEdge};

    fn ll(lat: f64, lng: f64) -> LatLng {
        LatLng {
            has_lat: Some(lat_lng::HasLat::Lat(lat)),
            has_lng: Some(lat_lng::HasLng::Lng(lng)),
        }
    }

    fn location(original_index: u32, edges: usize) -> Location {
        Location {
            ll: Some(ll(42.5, 1.5)),
            correlation: Some(Correlation {
                edges: (0..edges)
                    .map(|_| PathEdge {
                        ll: Some(ll(42.5001, 1.5)),
                        names: vec!["Main".to_string()],
                        ..PathEdge::default()
                    })
                    .collect(),
                original_index,
                waypoint_index: u32::MAX,
                route_index: 2,
                ..Correlation::default()
            }),
            ..Location::default()
        }
    }

    fn json(values: Vec<Json>) -> String {
        let mut writer = crate::tyr::writer::JsonWriter::new();
        Json::Array(values).write(&mut writer);
        writer.finish()
    }

    #[test]
    fn tracepoints_and_nulls() {
        let out = tracepoints(&[location(0, 2), location(1, 0)]).expect("serializes");
        assert_eq!(
            json(out),
            concat!(
                r#"[{"location":[1.500000,42.500100],"name":"Main","distance":11.132,"#,
                r#""alternatives_count":1,"waypoint_index":null,"matchings_index":2},null]"#
            )
        );
    }

    #[test]
    fn optimized_order() {
        let mut locations = [location(1, 1), location(0, 1)];
        let out = optimized_waypoints(&mut locations).expect("serializes");
        let text = json(out);
        assert!(
            text.starts_with(r#"[{"location":[1.500000,42.500100],"name":"Main","distance":11.132,"trips_index":0,"waypoint_index":1}"#),
            "{text}"
        );
        assert_eq!(
            locations[0].correlation.as_ref().map(|c| c.waypoint_index),
            Some(0)
        );
    }

    #[test]
    fn missing_edges_are_errors() {
        let trip = Trip {
            routes: vec![valhalla_proto::TripRoute {
                legs: vec![valhalla_proto::TripLeg {
                    location: vec![location(0, 0)],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        assert!(route_waypoints(&trip).is_err());
        assert!(route_waypoints(&Trip::default()).is_err());
    }
}
