use airmail::{
    geometry, importance,
    index::{AirmailIndex, AirmailIndexWriter},
    poi::{Kind, SchemafiedPoi, ToIndexPoi},
};
use s2::{cellid::CellID, latlng::LatLng};
use anyhow::Result;
use crossbeam::channel::{Receiver, Sender};
use futures_util::future::join_all;
use lingua::{IsoCode639_3, Language};
use log::{info, trace, warn};
use std::{
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};
use tokio::{
    spawn,
    task::{JoinHandle, spawn_blocking},
};

use crate::{
    cache::{IndexerCache, WofCacheItem},
    postcodes::Postcodes,
    pip_tree::PipTree,
    qrank::QRank,
    query_pip,
    wof::{ConcisePipResponse, WhosOnFirst},
};

/// A POI and the ISO 639-3 codes of the languages spoken where it is.
type WithLanguages = (ToIndexPoi, Vec<String>);

/// Where imported POIs go, once their admin areas and languages are known.
pub trait PoiSink: Send + 'static {
    /// `languages` are the ISO 639-3 codes Who's On First lists as spoken
    /// where the POI is.
    fn add(&mut self, poi: ToIndexPoi, languages: &[String]) -> Result<()>;

    fn finish(self) -> Result<()>;
}

/// Writes POIs into an Airmail index, and the postcodes of their addresses
/// once they all are.
pub struct IndexSink {
    writer: AirmailIndexWriter,
    source: String,
    postcodes: Postcodes,
}

impl IndexSink {
    pub fn new(index_path: &Path, source: &str) -> Result<Self> {
        let mut index = AirmailIndex::create(index_path)?;
        let mut writer = index.writer()?;
        writer.delete_source(source);
        Ok(Self {
            writer,
            source: source.to_string(),
            postcodes: Postcodes::default(),
        })
    }
}

impl PoiSink for IndexSink {
    fn add(&mut self, poi: ToIndexPoi, _languages: &[String]) -> Result<()> {
        self.postcodes.add(&poi);
        self.writer.add_poi(SchemafiedPoi::from(poi), &self.source)
    }

    fn finish(mut self) -> Result<()> {
        info!("Indexing {} postcodes", self.postcodes.len());
        for poi in std::mem::take(&mut self.postcodes).into_pois() {
            self.writer.add_poi(SchemafiedPoi::from(poi), &self.source)?;
        }
        self.writer.commit()
    }
}

pub struct ImporterBuilder {
    admin_cache_path: Option<PathBuf>,
    wof_db_path: PathBuf,
    pip_tree_path: Option<PathBuf>,
    qrank_path: Option<PathBuf>,
}

impl ImporterBuilder {
    pub fn new(wof_db_path: &Path) -> Result<Self> {
        Ok(Self {
            admin_cache_path: None,
            wof_db_path: wof_db_path.to_path_buf(),
            pip_tree_path: None,
            qrank_path: None,
        })
    }

    /// QRank, for the importance of POIs with Wikidata entities.
    pub fn qrank(mut self, qrank: &Path) -> Self {
        self.qrank_path = Some(qrank.to_path_buf());
        self
    }

    pub fn admin_cache(mut self, admin_cache: &Path) -> Self {
        self.admin_cache_path = Some(admin_cache.to_path_buf());
        self
    }

    pub fn pip_tree_cache(mut self, pip_tree_cache: &Path) -> Self {
        self.pip_tree_path = Some(pip_tree_cache.to_path_buf());
        self
    }

    pub async fn build(self) -> Result<Importer> {
        let admin_cache_path = if let Some(admin_cache) = self.admin_cache_path {
            admin_cache
        } else {
            std::env::temp_dir().join("admin_cache.db")
        };

        let admin_cache = IndexerCache::new(&admin_cache_path)?;

        let wof_db = WhosOnFirst::new(&self.wof_db_path).await?;

        let pip_tree = if let Some(pip_tree_cache) = self.pip_tree_path {
            Some(PipTree::new_or_load(&wof_db, &pip_tree_cache).await?)
        } else {
            None
        };

        let qrank = self.qrank_path.map(|path| QRank::load(&path)).transpose()?.map(Arc::new);

        Importer::new(admin_cache, wof_db, pip_tree, qrank).await
    }
}

pub struct Importer {
    indexer_cache: Arc<IndexerCache>,
    wof_db: WhosOnFirst,
    pip_tree: Option<PipTree<ConcisePipResponse>>,
    qrank: Option<Arc<QRank>>,
}

impl Importer {
    pub async fn new(
        indexer_cache: IndexerCache,
        wof_db: WhosOnFirst,
        pip_tree: Option<PipTree<ConcisePipResponse>>,
        qrank: Option<Arc<QRank>>,
    ) -> Result<Self> {
        Ok(Self {
            indexer_cache: Arc::new(indexer_cache),
            wof_db,
            pip_tree,
            qrank,
        })
    }

