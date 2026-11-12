//! PostgreSQL storage for POIs and their identities.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::identity::{PoiChange, Tracked, VersionRow};
use crate::osm::{ElementKey, OsmType};

const BATCH: usize = 50_000;

pub async fn connect(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(4).connect(url).await.context("connecting to PostgreSQL")?;
    sqlx::migrate!().run(&pool).await.context("migrating the database")?;
    Ok(pool)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub planet_timestamp: DateTime<Utc>,
    pub ruleset: i32,
    pub replication_url: String,
}

pub async fn origin(conn: impl sqlx::PgExecutor<'_>) -> Result<Option<Origin>> {
    let row = sqlx::query("SELECT planet_timestamp, ruleset, replication_url FROM origin").fetch_optional(conn).await?;
    Ok(row.map(|row| Origin { planet_timestamp: row.get(0), ruleset: row.get(1), replication_url: row.get(2) }))
}

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub sequence: i64,
    pub timestamp: DateTime<Utc>,
    pub next_poi_id: i64,
    pub checksum: Vec<u8>,
}

/// A `checkpoints` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckpointRow {
    pub checkpoint: Checkpoint,
    pub applied_at: DateTime<Utc>,
    /// Whether the diff's changes are recorded for `export`.
    pub journaled: bool,
}

impl PartialEq for Checkpoint {
    fn eq(&self, other: &Self) -> bool {
        (self.sequence, self.timestamp, self.next_poi_id, &self.checksum)
            == (other.sequence, other.timestamp, other.next_poi_id, &other.checksum)
    }
}

/// Every checkpoint, oldest first.
pub async fn checkpoints(conn: &mut sqlx::PgConnection) -> Result<Vec<CheckpointRow>> {
    let rows = sqlx::query(
        "SELECT sequence, timestamp, next_poi_id, checksum, applied_at, journaled FROM checkpoints ORDER BY sequence",
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| CheckpointRow {
            checkpoint: Checkpoint {
                sequence: row.get(0),
                timestamp: row.get(1),
                next_poi_id: row.get(2),
                checksum: row.get(3),
            },
            applied_at: row.get(4),
            journaled: row.get(5),
        })
        .collect())
}

pub async fn latest_checkpoint(pool: &PgPool) -> Result<Option<Checkpoint>> {
    let row = sqlx::query(
        "SELECT sequence, timestamp, next_poi_id, checksum FROM checkpoints ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| Checkpoint {
        sequence: row.get(0),
        timestamp: row.get(1),
        next_poi_id: row.get(2),
        checksum: row.get(3),
    }))
}

/// The tracked state of whichever of `keys` the database knows.
pub async fn tracked(pool: &PgPool, keys: impl Iterator<Item = ElementKey>) -> Result<HashMap<ElementKey, Tracked>> {
    let mut ids: HashMap<OsmType, Vec<i64>> = HashMap::new();
    for key in keys {
        ids.entry(key.osm_type).or_default().push(key.osm_id);
    }
    let mut tracked = HashMap::new();
    for (osm_type, ids) in ids {
        for chunk in ids.chunks(BATCH) {
            let rows = sqlx::query(
                "SELECT osm_id, version, poi_id, active, tags::text FROM pois
                 WHERE osm_type = $1 AND osm_id = ANY($2)",
            )
            .bind(osm_type)
            .bind(chunk)
            .fetch_all(pool)
            .await?;
            for row in rows {
                let key = ElementKey { osm_type, osm_id: row.get(0) };
                let tags = serde_json::from_str(row.get(4))?;
                tracked.insert(key, Tracked { version: row.get(1), poi_id: row.get(2), active: row.get(3), tags });
            }
        }
    }
    Ok(tracked)
}

/// Stores the result of applying replication `checkpoint.sequence`, which
/// must follow the latest checkpoint, and records its changes for `export`.
/// `applied_at` is now unless restoring a published diff.
pub async fn store(
    pool: &PgPool,
    updates: &[PoiChange],
    versions: &[VersionRow],
    checkpoint: &Checkpoint,
    applied_at: Option<DateTime<Utc>>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let latest: Option<i64> =
        sqlx::query_scalar("SELECT sequence FROM checkpoints ORDER BY sequence DESC LIMIT 1 FOR UPDATE")
            .fetch_optional(&mut *tx)
            .await?;
    if latest != Some(checkpoint.sequence - 1) {
        bail!("expected to follow sequence {}, but the latest is {latest:?}", checkpoint.sequence - 1);
    }

    for chunk in updates.chunks(BATCH) {
        sqlx::query(
            "INSERT INTO pois (osm_type, osm_id, version, poi_id, active, tags)
             SELECT * FROM unnest($1::text[]::osm_type[], $2::int8[], $3::int4[], $4::int8[], $5::bool[], $6::text[]::jsonb[])
             ON CONFLICT (osm_type, osm_id) DO UPDATE SET
                version = excluded.version, poi_id = excluded.poi_id, active = excluded.active, tags = excluded.tags",
        )
        .bind(chunk.iter().map(|c| c.key.osm_type.as_str()).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.key.osm_id).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.version).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.poi_id).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.active).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| serde_json::to_string(&c.state.tags)).collect::<Result<Vec<_>, _>>()?)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO poi_changes (sequence, osm_type, osm_id, version, poi_id, active, tags, previous_poi_id)
             SELECT $1::int8, * FROM unnest($2::text[]::osm_type[], $3::int8[], $4::int4[], $5::int8[], $6::bool[],
                $7::text[]::jsonb[], $8::int8[])",
        )
        .bind(checkpoint.sequence)
        .bind(chunk.iter().map(|c| c.key.osm_type.as_str()).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.key.osm_id).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.version).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.poi_id).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| c.state.active).collect::<Vec<_>>())
        .bind(chunk.iter().map(|c| serde_json::to_string(&c.state.tags)).collect::<Result<Vec<_>, _>>()?)
        .bind(chunk.iter().map(|c| c.previous_poi_id).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;
    }

    for chunk in versions.chunks(BATCH) {
        sqlx::query(
            "INSERT INTO poi_versions (osm_type, osm_id, version, poi_id, sequence)
             SELECT *, $5::int8 FROM unnest($1::text[]::osm_type[], $2::int8[], $3::int4[], $4::int8[])",
        )
        .bind(chunk.iter().map(|r| r.key.osm_type.as_str()).collect::<Vec<_>>())
        .bind(chunk.iter().map(|r| r.key.osm_id).collect::<Vec<_>>())
        .bind(chunk.iter().map(|r| r.version).collect::<Vec<_>>())
        .bind(chunk.iter().map(|r| r.poi_id).collect::<Vec<_>>())
        .bind(checkpoint.sequence)
        .execute(&mut *tx)
        .await?;
    }

    insert_checkpoint(&mut tx, checkpoint, applied_at, true).await?;
    tx.commit().await?;
    Ok(())
}

/// Inserts a checkpoint, applied now unless `applied_at` says otherwise.
pub async fn insert_checkpoint(
    conn: &mut sqlx::PgConnection,
    checkpoint: &Checkpoint,
    applied_at: Option<DateTime<Utc>>,
    journaled: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO checkpoints (sequence, timestamp, next_poi_id, checksum, applied_at, journaled)
         VALUES ($1, $2, $3, $4, coalesce($5, now()), $6)",
    )
    .bind(checkpoint.sequence)
    .bind(checkpoint.timestamp)
    .bind(checkpoint.next_poi_id)
    .bind(&checkpoint.checksum)
    .bind(applied_at)
    .bind(journaled)
    .execute(conn)
    .await?;
    Ok(())
}
