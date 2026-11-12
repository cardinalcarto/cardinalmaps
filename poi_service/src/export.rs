//! Writing the identity tables' releases to a staging directory, for
//! `cardinal-publish poi` to upload. See [`crate::published`] for the layout.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use chrono::TimeDelta;
use futures_util::TryStreamExt;
use sqlx::{PgConnection, PgPool};

use crate::db::{self, CheckpointRow, Origin};
use crate::identity::{PoiChange, Tracked, VersionRow};
use crate::osm::{ElementKey, OsmType};
use crate::published::{
    self, CHECKPOINTS_FILE, CheckpointJson, DELTA_FILE, Kind, POIS_FILE, Previous, RELEASE_FILE, Release, Releases,
    SNAPSHOTS, STATUS_FILE, VERSIONS_FILE, file_info, json_bytes, write_file,
};
use crate::tables::{
    Delta, POIS_MAGIC, PoisWriter, Status, VERSIONS_MAGIC, VersionsWriter, create_zst, finish_zst, write_header,
};

/// Writes a release for every checkpoint after the newest one in `staging`,
/// and a snapshot of the latest checkpoint if `force_snapshot`, if `staging`
/// has none, or if the newest is `snapshot_days` older than the latest
/// checkpoint. Everything is read in one transaction, so it's consistent
/// even while `update` runs.
pub async fn export(pool: &PgPool, staging: &Path, force_snapshot: bool, snapshot_days: i64) -> Result<()> {
    std::fs::create_dir_all(staging.join(SNAPSHOTS))?;
    let releases = Releases::new(staging);
    let mut tx = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY").execute(&mut *tx).await?;
    let origin = db::origin(&mut *tx).await?.context("the database isn't bootstrapped")?;
    let checkpoints = db::checkpoints(&mut tx).await?;
    let latest = checkpoints.last().context("no checkpoints")?.clone();

    // What's staged must be this database's.
    let staged = releases.dailies()?;
    if let Some(&newest) = staged.last() {
        let release = releases.daily(newest)?.unwrap();
        let row = checkpoints.iter().find(|c| c.checkpoint.sequence == newest);
        ensure!(
            release.origin() == origin && row.is_some_and(|row| row.checkpoint == release.checkpoint().unwrap()),
            "{} holds release {newest}, which isn't this database's checkpoint {newest}",
            staging.display()
        );
    }

    let mut snapshots = releases.snapshots()?;
    let newest_snapshot = match snapshots.last() {
        Some(&s) => releases.snapshot_release(s)?,
        None => None,
    };
    let take_snapshot = match &newest_snapshot {
        Some(s) if s.sequence == latest.checkpoint.sequence => false,
        Some(s) => force_snapshot || latest.checkpoint.timestamp - s.timestamp >= TimeDelta::days(snapshot_days),
        None => true,
    };
    if take_snapshot {
        write_snapshot(&mut tx, &releases, &origin, &checkpoints).await?;
        snapshots.push(latest.checkpoint.sequence);
    }

    for (i, row) in checkpoints.iter().enumerate() {
        let sequence = row.checkpoint.sequence;
        if staged.last().is_some_and(|&s| sequence <= s) {
            continue;
        }
        let Some(&snapshot) = snapshots.iter().rev().find(|&&s| s <= sequence) else {
            tracing::warn!("skipping {sequence}: no snapshot at or before it to restore it from");
            continue;
        };
        if i == 0 {
            write_daily(&releases, Release::new(Kind::Planet, &row.checkpoint, &origin, snapshot), None)?;
            continue;
        }
        if !row.journaled {
            bail!("checkpoint {sequence} was applied before its changes were journaled, so it can't be exported");
        }
        let delta = read_delta(&mut tx, &checkpoints[i - 1], row).await?;
        delta.verify().with_context(|| format!("the database's changes for {sequence} are inconsistent"))?;
        let mut release = Release::new(Kind::Delta, &row.checkpoint, &origin, snapshot);
        let previous = &checkpoints[i - 1].checkpoint;
        release.previous = Some(Previous {
            sequence: previous.sequence,
            next_poi_id: previous.next_poi_id,
            checksum: hex::encode(&previous.checksum),
        });
        release.counts.insert("versions".into(), delta.versions.len() as u64);
        release.counts.insert("pois".into(), delta.pois.len() as u64);
        write_daily(&releases, release, Some(&delta))?;
    }
    tx.commit().await?;
    Ok(())
}

/// Writes one checkpoint's release, under a temporary name until complete.
fn write_daily(releases: &Releases, mut release: Release, delta: Option<&Delta>) -> Result<()> {
    let sequence = release.sequence;
    let dir = releases.daily_dir(sequence);
    let partial = releases.root.join(format!(".{sequence}.partial"));
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(&partial)?;
    if let Some(delta) = delta {
        let path = partial.join(DELTA_FILE);
        let mut out = create_zst(&path)?;
        delta.write(&mut out)?;
        finish_zst(out)?;
        release.files.insert(DELTA_FILE.into(), file_info(&path)?);
    }
    write_file(&partial.join(RELEASE_FILE), &json_bytes(&release)?)?;
    ensure!(!dir.exists(), "{} exists but has no release.json; remove it", dir.display());
    std::fs::rename(&partial, &dir)?;
    tracing::info!(
        "exported {sequence} ({}): {} versions, {} pois rows, snapshot {}",
        release.timestamp,
        release.counts.get("versions").unwrap_or(&0),
        release.counts.get("pois").unwrap_or(&0),
        release.snapshot
    );
    Ok(())
}