    pub fn wof_db(&self) -> &WhosOnFirst {
        &self.wof_db
    }

    pub async fn run_import(
        self,
        receiver: Receiver<ToIndexPoi>,
        mut sink: impl PoiSink,
    ) -> Result<()> {
        let (to_cache_sender, to_cache_receiver): (Sender<WofCacheItem>, Receiver<WofCacheItem>) =
            crossbeam::channel::bounded(1024);
        let (to_index_sender, to_index_receiver): (Sender<WithLanguages>, Receiver<WithLanguages>) =
            crossbeam::channel::bounded(1024);
        let mut handles: Vec<JoinHandle<Result<()>>> = vec![];

        // Listen for items to cache
        let admin_cache = self.indexer_cache.clone();
        handles.push(spawn_blocking(move || {
            while let Ok(cache_item) = to_cache_receiver.recv() {
                admin_cache.buffered_write_item(cache_item)?;
            }
            Ok(())
        }));

        // Listen for items to write
        handles.push(spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut count = 0;
            loop {
                {
                    count += 1;
                    if count % 10000 == 0 {
                        info!(
                            "{} POIs parsed in {} seconds, {} per second.",
                            count,
                            start.elapsed().as_secs(),
                            count as f64 / start.elapsed().as_secs_f64(),
                        );
                    }
                }

                if let Ok((poi, languages)) = to_index_receiver.recv() {
                    if let Err(err) = sink.add(poi, &languages) {
                        warn!("Failed to write POI. {}", err);
                    }
                } else {
                    break;
                }
            }
            sink.finish()?;

            Ok(())
        }));

        // Spawn processing workers
        for _ in 0..num_cpus::get() {
            let no_admin_receiver = receiver.clone();
            let to_index_sender = to_index_sender.clone();
            let to_cache_sender = to_cache_sender.clone();
            let indexer_cache = self.indexer_cache.clone();
            let wof_db = self.wof_db.clone();
            let pip_tree = self.pip_tree.clone();
            let qrank = self.qrank.clone();

            handles.push(spawn(async move {
                let mut counter = 0;
                while let Ok(poi) = no_admin_receiver.recv() {
                    counter += 1;
                    if counter % 1000 == 0 {
                        trace!(
                            "Cache queue, index queue: {}, {}",
                            to_cache_sender.len(),
                            to_index_sender.len()
                        );
                    }

                    match Self::populate_admin_areas(
                        poi,
                        &indexer_cache,
                        to_cache_sender.clone(),
                        &wof_db,
                        &pip_tree,
                        qrank.as_deref(),
                    )
                    .await
                    {
                        Ok(poi_and_languages) => {
                            to_index_sender.send(poi_and_languages).unwrap();
                        }
                        Err(err) => {
                            warn!("Failed to populate admin areas, {}", err);
                        }
                    }
                }

                Ok(())
            }));
        }
        drop(to_index_sender);
        drop(to_cache_sender);

        trace!("Waiting for indexing to finish");
        join_all(handles).await;
        info!("Indexing complete");

        Ok(())
    }

    pub fn indexer_cache(&self) -> Arc<IndexerCache> {
        self.indexer_cache.clone()
    }

    async fn populate_admin_areas(
        mut poi: ToIndexPoi,
        indexer_cache: &IndexerCache,
        to_cache_sender: Sender<WofCacheItem>,
        wof_db: &WhosOnFirst,
        pip_tree: &Option<PipTree<ConcisePipResponse>>,
        qrank: Option<&QRank>,
    ) -> Result<WithLanguages> {
        let center = CellID::from(LatLng::from_degrees(poi.center.y(), poi.center.x()));
        let pip_response =
            query_pip::query_pip(indexer_cache, to_cache_sender, center.0, wof_db, pip_tree)
                .await?;
        poi.admin_ids = pip_response.admin_ids;
        poi.admin_names = pip_response.admin_names;
        poi.country_code = pip_response.country_code;
        poi.importance = match poi.kind {
            Kind::Address => importance::ADDRESS_IMPORTANCE,
            Kind::Postcode => importance::POSTCODE_IMPORTANCE,
            Kind::Street => importance::street_importance(poi.tag("highway")),
            _ => {
                let wikidata = poi.tag("wikidata");
                importance::poi_importance(importance::PoiSignals {
                    has_wikidata: wikidata.is_some(),
                    qrank: wikidata.zip(qrank).and_then(|(id, qrank)| qrank.get(id)),
                    area_m2: geometry::area_m2(&poi.geometry),
                    transit_stop: importance::is_transit_stop(&poi.tags),
                })
            }
        };
        for lang in &pip_response.admin_langs {
            if let Ok(iso) = IsoCode639_3::from_str(lang) {
                poi.languages.push(Language::from_iso_code_639_3(&iso))
            }
        }

        Ok((poi, pip_response.admin_langs))
    }
}
