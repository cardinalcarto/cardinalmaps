//! Samples documents of an index, a fixed number per kind, as JSON lines, for recall
//! benchmarks (e.g. the offline/server comparison in
//! crates/geocoder_flutter/examples/bench_quality.py): each line has the document's kind,
//! its result ID as `/v1/search` gives it, its name, address, center and areas.
//!
//!   cargo run --release --example sample_docs -- <index> <per kind> [seed] > sample.jsonl
//!
//! Uses `tagger_data::sample` (areas weighted by population, transit stops sparser), then
//! picks `per kind` of each kind at random with the seed. Areas are sampled over the whole
//! index's Who's On First places, so a regional benchmark samples its areas elsewhere.

use std::collections::{BTreeMap, HashSet};

use airmail::index::{AirmailIndex, SearchHit};
use airmail::poi::{Kind, PlaceId, WofPlace, one_per_placetype, osm_display_name, postcode_key};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use serde_json::json;

fn id(hit: &SearchHit, country: Option<&str>) -> String {
    if let Some((poi, _)) = hit.poi {
        format!("poi:{poi}")
    } else if let Some(wof) = &hit.wof {
        format!("wof:{}", wof.id)
    } else if let Some(osm) = &hit.osm {
        format!("osm:{}/{}", osm.osm_type.as_str(), osm.id)
    } else if let (Kind::Postcode, Some(country), Some(code)) =
        (hit.kind, country, hit.tag("postal_code"))
    {
        PlaceId::Postcode(postcode_key(country, code)).to_string()
    } else {
        format!(
            "{}@{:.6},{:.6}",
            hit.kind.as_str(),
            hit.center.x(),
            hit.center.y()
        )
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index = AirmailIndex::open_read_only(&args[0])?;
    let per_kind: usize = args[1].parse()?;
    let seed: u64 = args.get(2).map_or(Ok(1), |s| s.parse())?;
    let docs = index.searcher()?.num_docs();
    // Dense enough that a regional index's postcodes (about 0.1% of it) give `per kind`.
    let stride = u32::try_from((docs / (per_kind as u64 * 800)).max(1)).unwrap_or(u32::MAX);
    let (hits, admins) = airmail::tagger_data::sample(&index, stride, 0)?;
    let mut by_kind: BTreeMap<&str, Vec<&SearchHit>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for hit in &hits {
        let key = (
            hit.kind.as_str(),
            hit.center.x().to_bits(),
            hit.center.y().to_bits(),
            hit.tags.len(),
        );
        if seen.insert(key) {
            by_kind.entry(hit.kind.as_str()).or_default().push(hit);
        }
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    for (kind, mut hits) in by_kind {
        eprintln!(
            "{kind}: {} sampled, taking {}",
            hits.len(),
            per_kind.min(hits.len())
        );
        hits.shuffle(&mut rng);
        for hit in hits.into_iter().take(per_kind) {
            let containing: Vec<&WofPlace> = one_per_placetype(
                hit.admin_ids
                    .iter()
                    .filter_map(|id| admins.get(id))
                    .collect(),
            );
            let country = hit
                .wof
                .as_ref()
                .and_then(|w| w.country.clone())
                .or_else(|| containing.iter().find_map(|p| p.country.clone()));
            let name = match hit.kind {
                Kind::Admin => hit
                    .wof
                    .as_ref()
                    .map(|w| w.display_name(Some("en")).to_string()),
                Kind::Postcode => hit.tag("postal_code").map(str::to_string),
                Kind::Poi | Kind::Street => {
                    osm_display_name(&hit.tags, Some("en")).map(str::to_string)
                }
                Kind::Address => None,
            };
            let tags: BTreeMap<&str, &str> = hit
                .tags
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let line = json!({
                "kind": hit.kind.as_str(),
                "id": id(hit, country.as_deref()),
                "name": name,
                "housenumber": hit.tag("addr:housenumber"),
                "street": hit.tag("addr:street"),
                "unit": hit.tag("addr:unit"),
                "postcode": hit.tag("addr:postcode"),
                "center": [hit.center.x(), hit.center.y()],
                "importance": hit.importance,
                "placetype": hit.wof.as_ref().map(|w| w.placetype.clone()),
                "admins": containing.iter().map(|p| json!({"placetype": p.placetype, "name": p.display_name(Some("en")), "id": p.id})).collect::<Vec<_>>(),
                "tags": tags,
            });
            println!("{line}");
        }
    }
    Ok(())
}
