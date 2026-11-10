//! Real walking on the repository's Andorra tile fixture, with a synthetic feed.

use core::cell::Cell;
use std::path::PathBuf;

use insta::assert_json_snapshot;
use valinor_gtfs::{
    FeedId, FeedInput, InMemorySource, LatLon, LoadedTimetable, TimetableSet, compile,
};
use valinor_gtfs_rt::FeedKind;
use valinor_gtfs_rt::proto;
use valinor_gtfs_rt::proto::alert::Effect;

use crate::plan::{PlanStage, TransitRequest, TravelMode};
use crate::response::{JourneyLeg, PlanResponse};
use crate::state::FeedUpdate;
use crate::street::{StreetConfig, StreetPlanner};
use crate::testing::{alert_entity, encoded_feed, route_stop_selector};

const FEED: &str = "andorra";

/// 2026-06-15T00:00:00+02:00, a Monday in Andorra.
const MIDNIGHT: i64 = 1_781_474_400;

fn at(hours: i64, minutes: i64) -> i64 {
    MIDNIGHT + hours * 3600 + minutes * 60
}

fn tiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles.tar")
}

fn timetable() -> LoadedTimetable {
    timetable_with("", "")
}

/// [`timetable`], with these rows added to `trips.txt` and `stop_times.txt`.
fn timetable_with(trips: &str, stop_times: &str) -> LoadedTimetable {
    let source = InMemorySource::new()
        .with(
            "agency.txt",
            "agency_name,agency_timezone,agency_url\nAndorra Bus,Europe/Andorra,https://b.test\n",
        )
        .with(
            "routes.txt",
            "route_id,route_short_name,route_long_name,route_type\n\
             L1,1,Centre,3\nL2,2,Encamp,3\n",
        )
        .with(
            "stops.txt",
            "stop_id,stop_name,stop_lat,stop_lon\n\
             A,Plaça,42.5078,1.5211\n\
             B,Meritxell,42.5095,1.5345\n\
             C,Caldea,42.5111,1.5382\n\
             D,Encamp,42.5355,1.5830\n",
        )
        .with(
            "calendar.txt",
            "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\n\
             WK,1,1,1,1,1,0,0,20260601,20260630\n",
        )
        .with(
            "trips.txt",
            &format!(
                "route_id,service_id,trip_id,bikes_allowed\nL1,WK,T1,1\nL2,WK,T2,1\n{trips}"
            ),
        )
        .with(
            "stop_times.txt",
            &format!(
                "trip_id,arrival_time,departure_time,stop_id,stop_sequence\n\
             T1,09:00:00,09:00:00,A,1\n\
             T1,09:08:00,09:08:00,B,2\n\
             T2,09:20:00,09:20:00,C,1\n\
             T2,09:25:00,09:25:00,D,2\n\
             {stop_times}"
            ),
        );
    let compiled = compile(vec![FeedInput {
        id: FeedId::try_new(FEED.to_owned()).expect("valid id"),
        source: Box::new(source),
    }])
    .expect("compiles");
    LoadedTimetable::from_bytes(compiled.bytes).expect("loads")
}

fn planner() -> StreetPlanner {
    StreetPlanner::open_with_built_transfers(
        TimetableSet::single(timetable()).expect("set"),
        &[tiles()],
        StreetConfig::default(),
    )
    .expect("opens")
}

fn request(mode: TravelMode) -> TransitRequest {
    let mut request = TransitRequest::new(
        LatLon {
            lat: 42.5078,
            lon: 1.5205,
        },
        LatLon {
            lat: 42.5350,
            lon: 1.5825,
        },
        at(8, 55),
        at(8, 54),
    );
    request.mode = mode;
    // So that reaching the second line takes the first, and a walk between them.
    request.options.max_walk_seconds = 600;
    request
}

fn plan(planner: &StreetPlanner, request: &TransitRequest) -> PlanResponse {
    planner.plan(request, "en-US", &|| false).expect("plans")
}

#[test]
fn real_streets_expand_every_walking_leg() {
    let planner = planner();
    let response = plan(&planner, &request(TravelMode::Walking));
    assert_eq!(response.journeys.len(), 1);
    let journey = &response.journeys[0];
    assert_eq!(journey.trips, 2);
    let mut walks = 0;
    for leg in &journey.legs {
        let walk = match leg {
            JourneyLeg::Access(leg) | JourneyLeg::Egress(leg) => leg.walk.as_ref(),
            JourneyLeg::Transfer(leg) => leg.walk.as_ref(),
            JourneyLeg::Transit(_) => continue,
        }
        .expect("every walking leg has a path");
        walks += 1;
        assert!(!walk.shape.is_empty());
        assert!(walk.distance_meters > 0.0);
        assert!(walk.maneuvers.iter().all(|m| !m.instruction.is_empty()));
    }
    assert_eq!(walks, 3, "access, transfer, and egress");
    assert_json_snapshot!("real_streets_walking", &response);
}

