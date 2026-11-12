//! Reading and processing tiles on worker threads, off the render thread.
//!
//! One pool serves every source: a request names the source (by its index in the loader's
//! [`SourceConfig`] list) and the tile. Each worker opens its own reader per source.
//! GeoJSON sources are cut from their in-memory index instead. A tile server's tiles come
//! from its cache, or else are handed to a [`Fetcher`] and processed once they arrive, so
//! no worker waits on the network. A tile server source can also have a local MBTiles
//! file that is read first, so only the tiles it lacks are fetched.
//!
//! Workers and fetchers take the tiles the view wants most first: the view says, each
//! frame, which tiles it wants and in what order ([`TileLoader::set_wanted`]), and queued
//! tiles it no longer wants are dropped.
//!
//! A processed tile can take tens of megabytes, so there are only so many at once: a
//! worker processes one only while fewer than one per worker are being processed or
//! waiting for the map to take them in ([`TileMessage::Loaded`] holds its [`Slot`]).
//! Tiles the map is slow to take in, as when it isn't drawn, hold the workers back
//! instead of piling up.
//!
//! The style, and the data of GeoJSON sources, can be replaced while tiles load
//! ([`TileLoader::set_style`], [`TileLoader::set_geojson`]); every tile is processed with
//! what was current when its processing starts, and comes back tagged with that
//! generation so the map can tell stale results.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;

use anyhow::Result;
use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use sparkle::geojson::GeoJsonIndex;
use sparkle::glyphs::FontLibrary;
use sparkle::sprite::ImageAtlas;
use sparkle::{ProcessedTile, Style, TileId, TileProcessor, TileResources};

use crate::fetch::{FetchJob, Fetcher};
use crate::mbtiles::{MbTiles, TileFormat, decompress};
use crate::queue::{Queue, lowest};
use crate::remote::{Fetched, REVALIDATE_AFTER, TileServer};

/// A tile of one source: `(source index, tile)`.
pub type SourceTile = (usize, TileId);

/// Why the view wants a tile, the tiers it loads them in: all of one before any of the
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// It covers the view.
    Visible,
    /// An ancestor of a visible tile, to stand in for it while it loads.
    Fallback,
    /// It covers the view where the motion under way comes to rest.
    Destination,
    /// It's just past the edge of the view.
    Ring,
}

impl Tier {
    /// Wanted for later rather than for the view as it is.
    pub fn predicted(self) -> bool {
        self != Self::Visible
    }
}

/// Where a wanted tile stands: its place in the view's order, and its tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Want {
    pub rank: u32,
    pub tier: Tier,
}

/// The tiles the view wants, shared by the workers and fetchers.
pub type Wanted = Arc<Mutex<HashMap<SourceTile, Want>>>;

/// How a queue of jobs for `tile` ranks one: unwanted ones first, to be dropped, then
/// wanted ones in the view's order.
pub(crate) fn rank(wanted: &HashMap<SourceTile, Want>, tile: &SourceTile) -> (bool, u32) {
    wanted.get(tile).map_or((false, 0), |w| (true, w.rank))
}

pub enum TileMessage {
    /// A processed tile, with the generation of the style it was processed with, and the
    /// slot it takes until it's dropped.
    Loaded(usize, ProcessedTile, u64, Slot),
    /// The tileset has no such tile, or it could not be read; either way there's nothing to
    /// draw. A tile server that can't be reached is tried again instead.
    Empty(SourceTile),
    /// Dropped from the queue because it was no longer wanted by the time a worker got to it.
    Cancelled(SourceTile),
}

/// Where one style source's tiles come from.
pub struct SourceConfig {
    /// The style's source id.
    pub id: String,
    /// The MBTiles file, or for a GeoJSON source the file its data came from (if any).
    /// Unused for tile servers.
    pub path: PathBuf,
    /// Unused for GeoJSON sources.
    pub format: TileFormat,
    /// For an image source, the one tile its image file is drawn in; `path` is the image
    /// rather than an MBTiles file.
    pub image_tile: Option<TileId>,
    /// Set for GeoJSON sources, whose tiles are cut from this instead of read from `path`
    /// (until [`TileLoader::set_geojson`] replaces it).
    pub geojson: Option<Arc<GeoJsonIndex>>,
    /// Set for sources fetched from a tile server instead of read from `path`.
    pub server: Option<Arc<TileServer>>,
    /// For a tile server, an MBTiles file of its tiles to read first, fetching only those
    /// it lacks. It needn't exist yet, and may gain tiles as the map runs.
    pub local: Option<PathBuf>,
}

