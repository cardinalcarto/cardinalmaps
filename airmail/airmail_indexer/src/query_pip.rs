use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use crossbeam::channel::Sender;
use futures_util::future::join_all;
use serde::Deserialize;

use airmail::poi::{AdminBand, is_containing_admin};

use crate::{
    COUNTRIES,
    cache::{AdminInfo, IndexerCache, WofCacheItem},
    pip_tree::PipTree,
    wof::{ConcisePipResponse, PipLangsResponse, WhosOnFirst},
};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PipResponse {
    pub admin_ids: Vec<u64>,
    /// With the band of each area's placetype.
    pub admin_names: Vec<(AdminBand, String)>,
    pub admin_langs: Vec<String>,
    /// The ISO 3166-1 alpha-2 code of the country.
    pub country_code: Option<String>,
}

/// Countries' ISO codes by Who's On First ID, as they're looked up.
static COUNTRY_CODES: LazyLock<Mutex<HashMap<u64, Option<String>>>> = LazyLock::new(Mutex::default);

async fn country_code(country_id: u64, wof_db: &WhosOnFirst) -> Option<String> {
    if let Some(code) = COUNTRY_CODES.lock().expect("country codes").get(&country_id) {
        return code.clone();
    }
    let code = wof_db.country_code(country_id).await.ok().flatten();
    COUNTRY_CODES.lock().expect("country codes").insert(country_id, code.clone());
    code
}

struct AdminIds {
    all_admin_ids: Vec<u64>,
    country: Option<u64>,
}

async fn query_pip_inner(
    s2cell: u64,
    indexer_cache: &IndexerCache,
    to_cache_sender: Sender<WofCacheItem>,
    wof_db: &WhosOnFirst,
    pip_tree: &Option<PipTree<ConcisePipResponse>>,
) -> Result<AdminIds> {
    let desired_level = 15;
    let cell = s2::cellid::CellID(s2cell);
    let cell = if cell.level() > desired_level {
        cell.parent(desired_level)
    } else {
        cell
    };

    let ids = indexer_cache.query_area(cell.0)?;
    if !ids.is_empty() {
        let country = ids.iter().find(|id| COUNTRIES.contains(*id)).cloned();
        return Ok(AdminIds {
            all_admin_ids: ids,
            country,
        });
    }

    let lat_lng = s2::latlng::LatLng::from(cell);
    let lat = lat_lng.lat.deg();
    let lng = lat_lng.lng.deg();

    // Prefer the pip_tree, if in use
    let response = if let Some(pip_tree) = pip_tree {
        pip_tree.point_in_polygon(lng, lat).await?
    } else {
        wof_db.point_in_polygon(lng, lat).await?
    };

    let mut response_ids = Vec::new();
    for concise_response in response {
        let admin_id: u64 = concise_response.id.parse()?;

        // These filters are applied in SQL, to reduce allocations
        // if concise_response.r#type == "planet"
        //     || concise_response.r#type == "marketarea"
        //     || concise_response.r#type == "county"
        //     || concise_response.r#type == "timezone"
        // {
        //     continue;
        // }
        response_ids.push(admin_id);
    }

    {
        to_cache_sender
            .send(WofCacheItem::Admins(cell.0, response_ids.clone()))
            .unwrap();
    }

    Ok(AdminIds {
        country: response_ids
            .iter()
            .find(|id| COUNTRIES.contains(id))
            .cloned(),
        all_admin_ids: response_ids,
    })
}

/// An admin's placetype and names, from the cache or the database.
async fn admin_info(
    admin_id: u64,
    indexer_cache: &IndexerCache,
    to_cache_sender: &Sender<WofCacheItem>,
    wof_db: &WhosOnFirst,
) -> Result<AdminInfo> {
    if let Some(info) = indexer_cache.query_admin_info(admin_id)? {
        return Ok(info);
    }
    let info = AdminInfo {
        placetype: wof_db.placetype(admin_id).await?.unwrap_or_default(),
        names: wof_db.names(admin_id).await?,
    };
    to_cache_sender.send(WofCacheItem::AdminInfo(admin_id, info.clone())).unwrap();
    Ok(info)
}

/// The ancestors of an admin, from the cache or the database.
async fn ancestors(
    admin_id: u64,
    indexer_cache: &IndexerCache,
    to_cache_sender: &Sender<WofCacheItem>,
    wof_db: &WhosOnFirst,
) -> Result<Vec<u64>> {
    if let Some(ancestors) = indexer_cache.query_ancestors(admin_id)? {
        return Ok(ancestors);
    }
    let ancestors = wof_db.ancestors(admin_id).await?;
    to_cache_sender.send(WofCacheItem::Ancestors(admin_id, ancestors.clone())).unwrap();
    Ok(ancestors)
}

async fn query_langs(country_id: u64, wof_db: &WhosOnFirst) -> Option<(u64, Vec<String>)> {
    let response: PipLangsResponse = wof_db.properties_for_id(country_id).await.ok()?.into();
    let langs: Vec<String> = response
        .langs
        .map(|langs| langs.split(',').map(|s| s.to_string()).collect())?;

    Some((country_id, langs))
}

/// The admins containing a point, with the names a POI there is searched
/// by: every name in the languages spoken there, English, and the default
/// names, abbreviations included.
pub(crate) async fn query_pip(
    indexer_cache: &IndexerCache,
    to_cache_sender: Sender<WofCacheItem>,
    s2cell: u64,
    wof_db: &WhosOnFirst,
    pip_tree: &Option<PipTree<ConcisePipResponse>>,
) -> Result<PipResponse> {
    let wof_ids = query_pip_inner(s2cell, indexer_cache, to_cache_sender.clone(), wof_db, pip_tree).await?;

    // The point-in-polygon lookup skips some placetypes, such as counties,
    // which the hierarchy has.
    let mut admin_ids: BTreeSet<u64> = wof_ids.all_admin_ids.iter().copied().collect();
    let ancestor_lists = join_all(
        wof_ids.all_admin_ids.iter().map(|id| ancestors(*id, indexer_cache, &to_cache_sender, wof_db)),
    )
    .await;
    for list in ancestor_lists {
        admin_ids.extend(list?);
    }
    let country = wof_ids.country.or_else(|| admin_ids.iter().find(|id| COUNTRIES.contains(*id)).copied());

    let mut response = PipResponse::default();
    if let Some(country_id) = country {
        response.country_code = country_code(country_id, wof_db).await;
        if let Ok(Some(langs)) = indexer_cache.query_languages_cache(country_id) {
            response.admin_langs = langs;
        } else if let Some((_, langs)) = query_langs(country_id, wof_db).await {
            to_cache_sender.send(WofCacheItem::Langs(country_id, langs.clone())).unwrap();
            response.admin_langs = langs;
        }
    }

    let infos = join_all(admin_ids.iter().map(|id| admin_info(*id, indexer_cache, &to_cache_sender, wof_db))).await;
    let mut admin_names = BTreeSet::new();
    for (id, info) in admin_ids.iter().zip(infos) {
        let info = info?;
        if !is_containing_admin(&info.placetype) {
            continue;
        }
        response.admin_ids.push(*id);
        let Some(band) = AdminBand::of(&info.placetype) else { continue };
        for name in info.names {
            if name.is_searchable()
                && (name.lang == "und" || name.lang == "eng" || response.admin_langs.contains(&name.lang))
            {
                admin_names.insert((band, name.name));
            }
        }
    }
    response.admin_names = admin_names.into_iter().collect();
    Ok(response)
}
