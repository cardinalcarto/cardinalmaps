//! What Dart calls to plan public transit on the device (`valinor/docs/flutter-api.md`).
//!
//! Requests and responses are the same JSON as the server's `/transit` paths.
//! The app fetches GTFS-RT itself and hands in whatever bytes it has with each call;
//! none of the routing crates reach the network.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flutter_rust_bridge::frb;
use valinor_gtfs::LoadedTimetable;
use valinor_gtfs_rt::FeedKind;
use valinor_transit_plan::{FeedUpdate, IngestOutcome, JsonResponse, StreetConfig, StreetPlanner};

/// The timetable and streets of the device's offline regions. Opaque to Dart.
#[frb(opaque)]
pub struct OfflineTransit {
    planner: StreetPlanner,
}

/// A response, as the server would have sent it.
pub struct TransitResponse {
    /// The HTTP status the server would have sent (200, or the error's status).
    pub status: u16,
    /// The body: a plan, a vehicle, or `{"error": ..., "kind": ...}`.
    pub body: Vec<u8>,
}

/// What a GTFS-RT message carries.
pub enum TransitFeedKind {
    /// Trip updates.
    TripUpdates,
    /// Vehicle positions.
    VehiclePositions,
    /// Service alerts.
    Alerts,
    /// More than one of the above in a single message.
    Combined,
}

/// The latest GTFS-RT bytes for one feed.
pub struct TransitFeed {
    /// The feed, by its ID in the compiled timetable.
    pub feed: String,
    /// What the message carries.
    pub kind: TransitFeedKind,
    /// The message; `None` clears anything held for this feed.
    pub bytes: Option<Vec<u8>>,
}

/// Why the offline transit data couldn't be opened.
pub struct TransitOpenError {
    /// A description for logs.
    pub message: String,
}

fn feed_kind(kind: &TransitFeedKind) -> FeedKind {
    match kind {
        TransitFeedKind::TripUpdates => FeedKind::TripUpdates,
        TransitFeedKind::VehiclePositions => FeedKind::VehiclePositions,
        TransitFeedKind::Alerts => FeedKind::Alerts,
        TransitFeedKind::Combined => FeedKind::Combined,
    }
}

fn response(answer: JsonResponse) -> TransitResponse {
    TransitResponse {
        status: answer.status,
        body: answer.body,
    }
}

impl OfflineTransit {
    /// Opens a compiled timetable (`valinor-gtfs`) over tile sets, snapping every stop.
    ///
    /// Takes a few seconds on a large metro timetable: every stop is snapped for walking and
    /// for cycling. Run it once and keep the result.
    pub fn open(
        timetable: String,
        tile_sets: Vec<String>,
    ) -> Result<OfflineTransit, TransitOpenError> {
        // SAFETY: the app downloads the timetable to its own file and replaces it by renaming
        // over it, as `LoadedTimetable::map` asks, so it is never modified while mapped.
        let loaded = unsafe { LoadedTimetable::map(Path::new(&timetable)) }.map_err(|error| {
            TransitOpenError {
                message: format!("{timetable}: {error}"),
            }
        })?;
        Ok(Self {
            planner: StreetPlanner::open(loaded, &tile_sets, StreetConfig::default()).map_err(
                |error| TransitOpenError {
                    message: error.to_string(),
                },
            )?,
        })
    }

    /// Plans a trip from JSON request bytes (the server's `/transit/plan` body),
    /// applying `feeds` first.
    ///
    /// Each [`TransitFeed`] replaces what was held for its feed;
    /// feeds not mentioned stay as they are, until they age out.
    /// Undecodable bytes degrade to the schedule and are reported in the response.
    pub fn plan(&self, request: Vec<u8>, feeds: Vec<TransitFeed>) -> TransitResponse {
        self.plan_at(&request, feeds, unix_now())
    }

    /// The latest real-time state of a vehicle, as the server's vehicle lookup returns it.
    pub fn vehicle(&self, feed: String, id: String) -> TransitResponse {
        response(self.planner.vehicle_json(&feed, &id, unix_now()))
    }

