//! The device's offline map: one MBTiles file that every region's tiles go into.
//!
//! It is a standard MBTiles (`metadata` and `tiles`, rows counted from the bottom), so
//! sparkle reads it as `--local-tiles`, plus `tile_regions`: which regions each tile is in.
//! A tile shared by overlapping regions is stored once and removed only when the last region
//! holding it is.
//!
//! Tiles are stored as the archive has them (gzipped); each is checked first by inflating it
//! once, which checks the gzip trailer's CRC-32 and length.
//!
//! The file is in WAL mode, so the map reads it while a download writes it. It must be
//! written through the same SQLite library that sparkle reads it with: two copies of SQLite
//! in one process don't see each other's POSIX locks.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::Error;
use crate::plan::{ChunkTile, PlannedTile};
use crate::pmtiles::{Compression, Header};

/// How long a write waits for another writer (the database is written from one place, but a
/// region's chunks may unpack while another region is deleted).
const BUSY_TIMEOUT: Duration = Duration::from_secs(30);

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS metadata (name TEXT PRIMARY KEY, value TEXT);
    CREATE TABLE IF NOT EXISTS tiles (
        zoom_level INTEGER NOT NULL,
        tile_column INTEGER NOT NULL,
        tile_row INTEGER NOT NULL,
        tile_data BLOB NOT NULL
    );
    CREATE UNIQUE INDEX IF NOT EXISTS tile_index ON tiles (zoom_level, tile_column, tile_row);
    CREATE TABLE IF NOT EXISTS tile_regions (
        region INTEGER NOT NULL,
        zoom_level INTEGER NOT NULL,
        tile_column INTEGER NOT NULL,
        tile_row INTEGER NOT NULL,
        PRIMARY KEY (region, zoom_level, tile_column, tile_row)
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS tile_regions_tile
        ON tile_regions (zoom_level, tile_column, tile_row);
";

/// MBTiles rows count from the bottom.
fn tms_row(z: u8, y: u32) -> u32 {
    (1u32 << z) - 1 - y
}

/// Whether `data` is one whole gzip member whose CRC-32 and length match its contents.
pub fn gzip_is_whole(data: &[u8]) -> bool {
    let mut rest = data;
    let mut decoder = flate2::bufread::GzDecoder::new(&mut rest);
    let mut sink = [0u8; 16_384];
    loop {
        match decoder.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    drop(decoder);
    rest.is_empty()
}

/// What unpacking a chunk wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unpacked {
    /// Tiles written (each tile in the chunk, including repeats).
    pub tiles: u64,
    /// Their bytes, together.
    pub bytes: u64,
}

/// The offline map file, open for writing.
pub struct Basemap {
    conn: Connection,
}

impl Basemap {
    /// Opens the file, creating it and its tables if needed.
    ///
    /// # Errors
    ///
    /// If it can't be opened or isn't an MBTiles this wrote.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // Only takes effect before the first table is made: space from deleted regions'
        // tiles is then given back by `incremental_vacuum`.
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Writes the metadata that readers need, from the archive's header.
    ///
    /// # Errors
    ///
    /// If it can't be written.
    pub fn set_metadata(&self, header: &Header, name: &str) -> Result<(), Error> {
        let format = match header.tile_type {
            1 => "pbf",
            2 => "png",
            3 => "jpg",
            4 => "webp",
            6 => "mlt",
            other => return Err(Error::UnsupportedTileType(other)),
        };
        let [w, s, e, n] = header.bounds;
        let (lng, lat, zoom) = header.center;
        let rows = [
            ("name", name.to_owned()),
            ("format", format.to_owned()),
            ("type", "baselayer".to_owned()),
            ("minzoom", header.min_zoom.to_string()),
            ("maxzoom", header.max_zoom.to_string()),
            ("bounds", format!("{w},{s},{e},{n}")),
            ("center", format!("{lng},{lat},{zoom}")),
        ];
        let tx = self.conn.unchecked_transaction()?;
        for (key, value) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO metadata (name, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Which of `tiles` the file has.
    ///
    /// # Errors
    ///
    /// If it can't be read.
    pub fn present(&self, tiles: &[PlannedTile]) -> Result<HashSet<(u8, u32, u32)>, Error> {
        let mut statement = self.conn.prepare_cached(
            "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
        )?;
        let mut found = HashSet::new();
        for t in tiles {
            if statement.exists(params![t.z, t.x, tms_row(t.z, t.y)])? {
                found.insert((t.z, t.x, t.y));
            }
        }
        Ok(found)
    }

    /// Writes a downloaded chunk's tiles, as in `region`.
    ///
    /// The chunk must be `expected_length` bytes, and with `gzip`, every tile a whole gzip
    /// member; otherwise nothing is written, and the error says which check failed.
    ///
    /// # Errors
    ///
    /// [`Error::ChunkLength`], [`Error::BadTile`], or a database error.
    pub fn unpack(
        &mut self,
        chunk: &[u8],
        expected_length: u64,
        tiles: &[ChunkTile],
        region: i64,
        compression: Compression,
    ) -> Result<Unpacked, Error> {
        if chunk.len() as u64 != expected_length {
            return Err(Error::ChunkLength {
                expected: expected_length,
                found: chunk.len() as u64,
            });
        }
        let mut checked: HashMap<(u32, u32), bool> = HashMap::new();
        for t in tiles {
            let start = t.offset as usize;
            let Some(data) = chunk.get(start..start + t.length as usize) else {
                return Err(Error::BadTileList);
            };
            let good = *checked
                .entry((t.offset, t.length))
                .or_insert_with(|| compression != Compression::Gzip || gzip_is_whole(data));
            if !good {
                return Err(Error::BadTile {
                    z: t.z,
                    x: t.x,
                    y: t.y,
                });
            }
        }
        let tx = self.conn.transaction()?;
        let mut written = Unpacked::default();
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO tiles (zoom_level, tile_column, tile_row, tile_data)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut member = tx.prepare_cached(
                "INSERT OR IGNORE INTO tile_regions (region, zoom_level, tile_column, tile_row)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for t in tiles {
                let start = t.offset as usize;
                let data = &chunk[start..start + t.length as usize];
                let row = tms_row(t.z, t.y);
                insert.execute(params![t.z, t.x, row, data])?;
                member.execute(params![region, t.z, t.x, row])?;
                written.tiles += 1;
                written.bytes += u64::from(t.length);
            }
        }
        tx.commit()?;
        Ok(written)
    }

    /// Adds `region` to the tiles of `keys` that the file has (tiles another region brought),
    /// and returns those it doesn't have.
    ///
    /// # Errors
    ///
    /// If the database can't be written.
    pub fn claim(
        &mut self,
        region: i64,
        keys: &[(u8, u32, u32)],
    ) -> Result<Vec<(u8, u32, u32)>, Error> {
        let tx = self.conn.transaction()?;
        let mut missing = Vec::new();
        {
            let mut exists = tx.prepare_cached(
                "SELECT 1 FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
            )?;
            let mut member = tx.prepare_cached(
                "INSERT OR IGNORE INTO tile_regions (region, zoom_level, tile_column, tile_row)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for &(z, x, y) in keys {
                let row = tms_row(z, y);
                if exists.exists(params![z, x, row])? {
                    member.execute(params![region, z, x, row])?;
                } else {
                    missing.push((z, x, y));
                }
            }
        }
        tx.commit()?;
        Ok(missing)
    }

    /// Takes `region` off its tiles, deletes the tiles no region holds any more, and gives
    /// their space back. Returns how many tiles were deleted.
    ///
    /// # Errors
    ///
    /// If the database can't be written.
    pub fn release(&mut self, region: i64) -> Result<u64, Error> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS released (
                 zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER);
             DELETE FROM temp.released;",
        )?;
        tx.execute(
            "INSERT INTO temp.released
             SELECT zoom_level, tile_column, tile_row FROM tile_regions WHERE region = ?1",
            params![region],
        )?;
        tx.execute(
            "DELETE FROM tile_regions WHERE region = ?1",
            params![region],
        )?;
        let removed = tx.execute(
            "DELETE FROM tiles WHERE rowid IN (
                 SELECT tiles.rowid FROM temp.released r
                 JOIN tiles USING (zoom_level, tile_column, tile_row)
                 WHERE NOT EXISTS (
                     SELECT 1 FROM tile_regions m
                     WHERE m.zoom_level = r.zoom_level AND m.tile_column = r.tile_column
                       AND m.tile_row = r.tile_row))",
            [],
        )?;
        tx.execute("DELETE FROM temp.released", [])?;
        tx.commit()?;
        self.conn.execute_batch("PRAGMA incremental_vacuum;")?;
        Ok(removed as u64)
    }

    /// How many tiles `region` holds.
    ///
    /// # Errors
    ///
    /// If the database can't be read.
    pub fn region_tiles(&self, region: i64) -> Result<u64, Error> {
        let count: Option<i64> = self
            .conn
            .query_row(
                "SELECT count(*) FROM tile_regions WHERE region = ?1",
                params![region],
                |row| row.get(0),
            )
            .optional()?;
        Ok(count.unwrap_or(0).try_into().unwrap_or(0))
    }

    /// How many tiles the file has.
    ///
    /// # Errors
    ///
    /// If the database can't be read.
    pub fn tile_count(&self) -> Result<u64, Error> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM tiles", [], |row| row.get(0))?;
        Ok(count.try_into().unwrap_or(0))
    }
}
