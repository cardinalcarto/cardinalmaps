//! Independent review: targeted reproductions and a real-data invariant audit.
#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "review"
)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use valinor_gtfs::{
    FeedId as CompileFeedId, FeedInput as CompileInput, InMemorySource, LatLon, LoadedTimetable,
    ZipSource, compile,
};
use valinor_gtfs_rt::FeedKind;
use valinor_gtfs_rt::proto;

use crate::plan::{TransitRequest, TravelMode};
use crate::response::{JourneyLeg, PlanResponse, StopVisit};
use crate::state::{FeedUpdate, TransitState};
use crate::street::{StreetConfig, StreetPlanner};
use crate::testing::{encoded_feed, entity};

const ROOT: &str = "/mnt/maps/cardinal/transit/kcm";
const TILES: &str = "/mnt/maps/cardinal/routing-elevation-i2b/washington/tiles.tar";

fn p(lat: f64, lon: f64) -> LatLon {
    LatLon { lat, lon }
}

// ---------------------- Andorra fixture reproductions ----------------------
const MIDNIGHT: i64 = 1_781_474_400;
fn at(h: i64, m: i64) -> i64 {
    MIDNIGHT + h * 3600 + m * 60
}
fn andorra_tiles() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../valhalla-graphtile/fixtures/andorra-tiles.tar")
}
fn andorra() -> StreetPlanner {
    let source = InMemorySource::new()
        .with("agency.txt", "agency_name,agency_timezone,agency_url\nBus,Europe/Andorra,https://b.test\n")
        .with("routes.txt", "route_id,route_short_name,route_type\nL1,1,3\n")
        .with("stops.txt", "stop_id,stop_name,stop_lat,stop_lon\nA,P,42.5078,1.5211\nB,M,42.5095,1.5345\n")
        .with("calendar.txt", "service_id,monday,tuesday,wednesday,thursday,friday,saturday,sunday,start_date,end_date\nWK,1,1,1,1,1,0,0,20260601,20260630\n")
        .with("trips.txt", "route_id,service_id,trip_id\nL1,WK,T1\n")
        .with("stop_times.txt", "trip_id,arrival_time,departure_time,stop_id,stop_sequence\nT1,09:00:00,09:00:00,A,1\nT1,09:08:00,09:08:00,B,2\n");
    let compiled = compile(vec![CompileInput {
        id: CompileFeedId::try_new("bus".to_owned()).unwrap(),
        source: Box::new(source),
    }])
    .unwrap();
    let tt = LoadedTimetable::from_bytes(compiled.bytes).unwrap();
    StreetPlanner::open(tt, &[andorra_tiles()], StreetConfig::default()).unwrap()
}
fn body(lat: f64, lon: f64, extra: &str) -> Vec<u8> {
    format!(r#"{{"origin":{{"lat":{lat},"lon":{lon}}},"destination":{{"lat":42.51,"lon":1.535}},"departure":{}{extra}}}"#, at(8,55)).into_bytes()
}

#[test]
fn review_extreme_origin_status_codes() {
    let planner = andorra();
    for (lat, lon, label) in [
        (0.0, 0.0, "null island"),
        (90.0, 180.0, "pole"),
        (-90.0, -180.0, "south pole"),
        (42.0, 1.0, "outside tiles"),
        (47.6, -122.3, "another region"),
    ] {
        let answer = planner.plan_json(&body(lat, lon, ""), at(8, 54), &|| false);
        println!(
            "{label}: {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
    for raw in [
        r#"{"origin":{"lat":1e999,"lon":0},"destination":{"lat":0,"lon":0}}"#,
        r#"{"origin":{"lat":"NaN","lon":0},"destination":{"lat":0,"lon":0}}"#,
        r#"{"origin":{"lat":91,"lon":0},"destination":{"lat":0,"lon":0}}"#,
        r#"{"origin":{"lat":42.5,"lon":1.5},"destination":{"lat":42.5,"lon":1.5},"departure":9223372036854775807}"#,
        r#"{"origin":{"lat":42.5,"lon":1.5},"destination":{"lat":42.5,"lon":1.5},"departure":-9223372036854775808}"#,
        r#"{"origin":{"lat":42.5,"lon":1.5},"destination":{"lat":42.5,"lon":1.5},"window_seconds":86401}"#,
        r#"{"origin":{"lat":42.5,"lon":1.5},"destination":{"lat":42.5,"lon":1.5},"max_trips":0}"#,
    ] {
        let answer = planner.plan_json(raw.as_bytes(), at(8, 54), &|| false);
        println!(
            "{raw}: {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

fn new_stop_feed(now: i64, lat: f32, lon: f32) -> Vec<u8> {
    let mut e = entity("s1");
    e.stop = Some(proto::Stop {
        stop_id: Some("NEW".to_owned()),
        stop_lat: Some(lat),
        stop_lon: Some(lon),
        ..proto::Stop::default()
    });
    encoded_feed(now, vec![e])
}

/// A GTFS-RT `Stop` at a valid coordinate outside the tile set.
#[test]
fn review_new_stop_outside_tiles_does_not_break_planning() {
    for (lat, lon, label) in [(0.5_f32, 0.5_f32, "ocean"), (47.6, -122.3, "other region")] {
        let planner = andorra();
        let now = at(8, 54);
        planner
            .state()
            .ingest(FeedUpdate {
                feed: "bus",
                kind: FeedKind::TripUpdates,
                bytes: Some(&new_stop_feed(now, lat, lon)),
                received_at: now,
                stale_after: None,
            })
            .unwrap();
        let answer = planner.plan_json(
            &body(42.5078, 1.5205, r#","max_walk_seconds":600"#),
            now,
            &|| false,
        );
        println!(
            "{label}: {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body[..answer.body.len().min(300)])
        );
        let snap = planner.state().pin(now).unwrap();
        println!(
            "  new stops {} snap report {:?}",
            snap.new_stops().len(),
            snap.snaps()
                .unwrap()
                .for_mode(TravelMode::Walking)
                .reports()
        );
        assert_eq!(answer.status, 200, "{label}");
    }
}

// ---------------------- Real data ----------------------
fn kcm_bytes() -> &'static [u8] {
    static B: OnceLock<Vec<u8>> = OnceLock::new();
    B.get_or_init(|| {
        compile(vec![CompileInput {
            id: CompileFeedId::try_new("kcm".to_owned()).unwrap(),
            source: Box::new(
                ZipSource::from_path(&Path::new(ROOT).join("static/kcm-FAL26-161.2.zip")).unwrap(),
            ),
        }])
        .unwrap()
        .bytes
    })
}
fn kcm() -> StreetPlanner {
    StreetPlanner::open(
        LoadedTimetable::from_bytes(kcm_bytes().to_vec()).unwrap(),
        &[TILES],
        StreetConfig::default(),
    )
    .unwrap()
}
fn captures() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(Path::new(ROOT).join("rt"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}
fn ingest(state: &TransitState, dir: &Path) -> i64 {
    let mut newest = 0;
    for (f, k) in [
        ("tripupdates.pb", FeedKind::TripUpdates),
        ("vehiclepositions.pb", FeedKind::VehiclePositions),
        ("alerts.pb", FeedKind::Alerts),
    ] {
        let b = fs::read(dir.join(f)).unwrap();
        let t = valinor_gtfs_rt::DecodedFeed::decode(&b)
            .unwrap()
            .timestamp()
            .unwrap_or(0);
        newest = newest.max(t);
        state
            .ingest(FeedUpdate {
                feed: "kcm",
                kind: k,
                bytes: Some(&b),
                received_at: t,
                stale_after: None,
            })
            .unwrap();
    }
    newest
}

#[derive(Default)]
struct Audit {
    queries: usize,
    errors: HashMap<String, usize>,
    journeys: usize,
    legs: usize,
    issues: HashMap<&'static str, usize>,
    examples: HashMap<&'static str, String>,
    empty: usize,
    walk_diffs: Vec<f64>,
}
impl Audit {
    fn issue(&mut self, kind: &'static str, msg: String) {
        *self.issues.entry(kind).or_default() += 1;
        self.examples.entry(kind).or_insert(msg);
    }
    fn check(&mut self, q: &str, r: &PlanResponse) {
        if r.journeys.is_empty() {
            self.empty += 1;
        }
        for j in &r.journeys {
            self.journeys += 1;
            let mut clock = j.departure;
            let mut prev_alight: Option<&StopVisit> = None;
            for (i, leg) in j.legs.iter().enumerate() {
                self.legs += 1;
                match leg {
                    JourneyLeg::Access(l) => {
                        if l.departure != clock {
                            self.issue("access-departure", format!("{q}"));
                        }
                        clock = l.arrival;
                        if let Some(JourneyLeg::Transit(n)) = j.legs.get(i + 1) {
                            if n.stops.first().map(|s| &s.stop.stop_id) != Some(&l.stop.stop_id) {
                                self.issue(
                                    "access-stop-mismatch",
                                    format!(
                                        "{q}: {} vs {:?}",
                                        l.stop.stop_id,
                                        n.stops.first().map(|s| s.stop.stop_id.clone())
                                    ),
                                );
                            }
                        } else {
                            self.issue("access-not-followed-by-transit", q.to_owned());
                        }
                        self.walk(q, "access", l.duration_seconds, l.walk.as_ref());
                    }
                    JourneyLeg::Egress(l) => {
                        if l.departure != clock {
                            self.issue("egress-departure", q.to_owned());
                        }
                        clock = l.arrival;
                        if prev_alight.map(|s| &s.stop.stop_id) != Some(&l.stop.stop_id) {
                            self.issue("egress-stop-mismatch", q.to_owned());
                        }
                        self.walk(q, "egress", l.duration_seconds, l.walk.as_ref());
                    }
                    JourneyLeg::Transfer(l) => {
                        if l.departure != clock {
                            self.issue("transfer-departure", q.to_owned());
                        }
                        clock = l.arrival;
                        if prev_alight.map(|s| &s.stop.stop_id) != Some(&l.from.stop_id) {
                            self.issue("transfer-from-mismatch", q.to_owned());
                        }
                        if let Some(JourneyLeg::Transit(n)) = j.legs.get(i + 1) {
                            if n.stops.first().map(|s| &s.stop.stop_id) != Some(&l.to.stop_id) {
                                self.issue("transfer-to-mismatch", q.to_owned());
                            }
                        }
                        if l.from.stop_id == l.to.stop_id {
                            self.issue("transfer-same-stop", q.to_owned());
                        }
                        self.walk(q, "transfer", l.duration_seconds, l.walk.as_ref());
                    }
                    JourneyLeg::Transit(l) => {
                        if l.departure < clock {
                            self.issue(
                                "board-before-arrival",
                                format!("{q}: board {} clock {clock}", l.departure),
                            );
                        }
                        if l.wait_seconds != l.departure - clock {
                            self.issue("wait-mismatch", q.to_owned());
                        }
                        clock = l.arrival;
                        let (Some(first), Some(last)) = (l.stops.first(), l.stops.last()) else {
                            self.issue("no-stops", q.to_owned());
                            continue;
                        };
                        prev_alight = Some(last);
                        if first.departure != l.departure {
                            self.issue(
                                "first-stop-departure",
                                format!("{q}: {} {}", first.departure, l.departure),
                            );
                        }
                        if last.arrival != l.arrival {
                            self.issue(
                                "last-stop-arrival",
                                format!("{q}: {} {}", last.arrival, l.arrival),
                            );
                        }
                        if first.skipped {
                            self.issue("boarded-skipped", q.to_owned());
                        }
                        if last.skipped {
                            self.issue("alighted-skipped", q.to_owned());
                        }
                        if first.pickup_type == Some("none") {
                            self.issue("boarded-no-pickup", q.to_owned());
                        }
                        if last.drop_off_type == Some("none") {
                            self.issue("alighted-no-dropoff", q.to_owned());
                        }
                        let mut t = l.departure;
                        for s in &l.stops {
                            if s.arrival < t.min(s.arrival) || s.departure < s.arrival {
                                self.issue(
                                    "stop-times-not-monotonic",
                                    format!(
                                        "{q}: {} arr {} dep {}",
                                        s.stop.stop_id, s.arrival, s.departure
                                    ),
                                );
                            }
                            t = s.departure;
                            if let Some(pa) = s.predicted_arrival
                                && pa != s.arrival
                            {
                                self.issue(
                                    "predicted-arrival-not-routed",
                                    format!(
                                        "{q}: stop {} predicted {pa} routed {} sched {:?}",
                                        s.stop.stop_id, s.arrival, s.scheduled_arrival
                                    ),
                                );
                            }
                            if let Some(pd) = s.predicted_departure
                                && pd != s.departure
                            {
                                self.issue(
                                    "predicted-departure-not-routed",
                                    format!(
                                        "{q}: stop {} predicted {pd} routed {} sched {:?}",
                                        s.stop.stop_id, s.departure, s.scheduled_departure
                                    ),
                                );
                            }
                            if l.trip.realtime_state.is_none()
                                && s.predicted_departure.is_none()
                                && let Some(sd) = s.scheduled_departure
                                && sd != s.departure
                            {
                                self.issue(
                                    "scheduled-not-routed",
                                    format!(
                                        "{q}: stop {} sched {sd} routed {} trip {:?}",
                                        s.stop.stop_id, s.departure, l.trip.trip_id
                                    ),
                                );
                            }
                            if let (Some(d), Some(pd), Some(sd)) = (
                                s.departure_delay_seconds,
                                s.predicted_departure,
                                s.scheduled_departure,
                            ) && i64::from(d) != pd - sd
                            {
                                self.issue(
                                    "delay-not-predicted-minus-sched",
                                    format!(
                                        "{q}: stop {} delay {d} predicted-sched {}",
                                        s.stop.stop_id,
                                        pd - sd
                                    ),
                                );
                            }
                            if s.departure_delay_seconds.is_some()
                                && s.predicted_departure.is_none()
                            {
                                self.issue(
                                    "delay-without-prediction",
                                    format!(
                                        "{q}: stop {} trip {:?}",
                                        s.stop.stop_id, l.trip.trip_id
                                    ),
                                );
                            }
                        }
                        if l.delay_seconds != first.departure_delay_seconds {
                            self.issue("leg-delay", q.to_owned());
                        }
                    }
                }
            }
            if clock != j.arrival {
                self.issue("journey-arrival", format!("{q}: {clock} vs {}", j.arrival));
            }
            if !j.legs.iter().any(|l| matches!(l, JourneyLeg::Access(_))) {
                self.issue("no-access", q.to_owned());
            }
            if !j.legs.iter().any(|l| matches!(l, JourneyLeg::Egress(_))) {
                self.issue("no-egress", q.to_owned());
            }
        }
    }
    fn walk(
        &mut self,
        q: &str,
        kind: &'static str,
        duration: i64,
        walk: Option<&crate::response::WalkPath>,
    ) {
        match walk {
            None if duration > 1 => self.issue("walk-missing", format!("{q}: {kind} {duration}s")),
            None => {}
            Some(w) => {
                let diff = w.duration_seconds - duration as f64;
                self.walk_diffs.push(diff);
                if diff.abs() > 10.0_f64.max(0.1 * duration as f64) {
                    self.issue(
                        "walk-duration-disagrees",
                        format!(
                            "{q}: {kind} {duration}s vs walk {}s dist {}m",
                            w.duration_seconds, w.distance_meters
                        ),
                    );
                }
            }
        }
    }
    fn report(&mut self, label: &str) {
        self.walk_diffs.sort_by(f64::total_cmp);
        let n = self.walk_diffs.len().max(1);
        println!(
            "== {label}: queries={} errors={:?} journeys={} legs={} empty={} walk-diff p1/p50/p99/min/max={:?}/{:?}/{:?}/{:?}/{:?}",
            self.queries,
            self.errors,
            self.journeys,
            self.legs,
            self.empty,
            self.walk_diffs.get(n / 100),
            self.walk_diffs.get(n / 2),
            self.walk_diffs.get(n * 99 / 100),
            self.walk_diffs.first(),
            self.walk_diffs.last()
        );
        let mut items: Vec<_> = self.issues.iter().collect();
        items.sort();
        for (k, c) in items {
            println!("  {k}: {c}  e.g. {}", self.examples[k]);
        }
    }
}

fn run_audit(
    planner: &StreetPlanner,
    label: &str,
    now: i64,
    dep: i64,
    mode: TravelMode,
    count: usize,
) {
    use rand_lite::Lfsr;
    let mut audit = Audit::default();
    let mut rng = Lfsr(0x1234_5678_9abc_def1);
    let start = Instant::now();
    for _ in 0..count {
        let (a, b) = (
            p(47.45 + rng.f() * 0.3, -122.40 + rng.f() * 0.25),
            p(47.45 + rng.f() * 0.3, -122.40 + rng.f() * 0.25),
        );
        let mut r = TransitRequest::new(a, b, dep, now);
        r.mode = mode;
        audit.queries += 1;
        match planner.plan(&r, "en-US", &|| false) {
            Ok(resp) => audit.check(
                &format!("{:.5},{:.5}->{:.5},{:.5}", a.lat, a.lon, b.lat, b.lon),
                &resp,
            ),
            Err(e) => *audit.errors.entry(e.to_string()).or_default() += 1,
        }
    }
    println!("{label}: took {:?}", start.elapsed());
    audit.report(label);
}

mod rand_lite {
    pub struct Lfsr(pub u64);
    impl Lfsr {
        pub fn f(&mut self) -> f64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            (x >> 11) as f64 / (1u64 << 53) as f64
        }
    }
}

#[test]
#[ignore = "real"]
fn review_audit() {
    let planner = kcm();
    // A live capture, in its service time.
    let dirs = captures();
    let dir = dirs
        .iter()
        .find(|p| p.file_name().unwrap().to_str().unwrap() >= "20261109T073000Z")
        .unwrap();
    let t = Instant::now();
    let now = ingest(planner.state(), dir);
    println!("ingest {:?}", t.elapsed());
    let t = Instant::now();
    let snap = planner.state().refresh(now).unwrap();
    println!("refresh {:?} stats {:?}", t.elapsed(), snap.stats());
    run_audit(&planner, "rt-walking", now, now, TravelMode::Walking, 150);
    run_audit(&planner, "rt-cycling", now, now, TravelMode::Cycling, 60);
    // Daytime schedule alone.
    let day = 1_794_211_200 + 8 * 3600 + 30 * 60;
    run_audit(
        &planner,
        "day-sched-walking",
        day,
        day,
        TravelMode::Walking,
        150,
    );
    run_audit(
        &planner,
        "day-sched-cycling",
        day,
        day,
        TravelMode::Cycling,
        60,
    );
}

// ---------------------- Synthetic: alert semantics ----------------------
use crate::testing::{
    FEED, StraightLineAccess, StraightLineTransfers, alert_entity, june_15, synthetic_state,
};
use valinor_gtfs_rt::proto::alert::Effect;
use valinor_transit::Router;

fn straight_run(state: &TransitState, request: &TransitRequest) -> PlanResponse {
    let snapshot = state.pin(request.now).unwrap();
    let mut router = Router::new(StraightLineTransfers::new(
        snapshot.stop_locations(),
        500.0,
        1.33,
    ));
    crate::plan::plan(
        state,
        &mut router,
        &mut StraightLineAccess::default(),
        request,
        &|| false,
    )
    .unwrap()
}
fn ingest_alert(state: &TransitState, ts: i64, entity: proto::FeedEntity) {
    let bytes = encoded_feed(ts, vec![entity]);
    state
        .ingest(FeedUpdate {
            feed: FEED,
            kind: FeedKind::Alerts,
            bytes: Some(&bytes),
            received_at: ts,
            stale_after: None,
        })
        .unwrap();
}
const TS: i64 = 1_781_535_000;
fn tlegs(j: &crate::response::Journey) -> Vec<&crate::response::TransitLeg> {
    j.legs
        .iter()
        .filter_map(|l| {
            if let JourneyLeg::Transit(l) = l {
                Some(l.as_ref())
            } else {
                None
            }
        })
        .collect()
}

/// GTFS-RT allows a descriptor without trip_id: route_id, direction_id, start_date, start_time.
/// Routing compiles it (alerts.rs compile selector); display must attach it too.
#[test]
fn review_one_run_selector_detour_attaches_to_leg() {
    let state = synthetic_state();
    let selector = proto::EntitySelector {
        trip: Some(proto::TripDescriptor {
            route_id: Some("R1".into()),
            direction_id: Some(0),
            start_date: Some("20260615".into()),
            start_time: Some("08:00:30".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    ingest_alert(
        &state,
        TS,
        alert_entity("a1", Effect::Detour, vec![selector]),
    );
    let mut r = TransitRequest::new(
        LatLon {
            lat: 47.5995,
            lon: -122.33,
        },
        LatLon {
            lat: 47.6302,
            lon: -122.33,
        },
        june_15(7, 55),
        TS,
    );
    r.window_seconds = 900;
    let resp = straight_run(&state, &r);
    let leg = resp
        .journeys
        .iter()
        .flat_map(|j| tlegs(j))
        .find(|l| l.trip.trip_id == "T1")
        .expect("T1 ridden");
    assert!(
        !leg.alerts.is_empty(),
        "the detour names this run but the leg shows no alert"
    );
}

/// A selector naming one start_time of a frequency trip must not attach to another run.
#[test]
fn review_frequency_selector_start_time_only_attaches_to_its_run() {
    let state = synthetic_state();
    let selector = proto::EntitySelector {
        trip: Some(proto::TripDescriptor {
            trip_id: Some("F1".into()),
            start_date: Some("20260615".into()),
            start_time: Some("10:30:00".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    ingest_alert(
        &state,
        TS,
        alert_entity("a1", Effect::Detour, vec![selector]),
    );
    let mut r = TransitRequest::new(
        LatLon {
            lat: 47.6,
            lon: -122.33,
        },
        LatLon {
            lat: 47.61,
            lon: -122.33,
        },
        june_15(9, 55),
        TS,
    );
    r.window_seconds = 3000;
    let resp = straight_run(&state, &r);
    let mut any = false;
    for j in &resp.journeys {
        for l in tlegs(j) {
            if l.trip.trip_id == "F1" {
                let start = l.stops[0].departure - june_15(10, 0);
                println!("run at +{start}s alerts={}", l.alerts.len());
                any = true;
                if start != 1800 {
                    assert!(l.alerts.is_empty(), "alert leaked onto another run");
                }
            }
        }
    }
    assert!(any);
}

// ---------------------- Real data: cost, midnight, truthfulness ----------------------
fn rss_mb() -> u64 {
    fs::read_to_string("/proc/self/statm")
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse::<u64>()
        .unwrap()
        * 4
        / 1024
}

#[test]
#[ignore = "real"]
fn review_costs() {
    let planner = kcm();
    let day = 1_794_211_200 + 8 * 3600 + 30 * 60;
    let a = p(47.6105, -122.3376);
    let b = p(47.6585, -122.3130);
    println!("rss after open {}MB", rss_mb());
    let go = |label: &str, f: &dyn Fn(&mut TransitRequest), to: LatLon, from: LatLon| {
        let mut r = TransitRequest::new(from, to, day, day);
        f(&mut r);
        let t = Instant::now();
        let res = planner.plan(&r, "en-US", &|| false);
        let el = t.elapsed();
        match res {
            Ok(x) => {
                let bytes = serde_json::to_vec(&x).unwrap().len();
                println!(
                    "{label}: {el:?} journeys={} direct={:?} body={}KB rss={}MB",
                    x.journeys.len(),
                    x.direct,
                    bytes / 1024,
                    rss_mb()
                );
            }
            Err(e) => println!("{label}: {el:?} ERR {e}"),
        }
    };
    go("base 60m", &|_| {}, b, a);
    go("window 24h", &|r| r.window_seconds = 86400, b, a);
    go("walk 2h", &|r| r.options.max_walk_seconds = 7200, b, a);
    go("walk 12h", &|r| r.options.max_walk_seconds = 43200, b, a);
    go(
        "walk u32max",
        &|r| r.options.max_walk_seconds = u32::MAX,
        b,
        a,
    );
    go(
        "window 24h + walk 1h + trips 8",
        &|r| {
            r.window_seconds = 86400;
            r.options.max_walk_seconds = 3600;
        },
        b,
        a,
    );
    // Destination far from every stop but on a street (Stevens Pass); origin downtown.
    go("dest remote", &|_| {}, p(47.7467, -121.0890), a);
    go("origin remote", &|_| {}, b, p(47.7467, -121.0890));
    go(
        "dest remote, cycling",
        &|r| r.mode = TravelMode::Cycling,
        p(47.7467, -121.0890),
        a,
    );
    // A long walk origin: ~4 km from a stop.
    go("long-walk origin", &|_| {}, b, p(47.45, -121.95));
    // Interrupt always: what does the caller see?
    let mut r = TransitRequest::new(a, b, day, day);
    r.options.max_walk_seconds = 3600;
    let res = planner.plan(&r, "en-US", &|| true);
    println!(
        "interrupted: {:?}",
        res.map(|x| (x.journeys.len(), x.direct))
            .map_err(|e| e.to_string())
    );
}

#[test]
#[ignore = "real"]
fn review_midnight() {
    let planner = kcm();
    // Monday 2026-11-09 00:40 PST; service date stops running past 24:00 of Sunday.
    let midnight = 1_794_211_200_i64;
    for (label, dep, win) in [
        ("00:40 Monday", midnight + 2400, 3600),
        ("23:40 Sunday to 00:30", midnight - 1200, 3000),
    ] {
        let r = {
            let mut r = TransitRequest::new(p(47.6105, -122.3376), p(47.6585, -122.3130), dep, dep);
            r.window_seconds = win;
            r
        };
        let x = planner.plan(&r, "en-US", &|| false).unwrap();
        println!("{label}: {} journeys", x.journeys.len());
        for j in x.journeys.iter().take(8) {
            for l in tlegs(j) {
                println!(
                    "  {} start_date={:?} board {} first sched {:?}",
                    l.trip.trip_id, l.trip.start_date, l.departure, l.stops[0].scheduled_departure
                );
            }
        }
    }
    // pin latency right after an ingest: does a concurrent query block?
    let dirs = captures();
    let dir = dirs
        .iter()
        .find(|p| p.file_name().unwrap().to_str().unwrap() >= "20261109T073000Z")
        .unwrap();
    let now = ingest(planner.state(), dir);
    let t = Instant::now();
    let s = planner.state().pin(now).unwrap();
    println!(
        "first pin after ingest (does the rebuild): {:?} matched={}",
        t.elapsed(),
        s.stats().trip_updates_matched
    );
    let t = Instant::now();
    let _ = planner.state().pin(now).unwrap();
    println!("second pin (cached): {:?}", t.elapsed());
    // Cross-midnight query at the overnight capture: on-demand vs current.
    let t = Instant::now();
    let s = planner
        .state()
        .pin_for_departure(now, now, now + 3600)
        .unwrap();
    println!(
        "pin_for_departure {:?} window dates={:?} center_none={}",
        t.elapsed(),
        s.window.dates.len(),
        s.built_at()
    );
}

#[test]
#[ignore = "real"]
fn review_raw_trip_vs_response() {
    use prost::Message;
    let dir = captures()
        .into_iter()
        .find(|p| {
            let b = fs::read(p.join("tripupdates.pb")).unwrap();
            valinor_gtfs_rt::DecodedFeed::decode(&b)
                .unwrap()
                .timestamp()
                == Some(1794209399)
        })
        .expect("capture");
    println!("capture {}", dir.display());
    let bytes = fs::read(dir.join("tripupdates.pb")).unwrap();
    let msg = proto::FeedMessage::decode(&*bytes).unwrap();
    for e in &msg.entity {
        if let Some(t) = &e.trip_update
            && t.trip.trip_id.as_deref() == Some("839839011")
        {
            println!("trip {:?} delay {:?}", t.trip, t.delay);
            for s in t.stop_time_update.iter().take(4) {
                println!(
                    "  {:?}",
                    (
                        s.stop_sequence,
                        &s.stop_id,
                        s.arrival.as_ref().map(|a| (a.time, a.delay)),
                        s.departure.as_ref().map(|a| (a.time, a.delay)),
                        s.schedule_relationship
                    )
                );
            }
            println!("  ... {} updates", t.stop_time_update.len());
        }
    }
}