async fn read_delta(conn: &mut PgConnection, previous: &CheckpointRow, row: &CheckpointRow) -> Result<Delta> {
    let sequence = row.checkpoint.sequence;
    let versions = sqlx::query_as::<_, (OsmType, i64, i32, i64)>(
        "SELECT osm_type, osm_id, version, poi_id FROM poi_versions WHERE sequence = $1
         ORDER BY osm_type, osm_id, version",
    )
    .bind(sequence)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(osm_type, osm_id, version, poi_id)| VersionRow { key: ElementKey { osm_type, osm_id }, version, poi_id })
    .collect();
    let pois = sqlx::query_as::<_, (OsmType, i64, i32, i64, bool, String, Option<i64>)>(
        "SELECT osm_type, osm_id, version, poi_id, active, tags::text, previous_poi_id FROM poi_changes
         WHERE sequence = $1 ORDER BY osm_type, osm_id",
    )
    .bind(sequence)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|(osm_type, osm_id, version, poi_id, active, tags, previous_poi_id)| {
        Ok(PoiChange {
            key: ElementKey { osm_type, osm_id },
            state: Tracked { version, poi_id, active, tags: serde_json::from_str(&tags)? },
            previous_poi_id,
        })
    })
    .collect::<Result<_>>()?;
    Ok(Delta {
        previous: previous.checkpoint.clone(),
        checkpoint: row.checkpoint.clone(),
        applied_at: row.applied_at,
        versions,
        pois,
    })
}

/// Writes a snapshot of the latest checkpoint, then reads it back to check
/// it before marking it complete.
async fn write_snapshot(
    conn: &mut PgConnection,
    releases: &Releases,
    origin: &Origin,
    checkpoints: &[CheckpointRow],
) -> Result<()> {
    let latest = &checkpoints.last().unwrap().checkpoint;
    let sequence = latest.sequence;
    let base = checkpoints[0].checkpoint.sequence;
    let dir = releases.snapshot_dir(sequence);
    let partial = releases.root.join(SNAPSHOTS).join(format!(".{sequence}.partial"));
    if partial.exists() {
        std::fs::remove_dir_all(&partial)?;
    }
    std::fs::create_dir_all(&partial)?;
    let mut release = Release::new(Kind::Snapshot, latest, origin, sequence);
    let mut status = Status::new();
    tracing::info!("writing a snapshot of {sequence}");

    let path = partial.join(VERSIONS_FILE);
    let mut out = create_zst(&path)?;
    write_header(&mut out, VERSIONS_MAGIC)?;
    out.write_all(&base.to_le_bytes())?;
    let mut writer = VersionsWriter::new(Some(base));
    let mut rows = sqlx::query_as::<_, (OsmType, i64, i32, i64, Option<i64>)>(
        "SELECT osm_type, osm_id, version, poi_id, sequence FROM poi_versions ORDER BY osm_type, osm_id, version",
    )
    .fetch(&mut *conn);
    while let Some((osm_type, osm_id, version, poi_id, sequence)) = rows.try_next().await? {
        let row = VersionRow { key: ElementKey { osm_type, osm_id }, version, poi_id };
        writer.row(&mut out, &row, sequence)?;
        status.version(&row)?;
    }
    drop(rows);
    let count = writer.finish(&mut out)?;
    finish_zst(out)?;
    release.counts.insert("versions".into(), count);
    release.files.insert(VERSIONS_FILE.into(), file_info(&path)?);
    tracing::info!("{count} POI versions");

    let path = partial.join(POIS_FILE);
    let mut out = create_zst(&path)?;
    write_header(&mut out, POIS_MAGIC)?;
    let mut writer = PoisWriter::new(false);
    let mut rows = sqlx::query_as::<_, (OsmType, i64, i32, i64, bool, String)>(
        "SELECT osm_type, osm_id, version, poi_id, active, tags::text FROM pois ORDER BY osm_type, osm_id",
    )
    .fetch(&mut *conn);
    while let Some((osm_type, osm_id, version, poi_id, active, tags)) = rows.try_next().await? {
        let row = PoiChange {
            key: ElementKey { osm_type, osm_id },
            state: Tracked { version, poi_id, active, tags: serde_json::from_str(&tags)? },
            previous_poi_id: None,
        };
        writer.row(&mut out, &row)?;
        status.poi(&row)?;
    }
    drop(rows);
    let count = writer.finish(&mut out)?;
    finish_zst(out)?;
    release.counts.insert("pois".into(), count);
    release.files.insert(POIS_FILE.into(), file_info(&path)?);
    tracing::info!("{count} pois rows");

    let path = partial.join(STATUS_FILE);
    let mut out = create_zst(&path)?;
    status.write(&mut out, latest)?;
    finish_zst(out)?;
    release.files.insert(STATUS_FILE.into(), file_info(&path)?);

    let path = partial.join(CHECKPOINTS_FILE);
    let json: Vec<CheckpointJson> = checkpoints.iter().map(CheckpointJson::from).collect();
    write_file(&path, &json_bytes(&json)?)?;
    release.files.insert(CHECKPOINTS_FILE.into(), file_info(&path)?);
    release.counts.insert("checkpoints".into(), checkpoints.len() as u64);

    let snapshot =
        published::Snapshot { release: release.clone(), dir: partial.clone(), checkpoints: checkpoints.to_vec() };
    let checked = snapshot.verify().context("the snapshot written doesn't check out")?;
    ensure!(checked.records.len() as i64 == latest.next_poi_id, "status size");
    tracing::info!("checked the snapshot");

    write_file(&partial.join(RELEASE_FILE), &json_bytes(&release)?)?;
    ensure!(!dir.exists(), "{} exists but has no release.json; remove it", dir.display());
    std::fs::rename(&partial, &dir)?;
    tracing::info!("exported snapshot {sequence}: {:?}", release.files);
    Ok(())
}
