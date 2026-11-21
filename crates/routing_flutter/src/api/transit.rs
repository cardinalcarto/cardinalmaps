//! What Dart calls to plan public transit on the device (`valinor/docs/flutter-api.md`).
//!
//! Requests and responses are the same JSON as the server's `/transit` paths.
//! The app fetches GTFS-RT itself and hands in whatever bytes it has with each call;
//! none of the routing crates reach the network.

use core::time::Duration;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use flutter_rust_bridge::frb;
use serde_json::json;
use valinor_gtfs_rt::FeedKind;
use valinor_thor::Retention;
use valinor_transit_plan::{
    FeedUpdate, JsonResponse, ServiceLimits, StreetConfig, StreetPlanner, TransitBuild,
};

/// The longest access or egress leg a request may ask for on a device, in seconds.
///
/// Above the 30-minute default, but well below the server's limit:
/// what a phone keeps grows with the area a query covers.
const MAX_WALK_SECONDS: u32 = 2700;

/// How much search storage one idle worker may keep between plans, in bytes.
///
/// One plan's searches grow to 20 to 50 MB on a metro area (Seattle's seven feeds), and a
/// worker keeps each travel mode's separately, so after nearly every plan a worker is over this
/// and gives back its searches' storage. What it keeps then, its bound stops and the graph's
/// numbering, is a few megabytes. The next plan allocates its storage again, which cost a short
/// plan 2 to 10 ms on a desktop; keeping it would take this limit to about 130 MB.
const MAX_RETAINED_WORKER_BYTES: usize = 16 * 1024 * 1024;

/// How many workers stay idle between plans; more only while plans run at once.
const MAX_IDLE_WORKERS: usize = 2;

/// Labels a Thor workspace keeps between plans, per search tree: none, as for street routes
/// (see `router.rs`). Thor draws a plan's walks; a search reserves its starting room anyway.
const RETAINED_LABELS: usize = 0;

/// How long a plan may run on a device before it is abandoned.
const PLAN_DEADLINE: Duration = Duration::from_secs(30);

fn device_config() -> StreetConfig {
    StreetConfig {
        limits: ServiceLimits {
            max_walk_seconds: MAX_WALK_SECONDS,
            max_retained_worker_bytes: MAX_RETAINED_WORKER_BYTES,
            max_idle_workers: MAX_IDLE_WORKERS,
        },
        retention: Retention {
            bidirectional_labels: RETAINED_LABELS,
            unidirectional_labels: RETAINED_LABELS,
        },
        ..StreetConfig::default()
    }
}

/// A way to stop a plan from Dart, by a screen that no longer wants its answer. Opaque to Dart.
#[frb(opaque)]
pub struct TransitCancel {
    cancelled: Arc<AtomicBool>,
}

impl TransitCancel {
    /// A token that hasn't been cancelled.
    #[frb(sync)]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Asks every plan given this token to stop; each then answers 499.
    #[frb(sync)]
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Default for TransitCancel {
    fn default() -> Self {
        Self::new()
    }
}

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
    /// Feeds passed with this plan that aren't loaded, and so were ignored.
    pub ignored_feeds: Vec<String>,
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
        ignored_feeds: Vec::new(),
    }
}

impl OfflineTransit {
    /// Opens a transit build directory over tile sets, snapping every stop.
    ///
    /// `feeds` chooses which of the build's feeds to load, by ID; `None` loads every feed that
    /// compiled. Whichever transfer tiles of those feeds are on disk are used; a stop whose tile
    /// isn't simply has no transfers, so an offline area needn't hold an agency's whole network.
    ///
    /// Takes a few seconds on a large metro: every stop is snapped for walking and for cycling.
    /// Run it once and keep the result.
    ///
    /// The build's files are memory-mapped: to replace one, write a new file and rename it over
    /// the old one; truncating or rewriting it in place while it is open can fault.
    ///
    /// `expected_build_id` is as for `OfflineRouter.open` (the routing release's
    /// `valhalla_build_id`): with it, a street tile directory may have no tiles yet,
    /// and tiles that arrive later are used after [`Self::refresh_tiles`].
    pub fn open(
        build: String,
        feeds: Option<Vec<String>>,
        tile_sets: Vec<String>,
        expected_build_id: Option<u32>,
    ) -> Result<OfflineTransit, TransitOpenError> {
        let street_build =
            crate::api::router::street_build(expected_build_id).map_err(|error| {
                TransitOpenError {
                    message: error.message,
                }
            })?;
        let loaded = TransitBuild::load(Path::new(&build), feeds.as_deref()).map_err(|error| {
            TransitOpenError {
                message: format!("{build}: {error}"),
            }
        })?;
        Ok(Self {
            planner: StreetPlanner::from_build_for(
                loaded,
                &tile_sets,
                device_config(),
                street_build,
            )
            .map_err(|error| TransitOpenError {
                message: error.to_string(),
            })?,
        })
    }

