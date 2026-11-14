//! The edge walk's lenient shape checks against ground truth: mainline's own routes
//! (the trips of `valinor-tyr`'s Andorra route captures), whose shapes must walk back to the
//! routes' edges, through the front and Loki as a request goes.
//!
//! Each shape is walked as mainline walks it and as Valinor does (mainline's walk, then the
//! lenient one where it fails; see "Known divergences" in `docs/routing.md`).

use core::num::NonZeroUsize;
use std::path::PathBuf;

use http::Method;
use serde_json::{Value, json};
use valhalla_graphtile::tile_provider::DirectoryGraphTileProvider;
use valhalla_proto::Api;
use valhalla_proto::prost::Message;
use valinor_loki::{HttpOutcome, Loki, LokiConfig, handle_request};
use valinor_meili::Workspace as MeiliWorkspace;
use valinor_tyr::front::HttpRequest;

use crate::trace::LENIENT_EDGE_WALK;
use crate::{Thor, ThorConfig};

/// The oracle builds' config (`valhalla_build_config`'s defaults).
const ORACLE_CONFIG: &str = include_str!("../../valinor-loki/fixtures/valhalla.json");

fn tiles() -> DirectoryGraphTileProvider {
    DirectoryGraphTileProvider::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valhalla-graphtile/fixtures/andorra-tiles"),
        NonZeroUsize::new(64).expect("64"),
    )
}

/// Decodes a polyline at six digits (Valhalla's trip shapes): longitude and latitude.
fn decode_polyline6(encoded: &str) -> Vec<(f64, f64)> {
    let bytes = encoded.as_bytes();
    let mut i = 0;
    let mut next = || -> Option<i64> {
        let (mut result, mut shift) = (0_i64, 0);
        loop {
            let b = i64::from(*bytes.get(i)?) - 63;
            i += 1;
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
    let (mut lat, mut lng) = (0_i64, 0_i64);
    let mut points = Vec::new();
    while let (Some(dlat), Some(dlng)) = (next(), next()) {
        lat += dlat;
        lng += dlng;
        #[expect(clippy::cast_precision_loss, reason = "six-digit coordinates")]
        points.push((lng as f64 / 1e6, lat as f64 / 1e6));
    }
    points
}

/// Front, Loki, and Thor over the fixture tiles.
struct Walker {
    loki: Loki<DirectoryGraphTileProvider>,
    thor: Thor<DirectoryGraphTileProvider>,
}

impl Walker {
    fn new() -> Self {
        Self {
            loki: Loki::new(
                LokiConfig::from_valhalla_json(ORACLE_CONFIG).expect("a config"),
                tiles(),
                None,
            ),
            thor: Thor::new(
                ThorConfig::from_valhalla_json(ORACLE_CONFIG).expect("a config"),
                tiles(),
            ),
        }
    }

    /// A captured route: its request, and its first leg's shape and edges (an edge the leg
    /// goes along past an intermediate location only once).
    fn route(case: &str) -> (Value, Vec<(f64, f64)>, Vec<u64>) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../valinor-tyr/fixtures/captures/andorra")
            .join(case);
        let trip = Api::decode(
            std::fs::read(dir.join("trip.bin"))
                .expect("a trip")
                .as_slice(),
        )
        .expect("an Api");
        let recorded: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join("json/request.json")).expect("a request"),
        )
        .expect("JSON");
        let leg = &trip.trip.as_ref().expect("a trip").routes[0].legs[0];
        let mut edges: Vec<u64> = leg
            .node
            .iter()
            .filter_map(|n| n.edge.as_ref().map(|e| e.id))
            .collect();
        edges.dedup();
        (
            recorded["request"].clone(),
            decode_polyline6(&leg.shape),
            edges,
        )
    }

    /// The edges of a `trace_attributes` edge walk of a shape, or the error code;
    /// with `lenient`, Valinor's walk, and without, mainline's.
    fn walk(&self, request: &Value, shape: &[(f64, f64)], lenient: bool) -> Result<Vec<u64>, u16> {
        let mut body = json!({
            "costing": request["costing"],
            "shape_match": "edge_walk",
            "shape": shape.iter().map(|(lon, lat)| json!({"lat": lat, "lon": lon})).collect::<Vec<_>>(),
            "filters": {"attributes": ["edge.id"], "action": "include"},
        });
        if let Some(options) = request.get("costing_options") {
            body["costing_options"] = options.clone();
        }
        let body = body.to_string();
        let request = HttpRequest {
            method: &Method::POST,
            target: b"/trace_attributes",
            content_type: Some(b"application/json"),
            body: body.as_bytes(),
        };
        let HttpOutcome::Forward(mut api) = handle_request(&self.loki, &request) else {
            return Err(0);
        };
        LENIENT_EDGE_WALK.with(|l| l.set(lenient));
        let result = self
            .thor
            .trace_attributes(&mut MeiliWorkspace::new(), &mut api);
        LENIENT_EDGE_WALK.with(|l| l.set(true));
        let response = result.map_err(|e| e.code().code())?;
        let value: Value = serde_json::from_slice(&response.body).expect("JSON");
        Ok(value["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .filter_map(|e| e["id"].as_u64())
            .collect())
    }
}

