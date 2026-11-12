//! Plans the same trips over two transit builds and reports any difference, response by
//! response: for checking that a change to how a build is made (such as pruning walks no plan
//! can use) leaves every plan as it was.
//!
//! ```text
//! cargo run --release -p valinor-transit-plan --example compare_builds -- \
//!     --a DIR --b DIR --tiles PATH [--pairs 144] [--seed 1] [--day 0] [--midnight UNIX]
//! ```
//!
//! Trips run between stops drawn from the first build's stops (each destination within about
//! 25 km of its origin), at three departure times each, for walking, cycling,
//! and wheelchair riders, as a 60-minute profile, as leaving now (a one-minute window), and,
//! where a journey was found, as missing its first departure (leaving a second later).
//! The first trips are `multi_feed_bench`'s named ones (Fremont to Central District, Ballard to
//! `SeaTac`). Responses are compared as JSON, less `generated_at` and `realtime.built_at`.
//! Prints each difference, then totals.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "a reporting tool"
)]

use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use valinor_gtfs::LatLon;
use valinor_transit_plan::{StreetConfig, StreetPlanner, TransitBuild, TransitRequest, TravelMode};

/// 2026-11-10 00:00 Pacific, a Tuesday (`--midnight` gives another place's).
const TUESDAY_MIDNIGHT: i64 = 1_794_297_600;

fn flag(name: &str) -> Option<String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == format!("--{name}") {
            return args.next();
        }
    }
    None
}

/// A small deterministic generator (xorshift64*), so a run can be repeated.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn json(response: &valinor_transit_plan::PlanResponse) -> Result<String, Box<dyn Error>> {
    let mut value = serde_json::to_value(response)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("generated_at");
        // When the planner's real-time state was last rebuilt, which differs between two
        // planners opened one after the other.
        if let Some(realtime) = object.get_mut("realtime").and_then(|r| r.as_object_mut()) {
            realtime.remove("built_at");
        }
    }
    Ok(value.to_string())
}

fn main() -> Result<(), Box<dyn Error>> {
    let a_dir = PathBuf::from(flag("a").ok_or("--a DIR")?);
    let b_dir = PathBuf::from(flag("b").ok_or("--b DIR")?);
    let tiles = flag("tiles").ok_or("--tiles PATH")?;
    let pairs: usize = flag("pairs").and_then(|n| n.parse().ok()).unwrap_or(144);
    let seed: u64 = flag("seed").and_then(|n| n.parse().ok()).unwrap_or(1);
    let day: i64 = flag("day").and_then(|n| n.parse().ok()).unwrap_or(0);
    let midnight = flag("midnight")
        .and_then(|n| n.parse().ok())
        .unwrap_or(TUESDAY_MIDNIGHT)
        + day * 86_400;

    let started = Instant::now();
    let a = StreetPlanner::from_build(
        TransitBuild::load(&a_dir, None)?,
        &[tiles.as_str()],
        StreetConfig::default(),
    )?;
    let b = StreetPlanner::from_build(
        TransitBuild::load(&b_dir, None)?,
        &[tiles.as_str()],
        StreetConfig::default(),
    )?;
    println!("opened both builds in {:.1?}", started.elapsed());

    let probe = midnight + 8 * 3600;
    let snapshot = a.state().pin_for_departure(probe, probe, probe + 3600)?;
    let locations: Vec<(f64, f64)> = snapshot
        .stop_locations()
        .map(|location| (location.latitude, location.longitude))
        .collect();
    if locations.is_empty() {
        return Err("no stops".into());
    }
    let mut rng = Rng(seed.max(1));
    // `multi_feed_bench`'s named trips, as `(lat, lon)`.
    let named = [
        ((47.649_735, -122.349_869), (47.613_442, -122.302_605)),
        ((47.668_606, -122.385_345), (47.441_296, -122.298_523)),
    ];
    let (mut plans, mut differ, mut journeys) = (0_usize, 0_usize, 0_usize);
    let (mut a_ms, mut b_ms) = (0.0_f64, 0.0_f64);
    for pair in 0..pairs + named.len() {
        let (from, to) = if let Some(&trip) = named.get(pair) {
            trip
        } else {
            let from = locations[rng.below(locations.len())];
            // A destination within about 25 km, where transit is a plausible answer.
            let mut to = locations[rng.below(locations.len())];
            for _ in 0..200 {
                if (to.0 - from.0).abs() < 0.2 && (to.1 - from.1).abs() < 0.3 {
                    break;
                }
                to = locations[rng.below(locations.len())];
            }
            (from, to)
        };
        let origin = LatLon {
            lat: from.0,
            lon: from.1,
        };
        let destination = LatLon {
            lat: to.0,
            lon: to.1,
        };
        for hour in [8, 12, 17] {
            let departure = midnight + hour * 3600;
            for mode in [
                TravelMode::Walking,
                TravelMode::Cycling,
                TravelMode::Wheelchair,
            ] {
                let mut requests = Vec::new();
                let mut profile =
                    TransitRequest::new(origin, destination, departure, departure - 60);
                profile.mode = mode;
                requests.push(("profile", profile));
                let mut now = profile;
                now.window_seconds = 60;
                requests.push(("leave now", now));
                let mut index = 0;
                while index < requests.len() {
                    let (kind, request) = requests[index];
                    index += 1;
                    let t = Instant::now();
                    let left = a.plan(&request, "en-US", &|| false);
                    a_ms += t.elapsed().as_secs_f64() * 1000.0;
                    let t = Instant::now();
                    let right = b.plan(&request, "en-US", &|| false);
                    b_ms += t.elapsed().as_secs_f64() * 1000.0;
                    plans += 1;
                    let (left, right) = match (left, right) {
                        (Ok(left), Ok(right)) => (left, right),
                        (left, right) => {
                            let same = format!("{:?}", left.as_ref().err())
                                == format!("{:?}", right.as_ref().err());
                            if !same {
                                differ += 1;
                                println!(
                                    "pair {pair} {hour}h {mode:?} {kind}: a {:?}, b {:?}",
                                    left.err(),
                                    right.err()
                                );
                            }
                            continue;
                        }
                    };
                    journeys += left.journeys.len();
                    let (left_json, right_json) = (json(&left)?, json(&right)?);
                    if left_json != right_json {
                        differ += 1;
                        println!(
                            "pair {pair} {hour}h {mode:?} {kind}: {from:?} -> {to:?} differs\n  a: {left_json}\n  b: {right_json}"
                        );
                    }
                    // Missing the first journey: plan again a second after it leaves.
                    if kind == "leave now"
                        && let Some(first) = left.journeys.first()
                    {
                        let mut missed = request;
                        missed.departure = first.departure + 1;
                        missed.now = first.departure + 1;
                        requests.push(("missed it", missed));
                    }
                }
            }
        }
    }
    println!(
        "{plans} plans ({journeys} journeys in a's): {differ} differ; planning took {a_ms:.0} ms \
         over a, {b_ms:.0} ms over b"
    );
    if differ > 0 {
        return Err(format!("{differ} plans differ").into());
    }
    Ok(())
}
