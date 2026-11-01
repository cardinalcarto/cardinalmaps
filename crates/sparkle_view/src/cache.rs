//! A cache of fetched tiles (and TileJSON) on disk, keyed by URL: an SQLite file holding
//! each response's body as the server sent it, its ETag, and when it was fetched and last
//! used. The least recently used entries are evicted once it outgrows its size.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};

/// What [`DiskCache::get`] found.
pub struct Cached {
    /// `None` for a URL the server had nothing at.
    pub data: Option<Vec<u8>>,
    pub etag: Option<String>,
    /// Time since it was fetched or last revalidated.
    pub age: Duration,
}

pub struct DiskCache {
    conn: Mutex<Connection>,
    /// Bytes it may hold.
    capacity: u64,
    /// Bytes it holds.
    size: Mutex<u64>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl DiskCache {
    /// Open (or create) the cache in `dir`, or keep it in memory without one.
    pub fn open(dir: Option<&Path>, capacity: u64) -> Result<Self> {
        let conn = match dir {
            Some(dir) => {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
                let path = dir.join("tiles.sqlite");
                let conn = Connection::open(&path)
                    .with_context(|| format!("opening {}", path.display()))?;
                // Several apps may share the cache.
                conn.busy_timeout(Duration::from_secs(5))?;
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn.pragma_update(None, "synchronous", "NORMAL")?;
                conn
            }
            None => Connection::open_in_memory()?,
        };
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS responses (
                url TEXT PRIMARY KEY,
                data BLOB,
                etag TEXT,
                fetched INTEGER NOT NULL,
                used INTEGER NOT NULL,
                size INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS responses_used ON responses (used);",
        )?;
        let size: i64 =
            conn.query_row("SELECT COALESCE(SUM(size), 0) FROM responses", [], |r| {
                r.get(0)
            })?;
        let cache = Self {
            conn: Mutex::new(conn),
            capacity,
            size: Mutex::new(size as u64),
        };
        cache.evict()?;
        Ok(cache)
    }

    /// The response cached for `url`, marking it used.
    pub fn get(&self, url: &str) -> Result<Option<Cached>> {
        let conn = self.conn.lock().unwrap();
        let found = conn
            .prepare_cached("SELECT data, etag, fetched FROM responses WHERE url = ?1")?
            .query_row([url], |r| {
                Ok((
                    r.get::<_, Option<Vec<u8>>>(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .optional()?;
        let Some((data, etag, fetched)) = found else {
            return Ok(None);
        };
        let now = now();
        conn.prepare_cached("UPDATE responses SET used = ?2 WHERE url = ?1")?
            .execute((url, now))?;
        Ok(Some(Cached {
            data,
            etag,
            age: Duration::from_secs(now.saturating_sub(fetched) as u64),
        }))
    }

    /// Store the response for `url`: its body, or `None` if the server had nothing.
    pub fn put(&self, url: &str, data: Option<&[u8]>, etag: Option<&str>) -> Result<()> {
        let size = (url.len() + data.map_or(0, <[u8]>::len)) as u64;
        {
            let conn = self.conn.lock().unwrap();
            let old: Option<i64> = conn
                .prepare_cached("SELECT size FROM responses WHERE url = ?1")?
                .query_row([url], |r| r.get(0))
                .optional()?;
            let now = now();
            conn.prepare_cached(
                "INSERT OR REPLACE INTO responses (url, data, etag, fetched, used, size)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            )?
            .execute((url, data, etag, now, size as i64))?;
            let mut total = self.size.lock().unwrap();
            *total = (*total + size).saturating_sub(old.unwrap_or(0) as u64);
        }
        self.evict()
    }

    /// The server said the response cached for `url` is still current.
    pub fn revalidated(&self, url: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .prepare_cached("UPDATE responses SET fetched = ?2 WHERE url = ?1")?
            .execute((url, now()))?;
        Ok(())
    }

    /// Drop the least recently used responses until it fits in nine tenths of its
    /// capacity, so that it isn't evicting on every store.
    fn evict(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let mut total = self.size.lock().unwrap();
        if *total <= self.capacity {
            return Ok(());
        }
        let target = self.capacity / 10 * 9;
        let mut doomed = Vec::new();
        let mut left = *total;
        {
            let mut stmt = conn.prepare("SELECT url, size FROM responses ORDER BY used, rowid")?;
            let mut rows = stmt.query([])?;
            while left > target
                && let Some(row) = rows.next()?
            {
                doomed.push(row.get::<_, String>(0)?);
                left = left.saturating_sub(row.get::<_, i64>(1)? as u64);
            }
        }
        let tx = conn.unchecked_transaction()?;
        for url in &doomed {
            tx.execute("DELETE FROM responses WHERE url = ?1", [url])?;
        }
        tx.commit()?;
        log::debug!("tile cache: evicted {} responses", doomed.len());
        *total = left;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_the_least_recently_used() {
        let cache = DiskCache::open(None, 3000).unwrap();
        cache.put("a", Some(&[0; 1000]), None).unwrap();
        cache.put("b", Some(&[0; 1000]), Some("\"b\"")).unwrap();
        cache.put("c", None, None).unwrap();
        // Same second, so make `a` the most recently used by hand.
        let conn = cache.conn.lock().unwrap();
        conn.execute("UPDATE responses SET used = used + 10 WHERE url = 'a'", [])
            .unwrap();
        drop(conn);
        cache.put("d", Some(&[0; 1500]), None).unwrap();
        assert!(cache.get("b").unwrap().is_none());
        assert!(cache.get("a").unwrap().is_some());
        assert!(cache.get("d").unwrap().is_some());
        let empty = cache.get("c").unwrap().unwrap();
        assert!(empty.data.is_none() && empty.etag.is_none());
    }

    #[test]
    fn replacing_a_response_counts_its_size_once() {
        let cache = DiskCache::open(None, 10_000).unwrap();
        cache.put("a", Some(&[0; 4000]), None).unwrap();
        cache.put("a", Some(&[0; 4000]), None).unwrap();
        assert_eq!(*cache.size.lock().unwrap(), 4001);
        assert!(cache.get("a").unwrap().is_some());
    }
}
