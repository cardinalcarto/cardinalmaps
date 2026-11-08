//! A timing test for route searches: run it in release mode with
//!
//! ```shell
//! VALINOR_THOR_BENCH=/mnt/maps/cardinal/valhalla-oracle/thor/seattle-loki.tsv \
//! VALINOR_THOR_TILES=/mnt/maps/cardinal/valhalla-oracle/seattle/tiles.tar \
//!   cargo test --release -p valinor-thor route_timings -- --ignored --nocapture
//! ```
//!
//! It routes every `route` request of a Loki output file (the `loki` helper's, see
//! `tools/oracle/README.md`) twice, through one [`Workspace`], and reports the times of
//! the second run (with the tiles in the cache) by algorithm.
//! Without the variables it uses the Andorra fixtures.
//! It's a guide, not a benchmark suite: one machine, no statistics beyond percentiles.

use core::num::NonZeroUsize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jiff::Timestamp;
use valhalla_graphtile::tile_provider::{
    CachingTileProvider, DirectoryGraphTileProvider, GraphTileProvider, TarballTileProvider,
};
use valhalla_proto::Api;
use valhalla_proto::options::Action;
use valhalla_proto::prost::Message;

use crate::{Thor, ThorConfig, Workspace};

fn inputs(path: &PathBuf) -> Vec<Api> {
    std::fs::read_to_string(path)
        .expect("inputs")
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f.len() >= 3 && f[1] == "ok")
                .then(|| Api::decode(STANDARD.decode(f[2]).expect("base64").as_slice()))
                .and_then(Result::ok)
        })
        .filter(|api| {
            api.options
                .as_ref()
                .is_some_and(|o| o.action == Action::Route as i32)
        })
        .collect()
}

fn run<P: GraphTileProvider>(provider: P, apis: &[Api]) {
    let thor = Thor::new(ThorConfig::default(), provider);
    let mut workspace = Workspace::default();
    let now = Timestamp::from_second(1_794_116_280).expect("a time");
    let mut times: BTreeMap<String, Vec<Duration>> = BTreeMap::new();
    for pass in 0..2 {
        for input in apis {
            let mut api = input.clone();
            let started = Instant::now();
            let result = thor.route_at(&mut workspace, &mut api, now);
            let elapsed = started.elapsed();
            if pass == 0 {
                continue;
            }
            let key = match &result {
                Err(_) => "no path".to_owned(),
                Ok(paths) => paths
                    .routes
                    .first()
                    .and_then(|r| r.legs.first())
                    .and_then(|l| l.algorithms.first())
                    .map_or("none", |a| a)
                    .to_owned(),
            };
            times.entry(key).or_default().push(elapsed);
            times.entry("all".to_owned()).or_default().push(elapsed);
        }
    }
    for (key, mut t) in times {
        t.sort();
        let pct = |p: usize| t[(t.len() - 1) * p / 100];
        let mean = t.iter().sum::<Duration>() / u32::try_from(t.len()).unwrap_or(u32::MAX);
        println!(
            "{key}: {} requests, mean {mean:?}, median {:?}, p90 {:?}, p99 {:?}, max {:?}",
            t.len(),
            pct(50),
            pct(90),
            pct(99),
            pct(100),
        );
    }
}

#[test]
#[ignore = "a timing test; run it in release mode"]
fn route_timings() {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bench = std::env::var("VALINOR_THOR_BENCH").map_or_else(
        |_| here.join("fixtures/oracle/andorra-loki.tsv"),
        PathBuf::from,
    );
    let tiles = std::env::var("VALINOR_THOR_TILES").map_or_else(
        |_| here.join("../valhalla-graphtile/fixtures/andorra-tiles"),
        PathBuf::from,
    );
    let apis = inputs(&bench);
    let cache = NonZeroUsize::new(4096).expect("4096");
    if tiles.is_dir() {
        run(DirectoryGraphTileProvider::new(tiles, cache), &apis);
    } else {
        let provider =
            TarballTileProvider::<false, false>::new_readonly(&tiles).expect("a tile tarball");
        run(CachingTileProvider::new(provider, cache), &apis);
    }
}
