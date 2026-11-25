//! Indexes Who's On First places as administrative areas.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use airmail::{
    index::AirmailIndexWriter,
    poi::{SchemafiedPoi, ToIndexPoi},
    wof::{WofRecord, admin_document, country, place},
};
use anyhow::Result;
use futures_util::{StreamExt, stream};
use geo::Geometry;
use log::{info, warn};

use crate::wof::WhosOnFirst;

/// Ancestors and spoken languages of places that contain others, which many
/// places share.
#[derive(Default)]
struct Memo {
    /// With their placetypes, names and populations.
    ancestors: Mutex<HashMap<u64, Arc<WofRecord>>>,
    languages: Mutex<HashMap<u64, Arc<Vec<String>>>>,
}

impl Memo {
    async fn ancestor(&self, wof: &WhosOnFirst, id: u64) -> Result<Arc<WofRecord>> {
        if let Some(ancestor) = self.ancestors.lock().unwrap().get(&id) {
            return Ok(ancestor.clone());
        }
        let properties = wof
            .properties(id, &["wof:population", "wof:population_rank"])
            .await?;
        let ancestor = Arc::new(WofRecord {
            id,
            placetype: wof.placetype(id).await?.unwrap_or_default(),
            names: wof.names(id).await?,
            population: population(&properties),
            population_rank: population_rank(&properties),
            ..WofRecord::default()
        });
        self.ancestors.lock().unwrap().insert(id, ancestor.clone());
        Ok(ancestor)
    }

    async fn languages(&self, wof: &WhosOnFirst, country: u64) -> Result<Arc<Vec<String>>> {
        if let Some(languages) = self.languages.lock().unwrap().get(&country) {
            return Ok(languages.clone());
        }
        let properties = wof.properties(country, &["wof:lang_x_spoken"]).await?;
        let languages: Vec<String> = properties
            .get("wof:lang_x_spoken")
            .map(|langs| langs.split(',').map(|l| l.trim().to_string()).collect())
            .unwrap_or_default();
        let languages = Arc::new(languages);
        self.languages
            .lock()
            .unwrap()
            .insert(country, languages.clone());
        Ok(languages)
    }
}

/// Indexes every place in the database that's a search result, returning
/// how many were indexed.
pub async fn load_wof(wof: &WhosOnFirst, writer: &mut AirmailIndexWriter) -> Result<usize> {
    let places = wof.searchable_places().await?;
    info!("Indexing {} Who's On First places", places.len());
    let memo = Arc::new(Memo::default());
    let concurrency = num_cpus::get() * 2;
    let mut admins = stream::iter(places)
        .map(|(id, placetype)| {
            let wof = wof.clone();
            let memo = memo.clone();
            tokio::spawn(async move { (id, admin(&wof, &memo, id, placetype).await) })
        })
        .buffer_unordered(concurrency);

    let mut count = 0;
    let start = std::time::Instant::now();
    while let Some(joined) = admins.next().await {
        match joined? {
            (_, Ok(Some(admin))) => {
                writer.add_poi(SchemafiedPoi::from(admin), "wof")?;
                count += 1;
                if count % 100_000 == 0 {
                    info!(
                        "{count} places indexed in {} seconds",
                        start.elapsed().as_secs()
                    );
                }
            }
            (_, Ok(None)) => {}
            (id, Err(e)) => warn!("Failed to index Who's On First place {id}: {e}"),
        }
    }
    info!("Indexed {count} Who's On First places");
    Ok(count)
}

/// A place as a document, or `None` if it has no name or no location (see
/// `airmail::wof::admin_document`).
async fn admin(
    wof: &WhosOnFirst,
    memo: &Memo,
    id: u64,
    placetype: String,
) -> Result<Option<ToIndexPoi>> {
    let names = wof.names(id).await?;
    if place(id, &placetype, &names, None, None).is_none() {
        return Ok(None);
    }
    let ancestors = wof.ancestors(id).await?;
    // SpatiaLite databases have `wof:country` as `alpha2`.
    let properties = wof
        .properties(
            id,
            &[
                "wof:population",
                "wof:population_rank",
                "alpha2",
                "wof:country",
            ],
        )
        .await?;
    let mut geometries = wof.geometries(id).await?;
    let label = ["label_position", "reversegeo_position", "centroid"]
        .into_iter()
        .find_map(|role| match geometries.get(role) {
            Some(Geometry::Point(point)) => Some(*point),
            _ => None,
        });
    let boundary = geometries.remove("boundary");
    if boundary.is_none() && label.is_none() {
        return Ok(None);
    }
    let languages = match country(std::iter::once(&id).chain(&ancestors)) {
        Some(country) => memo.languages(wof, country).await?,
        None => Arc::new(Vec::new()),
    };
    let mut known = HashMap::new();
    for ancestor in &ancestors {
        known.insert(*ancestor, memo.ancestor(wof, *ancestor).await?);
    }
    let record = WofRecord {
        id,
        placetype,
        names,
        ancestors,
        population: population(&properties),
        population_rank: population_rank(&properties),
        country: properties
            .get("alpha2")
            .or_else(|| properties.get("wof:country"))
            .cloned(),
        languages: Vec::new(),
        label,
        boundary,
    };
    Ok(admin_document(
        &record,
        |ancestor| known.get(&ancestor).map(|a| &**a),
        &languages,
    ))
}

/// A place's `wof:population`, which Who's On First sometimes writes as a decimal.
fn population(properties: &HashMap<String, String>) -> Option<u64> {
    properties
        .get("wof:population")
        .and_then(|p| p.parse::<f64>().ok())
        .map(|p| p as u64)
}

fn population_rank(properties: &HashMap<String, String>) -> Option<u32> {
    properties
        .get("wof:population_rank")
        .and_then(|r| r.parse().ok())
}
