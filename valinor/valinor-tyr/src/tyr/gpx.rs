//! The GPX route response, as mainline's `pathToGPX` (`tyr/route_serializer.cc`) writes it.
//!
//! Only the first route is written (alternates are dropped, as in mainline).
//! Every shape point becomes a `wpt`,
//! and each leg's nodes become `rtept`s of one `rte`,
//! named after the OSM way of the edge leaving them
//! (the last node repeats the previous way, or `18446744073709551615`,
//! mainline's `uint64_t(-1)`, if the leg has no edges).
//! Coordinates have six fixed decimal places (`std::setprecision(6) << std::fixed`).
//! Send the response with [`super::GPX_CONTENT_DISPOSITION`] as mainline does.

use super::polyline::{self, LngLat};
use super::{GPX_CONTENT_TYPE, Response, unknown};
use core::fmt::Write;
use valhalla_proto::Api;
use valhalla_response::ValhallaError;

/// Serializes the first route as GPX.
///
/// # Errors
///
/// Fails with 299 where mainline would read out of bounds:
/// no trip route, a shape that does not decode, or a node whose shape index is past the shape.
pub(crate) fn serialize(api: &Api) -> Result<Response, ValhallaError> {
    let legs = api
        .trip
        .as_ref()
        .and_then(|trip| trip.routes.first())
        .map(|route| route.legs.as_slice())
        .ok_or_else(|| unknown("the trip has no routes"))?;

    let mut gpx = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><gpx version="1.1" creator="libvalhalla"><metadata/>"#,
    );
    for leg in legs {
        let points =
            polyline::decode(&leg.shape, 1e-6).map_err(|error| unknown(error.to_string()))?;
        for point in &points {
            write_point(&mut gpx, "wpt", *point);
            gpx.push_str("</wpt>");
        }
        gpx.push_str("<rte>");
        let mut last_id = u64::MAX;
        for node in &leg.node {
            let shape_index = match &node.edge {
                Some(edge) => {
                    last_id = edge.way_id;
                    usize::try_from(edge.begin_shape_index).unwrap_or(usize::MAX)
                }
                None => points.len().wrapping_sub(1),
            };
            let point = points
                .get(shape_index)
                .ok_or_else(|| unknown("a trip node's shape index is outside the leg's shape"))?;
            write_point(&mut gpx, "rtept", *point);
            let _ = write!(gpx, "<name>{last_id}</name></rtept>");
        }
        gpx.push_str("</rte>");
    }
    gpx.push_str("</gpx>");
    Ok(Response {
        content_type: GPX_CONTENT_TYPE,
        body: gpx.into_bytes(),
    })
}

fn write_point(gpx: &mut String, tag: &str, point: LngLat) {
    let _ = write!(
        gpx,
        r#"<{tag} lon="{:.6}" lat="{:.6}">"#,
        point.lng, point.lat
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use valhalla_proto::trip_leg::{Edge, Node};
    use valhalla_proto::{Trip, TripLeg, TripRoute};

    fn leg(points: &[LngLat], nodes: Vec<Node>) -> TripLeg {
        TripLeg {
            shape: polyline::encode(points, 1e6),
            node: nodes,
            ..TripLeg::default()
        }
    }

    fn edge_node(way_id: u64, begin_shape_index: u32) -> Node {
        Node {
            edge: Some(Edge {
                way_id,
                begin_shape_index,
                ..Edge::default()
            }),
            ..Node::default()
        }
    }

    fn api(legs: Vec<TripLeg>) -> Api {
        Api {
            trip: Some(Trip {
                routes: vec![
                    TripRoute {
                        legs,
                        ..TripRoute::default()
                    },
                    // Alternates are ignored.
                    TripRoute::default(),
                ],
            }),
            ..Api::default()
        }
    }

    #[test]
    fn two_legs() {
        let a = LngLat {
            lng: 1.5211,
            lat: 42.5078,
        };
        let b = LngLat {
            lng: -1.0,
            lat: 42.5,
        };
        let c = LngLat {
            lng: 1.583,
            lat: 42.535_5,
        };
        let api = api(vec![
            leg(&[a, b], vec![edge_node(7, 0), Node::default()]),
            leg(
                &[b, c],
                vec![edge_node(8, 0), edge_node(9, 1), Node::default()],
            ),
        ]);
        let response = serialize(&api).expect("serializable");
        assert_eq!(response.content_type, GPX_CONTENT_TYPE);
        assert_eq!(
            String::from_utf8(response.body).expect("UTF-8"),
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?><gpx version="1.1" creator="libvalhalla"><metadata/>"#,
                r#"<wpt lon="1.521100" lat="42.507800"></wpt><wpt lon="-1.000000" lat="42.500000"></wpt>"#,
                r#"<rte><rtept lon="1.521100" lat="42.507800"><name>7</name></rtept>"#,
                r#"<rtept lon="-1.000000" lat="42.500000"><name>7</name></rtept></rte>"#,
                r#"<wpt lon="-1.000000" lat="42.500000"></wpt><wpt lon="1.583000" lat="42.535500"></wpt>"#,
                r#"<rte><rtept lon="-1.000000" lat="42.500000"><name>8</name></rtept>"#,
                r#"<rtept lon="1.583000" lat="42.535500"><name>9</name></rtept>"#,
                r#"<rtept lon="1.583000" lat="42.535500"><name>9</name></rtept></rte></gpx>"#,
            )
        );
    }

    #[test]
    fn leg_without_edges_names_max_id() {
        let a = LngLat { lng: 1.0, lat: 2.0 };
        let api = api(vec![leg(&[a], vec![Node::default()])]);
        let body = String::from_utf8(serialize(&api).expect("serializable").body).expect("UTF-8");
        assert!(body.contains("<name>18446744073709551615</name>"), "{body}");
    }

    #[test]
    fn out_of_bounds() {
        assert!(serialize(&Api::default()).is_err());
        let a = LngLat { lng: 1.0, lat: 2.0 };
        assert!(serialize(&api(vec![leg(&[a], vec![edge_node(1, 5)])])).is_err());
        // An empty shape has no last point for the final node.
        assert!(serialize(&api(vec![leg(&[], vec![Node::default()])])).is_err());
    }
}
