use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use log::{trace, warn};
use redb::{Database, ReadableDatabase, TableDefinition};

const TABLE_NODE_LOCATION: TableDefinition<i64, (f64, f64)> =
    TableDefinition::new("admin_node_location");
/// Coordinates of ways that multipolygon relations need, as packed
/// little-endian (lon, lat) f64 pairs.
const TABLE_WAY_COORDS: TableDefinition<i64, &[u8]> = TableDefinition::new("way_coords_v1");
pub const BUFFER_SIZE: usize = 25000;

/// OSM coordinates that parsing needs later: node locations, for files without locations
/// on ways, and the ways multipolygon relations are made of.
pub struct IndexerCache {
    database: Database,
    buffer_size: AtomicUsize,
    buffer: Arc<RwLock<VecDeque<WofCacheItem>>>,
}

impl IndexerCache {
    /// Initialise a new indexer cache
    pub fn new(redb_path: &Path) -> Result<Self> {
        trace!("Opening cache database at {:?}", redb_path);
        let database = Database::create(redb_path)?;
        let txn = database.begin_write()?;
        txn.open_table(TABLE_NODE_LOCATION)?;
        txn.open_table(TABLE_WAY_COORDS)?;
        txn.commit()?;

        Ok(Self {
            database,
            buffer_size: AtomicUsize::new(BUFFER_SIZE),
            buffer: Arc::new(RwLock::new(VecDeque::new())),
        })
    }

    /// Lookup a node id in the cache and return the location
    pub fn query_node_location(&self, node_id: i64) -> Result<Option<(f64, f64)>> {
        let txn = self.database.begin_read()?;
        let table = txn.open_table(TABLE_NODE_LOCATION)?;
        if let Some(location) = table.get(node_id)? {
            return Ok(Some(location.value()));
        }
        Ok(None)
    }

    /// The (lon, lat) coordinates of a way, if cached.
    pub fn query_way_coords(&self, way: i64) -> Result<Option<Vec<(f64, f64)>>> {
        let txn = self.database.begin_read()?;
        let table = txn.open_table(TABLE_WAY_COORDS)?;
        Ok(table.get(way)?.map(|coords| {
            coords
                .value()
                .chunks_exact(16)
                .map(|pair| {
                    let lon = f64::from_le_bytes(pair[..8].try_into().unwrap());
                    let lat = f64::from_le_bytes(pair[8..].try_into().unwrap());
                    (lon, lat)
                })
                .collect()
        }))
    }

    /// Write an item to the cache, items will be written to a buffer
    /// and flushed to the database when the buffer is full.
    pub fn buffered_write_item(&self, item: WofCacheItem) -> Result<()> {
        let mut buffer = self.buffer.write().unwrap();
        buffer.push_back(item);

        if buffer.len() >= self.buffer_size.load(Ordering::Relaxed) {
            self.flush_buffer(&mut buffer)?;
        }
        Ok(())
    }

    /// Flush the buffer to the database
    fn flush_buffer(&self, buffer: &mut VecDeque<WofCacheItem>) -> Result<()> {
        if buffer.is_empty() {
            return Ok(());
        }

        trace!("Flushing cache buffer");

        let write = self.database.begin_write()?;
        {
            let mut locations_table = write.open_table(TABLE_NODE_LOCATION)?;
            let mut way_coords_table = write.open_table(TABLE_WAY_COORDS)?;

            for item in buffer.drain(..) {
                match item {
                    WofCacheItem::NodeLocation(node_id, location) => {
                        locations_table.insert(node_id, location)?;
                    }
                    WofCacheItem::WayCoords(way, coords) => {
                        let packed = coords
                            .iter()
                            .flat_map(|(lon, lat)| lon.to_le_bytes().into_iter().chain(lat.to_le_bytes()))
                            .collect::<Vec<_>>();
                        way_coords_table.insert(way, packed.as_slice())?;
                    }
                }
            }
        }
        write.commit()?;
        Ok(())
    }

    /// Writes buffered items to the database.
    pub fn flush(&self) -> Result<()> {
        let mut buffer = self.buffer.write().unwrap();
        self.flush_buffer(&mut buffer)
    }

    /// Set the buffer size for the cache and flush the buffer
    pub fn buffer_size(&self, size: usize) -> Result<()> {
        self.buffer_size.store(size, Ordering::Relaxed);
        self.flush()
    }

    /// Reset the buffer size to the default value
    pub fn buffer_size_default(&self) -> Result<()> {
        self.buffer_size.store(BUFFER_SIZE, Ordering::Relaxed);
        self.flush()
    }
}

/// Drop implementation for the cache, flushes the buffer to the database
/// as there is no guarantee that the buffer will be flushed before the cache is dropped.
impl Drop for IndexerCache {
    fn drop(&mut self) {
        if let Err(e) = self.flush() {
            warn!("Failed to flush cache: {}", e);
        }
    }
}
pub enum WofCacheItem {
    NodeLocation(i64, (f64, f64)),
    WayCoords(i64, Vec<(f64, f64)>),
}