/// Shapes mainline's walk follows keep mainline's walk; ones it fails on, with a node matched
/// one shape point early (`auto_french`, `auto_roundabout_third_exit`) or a via location
/// partway along an edge (`ignore_closures_waiting`), walk to the routes' edges.
#[test]
fn walks_mainline_routes() {
    let walker = Walker::new();
    for (case, mainline_walks) in [
        ("auto_town", true),
        ("pedestrian_town", true),
        ("bicycle_road", true),
        ("auto_french", false),
        ("auto_roundabout_third_exit", false),
        ("ignore_closures_waiting", false),
    ] {
        let (request, shape, edges) = Walker::route(case);
        let mainline = walker.walk(&request, &shape, false);
        if mainline_walks {
            assert_eq!(mainline.as_ref(), Ok(&edges), "{case}");
        } else {
            assert_eq!(mainline, Err(443), "{case}");
        }
        assert_eq!(walker.walk(&request, &shape, true), Ok(edges), "{case}");
    }
}

/// A route's shape as legs joined at a location partway along an edge (the second leg starting
/// with the point the first ended at) walks to the route's edges; mainline's walk fails.
#[test]
fn walks_joined_legs() {
    let walker = Walker::new();
    let (request, shape, edges) = Walker::route("auto_town");
    // The legs meet in the middle of the route's longest segment.
    let length = |i: usize| (shape[i].0 - shape[i - 1].0).hypot(shape[i].1 - shape[i - 1].1);
    let k = (1..shape.len())
        .max_by(|&a, &b| length(a).total_cmp(&length(b)))
        .expect("a segment");
    let round = |x: f64| (x * 1e6).round() / 1e6;
    let mid = (
        round(f64::midpoint(shape[k - 1].0, shape[k].0)),
        round(f64::midpoint(shape[k - 1].1, shape[k].1)),
    );
    let mut joined = shape[..k].to_vec();
    joined.extend([mid, mid]);
    joined.extend_from_slice(&shape[k..]);
    assert_eq!(walker.walk(&request, &joined, false), Err(443));
    assert_eq!(walker.walk(&request, &joined, true), Ok(edges));
}

/// A shape that isn't on the graph still fails: a point 11 meters off the road.
#[test]
fn still_fails_off_the_graph() {
    let walker = Walker::new();
    let (request, mut shape, _) = Walker::route("auto_town");
    let k = shape.len() / 2;
    shape[k].1 += 0.0001;
    assert_eq!(walker.walk(&request, &shape, false), Err(443));
    assert_eq!(walker.walk(&request, &shape, true), Err(443));
}