pub struct LoaderConfig {
    pub style: Arc<Style>,
    pub sources: Vec<SourceConfig>,
    /// Sprite images for icons and patterns, if any were loaded.
    pub images: Option<Arc<ImageAtlas>>,
    /// A directory of local fonts to rasterize glyphs from, searched before the system's.
    pub fonts: Option<PathBuf>,
}

/// The style workers process tiles with, and the data of GeoJSON sources.
struct CurrentStyle {
    generation: u64,
    style: Arc<Style>,
    /// Per source, the source layers some style layer draws: only those are worth
    /// converting from MVT.
    used_layers: Vec<HashSet<String>>,
    /// Per source, the index GeoJSON sources' tiles are cut from.
    geojson: Vec<Option<Arc<GeoJsonIndex>>>,
    /// Sprite images for icons and patterns.
    images: Option<Arc<ImageAtlas>>,
}

impl CurrentStyle {
    fn new(
        generation: u64,
        style: Arc<Style>,
        sources: &[SourceConfig],
        geojson: Vec<Option<Arc<GeoJsonIndex>>>,
        images: Option<Arc<ImageAtlas>>,
    ) -> Self {
        let used_layers = sources
            .iter()
            .map(|source| {
                style
                    .layers
                    .iter()
                    .filter(|l| l.source.as_deref() == Some(&source.id))
                    .filter_map(|l| l.source_layer.clone())
                    .collect()
            })
            .collect();
        Self {
            generation,
            style,
            used_layers,
            geojson,
            images,
        }
    }
}

/// What the workers do: always with a tile and the zoom to evaluate the style at (see
/// [`TileProcessor::process_at`]).
enum Job {
    /// Read the tile (or have it fetched) and process it.
    Load(SourceTile, u8),
    /// Process a tile that was fetched. `refresh` if a cached tile, drawn already, turned
    /// out to have changed.
    Process(SourceTile, u8, Vec<u8>, bool),
}

pub struct TileLoader {
    /// Closed when the loader is dropped, which stops the workers.
    requests: Arc<Queue<Job>>,
    /// Closed with `requests`.
    slots: Arc<Slots>,
    /// Tiles the view currently needs, and in what order. Workers skip queued requests
    /// that aren't in here.
    wanted: Wanted,
    current: Arc<RwLock<Arc<CurrentStyle>>>,
    config: Arc<LoaderConfig>,
    fetcher: Option<Arc<Fetcher>>,
}

impl Drop for TileLoader {
    fn drop(&mut self) {
        self.requests.close();
        self.slots.close();
    }
}

