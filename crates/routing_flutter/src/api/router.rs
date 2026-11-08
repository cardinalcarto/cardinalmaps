//! What Dart calls (`valinor/docs/flutter-api.md`): open the device's offline regions as one
//! router, and route Ferrostar's request bodies with it.
//!
//! Neither function is `#[frb(sync)]`: flutter_rust_bridge runs them on its worker pool,
//! off the UI isolate's thread, and concurrent calls are fine.

use core::num::NonZeroUsize;
use flutter_rust_bridge::frb;
use valinor_router::{Router, RouterConfig};
use valinor_thor::{Retention, ThorConfig};

/// How many routes run at once on a device; more calls wait for one to finish.
///
/// Each running route has its own path search storage, which grows as large as its search
/// needs (tens of megabytes for a long route), so a phone keeps few.
const WORKSPACES: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// The search storage each idle workspace keeps between routes, in labels per search tree.
///
/// A search starts with room for this many labels anyway, so ordinary routes don't allocate
/// as they go. After a longer search, the rest is given back as soon as the route finishes,
/// rather than kept for the next one as mainline's server defaults (a million labels or more)
/// would. A label is 72 bytes, so an idle workspace holds at most about 5 MB per search tree
/// (20 MB across its two bidirectional trees and two time-dependent searches).
/// This only bounds idle memory, not how far a search can go.
const RETAINED_LABELS: usize = 1 << 16;

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
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

impl OfflineRouter {
    /// Opens tile sets (mainline `build_extract` tarballs or tile directories) as one router.
    ///
    /// Fails if they don't all come from one build (the build ID in each tile's checksum).
    /// Opening maps the tarballs and reads their indexes and a sample of tile headers;
    /// it doesn't read the tiles.
    pub fn open(tile_sets: Vec<String>) -> Result<OfflineRouter, OpenError> {
        let config = RouterConfig {
            workspaces: WORKSPACES,
            thor: ThorConfig {
                retention: Retention {
                    bidirectional_labels: RETAINED_LABELS,
                    unidirectional_labels: RETAINED_LABELS,
                },
                ..ThorConfig::default()
            },
            ..RouterConfig::default()
        };
        Ok(Self {
            router: Router::open(config, &tile_sets)?,
        })
    }

    /// Answers a Valhalla `route` request body, as Ferrostar's request generator makes it,
    /// with what the server would have sent. Routing errors are responses, not exceptions.
    pub fn route(&self, body: Vec<u8>) -> RouterResponse {
        let response = self.router.route(&body);
        RouterResponse {
            status: response.status.as_u16(),
            body: response.body,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tiles() -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../valinor/valhalla-graphtile/fixtures/andorra-tiles.tar")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn routes_ferrostar_bodies() {
        let router = OfflineRouter::open(vec![tiles()]).ok().unwrap();
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

    #[test]
    fn reports_why_it_cannot_open() {
        let error = OfflineRouter::open(vec![]).err().unwrap();
        assert!(matches!(error.kind, OpenErrorKind::NoTileSets));
        let error = OfflineRouter::open(vec!["/nonexistent/region.tar".to_owned()])
            .err()
            .unwrap();
        assert!(matches!(error.kind, OpenErrorKind::Unreadable));
        assert!(error.message.contains("/nonexistent/region.tar"));
    }
}
