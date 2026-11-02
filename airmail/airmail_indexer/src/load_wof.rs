//! Indexes Who's On First places as administrative areas.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use airmail::{
    importance,
    index::AirmailIndexWriter,
    poi::{Kind, SchemafiedPoi, ToIndexPoi, WofPlace, is_containing_admin},
};
use anyhow::Result;
use futures_util::{StreamExt, stream};
use geo::Geometry;
use log::{info, warn};

use crate::COUNTRIES;
use crate::wof::{WhosOnFirst, WofName};

/// What indexing a place needs of a place containing it.
struct Ancestor {
    placetype: String,
    names: Vec<WofName>,
    importance: f64,
}

/// Ancestors and spoken languages of places that contain others, which many
/// places share.
#[derive(Default)]
struct Memo {
    ancestors: Mutex<HashMap<u64, Arc<Ancestor>>>,
    languages: Mutex<HashMap<u64, Arc<Vec<String>>>>,
}

impl Memo {
    async fn ancestor(&self, wof: &WhosOnFirst, id: u64) -> Result<Arc<Ancestor>> {
        if let Some(ancestor) = self.ancestors.lock().unwrap().get(&id) {
            return Ok(ancestor.clone());
        }
        let placetype = wof.placetype(id).await?.unwrap_or_default();
        let properties = wof.properties(id, &["wof:population_rank"]).await?;
        let rank = properties.get("wof:population_rank").and_then(|r| r.parse().ok());
        let ancestor = Arc::new(Ancestor {
            importance: importance::admin_importance(&placetype, rank),
            placetype,
            names: wof.names(id).await?,
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
        self.languages.lock().unwrap().insert(country, languages.clone());
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
                    info!("{count} places indexed in {} seconds", start.elapsed().as_secs());
                }
            }
            (_, Ok(None)) => {}
            (id, Err(e)) => warn!("Failed to index Who's On First place {id}: {e}"),
        }
    }
    info!("Indexed {count} Who's On First places");
    Ok(count)
}

/// A place as a document, or `None` if it has no name or no location.
async fn admin(wof: &WhosOnFirst, memo: &Memo, id: u64, placetype: String) -> Result<Option<ToIndexPoi>> {
    let names = wof.names(id).await?;
    if place(id, &placetype, &names, None).is_none() {
        return Ok(None);
    }
    let ancestors = wof.ancestors(id).await?;
    let properties = wof.properties(id, &["wof:population", "wof:population_rank"]).await?;
    let mut geometries = wof.geometries(id).await?;

    // Its shape if it has one, else its label point.
    let label = ["label_position", "reversegeo_position", "centroid"]
        .into_iter()
        .find_map(|role| match geometries.get(role) {
            Some(Geometry::Point(point)) => Some(*point),
            _ => None,
        });
    let shape = geometries.remove("boundary").or_else(|| label.map(Geometry::Point));
    let Some(shape) = shape else {
        return Ok(None);
    };

    // It's searched by the names of the places containing it in the
    // languages spoken there, English, and their default names.
    let country = std::iter::once(&id).chain(&ancestors).find(|a| COUNTRIES.contains(*a)).copied();
    let languages = match country {
        Some(country) => memo.languages(wof, country).await?,
        None => Arc::new(Vec::new()),
    };
    let mut admin_names = BTreeSet::new();
    let mut containing = Vec::new();
    let mut city_importance: f64 = 0.0;
    for id in &ancestors {
        let ancestor = memo.ancestor(wof, *id).await?;
        if !is_containing_admin(&ancestor.placetype) {
            continue;
        }
        containing.push(*id);
        if ancestor.placetype == "locality" {
            city_importance = city_importance.max(ancestor.importance);
        }
        for name in &ancestor.names {
            if name.is_searchable() && (name.lang == "und" || name.lang == "eng" || languages.contains(&name.lang)) {
                admin_names.insert(name.name.clone());
            }
        }
    }

    let searchable: BTreeSet<String> = names.iter().filter(|n| n.is_searchable()).map(|n| n.name.clone()).collect();
    let Some(mut doc) = ToIndexPoi::new(Kind::Admin, searchable.into_iter().collect(), shape, Vec::new()) else {
        return Ok(None);
    };
    if let Some(label) = label {
        doc.center = label;
    }
    let population_rank = properties.get("wof:population_rank").and_then(|r| r.parse().ok());
    doc.importance = importance::admin_importance(&placetype, population_rank);
    if importance::is_neighbourhood(&placetype) {
        doc.importance = importance::neighbourhood_importance(doc.importance, city_importance);
    }
    doc.admin_ids = containing;
    doc.admin_names = admin_names.into_iter().collect();
    let population = properties.get("wof:population").and_then(|p| p.parse::<f64>().ok()).map(|p| p as u64);
    doc.wof = place(id, &placetype, &names, population);
    Ok(Some(doc))
}

/// What results show of a place, from its names, or `None` if it has none.
fn place(id: u64, placetype: &str, names: &[WofName], population: Option<u64>) -> Option<WofPlace> {
    let full = || names.iter().filter(|n| !n.abbr);
    let default_name = full()
        .find(|n| n.lang == "und" && n.tag == "default")
        .or_else(|| full().find(|n| n.lang == "eng" && n.tag == "preferred"))
        .or_else(|| full().next())?
        .name
        .clone();
    let mut preferred = BTreeMap::new();
    for name in full().filter(|n| n.tag == "preferred") {
        preferred.entry(name.lang.clone()).or_insert_with(|| name.name.clone());
    }
    let abbreviation = names
        .iter()
        .filter(|n| n.abbr)
        .min_by_key(|n| match n.lang.as_str() {
            "eng" => 0,
            "und" => 1,
            _ => 2,
        })
        .map(|n| n.name.clone());
    Some(WofPlace {
        id,
        placetype: placetype.to_string(),
        population,
        default_name,
        names: preferred,
        abbreviation,
    })
}

#[cfg(test)]
mod test {
    use super::*;

    fn name(lang: &str, tag: &str, abbr: bool, name: &str) -> WofName {
        WofName { lang: lang.into(), tag: tag.into(), abbr, name: name.into() }
    }

    #[test]
    fn place_picks_default_preferred_and_abbreviation() {
        let names = [
            name("und", "default", false, "Zürich"),
            name("und", "default", true, "ZH"),
            name("eng", "preferred", false, "Zurich"),
            name("eng", "variant", false, "ZRH"),
            name("deu", "preferred", false, "Zürich"),
        ];
        let place = place(85682309, "region", &names, Some(1_500_000)).unwrap();
        assert_eq!(place.default_name, "Zürich");
        assert_eq!(place.names.get("eng").map(String::as_str), Some("Zurich"));
        assert_eq!(place.abbreviation.as_deref(), Some("ZH"));
        assert!(!place.names.values().any(|n| n == "ZRH"));
    }

    #[test]
    fn place_needs_a_name() {
        assert!(place(1, "locality", &[], None).is_none());
    }
}