impl TileLoader {
    pub fn spawn(
        config: LoaderConfig,
        deliver: impl Fn(TileMessage) + Send + Sync + 'static,
    ) -> Result<Self> {
        let requests = Arc::new(Queue::default());
        let wanted: Wanted = Arc::default();
        let deliver = Arc::new(deliver);
        let current = Arc::new(RwLock::new(Arc::new(CurrentStyle::new(
            0,
            config.style.clone(),
            &config.sources,
            config.sources.iter().map(|s| s.geojson.clone()).collect(),
            config.images.clone(),
        ))));
        // Fonts are found and read as text first needs them, shared by all workers.
        let glyphs = Arc::new(FontLibrary::new(config.fonts.clone()));
        let config = Arc::new(config);
        let fetcher = match config.sources.iter().any(|s| s.server.is_some()) {
            true => Some(Arc::new(fetcher(
                requests.clone(),
                wanted.clone(),
                deliver.clone(),
            )?)),
            false => None,
        };

        let workers = workers();
        let slots = Arc::new(Slots::new(workers));
        // Made first, so that if a worker can't start, dropping it stops the others.
        let loader = Self {
            requests: requests.clone(),
            slots: slots.clone(),
            wanted: wanted.clone(),
            current: current.clone(),
            config: config.clone(),
            fetcher: fetcher.clone(),
        };
        for i in 0..workers {
            let readers = config
                .sources
                .iter()
                .map(|s| match (&s.geojson, s.image_tile, &s.server) {
                    (None, None, None) => MbTiles::open(&s.path).map(Some),
                    _ => Ok(None),
                })
                .collect::<Result<Vec<_>>>()?;
            let mut locals: Vec<Option<LocalTiles>> = config
                .sources
                .iter()
                .map(|s| s.local.clone().map(LocalTiles::new))
                .collect();
            let (queue, slots, wanted, deliver, config, current, glyphs, fetcher) = (
                requests.clone(),
                slots.clone(),
                wanted.clone(),
                deliver.clone(),
                config.clone(),
                current.clone(),
                glyphs.clone(),
                fetcher.clone(),
            );
            thread::Builder::new()
                .name(format!("tile-worker-{i}"))
                .spawn(move || {
                    let mut processor = TileProcessor::new();
                    while let Some(slot) = slots.take()
                        && let Some(job) = next(&queue, &wanted)
                    {
                        let (Job::Load(request, zoom) | Job::Process(request, zoom, ..)) = job;
                        if !wanted.lock().unwrap().contains_key(&request) {
                            // A refresh is of a tile the map has, and isn't waiting for.
                            if !matches!(job, Job::Process(.., true)) {
                                deliver(TileMessage::Cancelled(request));
                            }
                            continue;
                        }
                        let (source, id) = request;
                        let current = current.read().unwrap().clone();
                        let mut worker = Worker {
                            reader: readers[source].as_ref(),
                            local: locals[source].as_mut(),
                            fetcher: fetcher.as_deref(),
                            style: &current.style,
                            source: &config.sources[source],
                            geojson: current.geojson[source].as_ref(),
                            used_layers: &current.used_layers[source],
                            images: current.images.as_deref(),
                            glyphs: &glyphs,
                        };
                        let fetched = match job {
                            Job::Load(..) => None,
                            Job::Process(_, _, data, _) => Some(data),
                        };
                        let message = match worker.load(&mut processor, request, zoom, fetched) {
                            Ok(Loaded::Tile(tile)) => {
                                TileMessage::Loaded(source, tile, current.generation, slot)
                            }
                            Ok(Loaded::Empty) => TileMessage::Empty(request),
                            // Processed once it arrives.
                            Ok(Loaded::Fetching) => continue,
                            Err(err) => {
                                log::warn!(
                                    "{} tile {}/{}/{}: {err:#}",
                                    config.sources[source].id,
                                    id.z,
                                    id.x,
                                    id.y
                                );
                                TileMessage::Empty(request)
                            }
                        };
                        deliver(message);
                    }
                })?;
        }
        Ok(loader)
    }

    /// Process tiles requested from now on with `style`. Returns the new style's
    /// generation, which [`TileMessage::Loaded`] carries.
    pub fn set_style(&self, style: Arc<Style>) -> u64 {
        let mut current = self.current.write().unwrap();
        let generation = current.generation + 1;
        let geojson = current.geojson.clone();
        *current = Arc::new(CurrentStyle::new(
            generation,
            style,
            &self.config.sources,
            geojson,
            current.images.clone(),
        ));
        generation
    }

    /// Process tiles from now on with `images`, which must have what the current ones
    /// have in the same places (see [`ImageAtlas::with_images`]).
    pub fn set_images(&self, images: Arc<ImageAtlas>) {
        let mut current = self.current.write().unwrap();
        *current = Arc::new(CurrentStyle::new(
            current.generation,
            current.style.clone(),
            &self.config.sources,
            current.geojson.clone(),
            Some(images),
        ));
    }

