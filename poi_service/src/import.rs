//! Reading published releases: restoring a database from them, and building
//! what `patch`, `covers` and the geocoder need without one.

use std::fmt::Write as _;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use sqlx::PgPool;

use crate::bootstrap::copy_escape;
use crate::db::{self, Checkpoint};
use crate::patch::PoiIds;
use crate::published::{Releases, Snapshot};
use crate::tables::Status;

/// Rows sent to PostgreSQL per COPY message.
const COPY_ROWS: usize = 50_000;

/// Brings the database to checkpoint `to` (default: the newest in
/// `releases`): an empty database is restored from the snapshot the target
/// names, then each later delta is applied as `update` would have. Every
/// file is checked against its `release.json`, and the checksum chain and
/// identifier counter against every checkpoint.
pub async fn import(pool: &PgPool, releases: &Releases, to: Option<i64>) -> Result<()> {
    let latest = db::latest_checkpoint(pool).await?;
    let chain = releases.chain(to, latest.as_ref())?;
    if let Some(release) = &chain.snapshot {
        let snapshot = releases.snapshot(release.sequence)?;
        restore(pool, &snapshot).await?;
    } else {
        let origin = db::origin(pool).await?.context("the database has checkpoints but no origin")?;
        if let Some(release) = chain.dailies.first() {
            ensure!(release.origin() == origin, "the releases are from another origin than the database's");
        }
    }
    for release in &chain.dailies {
        let delta = releases.delta(release)?;
        db::store(pool, &delta.pois, &delta.versions, &delta.checkpoint, Some(delta.applied_at)).await?;
        tracing::info!(
            "applied {} ({}): {} pois rows, {} versions; checksum {}",
            release.sequence,
            release.timestamp,
            delta.pois.len(),
            delta.versions.len(),
            release.checksum
        );
    }
    let reached = db::latest_checkpoint(pool).await?.context("no checkpoint")?;
    ensure!(reached == chain.target, "the database ended at {reached:?}, not {:?}", chain.target);
    println!("{} {}", reached.sequence, hex::encode(&reached.checksum));
    Ok(())
}

/// Loads a snapshot into an empty database.
async fn restore(pool: &PgPool, snapshot: &Snapshot) -> Result<()> {
    let empty: bool = sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT FROM checkpoints) AND NOT EXISTS (SELECT FROM origin)
            AND NOT EXISTS (SELECT FROM pois) AND NOT EXISTS (SELECT FROM poi_versions)",
    )
    .fetch_one(pool)
    .await?;
    ensure!(empty, "restoring a snapshot needs an empty database");
    let sequence = snapshot.release.sequence;
    tracing::info!("checking snapshot {sequence}");
    tokio::task::block_in_place(|| snapshot.verify())?;
    tracing::info!("restoring snapshot {sequence}");

    let mut tx = pool.begin().await?;
    let mut copy = tx.copy_in_raw("COPY poi_versions (osm_type, osm_id, version, poi_id, sequence) FROM STDIN").await?;
    let (mut input, mut reader) = snapshot.versions()?;
    let (mut buf, mut rows, mut total) = (String::new(), 0, 0u64);
    while let Some((row, sequence)) = reader.row(&mut input)? {
        let sequence = sequence.map_or("\\N".to_owned(), |s| s.to_string());
        writeln!(
            buf,
            "{}\t{}\t{}\t{}\t{sequence}",
            row.key.osm_type.as_str(),
            row.key.osm_id,
            row.version,
            row.poi_id
        )?;
        rows += 1;
        if rows == COPY_ROWS {
            copy.send(std::mem::take(&mut buf).into_bytes()).await?;
            total += rows as u64;
            rows = 0;
            if total.is_multiple_of(10_000_000) {
                tracing::info!("{total} versions");
            }
        }
    }
    copy.send(buf.into_bytes()).await?;
    copy.finish().await?;

    let mut copy = tx.copy_in_raw("COPY pois (osm_type, osm_id, version, poi_id, active, tags) FROM STDIN").await?;
    let (mut input, mut reader) = snapshot.pois()?;
    let (mut buf, mut rows, mut total) = (String::new(), 0, 0u64);
    while let Some(row) = reader.row(&mut input)? {
        let s = &row.state;
        let tags = copy_escape(&serde_json::to_string(&s.tags)?);
        let active = if s.active { 't' } else { 'f' };
        writeln!(
            buf,
            "{}\t{}\t{}\t{}\t{active}\t{tags}",
            row.key.osm_type.as_str(),
            row.key.osm_id,
            s.version,
            s.poi_id
        )?;
        rows += 1;
        if rows == COPY_ROWS {
            copy.send(std::mem::take(&mut buf).into_bytes()).await?;
            total += rows as u64;
            rows = 0;
            if total.is_multiple_of(10_000_000) {
                tracing::info!("{total} pois rows");
            }
        }
    }
    copy.send(buf.into_bytes()).await?;
    copy.finish().await?;

    let origin = snapshot.release.origin();
    sqlx::query("INSERT INTO origin (planet_timestamp, ruleset, replication_url) VALUES ($1, $2, $3)")
        .bind(origin.planet_timestamp)
        .bind(origin.ruleset)
        .bind(&origin.replication_url)
        .execute(&mut *tx)
        .await?;
    for row in &snapshot.checkpoints {
        db::insert_checkpoint(&mut tx, &row.checkpoint, Some(row.applied_at), row.journaled).await?;
    }
    tx.commit().await?;
    tracing::info!("restored snapshot {sequence}");
    Ok(())
}

/// Every POI version's identifier up to checkpoint `to`, as `patch` needs,
/// and that checkpoint.
pub fn load_ids(releases: &Releases, to: Option<i64>) -> Result<(PoiIds, Checkpoint)> {
    let chain = releases.chain(to, None)?;
    let snapshot = releases.snapshot(chain.snapshot.as_ref().context("no snapshot")?.sequence)?;
    let mut ids = PoiIds::default();
    snapshot.for_each_version(|row, _| {
        ids.push(row);
        Ok(())
    })?;
    for release in &chain.dailies {
        for row in &releases.delta(release)?.versions {
            ids.push(row);
        }
    }
    ids.finish();
    Ok((ids, chain.target))
}

/// The newest checkpoint `releases` can restore, checking the chain's
/// `release.json`s but not the files.
pub fn latest_checkpoint(releases: &Releases) -> Result<Checkpoint> {
    Ok(releases.chain(None, None)?.target)
}

/// Writes the status file for checkpoint `to`: the snapshot's, updated by
/// each delta since. The file is replaced by a rename, as the geocoder
/// expects.
pub fn write_status(releases: &Releases, to: Option<i64>, output: &Path) -> Result<Checkpoint> {
    let chain = releases.chain(to, None)?;
    let snapshot = releases.snapshot(chain.snapshot.as_ref().context("no snapshot")?.sequence)?;
    let mut status: Status = snapshot.status()?;
    for release in &chain.dailies {
        status.apply(&releases.delta(release)?)?;
    }
    let partial = output.with_extension("partial");
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&partial)?);
    status.write(&mut out, &chain.target)?;
    out.flush()?;
    out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    std::fs::rename(&partial, output)?;
    Ok(chain.target)
}