    /// [`Self::plan`] at a chosen moment, for tests.
    pub(crate) fn plan_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
    ) -> TransitResponse {
        for feed in feeds {
            let outcome = self.planner.state().ingest(FeedUpdate {
                feed: &feed.feed,
                kind: feed_kind(&feed.kind),
                bytes: feed.bytes.as_deref(),
                received_at: now,
                stale_after: None,
            });
            match outcome {
                // Undecodable bytes are reported by the plan itself, so carry on.
                Ok(IngestOutcome::Accepted | IngestOutcome::Cleared | IngestOutcome::Undecodable(_)) => {}
                Err(error) => {
                    return TransitResponse {
                        status: 404,
                        body: format!(r#"{{"error":{:?},"kind":"unknown_feed"}}"#, error.to_string())
                            .into_bytes(),
                    };
                }
            }
        }
        response(self.planner.plan_json(request, now, &|| false))
    }
}

/// POSIX seconds now.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
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

    fn timetable() -> (tempdir::Dir, String) {
        let compiled = valinor_gtfs::compile(vec![valinor_gtfs::FeedInput {
            id: valinor_gtfs::FeedId::try_new("bus".to_owned()).unwrap(),
            source: Box::new(
                valinor_gtfs::InMemorySource::new()
                    .with(
                        "agency.txt",
                        "agency_name,agency_timezone,agency_url\nBus,Europe/Andorra,https://b.test\n",
                    )
                    .with("routes.txt", "route_id,route_short_name,route_type\nL1,1,3\n")
                    .with(
                        "stops.txt",
                        "stop_id,stop_name,stop_lat,stop_lon\nA,Plaça,42.5078,1.5211\nB,Meritxell,42.5095,1.5345\n",
                    )
                    .with(
                        "calendar.txt",
                        "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
                         WK,1,1,1,1,1,0,0,20260601,20260630\n",
                    )
                    .with("trips.txt", "route_id,service_id,trip_id\nL1,WK,T1\n")
                    .with(
                        "stop_times.txt",
                        "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
                         T1,09:00:00,09:00:00,A,1\nT1,09:08:00,09:08:00,B,2\n",
                    ),
            ),
        }])
        .unwrap();
        let dir = tempdir::Dir::new();
        let path = dir.path().join("timetable.vgtfs");
        std::fs::write(&path, compiled.bytes).unwrap();
        (dir, path.to_string_lossy().into_owned())
    }

    /// A temp directory removed on drop, so the test depends on nothing else.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "routing-flutter-transit-{}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn plans_transit_from_request_bytes() {
        let (_dir, path) = timetable();
        let transit = OfflineTransit::open(path, vec![tiles()]).ok().unwrap();
        // 2026-06-15 08:54 local, a Monday.
        let now = 1_781_506_440;
        let request = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.5100,"lon":1.5350},"max_walk_seconds":600,"departure":1781506500}"#;
        let answer = transit.plan_at(request, Vec::new(), now);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["journeys"][0]["trips"], 1);
        assert_eq!(body["realtime"]["level"], "schedule");

        // Undecodable real-time bytes are accepted and reported, not fatal.
        let feeds = vec![TransitFeed {
            feed: "bus".to_owned(),
            kind: TransitFeedKind::Alerts,
            bytes: Some(vec![0xff, 0xff, 0xff]),
        }];
        let answer = transit.plan_at(request, feeds, now);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["realtime"]["feeds"][0]["state"], "undecodable");

        // A feed the timetable doesn't know is an error the app can see.
        let feeds = vec![TransitFeed {
            feed: "nope".to_owned(),
            kind: TransitFeedKind::Alerts,
            bytes: None,
        }];
        assert_eq!(transit.plan_at(request, feeds, now).status, 404);
    }

    #[test]
    fn reports_why_it_cannot_open() {
        let error = OfflineTransit::open("/nonexistent/timetable".to_owned(), vec![tiles()])
            .err()
            .unwrap();
        assert!(error.message.contains("/nonexistent/timetable"));
    }
}
