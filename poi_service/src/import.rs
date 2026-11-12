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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use chrono::{DateTime, Utc};

    use super::*;
    use crate::db::{CheckpointRow, Origin};
    use crate::identity::{Checksum, PoiChange, Tracked, VersionRow};
    use crate::osm::{ElementKey, OsmType};
    use crate::published::{
        CHECKPOINTS_FILE, CheckpointJson, DELTA_FILE, Kind, POIS_FILE, Previous, RELEASE_FILE, Release, STATUS_FILE,
        VERSIONS_FILE, file_info, json_bytes,
    };
    use crate::rules::tags;
    use crate::tables::{
        Delta, POIS_MAGIC, PoisWriter, State, VERSIONS_MAGIC, VersionsWriter, create_zst, finish_zst, unpack_status,
        write_header,
    };

    fn node(osm_id: i64) -> ElementKey {
        ElementKey { osm_type: OsmType::Node, osm_id }
    }

    fn poi(osm_id: i64, version: i32, poi_id: i64, active: bool, name: &str, previous: Option<i64>) -> PoiChange {
        PoiChange {
            key: node(osm_id),
            state: Tracked { version, poi_id, active, tags: tags([("amenity", "cafe"), ("name", name)]) },
            previous_poi_id: previous,
        }
    }

    fn time(sequence: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_158_403 + (sequence - 10) * 86_400, 0).unwrap()
    }

    fn checkpoint(sequence: i64, next_poi_id: i64, previous: &[u8], rows: &[VersionRow]) -> Checkpoint {
        let mut checksum = Checksum::new(previous);
        for row in rows {
            checksum.add(row);
        }
        Checkpoint { sequence, timestamp: time(sequence), next_poi_id, checksum: checksum.finish() }
    }

    fn origin() -> Origin {
        Origin { planet_timestamp: time(10), ruleset: 3, replication_url: "https://example.org/day/".into() }
    }

    /// Writes a snapshot of `versions` and `pois` with `checkpoints`, as
    /// `export` would.
    fn write_snapshot(
        releases: &Releases,
        versions: &[(VersionRow, Option<i64>)],
        pois: &[PoiChange],
        checkpoints: &[CheckpointRow],
    ) {
        let latest = &checkpoints.last().unwrap().checkpoint;
        let dir = releases.snapshot_dir(latest.sequence);
        std::fs::create_dir_all(&dir).unwrap();
        let mut release = Release::new(Kind::Snapshot, latest, &origin(), latest.sequence);
        let mut status = Status::new();
        let mut out = create_zst(&dir.join(VERSIONS_FILE)).unwrap();
        write_header(&mut out, VERSIONS_MAGIC).unwrap();
        out.write_all(&checkpoints[0].checkpoint.sequence.to_le_bytes()).unwrap();
        let mut writer = VersionsWriter::new(Some(checkpoints[0].checkpoint.sequence));
        for (row, sequence) in versions {
            writer.row(&mut out, row, *sequence).unwrap();
            status.version(row).unwrap();
        }
        writer.finish(&mut out).unwrap();
        finish_zst(out).unwrap();
        let mut out = create_zst(&dir.join(POIS_FILE)).unwrap();
        write_header(&mut out, POIS_MAGIC).unwrap();
        let mut writer = PoisWriter::new(false);
        for row in pois {
            writer.row(&mut out, row).unwrap();
            status.poi(row).unwrap();
        }
        writer.finish(&mut out).unwrap();
        finish_zst(out).unwrap();
        let mut out = create_zst(&dir.join(STATUS_FILE)).unwrap();
        status.write(&mut out, latest).unwrap();
        finish_zst(out).unwrap();
        let json: Vec<CheckpointJson> = checkpoints.iter().map(CheckpointJson::from).collect();
        std::fs::write(dir.join(CHECKPOINTS_FILE), json_bytes(&json).unwrap()).unwrap();
        for name in [VERSIONS_FILE, POIS_FILE, STATUS_FILE, CHECKPOINTS_FILE] {
            release.files.insert(name.into(), file_info(&dir.join(name)).unwrap());
        }
        release.counts.insert("pois".into(), pois.len() as u64);
        std::fs::write(dir.join(RELEASE_FILE), json_bytes(&release).unwrap()).unwrap();
    }

    fn write_daily(releases: &Releases, delta: Option<&Delta>, checkpoint: &Checkpoint, snapshot: i64) {
        let dir = releases.daily_dir(checkpoint.sequence);
        std::fs::create_dir_all(&dir).unwrap();
        let kind = if delta.is_some() { Kind::Delta } else { Kind::Planet };
        let mut release = Release::new(kind, checkpoint, &origin(), snapshot);
        if let Some(delta) = delta {
            let mut out = create_zst(&dir.join(DELTA_FILE)).unwrap();
            delta.write(&mut out).unwrap();
            finish_zst(out).unwrap();
            release.files.insert(DELTA_FILE.into(), file_info(&dir.join(DELTA_FILE)).unwrap());
            let p = &delta.previous;
            release.previous =
                Some(Previous { sequence: p.sequence, next_poi_id: p.next_poi_id, checksum: hex::encode(&p.checksum) });
            release.counts.insert("versions".into(), delta.versions.len() as u64);
            release.counts.insert("pois".into(), delta.pois.len() as u64);
        }
        std::fs::write(dir.join(RELEASE_FILE), json_bytes(&release).unwrap()).unwrap();
    }

    fn row(checkpoint: &Checkpoint) -> CheckpointRow {
        CheckpointRow { checkpoint: checkpoint.clone(), applied_at: time(30), journaled: checkpoint.sequence > 10 }
    }

    /// A planet with nodes 1 and 2 (identities 1 and 2) at sequence 10;
    /// diff 11 renames node 1 (same identity) and replaces node 2's business
    /// (identity 3); diff 12 adds node 5 (identity 4) and deletes node 1.
    /// Snapshots at 10 and 12.
    fn releases(dir: &Path) -> Releases {
        let releases = Releases::new(dir);
        let planet_rows =
            [VersionRow { key: node(1), version: 1, poi_id: 1 }, VersionRow { key: node(2), version: 4, poi_id: 2 }];
        let c10 = checkpoint(10, 3, &[0; 32], &planet_rows);
        let planet_pois = [poi(1, 1, 1, true, "Ritual", None), poi(2, 4, 2, true, "Blue Bottle", None)];
        write_snapshot(&releases, &planet_rows.map(|r| (r, None)), &planet_pois, &[row(&c10)]);
        write_daily(&releases, None, &c10, 10);

        let rows11 = vec![
            VersionRow { key: node(1), version: 2, poi_id: 1 },
            VersionRow { key: node(2), version: 5, poi_id: 3 },
        ];
        let c11 = checkpoint(11, 4, &c10.checksum, &rows11);
        let delta11 = Delta {
            previous: c10.clone(),
            checkpoint: c11.clone(),
            applied_at: time(30),
            versions: rows11.clone(),
            pois: vec![poi(1, 2, 1, true, "Ritual Coffee", Some(1)), poi(2, 5, 3, true, "Sightglass", Some(2))],
        };
        write_daily(&releases, Some(&delta11), &c11, 10);

        let rows12 = vec![VersionRow { key: node(5), version: 1, poi_id: 4 }];
        let c12 = checkpoint(12, 5, &c11.checksum, &rows12);
        let delta12 = Delta {
            previous: c11.clone(),
            checkpoint: c12.clone(),
            applied_at: time(30),
            versions: rows12.clone(),
            pois: vec![poi(1, 3, 1, false, "Ritual Coffee", Some(1)), poi(5, 1, 4, true, "Zeitgeist", None)],
        };
        write_daily(&releases, Some(&delta12), &c12, 12);

        let mut all: Vec<_> = planet_rows.iter().map(|r| (*r, None)).collect();
        all.extend(rows11.iter().map(|r| (*r, Some(11))));
        all.extend(rows12.iter().map(|r| (*r, Some(12))));
        all.sort_by_key(|(r, _)| (r.key, r.version));
        let pois = [
            poi(1, 3, 1, false, "Ritual Coffee", None),
            poi(2, 5, 3, true, "Sightglass", None),
            poi(5, 1, 4, true, "Zeitgeist", None),
        ];
        write_snapshot(&releases, &all, &pois, &[row(&c10), row(&c11), row(&c12)]);
        releases
    }

    #[test]
    fn snapshots_and_deltas_agree() {
        let dir = tempfile::tempdir().unwrap();
        let releases = releases(dir.path());
        for sequence in [10, 12] {
            releases.snapshot(sequence).unwrap().verify().unwrap();
        }

        // The status from snapshot 10 and both deltas is snapshot 12's, byte
        // for byte; so is the one at 12 that starts from snapshot 12.
        let output = dir.path().join("status.bin");
        let mut from_snapshot = Vec::new();
        Status::read(&mut crate::tables::open_zst(&releases.snapshot_dir(12).join(STATUS_FILE)).unwrap())
            .unwrap()
            .0
            .write(&mut from_snapshot, &releases.daily(12).unwrap().unwrap().checkpoint().unwrap())
            .unwrap();
        // Release 12 names snapshot 12; build 12 from snapshot 10 by
        // pretending release 11 is the target, then applying 12's delta.
        let chain = releases.chain(Some(11), None).unwrap();
        assert_eq!((chain.snapshot.as_ref().unwrap().sequence, chain.dailies.len()), (10, 1));
        let mut status = releases.snapshot(10).unwrap().status().unwrap();
        status.apply(&releases.delta(&chain.dailies[0]).unwrap()).unwrap();
        status.apply(&releases.delta(&releases.daily(12).unwrap().unwrap()).unwrap()).unwrap();
        let mut from_deltas = Vec::new();
        status.write(&mut from_deltas, &releases.daily(12).unwrap().unwrap().checkpoint().unwrap()).unwrap();
        assert_eq!(from_deltas, from_snapshot);
        let state = |poi_id: usize| unpack_status(status.records[poi_id]).0;
        assert_eq!(
            (state(1), state(2), state(3), state(4)),
            (State::Inactive, State::Replaced, State::Active, State::Active)
        );

        let reached = write_status(&releases, Some(11), &output).unwrap();
        assert_eq!(reached.sequence, 11);
        let (written, _) = Status::read(&mut std::fs::File::open(&output).unwrap()).unwrap();
        assert_eq!(unpack_status(written.records[2]).0, State::Replaced);
        assert_eq!(written.records.len(), 4);

        // Identifiers for patch are the same from either snapshot.
        let ids = |to| {
            let (ids, checkpoint) = load_ids(&releases, Some(to)).unwrap();
            let found: Vec<_> = [(1, 1), (1, 2), (2, 4), (2, 5), (5, 1), (5, 2)]
                .map(|(id, version)| ids.get_for_test(OsmType::Node, id, version))
                .to_vec();
            (found, checkpoint.sequence)
        };
        assert_eq!(ids(11), (vec![Some(1), Some(1), Some(2), Some(3), None, None], 11));
        assert_eq!(ids(12), (vec![Some(1), Some(1), Some(2), Some(3), Some(4), None], 12));
        assert_eq!(latest_checkpoint(&releases).unwrap().sequence, 12);
    }

    #[test]
    fn broken_releases_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let releases = releases(dir.path());
        // A delta whose file was changed.
        let path = releases.daily_dir(11).join(DELTA_FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(load_ids(&releases, Some(11)).is_err());

        // A release that doesn't follow the one before.
        let dir = tempfile::tempdir().unwrap();
        let releases = super::tests::releases(dir.path());
        let path = releases.daily_dir(11).join(RELEASE_FILE);
        let mut release: Release = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        release.previous.as_mut().unwrap().checksum = "00".repeat(32);
        std::fs::write(&path, json_bytes(&release).unwrap()).unwrap();
        let error = releases.chain(Some(11), None).err().unwrap().to_string();
        assert!(error.contains("doesn't follow"), "{error}");

        // A snapshot whose status file disagrees with its tables.
        let dir = tempfile::tempdir().unwrap();
        let releases = super::tests::releases(dir.path());
        let snapshot = releases.snapshot(12).unwrap();
        let mut status = snapshot.status().unwrap();
        status.records[2] = 0;
        let mut out = create_zst(&snapshot.path(STATUS_FILE)).unwrap();
        status.write(&mut out, snapshot.latest()).unwrap();
        finish_zst(out).unwrap();
        let path = snapshot.path(RELEASE_FILE);
        let mut release = snapshot.release.clone();
        release.files.insert(STATUS_FILE.into(), file_info(&snapshot.path(STATUS_FILE)).unwrap());
        std::fs::write(&path, json_bytes(&release).unwrap()).unwrap();
        let error = releases.snapshot(12).unwrap().verify().err().unwrap().to_string();
        assert!(error.contains("status file differs"), "{error}");
    }
}
