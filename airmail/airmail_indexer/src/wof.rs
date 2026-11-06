use anyhow::Result;
use geozero::{ToGeo, wkb::GpkgWkb};
use log::trace;
use serde::{Deserialize, Serialize};
use sqlx::{
    Pool, Sqlite,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::collections::HashMap;
use std::path::Path;

/// Performs simple point-in-polygon queries against the WhosOnFirst database.
/// Queries against the WOF database require the SQLite mod_spatialite extension to be loaded.
/// Requires package: libsqlite3-mod-spatialite on Debian/Ubuntu
#[derive(Clone)]
pub struct WhosOnFirst {
    pool: Pool<Sqlite>,
}

impl WhosOnFirst {
    /// Opens a connection to the WhosOnFirst database.
    /// Requires the SQLite mod_spatialite extension to be loaded.
    pub async fn new(path: &Path) -> Result<Self> {
        trace!("Opening WhosOnFirst database at {:?}", path);

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .pragma("cache_size", "2000")
            .pragma("synchronous", "OFF")
            .pragma("temp_store", "MEMORY")
            .pragma("foreign_keys", "OFF")
            .pragma("recursive_triggers", "OFF")
            .pragma("locking_mode", "NORMAL");
        // SAFETY: mod_spatialite is the system-installed SpatiaLite extension,
        // not something loaded from an untrusted path.
        let opts = unsafe { opts.extension("mod_spatialite") };

        // Connections with the total number of physical and virtual cores.
        // The sqlx pool isn't the most efficient, so keep it busy.
        let connections = num_cpus::get().try_into()?;

        // Never close these connections. Closing one makes mod_spatialite call
        // xmlCleanupParser, which isn't thread-safe: with libxml2 2.9.14
        // (Debian trixie), connections closing at once abort the process with
        // a double free. With the default limits, a long stretch without
        // queries expires the whole pool, so they would all close together
        // on the next use.
        let pool = SqlitePoolOptions::new()
            .min_connections(connections)
            .max_connections(connections)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        // Likewise when the pool would otherwise be dropped at exit.
        std::mem::forget(pool.clone());

        Ok(Self { pool })
    }

    /// Returns the WOF ID of polygons that contain the given point.
    /// Requires the spatialite extension to be loaded.
    pub async fn point_in_polygon(&self, lon: f64, lat: f64) -> Result<Vec<ConcisePipResponse>> {
        let lon: f32 = lon as f32;
        let lat: f32 = lat as f32;
        let rows = sqlx::query_as::<_, ConcisePipResponse>(
            r"
                SELECT place.source, place.id, place.class, place.type
                FROM main.point_in_polygon
                LEFT JOIN place USING (source, id)
                WHERE search_frame = MakePoint( ?1, ?2, 4326 )
                AND INTERSECTS( point_in_polygon.geom, MakePoint( ?1, ?2, 4326 ) )
                AND place.source IS NOT NULL
                AND (
                    place.type != 'planet'
                    AND place.type != 'marketarea'
                    AND place.type != 'county'
                    AND place.type != 'timezone'
                )
                LIMIT 1000
            ",
        )
        .bind(lon)
        .bind(lat)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Lookup the name of a place by its WOF ID.
    pub async fn place_name_by_id(&self, id: u64) -> Result<Vec<PipPlaceName>> {
        // Convert to i64 for SQLite
        let id: i64 = id.try_into()?;

        // Index for name is on (source, id)
        let rows = sqlx::query_as::<_, PipPlaceName>(
            r"
                SELECT name.lang, name.tag, name.abbr, name.name
                FROM main.name
                WHERE name.source = 'wof'
                AND name.id = ?1
                AND name.tag IN ('preferred', 'default')
                AND name.lang IN (
                    'ara', 'dan', 'deu', 'fra', 'fin', 'hun', 'gre', 'ita', 'nld', 'por',
                    'rus', 'ron', 'spa', 'eng', 'swe', 'tam', 'tur', 'zho'
                )
            ",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Lookup the properties of a place by its WOF ID.
    pub async fn properties_for_id(&self, id: u64) -> Result<Vec<WofKV>> {
        // Convert to i64 for SQLite
        let id: i64 = id.try_into()?;

        // Index for name is on (source, id)
        let rows = sqlx::query_as::<_, WofKV>(
            r"
                SELECT property.key, property.value
                FROM main.property
                WHERE property.source = 'wof'
                AND property.id = ?1
                AND property.key = 'wof:lang_x_spoken'
            ",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// A country's ISO 3166-1 alpha-2 code: `alpha2` in SpatiaLite
    /// databases, else `wof:country`.
    pub async fn country_code(&self, id: u64) -> Result<Option<String>> {
        let id: i64 = id.try_into()?;
        let row: Option<(String,)> = sqlx::query_as(
            r"
                SELECT property.value
                FROM main.property
                WHERE property.source = 'wof'
                AND property.id = ?1
                AND property.key IN ('alpha2', 'wof:country')
                ORDER BY property.key = 'alpha2' DESC
                LIMIT 1
            ",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|(code,)| airmail::poi::iso_country_code(&code)))
    }

    /// Every name of a place, in every language and with every tag
    /// (`default`, `preferred`, `variant`, `colloquial`).
    pub async fn names(&self, id: u64) -> Result<Vec<WofName>> {
        let id: i64 = id.try_into()?;
        Ok(sqlx::query_as::<_, WofName>(
            "SELECT lang, tag, abbr, name FROM main.name WHERE source = 'wof' AND id = ?1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// A place's placetype, if it's in the database.
    pub async fn placetype(&self, id: u64) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT type FROM main.place WHERE source = 'wof' AND id = ?1")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|(placetype,)| placetype))
    }

    /// The IDs of the places containing a place, in every branch of its
    /// hierarchy, not including itself.
    pub async fn ancestors(&self, id: u64) -> Result<Vec<u64>> {
        let id: i64 = id.try_into()?;
        let rows: Vec<(String,)> = sqlx::query_as(
            r"
                SELECT DISTINCT parent_id FROM main.hierarchy
                WHERE child_source = 'wof' AND child_id = ?1 AND parent_id != child_id
            ",
        )
        .bind(id.to_string())
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().filter_map(|(id,)| id.parse().ok()).collect())
    }

    /// The values of some of a place's properties.
    pub async fn properties(&self, id: u64, keys: &[&str]) -> Result<HashMap<String, String>> {
        let id: i64 = id.try_into()?;
        let rows = sqlx::query_as::<_, WofKV>(
            "SELECT key, value FROM main.property WHERE source = 'wof' AND id = ?1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|kv| keys.contains(&kv.key.as_str()))
            .map(|kv| (kv.key, kv.value))
            .collect())
    }

    /// A place's geometries by role (`boundary`, `label_position`,
    /// `centroid`, ...).
    pub async fn geometries(&self, id: u64) -> Result<HashMap<String, geo_types::Geometry<f64>>> {
        let id: i64 = id.try_into()?;
        let rows: Vec<(String, Option<Vec<u8>>)> = sqlx::query_as(
            "SELECT role, AsGPB(geom) FROM main.geometry WHERE source = 'wof' AND id = ?1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(role, blob)| Some((role, GpkgWkb(blob?).to_geo().ok()?)))
            .collect())
    }

    /// Every place that's a search result, by ID and placetype.
    pub async fn searchable_places(&self) -> Result<Vec<(u64, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            r"
                SELECT id, type FROM main.place
                WHERE source = 'wof'
                AND type NOT IN ('planet', 'timezone', 'marketarea', 'campus')
            ",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(id, placetype)| Some((id.parse().ok()?, placetype)))
            .collect())
    }

    /// Retrieve a flat representation of all polygons in the database.
    /// This call can be 10GB+ of data.
    pub async fn all_polygons(&self) -> Result<Vec<PipWithGeometry>> {
        // Geometry is stored as spatialite blob, so decode to WKB (geopackage compatible).
        let rows = sqlx::query_as::<_, PipWithGeometry>(
            r"
                SELECT
                    place.source,
                    place.id,
                    place.class,
                    place.type,
                    AsGPB(shard.geom) as geom
                FROM shard
                LEFT JOIN place USING (source, id)
                WHERE place.source IS NOT NULL
                AND (
                    place.type != 'planet'
                    AND place.type != 'marketarea'
                    AND place.type != 'county'
                    AND place.type != 'timezone'
                )
            ",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }
}

/// A key-value pair from the WhosOnFirst database.
#[derive(Debug, Clone, Deserialize, sqlx::FromRow)]
pub struct WofKV {
    key: String,
    value: String,
}

/// A concise representation of a place in the WhosOnFirst database.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct ConcisePipResponse {
    /// WOF data source, usually wof
    pub source: String,

    /// WOF ID of the place
    pub id: String,

    /// High level bucket of human activity - https://whosonfirst.org/docs/categories/
    /// POINT-OF-VIEW > CLASS > CATEGORY
    pub class: String,

    pub r#type: String,
}

/// One of a place's names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct WofName {
    /// ISO 639-3, or `und` for the default name.
    pub lang: String,
    /// `default`, `preferred`, `variant` or `colloquial`.
    pub tag: String,
    /// Whether it's an abbreviation.
    pub abbr: bool,
    pub name: String,
}

impl WofName {
    /// Whether the place is searched by this name. Colloquial names are
    /// nicknames, like Seattle's "Coffee Capital of the World", that would
    /// make every place there match their words.
    #[must_use]
    pub fn is_searchable(&self) -> bool {
        self.tag != "colloquial"
    }
}

#[derive(Debug, Clone, Deserialize, sqlx::FromRow)]
pub struct PipPlaceName {
    pub lang: String,
    pub tag: String,
    // #[allow(dead_code)]
    // pub abbr: bool,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PipLangsResponse {
    #[serde(rename = "wof:lang_x_spoken")]
    pub langs: Option<String>,
}

/// Represents a place in the WhosOnFirst database with a geometry.
#[derive(sqlx::FromRow)]
pub struct PipWithGeometry {
    /// WOF data source, usually wof
    pub source: String,

    /// WOF ID of the place
    pub id: String,

    /// High level bucket of human activity - https://whosonfirst.org/docs/categories/
    /// POINT-OF-VIEW > CLASS > CATEGORY
    pub class: String,

    pub r#type: String,

    /// GeoPackage-encoded geometry blob.
    pub geom: Option<Vec<u8>>,
}

/// Convert from a list of key-value pairs to a PipLangsResponse.
impl From<Vec<WofKV>> for PipLangsResponse {
    fn from(value: Vec<WofKV>) -> Self {
        let mut langs = None;
        for kv in value {
            if kv.key == "wof:lang_x_spoken" {
                langs = Some(kv.value);
            }
        }
        Self { langs }
    }
}

/// Deconstruct a PipWithGeometry into a geometry and a concise response.
impl From<PipWithGeometry> for (Option<geo_types::Geometry<f64>>, ConcisePipResponse) {
    fn from(value: PipWithGeometry) -> Self {
        (
            value.geom.and_then(|blob| GpkgWkb(blob).to_geo().ok()),
            ConcisePipResponse {
                source: value.source,
                id: value.id,
                class: value.class,
                r#type: value.r#type,
            },
        )
    }
}