#[test]
fn a_transfer_shared_by_journeys_is_drawn_once() {
    let planner = StreetPlanner::open_with_built_transfers(
        TimetableSet::single(timetable_with(
            "L1,WK,T1b,1\nL2,WK,T2b,1\n",
            "T1b,09:10:00,09:10:00,A,1\n\
             T1b,09:18:00,09:18:00,B,2\n\
             T2b,09:30:00,09:30:00,C,1\n\
             T2b,09:35:00,09:35:00,D,2\n",
        ))
        .expect("set"),
        &[tiles()],
        StreetConfig::default(),
    )
    .expect("opens");
    let draws = Cell::new(0);
    let response = planner
        .plan_probed(
            &request(TravelMode::Walking),
            "en-US",
            &|| false,
            &|stage| {
                draws.set(draws.get() + u32::from(stage == PlanStage::DrawTransfer));
            },
        )
        .expect("plans");
    let walks: Vec<_> = response
        .journeys
        .iter()
        .flat_map(|journey| &journey.legs)
        .filter_map(|leg| match leg {
            JourneyLeg::Transfer(leg) => leg.walk.as_ref(),
            _ => None,
        })
        .collect();
    assert_eq!(response.journeys.len(), 2);
    assert_eq!(walks.len(), 2, "each journey has its transfer's path");
    assert_eq!(walks[0], walks[1]);
    assert_eq!(draws.get(), 1);
}

#[test]
fn cycling_legs_use_bicycle_directions() {
    let planner = planner();
    let response = plan(&planner, &request(TravelMode::Cycling));
    let leg = response.journeys[0]
        .legs
        .iter()
        .find_map(|leg| match leg {
            JourneyLeg::Access(leg) => leg.walk.as_ref(),
            _ => None,
        })
        .expect("an access path");
    assert!(
        leg.maneuvers
            .iter()
            .any(|m| m.instruction.starts_with("Bike")),
        "{leg:?}"
    );
    assert!(matches!(
        &response.journeys[0].legs[0],
        JourneyLeg::Access(leg) if leg.mode == "cycling"
    ));
}

#[test]
fn a_moved_stop_marks_the_walk_to_it() {
    let planner = planner();
    let alerts = encoded_feed(
        at(8, 50),
        vec![alert_entity(
            "moved",
            Effect::StopMoved,
            vec![route_stop_selector("L1", "A")],
        )],
    );
    planner
        .state()
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&alerts),
            received_at: at(8, 50),
            stale_after: None,
        })
        .expect("known feed");
    let response = plan(&planner, &request(TravelMode::Walking));
    let JourneyLeg::Access(access) = &response.journeys[0].legs[0] else {
        panic!("the first leg is the walk to the stop");
    };
    assert_eq!(access.stop_moved.len(), 1);
    let notice = &access.stop_moved[0];
    assert_eq!((notice.walk_end, notice.stop_id.as_str()), ("end", "A"));
    assert_eq!(notice.header[0].text, "Stop relocated");
}

#[test]
fn an_added_stop_is_snapped_with_its_overlay() {
    let planner = planner();
    let mut entity = crate::testing::entity("stop-new");
    entity.stop = Some(proto::Stop {
        stop_id: Some("NEW".to_owned()),
        stop_lat: Some(42.5085),
        stop_lon: Some(1.5255),
        ..proto::Stop::default()
    });
    let bytes = encoded_feed(at(8, 50), vec![entity]);
    planner
        .state()
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::TripUpdates,
            bytes: Some(&bytes),
            received_at: at(8, 50),
            stale_after: None,
        })
        .expect("known feed");
    let snapshot = planner.state().pin(at(8, 51)).expect("pin");
    let new_stop = snapshot.new_stops().first().expect("the new stop").stop;
    let snaps = snapshot.snaps().expect("snaps");
    assert!(
        snaps
            .for_mode(TravelMode::Walking)
            .point(new_stop)
            .is_some()
    );
    assert!(
        snaps
            .for_mode(TravelMode::Cycling)
            .point(new_stop)
            .is_some()
    );
}