    /// Makes street tiles that arrived in the tile directories since this was opened (or last
    /// refreshed) available to plans. Call it once a download's files are in place.
    ///
    /// It is cheap and safe while plans run; each search worker forgets the tiles it found
    /// missing the next time it plans.
    pub fn refresh_tiles(&self) {
        self.planner.refresh_tiles();
    }

    /// Opens one compiled timetable, with its walks built in memory, for tests.
    #[cfg(test)]
    pub(crate) fn open_timetable(
        timetable: valinor_gtfs::LoadedTimetable,
        tile_sets: Vec<String>,
    ) -> Result<OfflineTransit, TransitOpenError> {
        Ok(Self {
            planner: StreetPlanner::open_with_built_transfers(
                valinor_gtfs::TimetableSet::single(timetable).map_err(|error| {
                    TransitOpenError {
                        message: error.to_string(),
                    }
                })?,
                &tile_sets,
                device_config(),
            )
            .map_err(|error| TransitOpenError {
                message: error.to_string(),
            })?,
        })
    }

    /// Plans a trip from JSON request bytes (the server's `/transit/plan` body),
    /// applying `feeds` first.
    ///
    /// Each [`TransitFeed`] replaces what was held for its feed;
    /// feeds not mentioned stay as they are, until they age out.
    /// Undecodable bytes degrade to the schedule and are reported in the response.
    ///
    /// A plan answers 504 past a 30-second deadline, or 499 once `cancel` is cancelled.
    pub fn plan(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        self.answer(Query::Plan, &request, feeds, cancel)
    }

    /// The routes a rider can catch near a place, in each direction, and when, from JSON
    /// request bytes (the server's `/transit/nearby` body), applying `feeds` first as
    /// [`Self::plan`] does.
    ///
    /// It answers 504 past a 30-second deadline, or 499 once `cancel` is cancelled.
    pub fn nearby(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        self.answer(Query::Nearby, &request, feeds, cancel)
    }

    /// One route and direction from one stop, for a detail view, from JSON request bytes
    /// (the server's `/transit/route` body), applying `feeds` first as [`Self::plan`] does.
    ///
    /// It answers 504 past a 30-second deadline, or 499 once `cancel` is cancelled.
    pub fn route(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        self.answer(Query::Route, &request, feeds, cancel)
    }

    /// One of a plan ride's departures, as the plan's leg on it would be, from JSON request
    /// bytes (the server's `/transit/ride` body), applying `feeds` first as [`Self::plan`] does.
    ///
    /// It answers 504 past a 30-second deadline, or 499 once `cancel` is cancelled.
    pub fn ride(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        self.answer(Query::Ride, &request, feeds, cancel)
    }

    /// Every route's departures between two stops around a moment, as a plan's ride lists
    /// them, from JSON request bytes (the server's `/transit/departures` body), applying
    /// `feeds` first as [`Self::plan`] does.
    ///
    /// It answers 504 past a 30-second deadline, or 499 once `cancel` is cancelled.
    pub fn departures(
        &self,
        request: Vec<u8>,
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        self.answer(Query::Departures, &request, feeds, cancel)
    }

    /// The latest real-time state of a vehicle, as the server's vehicle lookup returns it.
    pub fn vehicle(&self, feed: String, id: String) -> TransitResponse {
        response(self.planner.vehicle_json(&feed, &id, unix_now()))
    }

