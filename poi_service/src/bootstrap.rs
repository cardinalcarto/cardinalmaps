//! Loading every POI from a planet file into an empty database.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use osmpbf::{BlobDecode, BlobReader, Element};
use rayon::prelude::*;
use sqlx::PgPool;
use tokio::sync::mpsc;

use crate::db::{self, Checkpoint};
use crate::identity::{Checksum, VersionRow};
use crate::osm::{ElementKey, OsmType, poi_tags};
use crate::replication::Server;
use crate::rules::{RULESET_VERSION, Tags};

/// Blobs decoded in parallel at a time.
const BLOBS_PER_CHUNK: usize = 256;

struct PlanetPoi {
    key: ElementKey,
    version: i32,
    tags: Tags,
}

pub async fn bootstrap(pool: &PgPool, planet: &Path, server: &Server, replication_url: &str) -> Result<()> {
    if db::origin(pool).await?.is_some() {
        bail!("the database is already bootstrapped");
    }
    let planet_timestamp = replication_timestamp(planet)?;
    let sequence = server.sequence_at(planet_timestamp).await?;
    tracing::info!("planet is at {planet_timestamp}; diffs start after sequence {sequence}");

    let (sender, mut receiver) = mpsc::channel(4);
    let path = planet.to_owned();
    let reader = tokio::task::spawn_blocking(move || read_pois(path, sender));

    let mut tx = pool.begin().await?;
    let mut copy = tx.copy_in_raw("COPY pois (osm_type, osm_id, version, poi_id, active, tags) FROM STDIN").await?;
    let mut checksum = Checksum::new(&[0; 32]);
    let mut previous: Option<ElementKey> = None;
    let mut next_poi_id = 1;
    while let Some(pois) = receiver.recv().await {
        let mut buf = String::new();
        for poi in pois {
            // Planet files are sorted by type then ID; identifiers rely on it.
            if previous.is_some_and(|p| p >= poi.key) {
                bail!("planet isn't sorted: {:?} follows {previous:?}", poi.key);
            }
            previous = Some(poi.key);
            checksum.add(&VersionRow { key: poi.key, version: poi.version, poi_id: next_poi_id });
            let tags = serde_json::to_string(&poi.tags)?;
            buf.push_str(&format!(
                "{}\t{}\t{}\t{next_poi_id}\tt\t{}\n",
                poi.key.osm_type.as_str(),
                poi.key.osm_id,
                poi.version,
                copy_escape(&tags)
            ));
            next_poi_id += 1;
            if next_poi_id % 1_000_000 == 0 {
                tracing::info!("{} POIs, at {:?}", next_poi_id - 1, poi.key);
            }
        }
        copy.send(buf.into_bytes()).await?;
    }
    reader.await??;
    copy.finish().await?;
    tracing::info!("loaded {} POIs; recording versions", next_poi_id - 1);

    sqlx::query("INSERT INTO poi_versions SELECT osm_type, osm_id, version, poi_id FROM pois").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO origin (planet_timestamp, ruleset, replication_url) VALUES ($1, $2, $3)")
        .bind(planet_timestamp)
        .bind(RULESET_VERSION)
        .bind(replication_url)
        .execute(&mut *tx)
        .await?;
    let checkpoint = Checkpoint { sequence, timestamp: planet_timestamp, next_poi_id, checksum: checksum.finish() };
    db::insert_checkpoint(&mut tx, &checkpoint).await?;
    tx.commit().await?;
    tracing::info!("bootstrapped; checksum {}", hex::encode(&checkpoint.checksum));
    Ok(())
}

pub fn replication_timestamp(planet: &Path) -> Result<DateTime<Utc>> {
    let mut reader = BlobReader::from_path(planet).with_context(|| format!("opening {}", planet.display()))?;
    let header = reader.next().context("empty planet file")??.to_headerblock()?;
    let seconds = header.osmosis_replication_timestamp().context("planet header has no replication timestamp")?;
    DateTime::from_timestamp(seconds, 0).context("invalid replication timestamp")
}

/// Sends the planet's POIs to `sender` in file order.
fn read_pois(path: PathBuf, sender: mpsc::Sender<Vec<PlanetPoi>>) -> Result<()> {
    let mut blobs = BlobReader::from_path(&path)?;
    loop {
        let chunk = blobs.by_ref().take(BLOBS_PER_CHUNK).collect::<Result<Vec<_>, _>>()?;
        if chunk.is_empty() {
            return Ok(());
        }
        let decoded = chunk
            .par_iter()
            .map(|blob| match blob.decode()? {
                BlobDecode::OsmData(block) => block_pois(&block),
                _ => Ok(Vec::new()),
            })
            .collect::<Result<Vec<_>>>()?;
        for pois in decoded {
            if sender.blocking_send(pois).is_err() {
                return Ok(()); // The writer failed and will report why.
            }
        }
    }
}

fn block_pois(block: &osmpbf::PrimitiveBlock) -> Result<Vec<PlanetPoi>> {
    let mut pois = Vec::new();
    for element in block.elements() {
        let (osm_type, osm_id, tags, version) = match &element {
            Element::DenseNode(n) => {
                let Some(tags) = poi_tags(n.tags()) else { continue };
                (OsmType::Node, n.id(), tags, n.info().map(|i| i.version()))
            }
            Element::Node(n) => {
                let Some(tags) = poi_tags(n.tags()) else { continue };
                (OsmType::Node, n.id(), tags, n.info().version())
            }
            Element::Way(w) => {
                let Some(tags) = poi_tags(w.tags()) else { continue };
                (OsmType::Way, w.id(), tags, w.info().version())
            }
            Element::Relation(r) => {
                let Some(tags) = poi_tags(r.tags()) else { continue };
                (OsmType::Relation, r.id(), tags, r.info().version())
            }
        };
        let key = ElementKey { osm_type, osm_id };
        let Some(version) = version.filter(|&v| v > 0) else {
            bail!("{key:?} has no version; the planet must include element metadata");
        };
        pois.push(PlanetPoi { key, version, tags });
    }
    Ok(pois)
}

/// Escapes a value for PostgreSQL's COPY text format.
fn copy_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}
