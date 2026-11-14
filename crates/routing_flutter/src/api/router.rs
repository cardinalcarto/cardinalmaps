//! What Dart calls (`valinor/docs/flutter-api.md`): open the device's offline regions as one
//! router, route Ferrostar's request bodies with it, and map match traces
//! (`trace_route` and `trace_attributes`, as the server serves them).
//!
//! No function here is `#[frb(sync)]`: flutter_rust_bridge runs them on its worker pool,
//! off the UI isolate's thread, and concurrent calls are fine.

use super::matching::{self, MatchedPosition};
use super::navigation::{NavLocation, NavigationMode};
use core::num::NonZeroUsize;
use flutter_rust_bridge::frb;
use valhalla_graphtile::graph_tile::BuildId;
use valinor_loki::LokiConfig;
use valinor_router::{Router, RouterConfig};
use valinor_thor::{Retention, ThorConfig};

/// How many routes run at once on a device; more calls wait for one to finish.
///
/// Each running route has its own path search storage, which grows as large as its search
/// needs (tens of megabytes for a long route), so a phone keeps few.
const WORKSPACES: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// The search storage each idle workspace keeps between routes, in labels per search tree:
/// none.
///
/// A search reserves room for 65,536 labels (72 bytes each) when it starts, so ordinary routes
/// still don't allocate as they go; what any search used is given back as soon as the route
/// finishes, rather than kept for the next one as mainline's server defaults
/// (a million labels or more) would. Keeping the starting room instead held 9 MB in an idle
/// workspace, and giving it back costs the next route one allocation per tree,
/// which measured within noise of keeping it.
/// An idle workspace still keeps its queues' buckets, a few megabytes after a long route.
/// This only bounds idle memory, not how far a search can go.
const RETAINED_LABELS: usize = 0;

/// The nodes a location's edge must reach, in and out, for the route to start or end on it:
/// the bound transit snaps its stops and plan endpoints by
/// ([`valinor_walk::DEFAULT_MIN_REACHABILITY`]), not Loki's default of 50.
///
/// At 50, Cal Anderson Park's point lands on a park path a bicycle can't enter or leave, though
/// more than 50 nodes connect to it, so neither mainline nor valinor finds a bicycle route to or
/// from the park; at 100 it lands on Nagle Place, as a car's does.
const MIN_REACHABILITY: u32 = valinor_walk::DEFAULT_MIN_REACHABILITY;

/// How many bins' candidate grids a map matching workspace keeps before it drops them all
/// (`meili.grid.cache_size`). A bin is a 25th of a level-2 tile, about 5 km across.
///
/// Mainline's default (100,240) suits a server; on a device it would let the grids of every
/// bin a navigation session passes through pile up. Over Seattle's tiles a bin's grid averages
/// 116 KB (half are under 58 KB; the densest downtown bin is 2.1 MB), so 16 bins hold about
/// 2 MB typically and at most about 34 MB, and cover the few kilometers a trace being matched
/// during navigation spans. Dropping them costs only rebuilding the next trace's bins.
const GRID_CACHE_BINS: usize = 16;

/// The router over the device's offline regions. Opaque to Dart.
#[frb(opaque)]
pub struct OfflineRouter {
    router: Router,
}

/// A response, as the server would have sent it.
pub struct RouterResponse {
    /// The HTTP status the server would have sent (200, or the error's status, such as 400).
    pub status: u16,
    /// The body: OSRM-format JSON for Ferrostar's requests, a route or an error such as
    /// `{"code": "NoSegment", "message": ...}`.
    pub body: Vec<u8>,
}

/// What kind of problem kept the regions from opening.
pub enum OpenErrorKind {
    /// No tile sets were given.
    NoTileSets,
    /// A tile set is missing, unreadable, empty, or not a tile extract.
    Unreadable,
    /// A tile has no build ID (it was built before Valhalla 3.9).
    NoBuildId,
    /// The tile sets come from different builds: every region must come from the same one.
    MixedBuilds,
    /// A tile is from another build than the expected one, or the expected build ID
    /// isn't one (Valhalla's are 16 bits).
    WrongBuild,
}