    /// Answers a query now, under `cancel` and the device's deadline.
    fn answer(
        &self,
        query: Query,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        cancel: &TransitCancel,
    ) -> TransitResponse {
        let cancelled = Arc::clone(&cancel.cancelled);
        let deadline = Instant::now() + PLAN_DEADLINE;
        let timed_out = move || Instant::now() >= deadline;
        let answer = self.answer_at(query, request, feeds, unix_now(), &|| {
            cancelled.load(Ordering::Relaxed) || timed_out()
        });
        if answer.status == 499 && !cancel.cancelled.load(Ordering::Relaxed) {
            return error_response(504, "timed_out", "the query ran past its deadline");
        }
        answer
    }

    /// [`Self::plan`] at a chosen moment, with a chosen interrupt,
    /// for tests and the desktop memory harness (`examples/device_memory.rs`),
    /// which replays fixed requests and needs the same answer each time.
    /// Not part of the Dart API.
    #[frb(ignore)]
    pub fn plan_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        self.answer_at(Query::Plan, request, feeds, now, interrupt)
    }

    /// [`Self::nearby`] at a chosen moment, with a chosen interrupt, for tests.
    /// Not part of the Dart API.
    #[frb(ignore)]
    pub fn nearby_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        self.answer_at(Query::Nearby, request, feeds, now, interrupt)
    }

    /// [`Self::route`] at a chosen moment, with a chosen interrupt, for tests.
    /// Not part of the Dart API.
    #[frb(ignore)]
    pub fn route_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        self.answer_at(Query::Route, request, feeds, now, interrupt)
    }

    /// [`Self::ride`] at a chosen moment, with a chosen interrupt, for tests.
    /// Not part of the Dart API.
    #[frb(ignore)]
    pub fn ride_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        self.answer_at(Query::Ride, request, feeds, now, interrupt)
    }

    /// [`Self::departures`] at a chosen moment, with a chosen interrupt, for tests.
    /// Not part of the Dart API.
    #[frb(ignore)]
    pub fn departures_at(
        &self,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        self.answer_at(Query::Departures, request, feeds, now, interrupt)
    }

    /// Applies `feeds`, then answers a query at `now`.
    fn answer_at(
        &self,
        query: Query,
        request: &[u8],
        feeds: Vec<TransitFeed>,
        now: i64,
        interrupt: &dyn Fn() -> bool,
    ) -> TransitResponse {
        // A feed that isn't loaded is about nothing this query can use, and only affects itself.
        let state = self.planner.state();
        let mut ignored_feeds = Vec::new();
        for feed in &feeds {
            if !state
                .feeds()
                .iter()
                .any(|known| known.as_str() == feed.feed)
            {
                ignored_feeds.push(feed.feed.clone());
                continue;
            }
            // Undecodable bytes are reported by the response itself.
            let _ = state.ingest(FeedUpdate {
                feed: &feed.feed,
                kind: feed_kind(&feed.kind),
                bytes: feed.bytes.as_deref(),
                received_at: now,
                stale_after: None,
            });
        }
        let answer = match query {
            Query::Plan => self.planner.plan_json(request, now, interrupt),
            Query::Nearby => self.planner.nearby_json(request, now, interrupt),
            Query::Route => self.planner.route_json(request, now, interrupt),
            Query::Ride => self.planner.ride_json(request, now, interrupt),
            Query::Departures => self.planner.departures_json(request, now, interrupt),
        };
        TransitResponse {
            ignored_feeds,
            ..response(answer)
        }
    }
}

/// Which of the server's transit paths a call answers as.
enum Query {
    /// `POST /transit/plan`.
    Plan,
    /// `POST /transit/nearby`.
    Nearby,
    /// `POST /transit/route`.
    Route,
    /// `POST /transit/ride`.
    Ride,
    /// `POST /transit/departures`.
    Departures,
}