    /// Cut tiles of GeoJSON source `source` requested from now on from `index`. Returns
    /// the new generation, which [`TileMessage::Loaded`] carries.
    pub fn set_geojson(&self, source: usize, index: Arc<GeoJsonIndex>) -> u64 {
        let mut current = self.current.write().unwrap();
        let generation = current.generation + 1;
        let mut geojson = current.geojson.clone();
        geojson[source] = Some(index);
        *current = Arc::new(CurrentStyle::new(
            generation,
            current.style.clone(),
            &self.config.sources,
            geojson,
            current.images.clone(),
        ));
        generation
    }

    /// Replace the tiles the view needs with `tiles`, most needed first; a tile listed
    /// twice keeps its first place.
    pub fn set_wanted(&self, tiles: impl IntoIterator<Item = (SourceTile, Tier)>) {
        {
            let mut wanted = self.wanted.lock().unwrap();
            wanted.clear();
            for (tile, tier) in tiles {
                let rank = wanted.len() as u32;
                wanted.entry(tile).or_insert(Want { rank, tier });
            }
        }
        if let Some(fetcher) = &self.fetcher {
            fetcher.wake();
        }
    }

    /// Process `tile` with the style evaluated at `zoom`.
    pub fn request(&self, tile: SourceTile, zoom: u8) {
        self.requests.push(Job::Load(tile, zoom));
    }
}

/// How many threads process tiles: all but one of the CPUs, for the one drawing.
fn workers() -> usize {
    thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(1).max(1))
}

/// Places for the tiles workers process, taken until the map takes the tile in.
struct Slots {
    /// How many are free, and whether the loader is gone.
    state: Mutex<(usize, bool)>,
    freed: Condvar,
}

impl Slots {
    fn new(count: usize) -> Self {
        Self {
            state: Mutex::new((count, false)),
            freed: Condvar::new(),
        }
    }

    /// A free slot, once there is one; `None` once the loader is gone.
    fn take(self: &Arc<Self>) -> Option<Slot> {
        let mut state = self.state.lock().unwrap();
        loop {
            match *state {
                (_, true) => return None,
                (0, false) => state = self.freed.wait(state).unwrap(),
                (ref mut free, false) => {
                    *free -= 1;
                    return Some(Slot(self.clone()));
                }
            }
        }
    }

    fn close(&self) {
        self.state.lock().unwrap().1 = true;
        self.freed.notify_all();
    }
}

/// A place for one processed tile, freed when it's dropped.
pub struct Slot(Arc<Slots>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().0 += 1;
        self.0.freed.notify_one();
    }
}

/// A fetcher that hands what it fetches to the workers through `requests`.
fn fetcher(
    requests: Arc<Queue<Job>>,
    wanted: Wanted,
    deliver: Arc<impl Fn(TileMessage) + Send + Sync + 'static>,
) -> Result<Fetcher> {
    Fetcher::spawn(wanted, move |job, fetched| {
        let job = match fetched {
            Some(Fetched::Tile(data)) => Job::Process(job.tile, job.zoom, data, job.refresh),
            Some(Fetched::Empty) => return deliver(TileMessage::Empty(job.tile)),
            Some(Fetched::Unchanged) => return,
            None if job.refresh => return,
            None => return deliver(TileMessage::Cancelled(job.tile)),
        };
        // Dropped once the loader is gone.
        requests.push(job);
    })
}

/// The job for the tile the view wants most, or one it no longer wants, to drop.
fn next(queue: &Queue<Job>, wanted: &Mutex<HashMap<SourceTile, Want>>) -> Option<Job> {
    queue.pop(|jobs| {
        let wanted = wanted.lock().unwrap();
        lowest(jobs, |job| {
            let (Job::Load(tile, _) | Job::Process(tile, ..)) = job;
            Some(rank(&wanted, tile))
        })
    })
}

enum Loaded {
    Tile(ProcessedTile),
    /// There's nothing to draw.
    Empty,
    /// It's being fetched, and is processed once it arrives.
    Fetching,
}

impl Loaded {
    fn from(tile: ProcessedTile, drawn: bool) -> Self {
        if drawn { Self::Tile(tile) } else { Self::Empty }
    }
}

/// A tile server's local MBTiles file, opened once it exists.
struct LocalTiles {
    path: PathBuf,
    reader: Option<MbTiles>,
}

