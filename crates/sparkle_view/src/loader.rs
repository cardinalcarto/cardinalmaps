//! Reading and processing tiles on worker threads, off the render thread.
//!
//! One pool serves every source: a request names the source (by its index in the loader's
//! [`SourceConfig`] list) and the tile. Each worker opens its own reader per source.
//! GeoJSON sources are cut from their in-memory index instead. A tile server's tiles come
//! from its cache, or else are handed to a [`Fetcher`] and processed once they arrive, so
//! no worker waits on the network.
//!
//! The style, and the data of GeoJSON sources, can be replaced while tiles load
//! ([`TileLoader::set_style`], [`TileLoader::set_geojson`]); every tile is processed with
//! what was current when its processing starts, and comes back tagged with that
//! generation so the map can tell stale results.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;

use anyhow::Result;
use mlt_core::encoder::{EncoderConfig, Network, Profile, Tessellation};
use sparkle::geojson::GeoJsonIndex;
use sparkle::glyphs::FontLibrary;
use sparkle::sprite::ImageAtlas;
use sparkle::{ProcessedTile, Style, TileId, TileProcessor, TileResources};

use crate::fetch::{FetchJob, Fetcher};
use crate::mbtiles::{MbTiles, TileFormat, decompress};
use crate::remote::{Fetched, REVALIDATE_AFTER, TileServer};

/// A tile of one source: `(source index, tile)`.
pub type SourceTile = (usize, TileId);

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
}

impl CurrentStyle {
    fn new(
        generation: u64,
        style: Arc<Style>,
        sources: &[SourceConfig],
        geojson: Vec<Option<Arc<GeoJsonIndex>>>,
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
    /// The only strong reference, so that dropping the loader stops the workers: the
    /// fetcher's threads hold weak ones.
    requests: Arc<Sender<Job>>,
    /// Tiles the view currently needs. Workers skip queued requests that aren't in here.
    wanted: Arc<Mutex<HashSet<SourceTile>>>,
    current: Arc<RwLock<Arc<CurrentStyle>>>,
    config: Arc<LoaderConfig>,
}

impl TileLoader {
    pub fn spawn(
        config: LoaderConfig,
        deliver: impl Fn(TileMessage) + Send + Sync + 'static,
    ) -> Result<Self> {
        let (requests, queue) = mpsc::channel::<Job>();
        let requests = Arc::new(requests);
        let queue = Arc::new(Mutex::new(queue));
        let wanted = Arc::new(Mutex::new(HashSet::new()));
        let deliver = Arc::new(deliver);
        let current = Arc::new(RwLock::new(Arc::new(CurrentStyle::new(
            0,
            config.style.clone(),
            &config.sources,
            config.sources.iter().map(|s| s.geojson.clone()).collect(),
        ))));
        // Fonts are found and read as text first needs them, shared by all workers.
        let glyphs = Arc::new(FontLibrary::new(config.fonts.clone()));
        let config = Arc::new(config);
        let fetcher = match config.sources.iter().any(|s| s.server.is_some()) {
            true => Some(Arc::new(fetcher(
                Arc::downgrade(&requests),
                wanted.clone(),
                deliver.clone(),
            )?)),
            false => None,
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
            let (queue, wanted, deliver, config, current, glyphs, fetcher) = (
                queue.clone(),
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
                    while let Some(job) = next(&queue) {
                        let (Job::Load(request, zoom) | Job::Process(request, zoom, ..)) = job;
                        if !wanted.lock().unwrap().contains(&request) {
                            // A refresh is of a tile the map has, and isn't waiting for.
                            if !matches!(job, Job::Process(.., true)) {
                                deliver(TileMessage::Cancelled(request));
                            }
                            continue;
                        }
                        let (source, id) = request;
                        let current = current.read().unwrap().clone();
                        let worker = Worker {
                            reader: readers[source].as_ref(),
                            fetcher: fetcher.as_deref(),
                            config: &config,
                            style: &current.style,
                            source: &config.sources[source],
                            geojson: current.geojson[source].as_ref(),
                            used_layers: &current.used_layers[source],
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
        Ok(Self {
            requests,
            wanted,
            current,
            config,
        })
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
        ));
        generation
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
        ));
        generation
    }

    /// Replace the set of tiles the view needs.
    pub fn set_wanted(&self, tiles: impl IntoIterator<Item = SourceTile>) {
        let mut wanted = self.wanted.lock().unwrap();
        wanted.clear();
        wanted.extend(tiles);
    }

    /// Process `tile` with the style evaluated at `zoom`.
    pub fn request(&self, tile: SourceTile, zoom: u8) {
        // The workers only stop when this loader is dropped, so sending can't fail before then.
        let _ = self.requests.send(Job::Load(tile, zoom));
    }
}

/// A fetcher that hands what it fetches to the workers through `requests`.
fn fetcher(
    requests: Weak<Sender<Job>>,
    wanted: Arc<Mutex<HashSet<SourceTile>>>,
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
        // Gone once the loader is.
        if let Some(requests) = requests.upgrade() {
            let _ = requests.send(job);
        }
    })
}

fn next(queue: &Mutex<Receiver<Job>>) -> Option<Job> {
    queue.lock().unwrap().recv().ok()
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

struct Worker<'a> {
    /// `None` for image, GeoJSON and tile server sources.
    reader: Option<&'a MbTiles>,
    /// Set if some source is a tile server.
    fetcher: Option<&'a Fetcher>,
    config: &'a LoaderConfig,
    style: &'a Style,
    source: &'a SourceConfig,
    /// The index of a GeoJSON source.
    geojson: Option<&'a Arc<GeoJsonIndex>>,
    used_layers: &'a HashSet<String>,
    glyphs: &'a FontLibrary,
}

impl Worker<'_> {
    /// Read and process a tile, or process its `fetched` data.
    fn load(
        &self,
        processor: &mut TileProcessor,
        request: SourceTile,
        zoom: u8,
        fetched: Option<Vec<u8>>,
    ) -> Result<Loaded> {
        let (_, id) = request;
        let resources = TileResources {
            images: self.config.images.as_deref(),
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
