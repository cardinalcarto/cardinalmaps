use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, LazyLock, RwLock};

use anyhow::Result;
use futures_util::future::join_all;
use serde::Deserialize;

use airmail::poi::AdminBand;

use crate::{
    COUNTRIES,
    pip_tree::PipTree,
    wof::{ConcisePipResponse, PipLangsResponse, WhosOnFirst, WofName},
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

/// What indexing needs of an admin that contains places.
struct AdminInfo {
    placetype: String,
    names: Vec<WofName>,
}

/// What's looked up about an area, once each, by its Who's On First ID: few areas contain
/// very many places.
type Memo<T> = LazyLock<RwLock<HashMap<u64, T>>>;

static COUNTRY_CODES: Memo<Option<String>> = LazyLock::new(Default::default);
static ADMIN_INFO: Memo<Arc<AdminInfo>> = LazyLock::new(Default::default);
static ANCESTORS: Memo<Arc<Vec<u64>>> = LazyLock::new(Default::default);
static LANGUAGES: Memo<Arc<Vec<String>>> = LazyLock::new(Default::default);

async fn remembered<T: Clone>(memo: &Memo<T>, id: u64, look_up: impl Future<Output = Result<T>>) -> Result<T> {
    if let Some(value) = memo.read().expect("memo").get(&id) {
        return Ok(value.clone());
    }
    let value = look_up.await?;
    memo.write().expect("memo").insert(id, value.clone());
    Ok(value)
}

async fn country_code(country_id: u64, wof_db: &WhosOnFirst) -> Option<String> {
    remembered(&COUNTRY_CODES, country_id, async { Ok(wof_db.country_code(country_id).await.ok().flatten()) })
        .await
        .ok()
        .flatten()
}

async fn admin_info(admin_id: u64, wof_db: &WhosOnFirst) -> Result<Arc<AdminInfo>> {
    remembered(&ADMIN_INFO, admin_id, async {
        Ok(Arc::new(AdminInfo {
            placetype: wof_db.placetype(admin_id).await?.unwrap_or_default(),
            names: wof_db.names(admin_id).await?,
        }))
    })
    .await
}

async fn ancestors(admin_id: u64, wof_db: &WhosOnFirst) -> Result<Arc<Vec<u64>>> {
    remembered(&ANCESTORS, admin_id, async { Ok(Arc::new(wof_db.ancestors(admin_id).await?)) }).await
}

async fn languages(country_id: u64, wof_db: &WhosOnFirst) -> Arc<Vec<String>> {
    remembered(&LANGUAGES, country_id, async {
        Ok(Arc::new(query_langs(country_id, wof_db).await.map(|(_, langs)| langs).unwrap_or_default()))
    })
    .await
    .unwrap_or_default()
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
    lng: f64,
    lat: f64,
    wof_db: &WhosOnFirst,
    pip_tree: &Option<PipTree<ConcisePipResponse>>,
) -> Result<PipResponse> {
    let containing = if let Some(pip_tree) = pip_tree {
        pip_tree.point_in_polygon(lng, lat).await?
    } else {
        wof_db.point_in_polygon(lng, lat).await?
    };
    let containing = containing.iter().map(|area| Ok(area.id.parse()?)).collect::<Result<Vec<u64>>>()?;

    // The point-in-polygon lookup skips some placetypes, such as counties,
    // which the hierarchy has.
    let mut admin_ids: BTreeSet<u64> = containing.iter().copied().collect();
    let ancestor_lists = join_all(containing.iter().map(|id| ancestors(*id, wof_db))).await;
    for list in ancestor_lists {
        admin_ids.extend(list?.iter().copied());
    }
    let country = containing
        .iter()
        .find(|id| COUNTRIES.contains(*id))
        .or_else(|| admin_ids.iter().find(|id| COUNTRIES.contains(*id)))
        .copied();

    let mut response = PipResponse::default();
    if let Some(country_id) = country {
        response.country_code = country_code(country_id, wof_db).await;
        response.admin_langs = languages(country_id, wof_db).await.to_vec();
    }

    let infos = join_all(admin_ids.iter().map(|id| admin_info(*id, wof_db))).await.into_iter().collect::<Result<Vec<_>>>()?;
    let areas = admin_ids.iter().zip(&infos).map(|(id, info)| (*id, info.placetype.as_str(), info.names.as_slice()));
    (response.admin_ids, response.admin_names) = airmail::wof::containing(areas, &response.admin_langs);
    Ok(response)
}