#[test]
fn json_requests_round_trip_and_report_errors() {
    let planner = planner();
    let body = format!(
        r#"{{"origin":{{"lat":42.5078,"lon":1.5205}},"destination":{{"lat":42.535,"lon":1.5825}},"departure":{},"max_walk_seconds":600}}"#,
        at(8, 55)
    );
    let answer = planner.plan_json(body.as_bytes(), at(8, 54), &|| false);
    assert_eq!(answer.status, 200);
    let value: serde_json::Value = serde_json::from_slice(&answer.body).expect("json");
    assert_eq!(value["journeys"][0]["trips"], 2);

    let bad = planner.plan_json(b"{not json", at(8, 54), &|| false);
    assert_eq!(bad.status, 400);
    assert!(String::from_utf8_lossy(&bad.body).contains("invalid_request"));

    let far = br#"{"origin":{"lat":10.0,"lon":10.0},"destination":{"lat":42.5,"lon":1.5}}"#;
    let answer = planner.plan_json(far, at(8, 54), &|| false);
    assert_eq!(answer.status, 400);
    assert!(String::from_utf8_lossy(&answer.body).contains("no_street_nearby"));

    let unknown = br#"{"origin":{"lat":42.5,"lon":1.5},"destination":{"lat":42.5,"lon":1.5},"mode":"flying"}"#;
    assert_eq!(planner.plan_json(unknown, at(8, 54), &|| false).status, 400);

    let over_limit = br#"{"origin":{"lat":42.5078,"lon":1.5205},"destination":{"lat":42.535,"lon":1.5825},"max_walk_seconds":3601}"#;
    let answer = planner.plan_json(over_limit, at(8, 54), &|| false);
    assert_eq!(answer.status, 400);
    assert!(String::from_utf8_lossy(&answer.body).contains("service limit"));

    let vehicle = planner.vehicle_json(FEED, "nope", at(8, 54));
    assert_eq!(vehicle.status, 200);
    assert!(String::from_utf8_lossy(&vehicle.body).contains("built_at"));
}

#[test]
fn a_modes_tables_share_one_copy_of_their_stop_attachments() {
    let planner = planner();
    for mode in [TravelMode::Walking, TravelMode::Cycling] {
        plan(&planner, &request(mode));
    }
    let idle = planner.idle_workers();
    assert_eq!(idle.len(), 1, "plans one at a time share a worker");
    let [walking, cycling, wheelchair] = idle[0].attachments;
    // Access, egress, and the drawn transfer's table, all on one copy.
    assert_eq!(walking, (3, 1));
    // Cycling draws no transfer here; access and egress share.
    assert_eq!(cycling.1, 1);
    assert!(cycling.0 >= 2);
    assert_eq!(wheelchair, (0, 0), "wheelchair wasn't planned");
}

/// A planner over [`timetable`] with this limit on what an idle worker keeps,
/// whose Thor workspace keeps nothing (so the limit is about the other searches).
fn planner_keeping(max_retained_worker_bytes: usize) -> StreetPlanner {
    let mut config = StreetConfig::default();
    config.limits.max_retained_worker_bytes = max_retained_worker_bytes;
    config.retention = valinor_thor::Retention {
        bidirectional_labels: 0,
        unidirectional_labels: 0,
    };
    StreetPlanner::open_with_built_transfers(
        TimetableSet::single(timetable()).expect("set"),
        &[tiles()],
        config,
    )
    .expect("opens")
}

#[test]
fn a_worker_over_its_limit_gives_back_search_storage_and_plans_the_same() {
    let modes = [
        TravelMode::Walking,
        TravelMode::Cycling,
        TravelMode::Wheelchair,
    ];
    // Plenty of room: the worker keeps everything its searches grew to.
    let roomy = planner_keeping(usize::MAX);
    let mut kept = Vec::new();
    let mut held = 0;
    for mode in modes {
        kept.push(plan(&roomy, &request(mode)));
        let idle = roomy.idle_workers();
        assert_eq!(idle.len(), 1);
        assert!(idle[0].retained_bytes > held, "each mode adds its storage");
        held = idle[0].retained_bytes;
    }

    // Too little room for that, but enough for what the searches keep without their storage.
    let tight = planner_keeping(held / 2);
    for (mode, want) in modes.into_iter().zip(&kept) {
        assert_eq!(&plan(&tight, &request(mode)), want, "{mode:?}");
        let idle = tight.idle_workers();
        assert_eq!(idle.len(), 1, "the worker is kept, not dropped");
        assert!(idle[0].retained_bytes <= held / 2);
    }
    // Planned again, on the released storage: the same answers.
    for (mode, want) in modes.into_iter().zip(&kept) {
        assert_eq!(&plan(&tight, &request(mode)), want, "{mode:?} again");
    }

    // No room at all: the worker goes, and plans are still the same.
    let none = planner_keeping(0);
    for (mode, want) in modes.into_iter().zip(&kept) {
        assert_eq!(&plan(&none, &request(mode)), want, "{mode:?}");
        assert!(none.idle_workers().is_empty());
    }
}