impl LocalTiles {
    fn new(path: PathBuf) -> Self {
        Self { path, reader: None }
    }

    /// The tile, if the file exists and has it.
    fn tile(&mut self, id: TileId) -> Option<Vec<u8>> {
        if self.reader.is_none() {
            // Opening read-only doesn't create it.
            if !self.path.exists() {
                return None;
            }
            match MbTiles::open(&self.path) {
                Ok(reader) => self.reader = Some(reader),
                Err(err) => {
                    log::debug!("{err:#}");
                    return None;
                }
            }
        }
        // A file just created may not have its tables yet.
        let reader = self.reader.as_ref()?;
        reader
            .tile(id)
            .inspect_err(|err| log::debug!("{}: {err:#}", self.path.display()))
            .ok()?
    }
}

struct Worker<'a> {
    /// `None` for image, GeoJSON and tile server sources.
    reader: Option<&'a MbTiles>,
    /// A tile server's local tiles, if it has any.
    local: Option<&'a mut LocalTiles>,
    /// Set if some source is a tile server.
    fetcher: Option<&'a Fetcher>,
    style: &'a Style,
    source: &'a SourceConfig,
    /// The index of a GeoJSON source.
    geojson: Option<&'a Arc<GeoJsonIndex>>,
    used_layers: &'a HashSet<String>,
    images: Option<&'a ImageAtlas>,
    glyphs: &'a FontLibrary,
}

impl Worker<'_> {
    /// Read and process a tile, or process its `fetched` data.
    fn load(
        &mut self,
        processor: &mut TileProcessor,
        request: SourceTile,
        zoom: u8,
        fetched: Option<Vec<u8>>,
    ) -> Result<Loaded> {
        let (_, id) = request;
        let resources = TileResources {
            images: self.images,
            glyphs: Some(self.glyphs),
        };
        if let Some(index) = self.geojson {
            let tile = processor.process_geojson_at(
                self.style,
                &self.source.id,
                id,
                zoom,
                index,
                resources,
            );
            let drawn = !tile.buckets.is_empty();
            return Ok(Loaded::from(tile, drawn));
        }
        let data = match (
            fetched,
            self.reader,
            self.source.image_tile,
            &self.source.server,
        ) {
            (Some(data), ..) => Some(data),
            (None, Some(reader), ..) => reader.tile(id)?,
            (None, None, Some(tile), _) if tile == id => Some(std::fs::read(&self.source.path)?),
            (None, None, _, Some(server)) => {
                if let Some(data) = self.local.as_mut().and_then(|l| l.tile(id)) {
                    return self.process(processor, id, zoom, data, resources);
                }
                let Some(cached) = server.cached(id)? else {
                    let fetcher = self.fetcher.expect("spawned for tile servers");
                    fetcher.fetch(FetchJob::new(request, zoom, server.clone()));
                    return Ok(Loaded::Fetching);
                };
                if cached.age > REVALIDATE_AFTER && server.start_revalidating(id) {
                    let fetcher = self.fetcher.expect("spawned for tile servers");
                    let job = FetchJob::refresh(request, zoom, server.clone(), cached.etag);
                    fetcher.fetch(job);
                }
                cached.data
            }
            (None, None, ..) => None,
        };
        let Some(data) = data else {
            return Ok(Loaded::Empty);
        };
        self.process(processor, id, zoom, data, resources)
    }

    fn process(
        &self,
        processor: &mut TileProcessor,
        id: TileId,
        zoom: u8,
        data: Vec<u8>,
        resources: TileResources<'_>,
    ) -> Result<Loaded> {
        // MBTiles readers decompress already; servers' tiles are cached as sent.
        let data = match self.source.server {
            Some(_) => decompress(data)?,
            None => data,
        };
        let data = match self.source.format {
            TileFormat::Mlt | TileFormat::Image => data,
            TileFormat::Mvt => mvt_to_mlt(&data, self.used_layers)?,
        };
        let tile = processor.process_at(self.style, &self.source.id, id, zoom, &data, resources)?;
        // Terrain tiles matter even when no layer draws them.
        let drawn = !tile.buckets.is_empty() || tile.dem.is_some();
        Ok(Loaded::from(tile, drawn))
    }
}