/// Why the regions couldn't be opened.
pub struct OpenError {
    /// What kind of problem it is.
    pub kind: OpenErrorKind,
    /// A description for logs, naming the file and tile.
    pub message: String,
}

impl From<valinor_router::OpenError> for OpenError {
    fn from(error: valinor_router::OpenError) -> Self {
        use valinor_router::OpenError as E;
        let kind = match &error {
            E::NoTileSets => OpenErrorKind::NoTileSets,
            E::Io { .. } | E::Tiles { .. } | E::Empty { .. } => OpenErrorKind::Unreadable,
            E::NoBuildId { .. } => OpenErrorKind::NoBuildId,
            E::MixedBuilds { .. } => OpenErrorKind::MixedBuilds,
            E::WrongBuild { .. } => OpenErrorKind::WrongBuild,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Why tiles that arrived couldn't be made available.
pub struct RefreshError {
    /// A description for logs, naming the directory.
    pub message: String,
}

/// The Valhalla build ID to open tiles for: the routing release's `valhalla_build_id`.
pub(crate) fn street_build(expected_build_id: Option<u32>) -> Result<Option<BuildId>, OpenError> {
    expected_build_id
        .map(|id| {
            u16::try_from(id).map(BuildId::new).map_err(|_| OpenError {
                kind: OpenErrorKind::WrongBuild,
                message: format!("{id} is not a Valhalla build ID (they are 16 bits)"),
            })
        })
        .transpose()
}

impl OfflineRouter {
    /// Opens tile sets (mainline `build_extract` tarballs or tile directories) as one router.
    ///
    /// Fails if they don't all come from one build (the build ID in each tile's checksum).
    /// Opening maps the tarballs and reads their indexes and a sample of tile headers;
    /// it doesn't read the tiles.
    ///
    /// With `expected_build_id` (the routing release's `valhalla_build_id`), every tile must
    /// be from that build, and a tile directory may have no tiles yet: tiles that arrive later
    /// are used after [`Self::refresh_tiles`].
    /// Without it, every tile set must have tiles.
    ///
    /// Tile files must arrive whole: write each to a temporary file and rename it into place.
    pub fn open(
        tile_sets: Vec<String>,
        expected_build_id: Option<u32>,
    ) -> Result<OfflineRouter, OpenError> {
        Ok(Self {
            router: Router::open_for_build(
                device_config(),
                &tile_sets,
                street_build(expected_build_id)?,
            )?,
        })
    }

    /// Makes tiles that arrived in the tile directories since the router was opened (or last
    /// refreshed) available to routes. Call it once a download's files are in place.
    ///
    /// It reads the tile directories' listings (not the tiles), and is safe while routes run.
    pub fn refresh_tiles(&self) -> Result<(), RefreshError> {
        self.router.refresh_tiles().map_err(|error| RefreshError {
            message: error.to_string(),
        })
    }

    /// Answers a Valhalla `route` request body, as Ferrostar's request generator makes it,
    /// with what the server would have sent. Routing errors are responses, not exceptions.
    pub fn route(&self, body: Vec<u8>) -> RouterResponse {
        self.router.route(&body).into()
    }

    /// Answers a Valhalla `trace_route` request body (a trace to map match into a route,
    /// such as the device's recent positions; `"format": "osrm"` for Ferrostar's parser)
    /// with what the server's `/trace_route` would have sent.
    /// Matching errors are responses, not exceptions.
    pub fn trace_route(&self, body: Vec<u8>) -> RouterResponse {
        self.router.trace_route(&body).into()
    }

    /// Answers a Valhalla `trace_attributes` request body (a trace to map match, answered with
    /// the matched edges, their attributes, and where each point matched)
    /// with what the server's `/trace_attributes` would have sent.
    /// Matching errors are responses, not exceptions.
    pub fn trace_attributes(&self, body: Vec<u8>) -> RouterResponse {
        self.router.trace_attributes(&body).into()
    }

    /// Where on the roads the last of `fixes` (the recent fixes, oldest first) was, matched
    /// together with those before it, for a trip traveling by `mode` (by wheelchair if `mode`
    /// is walking and `wheelchair` is set); none if it couldn't be matched, or the match
    /// failed.
    ///
    /// It builds the `trace_attributes` request and reads the answer
    /// ([`matching::match_body`], [`matching::parse_match`]), so a fix takes one call.
    pub fn match_position(
        &self,
        mode: NavigationMode,
        wheelchair: bool,
        fixes: Vec<NavLocation>,
    ) -> Option<MatchedPosition> {
        if fixes.is_empty() {
            return None;
        }
        let response = self
            .router
            .trace_attributes(&matching::match_body(mode, wheelchair, &fixes));
        matching::parse_match(response.status.as_u16(), &response.body)
    }
}

impl From<valinor_router::HttpResponse> for RouterResponse {
    fn from(response: valinor_router::HttpResponse) -> Self {
        Self {
            status: response.status.as_u16(),
            body: response.body,
        }
    }
}

/// The router's configuration on a device.
fn device_config() -> RouterConfig {
    let mut loki = LokiConfig::default();
    loki.service_defaults.minimum_reachability = MIN_REACHABILITY;
    // A request that names its own reachability is capped by this; keep it from being
    // capped below the default.
    loki.service_limits.max_reachability =
        loki.service_limits.max_reachability.max(MIN_REACHABILITY);
    let mut thor = ThorConfig {
        retention: Retention {
            bidirectional_labels: RETAINED_LABELS,
            unidirectional_labels: RETAINED_LABELS,
        },
        ..ThorConfig::default()
    };
    thor.meili.candidate_search.cache_size = GRID_CACHE_BINS;
    RouterConfig {
        loki,
        workspaces: WORKSPACES,
        thor,
        ..RouterConfig::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn locations_need_transits_reachability() {
        let loki = device_config().loki;
        assert_eq!(loki.service_defaults.minimum_reachability, 250);
        assert!(loki.service_limits.max_reachability >= 250);
    }

    /// Street tiles covering Seattle (a phone bundle's `tiles.tar`), from `CARDINAL_SEATTLE_TILES`;
    /// real data, so not in the repository.
    #[test]
    #[ignore = "needs CARDINAL_SEATTLE_TILES"]
    fn bicycles_route_to_and_from_cal_anderson_park() {
        let tiles = std::env::var("CARDINAL_SEATTLE_TILES").expect("CARDINAL_SEATTLE_TILES");
        let router = OfflineRouter::open(vec![tiles], None).ok().unwrap();
        // The geocoder's point for the park, and Westlake.
        let park = r#"{"lat": 47.6170378, "lon": -122.3191692}"#;
        let westlake = r#"{"lat": 47.6114, "lon": -122.3375}"#;
        for (from, to) in [(westlake, park), (park, westlake)] {
            let body = format!(
                r#"{{"format": "osrm", "costing": "bicycle", "locations": [{from}, {to}]}}"#
            );
            let response = router.route(body.into_bytes());
            let body = String::from_utf8(response.body).unwrap();
            assert_eq!(response.status, 200, "{body}");
            assert!(body.contains(r#""code":"Ok""#), "{body}");
        }
    }

    fn tiles() -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn routes_ferrostar_bodies() {
        let router = OfflineRouter::open(vec![tiles()], None).ok().unwrap();
        let body = br#"{"format": "osrm", "banner_instructions": true, "voice_instructions": true, "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211, "street_side_tolerance": 5, "heading": 90}, {"lat": 42.5355, "lon": 1.583, "type": "break"}]}"#;
        let response = router.route(body.to_vec());
        assert_eq!(response.status, 200);
        assert!(
            String::from_utf8(response.body)
                .unwrap()
                .contains(r#""code":"Ok""#)
        );

        // Errors are OSRM-format bodies.
        let response = router.route(br#"{"format": "osrm", "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211}]}"#.to_vec());
        assert_eq!(response.status, 400);
        assert!(
            String::from_utf8(response.body)
                .unwrap()
                .contains(r#""code":"#)
        );
    }

    /// A trace from mainline's captures (`valinor-tyr/fixtures/traces`), as a request body.
    fn trace(case: &str) -> serde_json::Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valinor-tyr/fixtures/traces/andorra")
            .join(case)
            .join("request.json");
        let recorded: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        recorded["request"].clone()
    }

    #[test]
    fn caps_the_map_matching_grid_cache() {
        let thor = device_config().thor;
        assert_eq!(thor.meili.candidate_search.cache_size, GRID_CACHE_BINS);
    }

    #[test]
    fn map_matches_traces() {
        let router = OfflineRouter::open(vec![tiles()], None).ok().unwrap();
        let mut body = trace("andorra-0000-clean");
        body["format"] = "osrm".into();
        let response = router.trace_route(serde_json::to_vec(&body).unwrap());
        let text = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 200, "{text}");
        let osrm: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(osrm["code"], "Ok");
        assert!(!osrm["matchings"].as_array().unwrap().is_empty());

        let response =
            router.trace_attributes(serde_json::to_vec(&trace("andorra-0000-clean")).unwrap());
        let text = String::from_utf8(response.body).unwrap();
        assert_eq!(response.status, 200, "{text}");
        let attributes: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!attributes["edges"].as_array().unwrap().is_empty());

        // Errors are OSRM-format bodies, as for routes: a trace far from any road.
        let response = router.trace_route(
            br#"{"format": "osrm", "costing": "auto", "shape_match": "map_snap", "shape": [{"lat": 10.0, "lon": 10.0}, {"lat": 10.001, "lon": 10.001}]}"#.to_vec(),
        );
        assert_eq!(response.status, 400);
        let error: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert!(error["code"].is_string(), "{error}");
    }

    #[test]
    fn reports_why_it_cannot_open() {
        let error = OfflineRouter::open(vec![], None).err().unwrap();
        assert!(matches!(error.kind, OpenErrorKind::NoTileSets));
        let error = OfflineRouter::open(vec!["/nonexistent/region.tar".to_owned()], None)
            .err()
            .unwrap();
        assert!(matches!(error.kind, OpenErrorKind::Unreadable));
        assert!(error.message.contains("/nonexistent/region.tar"));
    }

    /// An empty tile directory opened for the fixture's build, then filled as a download does.
    #[test]
    fn routes_over_tiles_that_arrive_after_a_refresh() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valhalla-graphtile/fixtures/andorra-tiles");
        let dir =
            std::env::temp_dir().join(format!("routing-flutter-arrive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let tiles = vec![dir.to_string_lossy().into_owned()];
        // The fixture's build ID, as mainline 3.9.1 stamped it.
        let build = 53174;

        let error = OfflineRouter::open(tiles.clone(), None).err().unwrap();
        assert!(matches!(error.kind, OpenErrorKind::Unreadable));
        let error = OfflineRouter::open(tiles.clone(), Some(70_000))
            .err()
            .unwrap();
        assert!(matches!(error.kind, OpenErrorKind::WrongBuild));
        let router = OfflineRouter::open(tiles.clone(), Some(build))
            .ok()
            .unwrap();
        let body = br#"{"format": "osrm", "costing": "auto", "locations": [{"lat": 42.5078, "lon": 1.5211}, {"lat": 42.5355, "lon": 1.583}]}"#;
        assert_eq!(router.route(body.to_vec()).status, 400);

        for tile in [
            "0/003/015.gph",
            "1/047/701.gph",
            "2/000/762/485.gph",
            "2/000/762/486.gph",
            "2/000/763/925.gph",
            "2/000/763/926.gph",
            "2/000/763/927.gph",
        ] {
            let to = dir.join(tile);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            let part = to.with_extension("part");
            std::fs::copy(fixture.join(tile), &part).unwrap();
            std::fs::rename(&part, &to).unwrap();
        }
        router.refresh_tiles().ok().unwrap();
        let response = router.route(body.to_vec());
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );

        // Tiles there must be from the expected build.
        let error = OfflineRouter::open(tiles, Some(build + 1)).err().unwrap();
        assert!(matches!(error.kind, OpenErrorKind::WrongBuild));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
