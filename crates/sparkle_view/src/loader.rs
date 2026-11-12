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
//! The style, and the data of GeoJSON sources, can be replaced while tiles load
//! ([`TileLoader::set_style`], [`TileLoader::set_geojson`]); every tile is processed with
//! what was current when its processing starts, and comes back tagged with that
//! generation so the map can tell stale results.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
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
    /// A processed tile, with the generation of the style it was processed with.
    Loaded(usize, ProcessedTile, u64),
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

        // Made first, so that if a worker can't start, dropping it stops the others.
        let loader = Self {
            requests: requests.clone(),
            wanted: wanted.clone(),
            current: current.clone(),
            config: config.clone(),
            fetcher: fetcher.clone(),
        };
        let workers =
            thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(1).max(1));
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
            let (queue, wanted, deliver, config, current, glyphs, fetcher) = (
                requests.clone(),
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
                    while let Some(job) = next(&queue, &wanted) {
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
                                TileMessage::Loaded(source, tile, current.generation)
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