/// Re-encode an MVT tile as MLT, keeping only the layers in `keep`.
fn mvt_to_mlt(data: &[u8], keep: &HashSet<String>) -> Result<Vec<u8>> {
    // Encoding speed matters more than size here, so skip the encoder's search over feature
    // orders. Triangles are left out because sparkle cuts polygons itself when they're missing.
    let cfg = EncoderConfig::from_profile(Profile::Native(Network::Offline))
        .with_tessellation(Tessellation::Off)
        .with_spatial_morton_sort(false)
        .with_spatial_hilbert_sort(false)
        .with_id_sort(false);
    let mut out = Vec::new();
    for layer in mlt_core::mvt::mvt_to_tile_layers(data)? {
        if keep.contains(layer.name()) {
            out.extend(layer.encode(cfg)?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;
    use sparkle::style::Source;

    use super::*;

    /// A loader of a GeoJSON source that covers the world, so every tile has something to
    /// draw, and the messages it delivers.
    fn world_loader() -> (TileLoader, Arc<Mutex<Vec<TileMessage>>>) {
        let style: Style = json!({
            "version": 8,
            "sources": {"world": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []}}},
            "layers": [{"id": "land", "type": "fill", "source": "world"}],
        })
        .to_string()
        .parse()
        .unwrap();
        let Some(Source::GeoJson(source)) = style.source("world") else {
            unreachable!()
        };
        let world = json!({"type": "Polygon", "coordinates": [[
            [-179.0, -80.0], [179.0, -80.0], [179.0, 80.0], [-179.0, 80.0], [-179.0, -80.0]
        ]]});
        let index = Arc::new(GeoJsonIndex::new(source, &world).unwrap());
        let config = LoaderConfig {
            style: Arc::new(style),
            sources: vec![SourceConfig {
                id: "world".into(),
                path: PathBuf::new(),
                format: TileFormat::Mlt,
                image_tile: None,
                geojson: Some(index),
                server: None,
                local: None,
            }],
            images: None,
            fonts: None,
        };
        let messages = Arc::new(Mutex::new(Vec::new()));
        let sink = messages.clone();
        let loader = TileLoader::spawn(config, move |m| sink.lock().unwrap().push(m)).unwrap();
        (loader, messages)
    }

    fn loaded(messages: &Mutex<Vec<TileMessage>>) -> usize {
        let messages = messages.lock().unwrap();
        messages
            .iter()
            .filter(|m| matches!(m, TileMessage::Loaded(..)))
            .count()
    }

    /// Wait until `messages` holds `count` processed tiles, or a while longer than that
    /// should take.
    fn wait_for(messages: &Mutex<Vec<TileMessage>>, count: usize) -> usize {
        let deadline = Instant::now() + Duration::from_secs(10);
        while loaded(messages) < count && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        // Long enough for workers that shouldn't deliver more to have done so.
        thread::sleep(Duration::from_millis(200));
        loaded(messages)
    }

    #[test]
    fn tiles_not_taken_in_hold_the_workers_back() {
        let (loader, messages) = world_loader();
        let tiles: Vec<SourceTile> = (0..64).map(|x| (0, TileId::new(6, x, 30))).collect();
        loader.set_wanted(tiles.iter().map(|&t| (t, Tier::Visible)));
        for &tile in &tiles {
            loader.request(tile, 6);
        }
        // Only as many as there are workers, however many are wanted.
        let workers = workers();
        assert_eq!(wait_for(&messages, workers), workers);
        // Taking one in frees its slot for another.
        let taken = messages.lock().unwrap().remove(0);
        drop(taken);
        assert_eq!(wait_for(&messages, workers), workers);
        // Taking all in lets the rest load.
        let mut total = 1;
        while total < tiles.len() {
            let taken = std::mem::take(&mut *messages.lock().unwrap());
            total += taken
                .iter()
                .filter(|m| matches!(m, TileMessage::Loaded(..)))
                .count();
            drop(taken);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(total, tiles.len());
    }
}