/// A JSON error body, as the server's would be.
fn error_response(status: u16, kind: &str, message: &str) -> TransitResponse {
    TransitResponse {
        status,
        body: serde_json::to_vec(&json!({ "error": message, "kind": kind })).unwrap_or_default(),
        ignored_feeds: Vec::new(),
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

    fn timetable() -> (tempdir::Dir, valinor_gtfs::LoadedTimetable) {
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
                        "stop_id,stop_name,stop_lat,stop_lon\nA,Plaça,42.5078,1.5211\nB,Meritxell,42.5095,1.5345\nC,Escaldes,42.5101,1.5402\n",
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
                         T1,09:00:00,09:00:00,A,1\nT1,09:08:00,09:08:00,B,2\nT1,09:11:00,09:11:00,C,3\n",
                    ),
            ),
        }])
        .unwrap();
        let dir = tempdir::Dir::new();
        let path = dir.path().join("timetable.vgtfs");
        std::fs::write(&path, &compiled.bytes).unwrap();
        (
            dir,
            valinor_gtfs::LoadedTimetable::from_bytes(compiled.bytes).unwrap(),
        )
    }

    /// A temp directory removed on drop, so the test depends on nothing else.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Self {
                static NEXT: AtomicU64 = AtomicU64::new(0);
                let path = std::env::temp_dir().join(format!(
                    "routing-flutter-transit-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
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
        let transit = OfflineTransit::open_timetable(path, vec![tiles()])
            .ok()
            .unwrap();
        // 2026-06-15 08:54 local, a Monday.
        let now = 1_781_506_440;
        let request = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.5100,"lon":1.5350},"max_walk_seconds":600,"departure":1781506500}"#;
        let answer = transit.plan_at(request, Vec::new(), now, &|| false);
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
        let answer = transit.plan_at(request, feeds, now, &|| false);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["realtime"]["feeds"][0]["state"], "undecodable");

        // A feed that isn't loaded is ignored and reported; the plan and other feeds go on.
        let feeds = vec![
            TransitFeed {
                feed: "bus".to_owned(),
                kind: TransitFeedKind::Alerts,
                bytes: None,
            },
            TransitFeed {
                feed: "nope\u{1}".to_owned(),
                kind: TransitFeedKind::Alerts,
                bytes: None,
            },
        ];
        let answer = transit.plan_at(request, feeds, now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(answer.ignored_feeds, vec!["nope\u{1}".to_owned()]);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        // The known feed was still applied: its undecodable bytes are gone.
        assert_eq!(body["realtime"]["level"], "schedule");
    }

    #[test]
    fn a_wheelchair_request_is_answered_and_reports_accessibility() {
        let (_dir, path) = timetable();
        let transit = OfflineTransit::open_timetable(path, vec![tiles()])
            .ok()
            .unwrap();
        let now = 1_781_506_440;
        let request = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.5100,"lon":1.5350},"max_walk_seconds":600,"departure":1781506500,"mode":"wheelchair"}"#;
        let answer = transit.plan_at(request, Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        let journey = &body["journeys"][0];
        assert_eq!(journey["legs"][0]["accessibility"], "accessible");
        assert_eq!(journey["legs"][1]["accessibility"], "unknown");
        assert!(journey["legs"][1]["stops"][0]["accessibility"].is_string());
        // A ride's departures reach the app in the same JSON: here only its own runs, the day's last.
        let departures = &journey["legs"][1]["departures"];
        assert_eq!(departures.as_array().map(Vec::len), Some(1));
        assert_eq!(departures[0]["suggested"], true);
        assert_eq!(departures[0]["last_of_day"], true);
        assert_eq!(departures[0]["departure"], journey["legs"][1]["departure"]);
    }

    #[test]
    fn a_request_above_the_device_limit_is_refused() {
        let (_dir, path) = timetable();
        let transit = OfflineTransit::open_timetable(path, vec![tiles()])
            .ok()
            .unwrap();
        let request = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.51,"lon":1.535},"max_walk_seconds":2701}"#;
        assert_eq!(
            transit
                .plan_at(request, Vec::new(), 1_781_506_440, &|| false)
                .status,
            400
        );
    }

    #[test]
    fn a_cancelled_plan_stops_and_says_so() {
        let (_dir, path) = timetable();
        let transit = OfflineTransit::open_timetable(path, vec![tiles()])
            .ok()
            .unwrap();
        let request = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.51,"lon":1.535},"max_walk_seconds":600,"departure":1781506500}"#;
        let answer = transit.plan_at(request, Vec::new(), 1_781_506_440, &|| true);
        assert_eq!(answer.status, 499);
    }

    /// The device answers nearby and route-detail requests with the server's bytes: the server
    /// calls the same planner methods, over a planner with the server's own limits.
    #[test]
    fn nearby_and_route_answer_with_the_servers_bytes() {
        let (_dir, on_device) = timetable();
        let (_server_dir, on_server) = timetable();
        let transit = OfflineTransit::open_timetable(on_device, vec![tiles()])
            .ok()
            .unwrap();
        let server = valinor_transit_plan::StreetPlanner::open_with_built_transfers(
            valinor_gtfs::TimetableSet::single(on_server).unwrap(),
            &[tiles()],
            valinor_transit_plan::StreetConfig::default(),
        )
        .unwrap();
        // 2026-06-15 08:54 local, a Monday.
        let now = 1_781_506_440;
        let nearby = br#"{"location":{"lat":42.5078,"lon":1.5205},"max_walk_seconds":600}"#;
        let answer = transit.nearby_at(nearby, Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(
            answer.body,
            server.nearby_json(nearby, now, &|| false).body,
            "the same bytes as the server's /transit/nearby"
        );
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        let page = &body["routes"][0]["pages"][0];
        assert_eq!(page["departures"][0]["trip_id"], "T1");

        let route = format!(
            r#"{{"feed":"bus","route_id":"L1","headsigns":{},"stop_id":"A","walk_seconds":{}}}"#,
            page.get("headsigns")
                .unwrap_or(&serde_json::Value::Array(Vec::new())),
            page["walk_seconds"]
        );
        let answer = transit.route_at(route.as_bytes(), Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(
            answer.body,
            server.route_json(route.as_bytes(), now, &|| false).body,
            "the same bytes as the server's /transit/route"
        );
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["ride"]["stops"][1]["stop_id"], "B");
        assert!(
            body.get("shape_to_stop").is_none(),
            "A is the trip's first stop"
        );

        // From the trip's second stop, with its line before the stop: straight, without a shape.
        let from_b = route.replace(r#""stop_id":"A""#, r#""stop_id":"B""#);
        let answer = transit.route_at(from_b.as_bytes(), Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(
            answer.body,
            server.route_json(from_b.as_bytes(), now, &|| false).body,
            "the same bytes as the server's /transit/route from B"
        );
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert!(body["shape_to_stop"].is_string(), "{body}");

        // Feeds are applied as for a plan, and a cancelled query stops.
        let feeds = vec![TransitFeed {
            feed: "bus".to_owned(),
            kind: TransitFeedKind::Alerts,
            bytes: Some(vec![0xff, 0xff, 0xff]),
        }];
        let answer = transit.nearby_at(nearby, feeds, now, &|| false);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["realtime"]["feeds"][0]["state"], "undecodable");
        assert_eq!(
            transit.nearby_at(nearby, Vec::new(), now, &|| true).status,
            499
        );
        // The device's walk limit holds here as for a plan.
        let far = br#"{"location":{"lat":42.5078,"lon":1.5205},"max_walk_seconds":2701}"#;
        assert_eq!(
            transit.nearby_at(far, Vec::new(), now, &|| false).status,
            400
        );
    }

    /// The device looks up a ride's departure with the server's bytes, and it's the plan's leg
    /// less what only the plan knows.
    #[test]
    fn a_rides_departure_answers_with_the_servers_bytes() {
        let (_dir, on_device) = timetable();
        let (_server_dir, on_server) = timetable();
        let transit = OfflineTransit::open_timetable(on_device, vec![tiles()])
            .ok()
            .unwrap();
        let server = valinor_transit_plan::StreetPlanner::open_with_built_transfers(
            valinor_gtfs::TimetableSet::single(on_server).unwrap(),
            &[tiles()],
            valinor_transit_plan::StreetConfig::default(),
        )
        .unwrap();
        let now = 1_781_506_440;
        let plan = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.51,"lon":1.535},"max_walk_seconds":600,"departure":1781506500}"#;
        let answer = transit.plan_at(plan, Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        let leg = body["journeys"][0]["legs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|leg| leg["type"] == "transit")
            .expect("a ride")
            .clone();
        let ride = format!(
            r#"{{"feed":"bus","board_stop_id":{},"alight_stop_id":{},"trip_id":"T1","start_date":{}}}"#,
            leg["stops"][0]["stop_id"],
            leg["stops"].as_array().unwrap().last().unwrap()["stop_id"],
            leg["trip"]["start_date"],
        );
        let answer = transit.ride_at(ride.as_bytes(), Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(
            answer.body,
            server.ride_json(ride.as_bytes(), now, &|| false).body,
            "the same bytes as the server's /transit/ride"
        );
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        assert_eq!(body["ride"]["shape"], leg["shape"]);
        assert_eq!(body["ride"]["stops"], leg["stops"]);
        assert_eq!(
            transit
                .ride_at(ride.as_bytes(), Vec::new(), now, &|| true)
                .status,
            499
        );
    }

    /// The device lists the departures between two stops with the server's bytes, and they are
    /// the plan's leg's own list with everything stated.
    #[test]
    fn the_departures_between_two_stops_answer_with_the_servers_bytes() {
        let (_dir, on_device) = timetable();
        let (_server_dir, on_server) = timetable();
        let transit = OfflineTransit::open_timetable(on_device, vec![tiles()])
            .ok()
            .unwrap();
        let server = valinor_transit_plan::StreetPlanner::open_with_built_transfers(
            valinor_gtfs::TimetableSet::single(on_server).unwrap(),
            &[tiles()],
            valinor_transit_plan::StreetConfig::default(),
        )
        .unwrap();
        let now = 1_781_506_440;
        let plan = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.51,"lon":1.535},"max_walk_seconds":600,"departure":1781506500}"#;
        let answer = transit.plan_at(plan, Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        let leg = body["journeys"][0]["legs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|leg| leg["type"] == "transit")
            .expect("a ride")
            .clone();
        let planned = leg["departures"].as_array().unwrap();
        // One more after than a plan's six: the departure at `around` counts among them.
        let lookup = format!(
            r#"{{"feed":"bus","board_stop_id":{},"alight_stop_id":{},"around":{},"departures_after":7,"ready":{}}}"#,
            leg["stops"][0]["stop_id"],
            leg["stops"].as_array().unwrap().last().unwrap()["stop_id"],
            leg["departure"],
            leg["departure"],
        );
        let answer = transit.departures_at(lookup.as_bytes(), Vec::new(), now, &|| false);
        assert_eq!(answer.status, 200);
        assert_eq!(
            answer.body,
            server
                .departures_json(lookup.as_bytes(), now, &|| false)
                .body,
            "the same bytes as the server's /transit/departures"
        );
        let body: serde_json::Value = serde_json::from_slice(&answer.body).unwrap();
        let listed = body["departures"].as_array().unwrap();
        let trips = |departures: &[serde_json::Value]| -> Vec<(String, i64)> {
            departures
                .iter()
                .map(|d| {
                    (
                        d["trip_id"].as_str().unwrap().to_owned(),
                        d["departure"].as_i64().unwrap(),
                    )
                })
                .collect()
        };
        assert!(!listed.is_empty());
        assert_eq!(trips(listed), trips(planned));
        for departure in listed {
            assert_eq!(departure["start_date"], leg["trip"]["start_date"]);
            assert_eq!(departure["route"], leg["route"]);
            assert!(departure.get("suggested").is_none());
            assert_eq!(
                departure.get("missed").is_some(),
                departure["departure"].as_i64() < leg["departure"].as_i64()
            );
        }
        // Errors are the server's bytes too.
        let unknown = lookup.replace(r#""feed":"bus""#, r#""feed":"tram""#);
        let answer = transit.departures_at(unknown.as_bytes(), Vec::new(), now, &|| false);
        assert_eq!(answer.status, 404);
        assert_eq!(
            answer.body,
            server
                .departures_json(unknown.as_bytes(), now, &|| false)
                .body
        );
        assert_eq!(
            transit
                .departures_at(lookup.as_bytes(), Vec::new(), now, &|| true)
                .status,
            499
        );
    }

    #[test]
    fn reports_why_it_cannot_open() {
        let error =
            OfflineTransit::open("/nonexistent/build".to_owned(), None, vec![tiles()], None)
                .err()
                .unwrap();
        assert!(error.message.contains("/nonexistent/build"));
    }
}
