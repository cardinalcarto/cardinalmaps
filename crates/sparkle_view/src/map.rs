//! Which tiles to draw: per source, the tiles covering the view, with loaded ancestors or
//! children standing in for tiles that are still loading. And which to load ahead of the
//! view, after the tiles it covers: ancestors to stand in, the tiles where a motion under
//! way comes to rest, and a ring just past the view's edges. Also what the map changes at
//! runtime: the style (tiles are processed again or repainted as the change requires),
//! the data of GeoJSON sources (swapped in whole once cut, so they never blink), feature
//! state, and queries of what is under a point.
//!
//! The map keeps whatever a renderer made of each tile, through [`TileUploader`], so the
//! same logic serves every frontend.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use maplibre_expr::Value;
use sparkle::geojson::GeoJsonIndex;
use sparkle::query::{FeatureIndex, PointQuery, QueriedFeature};
use sparkle::sprite::ImageAtlas;
use sparkle::state::FeatureStates;
use sparkle::style::{Source, StyleDiff};
use sparkle::terrain::TerrainTiles;
use sparkle::{ProcessedTile, Style, TileId};

use crate::camera::{Camera, MAX_ZOOM, Placement};
use crate::loader::{SourceTile, Tier, TileLoader, TileMessage};

/// How many levels up to look for a loaded tile to show while a tile is loading.
const MAX_FALLBACK_LEVELS: u8 = 8;
/// How many levels down to look for loaded tiles to show while a tile is loading.
const MAX_DESCENDANT_LEVELS: u8 = 2;
/// How many levels up the ancestors of visible tiles loaded to stand in for them are.
const FALLBACK_LEVELS: u8 = 2;
/// The most tiles, across sources, wanted ahead of the view (loaded or loading).
const MAX_PREDICTED: usize = 100;
/// Tiles wanted ahead of the view, with those drawn, take at most this share of the
/// memory budget, leaving the rest for tiles the view comes to need.
const PREDICTED_SHARE: [usize; 2] = [3, 4];
/// What a tile is taken to need in memory until some have loaded.
const GUESSED_TILE_BYTES: usize = 256 << 10;

/// What a renderer keeps of each loaded tile, and how it changes them in place.
pub trait TileUploader {
    /// A tile ready to draw, such as its buffers on the GPU.
    type Tile;

    /// Make a processed tile ready to draw, with the current feature state applied to
    /// buckets whose paint reads it.
    fn upload(&mut self, style: &Style, tile: ProcessedTile, states: &FeatureStates) -> Self::Tile;

    /// Bytes the tile takes, on the GPU and off it, as of its upload.
    fn tile_bytes(&self, tile: &Self::Tile) -> usize;

    /// Forget what is kept per style layer, before drawing with a new style.
    fn style_changed(&mut self);

    /// Rebuild the paint of the tile's bucket of layer `index` after a style change.
    /// Returns false when the bucket can't do that, so the tile must be processed again.
    fn rebuild_paint(
        &mut self,
        tile: &mut Self::Tile,
        style: &Style,
        index: usize,
        states: &FeatureStates,
    ) -> bool;

    /// Evaluate the paint of features `ids` (all for `None`) of `source_layer` again with
    /// their state in `states`. Returns whether anything changed.
    fn update_feature_state(
        &mut self,
        tile: &mut Self::Tile,
        style: &Style,
        states: &FeatureStates,
        source_layer: Option<&str>,
        ids: Option<&[u64]>,
    ) -> bool;
}

/// The tiles of one source to draw this frame.
pub struct SourceTiles<'a, T> {
    pub source: &'a str,
    pub tiles: Vec<TileRef<'a, T>>,
}

/// A tile to draw: `placement` is where its geometry goes, `clip` the area it may draw
/// to. They differ when a parent tile stands in for a child that isn't loaded yet.
pub struct TileRef<'a, T> {
    pub tile: &'a T,
    pub placement: Placement,
    pub clip: Placement,
}

impl<T> Clone for TileRef<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for TileRef<'_, T> {}

/// What the terrain is for a frame: the loaded DEM tiles and the style's exaggeration.
#[derive(Clone, Copy)]
pub struct Terrain<'a> {
    pub tiles: &'a TerrainTiles,
    pub exaggeration: f64,
}

/// A source the map draws from.
pub struct MapSource {
    /// The style's source id.
    pub id: String,
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// Logical pixels a tile spans at its own zoom.
    pub tile_size: u32,
    /// `[west, south, east, north]`: tiles outside have nothing to draw and aren't
    /// requested.
    pub bounds: Option<[f64; 4]>,
}

enum CacheEntry<T> {
    Loading,
    /// The tileset has nothing to draw here.
    Empty,
    Ready {
        tile: T,
        features: Option<FeatureIndex>,
        last_used: u64,
        /// The zoom the style was evaluated at.
        zoom: u8,
        /// Processed with a style older than the source needs: still drawn, but replaced
        /// once it has been processed again.
        stale: bool,
        /// A request to process it again is under way.
        requested: bool,
        /// What the tile and its features take in memory.
        bytes: usize,
    },
}

pub struct MapState<T> {
    /// The style tiles are processed with; what is drawn may differ while paint
    /// transitions run.
    pub style: Arc<Style>,
    sources: Vec<MapSource>,
    /// Per source, the tiles drawn.
    caches: Vec<HashMap<TileId, CacheEntry<T>>>,
    /// Per source, tiles of GeoJSON data newer than the drawn tiles have, while some of
    /// those covering the view are still being cut: see [`MapState::set_geojson`].
    pending: Vec<Option<HashMap<TileId, CacheEntry<T>>>>,
    /// Per source, the newest GeoJSON data set while older data was still on its way in:
    /// it follows once that has been swapped in.
    waiting: Vec<Option<Arc<GeoJsonIndex>>>,
    /// Bytes loaded tiles may take, across sources, before the least recently drawn are
    /// dropped (so that panning back to them is instant until then).
    memory_budget: usize,
    /// Per source, the oldest style generation its tiles may have been processed with.
    min_generation: Vec<u64>,
    loader: TileLoader,
    frame: u64,
    pub states: FeatureStates,
    /// The tiles drawn last, for queries.
    drawn: Vec<(usize, Placement, Placement)>,
    /// The loaded tiles of the style's terrain source, and that source's index.
    terrain: TerrainTiles,
    terrain_source: Option<usize>,
}

/// What [`MapState::frame`] picked: per source, the tiles to draw.
pub struct Frame {
    /// `(source index, clip placement, drawn placement)`.
    tiles: Vec<(usize, Placement, Placement)>,
    /// Every covering tile is loaded (or known to be empty), so nothing is a stand-in.
    pub complete: bool,
    pub loading: usize,
}

impl Frame {
    pub fn tiles_drawn(&self) -> usize {
        self.tiles.len()
    }
}

impl<T> MapState<T> {
    /// A map of `sources`, keeping loaded tiles in up to `memory_budget` bytes (more only
    /// when the tiles on screen alone take more).
    pub fn new(
        style: Arc<Style>,
        sources: Vec<MapSource>,
        loader: TileLoader,
        memory_budget: usize,
    ) -> Self {
        let mut map = Self {
            caches: sources.iter().map(|_| HashMap::new()).collect(),
            pending: sources.iter().map(|_| None).collect(),
            waiting: sources.iter().map(|_| None).collect(),
            memory_budget,
            min_generation: vec![0; sources.len()],
            style,
            sources,
            loader,
            frame: 0,
            states: FeatureStates::new(),
            drawn: Vec::new(),
            terrain: TerrainTiles::new(),
            terrain_source: None,
        };
        map.find_terrain_source();
        map
    }

    fn find_terrain_source(&mut self) {
        self.terrain_source = self.style.terrain.as_ref().and_then(|t| {
            let s = self.sources.iter().position(|s| s.id == t.source);
            if s.is_none() {
                log::warn!("no tiles for the terrain source {:?}", t.source);
            }
            s
        });
    }

    /// The terrain to draw with, if the style has one.
    pub fn terrain(&self) -> Option<Terrain<'_>> {
        self.terrain_source?;
        Some(Terrain {
            tiles: &self.terrain,
            exaggeration: self.style.terrain.as_ref()?.exaggeration,
        })
    }

    /// The terrain's elevation at `world` in meters, exaggerated: 0 where it isn't known.
    fn ground(&self) -> Option<impl Fn([f64; 2]) -> Option<f64> + '_> {
        let terrain = self.terrain()?;
        Some(move |world: [f64; 2]| {
            Some(f64::from(terrain.tiles.elevation_at(world).unwrap_or(0.0)) * terrain.exaggeration)
        })
    }

    /// Set what the camera takes from the style and the terrain for this frame: the
    /// projection at its zoom, and the elevation it orbits. Keeps it above the terrain.
    pub fn update_camera(&self, camera: &mut Camera) {
        camera.globe = sparkle::projection::globe_factor(&self.style.projection, camera.zoom);
        match self.ground() {
            Some(ground) => {
                camera.center_elevation = ground(camera.center).unwrap_or(0.0);
                camera.avoid_terrain(&ground);
            }
            None => camera.center_elevation = 0.0,
        }
    }

    /// Pick the tiles to draw for `camera`, requesting the ones that are missing, and
    /// then, as far as memory allows, tiles it may need soon: ancestors to stand in for
    /// the tiles it covers, those where it comes to rest (`end`, if it's moving; see
    /// [`crate::motion::Motion::end`]), and those around it.
    pub fn frame(&mut self, camera: &Camera, end: Option<&Camera>) -> Frame {
        self.frame += 1;
        let zoom = camera.zoom as f32;
        // Only sources that a visible layer draws from need tiles.
        let used: HashSet<&str> = self
            .style
            .layers
            .iter()
            .filter(|l| l.is_visible_at(zoom))
            .filter_map(|l| l.source.as_deref())
            .chain(self.style.terrain.as_ref().map(|t| t.source.as_str()))
            .collect();
        let exaggeration = self.style.terrain.as_ref().map_or(0.0, |t| t.exaggeration);
        let terrain = &self.terrain;
        let range = |id: TileId| {
            let (lo, hi) = terrain.min_max(id)?;
            Some((f64::from(lo) * exaggeration, f64::from(hi) * exaggeration))
        };

        let mut wanted = Vec::new();
        // Tiles to load ahead, per tier: `(tier, source index, tile, zoom to process at)`.
        let mut ahead: Vec<(Tier, usize, TileId, u8)> = Vec::new();
        let mut tiles = Vec::new();
        // Per source, the tiles drawn this frame.
        let mut drawn: Vec<HashSet<TileId>> = vec![HashSet::new(); self.sources.len()];
        let mut complete = true;
        for (s, source) in self.sources.iter().enumerate() {
            if !used.contains(source.id.as_str()) {
                continue;
            }
            let mut covering = camera.covering_tiles_with(
                source.min_zoom,
                source.max_zoom,
                source.tile_size,
                &range,
            );
            if let Some(bounds) = source.bounds {
                covering.retain(|p| intersects(p.id(), bounds));
            }
            // Past the source's max zoom its deepest tiles are drawn magnified. They're
            // processed with the style at the zoom the view would draw otherwise, as
            // MapLibre lays out overscaled tiles, so filters and layout follow the view.
            let deepest = camera.tile_zoom(source.min_zoom, MAX_ZOOM as u8, source.tile_size);
            let zoom_for = |id: TileId| {
                if id.z == source.max_zoom {
                    deepest.max(id.z)
                } else {
                    id.z
                }
            };
            let destination = end.map(|end| {
                let mut covering = end.covering_tiles_with(
                    source.min_zoom,
                    source.max_zoom,
                    source.tile_size,
                    &range,
                );
                if let Some(bounds) = source.bounds {
                    covering.retain(|p| intersects(p.id(), bounds));
                }
                covering
            });
            ahead.extend(
                predictions(&covering, destination.as_deref(), source.min_zoom)
                    .into_iter()
                    .filter(|&(_, id)| source.bounds.is_none_or(|b| intersects(id, b)))
                    .map(|(tier, id)| (tier, s, id, zoom_for(id))),
            );
            wanted.extend(covering.iter().map(|p| ((s, p.id()), Tier::Visible)));
            // Tiles are loaded into the new data's tiles while it's cut, and drawn from the
            // old ones until it's all in.
            loop {
                let cache = filling(&mut self.pending, &mut self.caches, s);
                for p in &covering {
                    let id = p.id();
                    let zoom = zoom_for(id);
                    match cache.get_mut(&id) {
                        None => {
                            self.loader.request((s, id), zoom);
                            cache.insert(id, CacheEntry::Loading);
                        }
                        Some(CacheEntry::Ready {
                            zoom: processed,
                            stale,
                            requested,
                            ..
                        }) if (*stale || *processed != zoom) && !*requested => {
                            self.loader.request((s, id), zoom);
                            *requested = true;
                        }
                        Some(_) => {}
                    }
                }
                let all_in = self.pending[s].as_ref().is_some_and(|pending| {
                    covering
                        .iter()
                        .all(|p| settled(pending, p.id(), zoom_for(p.id())))
                });
                if !all_in {
                    break;
                }
                self.caches[s] = self.pending[s].take().expect("just matched");
                // Data set meanwhile is next, its tiles asked for in this same frame.
                let Some(index) = self.waiting[s].take() else {
                    break;
                };
                self.min_generation[s] = self.loader.set_geojson(s, index);
                self.pending[s] = Some(HashMap::new());
            }
            let cache = &mut self.caches[s];
            let chosen = choose(cache, &covering, source.max_zoom);
            complete &= self.pending[s].is_none()
                && covering
                    .iter()
                    .all(|p| settled(cache, p.id(), zoom_for(p.id())));
            let used: HashSet<TileId> = chosen.iter().map(|(_, p)| p.id()).collect();
            for id in &used {
                if let Some(CacheEntry::Ready { last_used, .. }) = cache.get_mut(id) {
                    *last_used = self.frame;
                }
            }
            drawn[s] = used;
            tiles.extend(chosen.into_iter().map(|(clip, p)| (s, clip, p)));
        }
        // Tiers across sources: all fallbacks before any destination tile, and so on.
        ahead.sort_by_key(|&(tier, ..)| tier);
        let drawn_bytes: usize = drawn
            .iter()
            .enumerate()
            .flat_map(|(s, ids)| ids.iter().map(move |id| (s, id)))
            .map(|(s, id)| match self.caches[s].get(id) {
                Some(CacheEntry::Ready { bytes, .. }) => *bytes,
                _ => 0,
            })
            .sum();
        let allowance = (self.memory_budget / PREDICTED_SHARE[1] * PREDICTED_SHARE[0])
            .saturating_sub(drawn_bytes);
        let candidates: Vec<SourceTile> = ahead.iter().map(|&(_, s, id, _)| (s, id)).collect();
        for i in select(&self.caches, &candidates, allowance, MAX_PREDICTED) {
            let (tier, s, id, zoom) = ahead[i];
            let cache = filling(&mut self.pending, &mut self.caches, s);
            if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(id) {
                self.loader.request((s, id), zoom);
                entry.insert(CacheEntry::Loading);
            }
            wanted.push(((s, id), tier));
        }
        let ranks: HashMap<SourceTile, usize> = wanted
            .iter()
            .enumerate()
            .rev()
            .map(|(rank, &(tile, _))| (tile, rank))
            .collect();
        for (s, id) in evict(&mut self.caches, &drawn, &ranks, self.memory_budget) {
            if Some(s) == self.terrain_source {
                self.terrain.remove(id);
            }
        }
        self.loader.set_wanted(wanted);
        let loading = self
            .caches
            .iter()
            .chain(self.pending.iter().flatten())
            .flat_map(|c| c.values())
            .filter(|e| matches!(e, CacheEntry::Loading))
            .count();
        self.drawn.clone_from(&tiles);
        Frame {
            tiles,
            complete,
            loading,
        }
    }

    /// The tiles of `frame`, grouped by source, for the renderer.
    pub fn draws(&self, frame: &Frame) -> Vec<SourceTiles<'_, T>> {
        let mut out: Vec<SourceTiles<'_, T>> = Vec::new();
        for &(s, clip, placement) in &frame.tiles {
            let Some(CacheEntry::Ready { tile, .. }) = self.caches[s].get(&placement.id()) else {
                continue;
            };
            let source = self.sources[s].id.as_str();
            let entry = match out.iter_mut().position(|t| t.source == source) {
                Some(i) => &mut out[i],
                None => {
                    out.push(SourceTiles {
                        source,
                        tiles: Vec::new(),
                    });
                    out.last_mut().unwrap()
                }
            };
            entry.tiles.push(TileRef {
                tile,
                placement,
                clip,
            });
        }
        out
    }

    /// Take in a message from the loader. Returns whether anything changed on screen.
    pub fn receive(
        &mut self,
        renderer: &mut impl TileUploader<Tile = T>,
        message: TileMessage,
    ) -> bool {
        match message {
            // The slot is freed once the tile has been made ready to draw.
            TileMessage::Loaded(s, mut tile, generation, _slot) => {
                if generation < self.min_generation[s] {
                    // Processed with a style that has changed since; a newer request is
                    // under way for anything still wanted.
                    if let Some(CacheEntry::Loading) = self.caches[s].get(&tile.id) {
                        self.caches[s].remove(&tile.id);
                    }
                    return false;
                }
                let features = tile.features.take();
                let (id, zoom) = (tile.id, tile.zoom);
                if Some(s) == self.terrain_source
                    && let Some(dem) = tile.dem.take()
                {
                    self.terrain.insert(id, dem);
                }
                let gpu = renderer.upload(&self.style, tile, &self.states);
                let bytes = renderer.tile_bytes(&gpu)
                    + features.as_ref().map_or(0, FeatureIndex::heap_bytes);
                filling(&mut self.pending, &mut self.caches, s).insert(
                    id,
                    CacheEntry::Ready {
                        tile: gpu,
                        features,
                        last_used: self.frame,
                        zoom,
                        stale: false,
                        requested: false,
                        bytes,
                    },
                );
            }
            TileMessage::Empty((s, id), generation) => {
                if generation.is_some_and(|g| g < self.min_generation[s]) {
                    // Empty for data or a style replaced since, as above.
                    if let Some(CacheEntry::Loading) = self.caches[s].get(&id) {
                        self.caches[s].remove(&id);
                    }
                    return false;
                }
                filling(&mut self.pending, &mut self.caches, s).insert(id, CacheEntry::Empty);
            }
            TileMessage::Cancelled((s, id)) => {
                for cache in std::iter::once(&mut self.caches[s]).chain(&mut self.pending[s]) {
                    match cache.get_mut(&id) {
                        Some(CacheEntry::Loading) => {
                            cache.remove(&id);
                        }
                        Some(CacheEntry::Ready { requested, .. }) => *requested = false,
                        _ => {}
                    }
                }
                return false;
            }
        }
        true
    }

    /// Switch to `style`, which differs from the current one as `diff` says: tiles are
    /// processed again where needed (still drawn meanwhile, unless the layers changed),
    /// and repainted in place where that's enough.
    pub fn set_style(
        &mut self,
        renderer: &mut impl TileUploader<Tile = T>,
        style: Arc<Style>,
        diff: &StyleDiff,
    ) {
        let generation = self.loader.set_style(style.clone());
        let terrain_changed = self.style.terrain.as_ref().map(|t| &t.source)
            != style.terrain.as_ref().map(|t| &t.source);
        self.style = style;
        renderer.style_changed();
        if diff.reload_all || terrain_changed {
            // Tiles only bring their elevations when processed with the terrain in the
            // style, so a new terrain source means loading everything again.
            for (s, cache) in self.caches.iter_mut().enumerate() {
                cache.clear();
                self.pending[s] = None;
                // With nothing left to draw meanwhile, data that was waiting for older
                // data to come in is loaded in its place.
                self.min_generation[s] = match self.waiting[s].take() {
                    Some(index) => self.loader.set_geojson(s, index),
                    None => generation,
                };
            }
            self.terrain = TerrainTiles::new();
            self.find_terrain_source();
            return;
        }
        let mut reprocess: HashSet<usize> = diff
            .sources_to_reprocess(&self.style)
            .into_iter()
            .filter_map(|id| self.sources.iter().position(|s| s.id == id))
            .collect();
        for &index in &diff.repaint {
            let Some(source) = self.style.layers[index].source.as_deref() else {
                continue;
            };
            let Some(s) = self.sources.iter().position(|m| m.id == source) else {
                continue;
            };
            let pending = self.pending[s].iter_mut().flat_map(|p| p.values_mut());
            for entry in self.caches[s].values_mut().chain(pending) {
                if let CacheEntry::Ready { tile, .. } = entry
                    && !renderer.rebuild_paint(tile, &self.style, index, &self.states)
                {
                    reprocess.insert(s);
                }
            }
        }
        for s in reprocess {
            self.reprocess(s, generation);
        }
    }

    /// Process tiles from now on with `images`, which must have what the current ones
    /// have in the same places (see `ImageAtlas::with_images`). Tiles already loaded
    /// stay as they are: a layer that needs a new image is drawn again by setting its
    /// data.
    pub fn set_images(&self, images: Arc<ImageAtlas>) {
        self.loader.set_images(images);
    }

    /// Replace the data of the style's GeoJSON source `id` with `data`, a GeoJSON object.
    ///
    /// Its tiles are cut again, and the old ones are drawn until the new ones covering
    /// the view are all in, then dropped for them in the same frame: the source never
    /// blinks out, as when data changes about once a second (a route, the location dot).
    /// The swap is of the whole source at once, not tile by tile, so old and new never
    /// show side by side: a line crossing tiles would be broken where they meet, and a
    /// point near an edge shown twice or not at all. That waits for the slowest tile
    /// in view, but they're cut from memory, in milliseconds.
    ///
    /// Data set again while tiles of earlier data are being cut waits for those to be
    /// swapped in, then follows: a source set every frame shows, each frame, the newest
    /// data that has been cut, rather than nothing new until the changes stop. Only the
    /// newest data waits (any set in between is skipped), so what is drawn only ever
    /// goes forward. Data set again before any tile of the earlier data was asked for
    /// replaces it outright.
    pub fn set_geojson(&mut self, id: &str, data: &serde_json::Value) -> Result<()> {
        let Some(Source::GeoJson(source)) = self.style.source(id) else {
            bail!("the style has no GeoJSON source {id:?}");
        };
        let s = self
            .sources
            .iter()
            .position(|m| m.id == id)
            .with_context(|| format!("GeoJSON source {id:?} isn't loaded"))?;
        let index =
            GeoJsonIndex::new(source, data).with_context(|| format!("GeoJSON source {id:?}"))?;
        // The source's own, whatever its data.
        self.sources[s].max_zoom = index.max_zoom();
        let index = Arc::new(index);
        if self.pending[s].as_ref().is_some_and(|p| !p.is_empty()) {
            self.waiting[s] = Some(index);
        } else {
            self.waiting[s] = None;
            self.min_generation[s] = self.loader.set_geojson(s, index);
            self.pending[s] = Some(HashMap::new());
        }
        Ok(())
    }

    /// Whether a GeoJSON source's new data is still being cut or waiting to be, older
    /// tiles drawn meanwhile.
    pub fn swapping(&self, id: &str) -> bool {
        self.sources
            .iter()
            .position(|m| m.id == id)
            .is_some_and(|s| self.pending[s].is_some())
    }

    /// Have the tiles of source `s` processed again, as of `generation`: loaded ones are
    /// drawn until then.
    fn reprocess(&mut self, s: usize, generation: u64) {
        self.min_generation[s] = generation;
        for cache in std::iter::once(&mut self.caches[s]).chain(&mut self.pending[s]) {
            cache.retain(|_, e| matches!(e, CacheEntry::Ready { .. }));
            for entry in cache.values_mut() {
                if let CacheEntry::Ready {
                    stale, requested, ..
                } = entry
                {
                    *stale = true;
                    *requested = false;
                }
            }
        }
    }

    /// Merge `state` into the feature state of a feature, like MapLibre's
    /// `setFeatureState`, repainting what it changes. Returns whether anything changed.
    pub fn set_feature_state(
        &mut self,
        renderer: &mut impl TileUploader<Tile = T>,
        source: &str,
        source_layer: Option<&str>,
        id: u64,
        state: impl IntoIterator<Item = (String, Value)>,
    ) -> bool {
        if !self.states.set(source, source_layer, id, state) {
            return false;
        }
        self.repaint_state(renderer, source, source_layer, Some(&[id]))
    }

    /// Remove feature state like MapLibre's `removeFeatureState`: one key, a feature's
    /// whole state (`key` `None`) or every feature's (`id` `None`).
    pub fn remove_feature_state(
        &mut self,
        renderer: &mut impl TileUploader<Tile = T>,
        source: &str,
        source_layer: Option<&str>,
        id: Option<u64>,
        key: Option<&str>,
    ) -> bool {
        let changed = self.states.remove(source, source_layer, id, key);
        !changed.is_empty() && self.repaint_state(renderer, source, source_layer, Some(&changed))
    }

    fn repaint_state(
        &mut self,
        renderer: &mut impl TileUploader<Tile = T>,
        source: &str,
        source_layer: Option<&str>,
        ids: Option<&[u64]>,
    ) -> bool {
        let Some(s) = self.sources.iter().position(|m| m.id == source) else {
            return false;
        };
        let mut changed = false;
        let pending = self.pending[s].iter_mut().flat_map(|p| p.values_mut());
        for entry in self.caches[s].values_mut().chain(pending) {
            if let CacheEntry::Ready { tile, .. } = entry {
                changed |= renderer.update_feature_state(
                    tile,
                    &self.style,
                    &self.states,
                    source_layer,
                    ids,
                );
            }
        }
        changed
    }

    /// The source and source layer style layer `layer` draws from.
    pub fn layer_source(&self, layer: usize) -> Option<(&str, Option<&str>)> {
        let layer = self.style.layers.get(layer)?;
        Some((layer.source.as_deref()?, layer.source_layer.as_deref()))
    }

    /// The properties of the feature with `id` in `source_layer` of `source`'s tile at
    /// `placement`, if that tile is still loaded and has one.
    pub fn tile_feature(
        &self,
        source: &str,
        placement: Placement,
        source_layer: Option<&str>,
        id: u64,
    ) -> Option<BTreeMap<String, Value>> {
        let s = self.sources.iter().position(|m| m.id == source)?;
        let Some(CacheEntry::Ready {
            features: Some(index),
            ..
        }) = self.caches[s].get(&placement.id())
        else {
            return None;
        };
        match index.properties(&self.style, source, source_layer, id) {
            Ok(properties) => properties,
            Err(err) => {
                log::warn!("query: {err}");
                None
            }
        }
    }

    /// The features drawn at a screen point (physical pixels), topmost first, as in the
    /// last frame.
    pub fn query_point(&self, camera: &Camera, point: [f64; 2]) -> Vec<QueriedFeature> {
        let world = match self.ground() {
            Some(ground) => camera.pick(point, &ground),
            None => camera.screen_to_world(point),
        };
        let Some(world) = world else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for &(s, clip, placement) in &self.drawn {
            let scale = f64::from(clip.z).exp2();
            let (x, y) = (
                world[0] * scale - clip.x as f64,
                world[1] * scale - f64::from(clip.y),
            );
            if !(0.0..1.0).contains(&x) || !(0.0..1.0).contains(&y) {
                continue;
            }
            let Some(CacheEntry::Ready {
                features: Some(index),
                ..
            }) = self.caches[s].get(&placement.id())
            else {
                continue;
            };
            let scale = f64::from(placement.z).exp2();
            let query = PointQuery {
                position: [
                    world[0] * scale - placement.x as f64,
                    world[1] * scale - f64::from(placement.y),
                ],
                tile_size: camera.tile_world_size(placement.z),
                zoom: camera.zoom,
                tolerance: 3.0,
                layers: Vec::new(),
            };
            match index.query_point(&self.style, &self.sources[s].id, &query, Some(&self.states)) {
                Ok(features) => found.extend(features),
                Err(err) => log::warn!("query: {err}"),
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.layer));
        found
    }
}

/// Whether `tile` overlaps `[west, south, east, north]`.
fn intersects(tile: TileId, [w, s, e, n]: [f64; 4]) -> bool {
    let (west, north) = crate::camera::lng_lat_to_world(w, n).into();
    let (east, south) = crate::camera::lng_lat_to_world(e, s).into();
    let size = 1.0 / f64::from(1u32 << tile.z);
    let (x, y) = (f64::from(tile.x) * size, f64::from(tile.y) * size);
    x < east && x + size > west && y < south && y + size > north
}

/// The tiles of source `s` that tiles loaded now go into: those of new GeoJSON data on
/// its way in, if there is some, else those drawn.
fn filling<'a, T>(
    pending: &'a mut [Option<HashMap<TileId, CacheEntry<T>>>],
    caches: &'a mut [HashMap<TileId, CacheEntry<T>>],
    s: usize,
) -> &'a mut HashMap<TileId, CacheEntry<T>> {
    match &mut pending[s] {
        Some(pending) => pending,
        None => &mut caches[s],
    }
}

fn ready<T>(cache: &HashMap<TileId, CacheEntry<T>>, id: TileId) -> bool {
    matches!(cache.get(&id), Some(CacheEntry::Ready { .. }))
}

/// Loaded (with the current style, evaluated at `zoom`) or known to be empty.
fn settled<T>(cache: &HashMap<TileId, CacheEntry<T>>, id: TileId, zoom: u8) -> bool {
    match cache.get(&id) {
        Some(CacheEntry::Ready { stale, zoom: z, .. }) => !stale && *z == zoom,
        Some(CacheEntry::Empty) => true,
        _ => false,
    }
}

/// Loaded, perhaps with an older style, or known to be empty.
fn known<T>(cache: &HashMap<TileId, CacheEntry<T>>, id: TileId) -> bool {
    matches!(
        cache.get(&id),
        Some(CacheEntry::Ready { .. } | CacheEntry::Empty)
    )
}

/// What to draw for each covering tile, as `(clip, drawn)`: itself, or while it's loading,
/// its loaded descendants (when zooming out) with the nearest loaded ancestor filling
/// what they leave uncovered, or that ancestor alone (when zooming in).
fn choose<T>(
    cache: &HashMap<TileId, CacheEntry<T>>,
    covering: &[Placement],
    max_zoom: u8,
) -> Vec<(Placement, Placement)> {
    let mut chosen = Vec::new();
    for &p in covering {
        if known(cache, p.id()) {
            chosen.extend(ready(cache, p.id()).then_some((p, p)));
            continue;
        }
        let ancestor = (1..=p.z.min(MAX_FALLBACK_LEVELS))
            .map(|k| p.ancestor(k))
            .find(|a| ready(cache, a.id()));
        if !descendants(
            cache,
            p,
            MAX_DESCENDANT_LEVELS,
            max_zoom,
            ancestor,
            &mut chosen,
        ) {
            chosen.extend(ancestor.map(|a| (p, a)));
        }
    }
    chosen
}

/// Cover `p` with its known descendants up to `levels` below it, drawing `fallback` over
/// the children of `p` that have none. Returns false, adding nothing, when no descendant
/// is known, so that `p` is better covered by `fallback` as a whole.
fn descendants<T>(
    cache: &HashMap<TileId, CacheEntry<T>>,
    p: Placement,
    levels: u8,
    max_zoom: u8,
    fallback: Option<Placement>,
    chosen: &mut Vec<(Placement, Placement)>,
) -> bool {
    if levels == 0 || p.z >= max_zoom {
        return false;
    }
    let mut found = false;
    let mut missing = Vec::new();
    for c in p.children() {
        if known(cache, c.id()) {
            chosen.extend(ready(cache, c.id()).then_some((c, c)));
            found = true;
        } else if descendants(cache, c, levels - 1, max_zoom, fallback, chosen) {
            found = true;
        } else {
            missing.push(c);
        }
    }
    if found {
        chosen.extend(
            fallback
                .into_iter()
                .flat_map(|a| missing.iter().map(move |&c| (c, a))),
        );
    }
    found
}

/// Tiles to load ahead of a view that `covering` covers, in the order they're wanted,
/// none of them covering and none twice: the ancestors [`FALLBACK_LEVELS`] up (no
/// higher than `min_zoom`) to stand in for them, then the tiles covering the view where
/// it comes to rest (`destination`, if it's moving), then those just past the covering
/// tiles at their own zoom, nearest the middle of the view first.
fn predictions(
    covering: &[Placement],
    destination: Option<&[Placement]>,
    min_zoom: u8,
) -> Vec<(Tier, TileId)> {
    let visible: HashSet<TileId> = covering.iter().map(|p| p.id()).collect();
    let mut seen = visible.clone();
    let mut out = Vec::new();
    let mut add = |tier, id| {
        if seen.insert(id) {
            out.push((tier, id));
        }
    };
    for p in covering {
        let levels = FALLBACK_LEVELS.min(p.z.saturating_sub(min_zoom));
        if levels > 0 {
            add(Tier::Fallback, p.ancestor(levels).id());
        }
    }
    for p in destination.unwrap_or_default() {
        add(Tier::Destination, p.id());
    }
    // A neighbor whose parent or child covers part of the view is in it, not past it.
    let in_view = |n: Placement| {
        visible.contains(&n.id())
            || (n.z > 0 && visible.contains(&n.ancestor(1).id()))
            || n.children().iter().any(|c| visible.contains(&c.id()))
    };
    for p in covering {
        for (dx, dy) in [
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1),
        ] {
            let y = i64::from(p.y) + dy;
            if !(0..1 << p.z).contains(&y) {
                continue;
            }
            let n = Placement {
                z: p.z,
                x: p.x + dx,
                y: y as u32,
            };
            if !in_view(n) {
                add(Tier::Ring, n.id());
            }
        }
    }
    out
}

/// Which of `candidates` (in the order they're wanted) to keep loaded or load: the first
/// of them, up to `max` and as many as fit in `allowance` bytes, those loaded counting what
/// they take and those not yet what loaded tiles take on average. Those known to be empty
/// need nothing and are left out. Returns their indices.
fn select<T>(
    caches: &[HashMap<TileId, CacheEntry<T>>],
    candidates: &[SourceTile],
    allowance: usize,
    max: usize,
) -> Vec<usize> {
    let (total, count) = caches
        .iter()
        .flat_map(|c| c.values())
        .filter_map(|e| match e {
            CacheEntry::Ready { bytes, .. } => Some(*bytes),
            _ => None,
        })
        .fold((0, 0), |(total, count), bytes| (total + bytes, count + 1));
    let guess = total.checked_div(count).unwrap_or(GUESSED_TILE_BYTES);
    let mut spent = 0;
    let mut chosen = Vec::new();
    for (i, &(s, id)) in candidates.iter().enumerate() {
        if chosen.len() >= max {
            break;
        }
        let bytes = match caches[s].get(&id) {
            Some(CacheEntry::Empty) => continue,
            Some(CacheEntry::Ready { bytes, .. }) => *bytes,
            Some(CacheEntry::Loading) | None => guess,
        };
        if spent + bytes > allowance {
            break;
        }
        spent += bytes;
        chosen.push(i);
    }
    chosen
}

/// Drop tiles, of any source, until the loaded ones take no more than `budget` bytes,
/// never ones drawn this frame (`drawn`, per source): first the least recently drawn of
/// those not wanted, then the wanted ones (`wanted`, by their place in the view's order),
/// least wanted first. Returns the tiles dropped, as `(source index, tile)`.
fn evict<T>(
    caches: &mut [HashMap<TileId, CacheEntry<T>>],
    drawn: &[HashSet<TileId>],
    wanted: &HashMap<SourceTile, usize>,
    budget: usize,
) -> Vec<(usize, TileId)> {
    let mut total = 0;
    let mut droppable = Vec::new();
    for (s, cache) in caches.iter().enumerate() {
        for (id, entry) in cache {
            if let CacheEntry::Ready {
                last_used, bytes, ..
            } = entry
            {
                total += bytes;
                if !drawn[s].contains(id) {
                    let order = match wanted.get(&(s, *id)) {
                        None => (false, *last_used),
                        Some(&rank) => (true, u64::MAX - rank as u64),
                    };
                    droppable.push((order, s, *id, *bytes));
                }
            }
        }
    }
    if total <= budget {
        return Vec::new();
    }
    droppable.sort_unstable_by_key(|&(order, ..)| order);
    let mut dropped = Vec::new();
    for (_, s, id, bytes) in droppable {
        if total <= budget {
            break;
        }
        caches[s].remove(&id);
        total -= bytes;
        dropped.push((s, id));
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stamps each tile with the data it was cut from, as the test sets it.
    struct Stamper {
        data: u32,
    }

    impl TileUploader for Stamper {
        type Tile = u32;

        fn upload(&mut self, _: &Style, _: ProcessedTile, _: &FeatureStates) -> u32 {
            self.data
        }

        fn tile_bytes(&self, _: &u32) -> usize {
            1
        }

        fn style_changed(&mut self) {}

        fn rebuild_paint(&mut self, _: &mut u32, _: &Style, _: usize, _: &FeatureStates) -> bool {
            true
        }

        fn update_feature_state(
            &mut self,
            _: &mut u32,
            _: &Style,
            _: &FeatureStates,
            _: Option<&str>,
            _: Option<&[u64]>,
        ) -> bool {
            false
        }
    }

    /// A line along the parallel at `lat`, across most of the world.
    fn line(lat: f64) -> serde_json::Value {
        let coordinates: Vec<[f64; 2]> = (-170..=170)
            .step_by(5)
            .map(|lng| [f64::from(lng), lat])
            .collect();
        serde_json::json!({"type": "LineString", "coordinates": coordinates})
    }

    #[test]
    fn new_geojson_is_swapped_in_whole_once_cut_never_blanking() {
        let dir = std::env::temp_dir().join(format!("sparkle-swap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("style.json");
        let style = serde_json::json!({"version": 8,
            "sources": {"route": {"type": "geojson", "data": line(10.0)}},
            "layers": [{"id": "route", "type": "line", "source": "route"}]});
        std::fs::write(&path, style.to_string()).unwrap();
        let args =
            crate::args::parse(["--style".into(), path.to_string_lossy().into_owned()]).unwrap();
        let style = Arc::new(crate::setup::load_style(args.style.as_deref(), &args).unwrap());
        let (configs, sources, _) = crate::setup::sources(&args, &style).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let loader = TileLoader::spawn(
            crate::loader::LoaderConfig {
                style: style.clone(),
                sources: configs,
                images: None,
                fonts: None,
            },
            move |m| drop(tx.send(m)),
        )
        .unwrap();
        let mut map = MapState::new(style, sources, loader, 64 << 20);
        let mut stamper = Stamper { data: 1 };
        // Zoom 3 over the line: a few tiles cover it.
        let mut camera = Camera::new([1600.0, 1200.0], 1.0);
        camera.look_at(0.0, 10.0, 3.0);
        let stamps = |map: &mut MapState<u32>| {
            let frame = map.frame(&camera, None);
            let draws = map.draws(&frame);
            let stamps: Vec<u32> = draws
                .iter()
                .flat_map(|s| s.tiles.iter().map(|t| *t.tile))
                .collect();
            (stamps, frame.complete)
        };
        let wait = std::time::Duration::from_secs(10);
        loop {
            let (_, complete) = stamps(&mut map);
            if complete {
                break;
            }
            map.receive(&mut stamper, rx.recv_timeout(wait).unwrap());
        }
        let (before, _) = stamps(&mut map);
        assert!(
            before.len() > 1 && before.iter().all(|&d| d == 1),
            "{before:?}"
        );

        stamper.data = 2;
        map.set_geojson("route", &line(20.0)).unwrap();
        assert!(map.swapping("route"));
        let mut frames = 0;
        loop {
            let (now, complete) = stamps(&mut map);
            assert!(!now.is_empty(), "blank while cutting");
            assert!(
                now.iter().all(|&d| d == now[0]),
                "old and new mixed: {now:?}"
            );
            if complete {
                assert!(now.iter().all(|&d| d == 2) && !map.swapping("route"));
                break;
            }
            assert!(
                now.iter().all(|&d| d == 1),
                "swapped before it was all in: {now:?}"
            );
            frames += 1;
            map.receive(&mut stamper, rx.recv_timeout(wait).unwrap());
        }
        assert!(
            frames > 1,
            "the old tiles were drawn while the new ones were cut"
        );
    }

    fn at(z: u8, x: i64, y: u32) -> Placement {
        Placement { z, x, y }
    }

    fn cache(ready: &[Placement]) -> HashMap<TileId, CacheEntry<()>> {
        ready
            .iter()
            .map(|p| {
                let entry = CacheEntry::Ready {
                    tile: (),
                    features: None,
                    last_used: 0,
                    zoom: p.z,
                    stale: false,
                    requested: false,
                    bytes: 0,
                };
                (p.id(), entry)
            })
            .collect()
    }

    #[test]
    fn zooming_out_two_levels_draws_the_grandchildren() {
        let p = at(1, 0, 0);
        let grandchildren: Vec<_> = p.children().iter().flat_map(|c| c.children()).collect();
        let chosen = choose(&cache(&grandchildren), &[p], 14);
        assert_eq!(
            chosen,
            grandchildren.iter().map(|&c| (c, c)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn descendants_three_levels_down_are_not_used() {
        let p = at(1, 0, 0);
        let deep: Vec<_> = p
            .children()
            .iter()
            .flat_map(|c| c.children())
            .flat_map(|c| c.children())
            .collect();
        assert!(choose(&cache(&deep), &[p], 14).is_empty());
    }

    #[test]
    fn the_ancestor_fills_what_descendants_leave_uncovered() {
        let p = at(2, 1, 1);
        let [a, b, c, d] = p.children();
        let [b0, ..] = b.children();
        let root = at(0, 0, 0);
        let chosen = choose(&cache(&[root, a, b0]), &[p], 14);
        assert_eq!(
            chosen,
            vec![
                (a, a),
                (b0, b0),
                (b.children()[1], root),
                (b.children()[2], root),
                (b.children()[3], root),
                (c, root),
                (d, root)
            ]
        );
    }

    #[test]
    fn without_descendants_the_ancestor_covers_the_tile_whole() {
        let p = at(2, 1, 1);
        let root = at(0, 0, 0);
        assert_eq!(choose(&cache(&[root]), &[p], 14), vec![(p, root)]);
    }

    #[test]
    fn no_descendants_past_the_source_max_zoom() {
        let p = at(2, 1, 1);
        assert!(choose(&cache(&p.children()), &[p], 2).is_empty());
    }

    fn sized(entries: &[(Placement, u64, usize)]) -> HashMap<TileId, CacheEntry<()>> {
        entries
            .iter()
            .map(|&(p, last_used, bytes)| {
                let entry = CacheEntry::Ready {
                    tile: (),
                    features: None,
                    last_used,
                    zoom: p.z,
                    stale: false,
                    requested: false,
                    bytes,
                };
                (p.id(), entry)
            })
            .collect()
    }

    #[test]
    fn evicts_the_least_recently_drawn_of_any_source_down_to_the_budget() {
        let [a, b, c, d] = at(1, 0, 0).children();
        let mut caches = vec![
            sized(&[(a, 1, 400), (b, 3, 400)]),
            sized(&[(c, 2, 400), (d, 4, 400)]),
        ];
        let drawn = vec![HashSet::new(), HashSet::new()];
        let dropped = evict(&mut caches, &drawn, &HashMap::new(), 900);
        assert_eq!(dropped, vec![(0, a.id()), (1, c.id())]);
        assert!(caches[0].contains_key(&b.id()) && caches[1].contains_key(&d.id()));
    }

    #[test]
    fn keeps_tiles_drawn_this_frame_even_over_the_budget() {
        let [a, b, ..] = at(1, 0, 0).children();
        let mut caches = vec![sized(&[(a, 1, 1000), (b, 2, 1000)])];
        let drawn = vec![HashSet::from([a.id()])];
        assert_eq!(
            evict(&mut caches, &drawn, &HashMap::new(), 500),
            vec![(0, b.id())]
        );
        assert!(caches[0].contains_key(&a.id()));
    }

    #[test]
    fn wanted_tiles_go_after_unwanted_ones_least_wanted_first() {
        let [a, b, c, d] = at(1, 0, 0).children();
        // `c` and `d` were drawn longest ago, but are wanted, `d` more than `c`.
        let mut caches = vec![sized(&[(a, 5, 400), (b, 6, 400), (c, 1, 400), (d, 2, 400)])];
        let wanted = HashMap::from([((0, c.id()), 7), ((0, d.id()), 3)]);
        let dropped = evict(&mut caches, &[HashSet::new()], &wanted, 500);
        assert_eq!(dropped, vec![(0, a.id()), (0, b.id()), (0, c.id())]);
    }

    fn kinds(predicted: &[(Tier, TileId)]) -> Vec<Tier> {
        predicted.iter().map(|&(tier, _)| tier).collect()
    }

    #[test]
    fn predicts_fallbacks_then_the_destination_then_the_ring() {
        let view = [at(10, 100, 100), at(10, 101, 100)];
        let destination = [at(10, 101, 100), at(10, 102, 100), at(10, 103, 100)];
        let predicted = predictions(&view, Some(&destination), 0);
        let fallback = view[0].ancestor(2).id();
        // One fallback for both; the destination less what's in view; then the ring of 10
        // around the two, less the two destination tiles it overlaps.
        assert_eq!(predicted[0], (Tier::Fallback, fallback));
        assert_eq!(
            predicted[1..3],
            [
                (Tier::Destination, at(10, 102, 100).id()),
                (Tier::Destination, at(10, 103, 100).id())
            ]
        );
        assert_eq!(kinds(&predicted[3..]), vec![Tier::Ring; 9]);
        let ids: HashSet<TileId> = predicted.iter().map(|&(_, id)| id).collect();
        assert_eq!(ids.len(), predicted.len(), "no tile twice");
        assert!(view.iter().all(|p| !ids.contains(&p.id())));
        assert!(ids.contains(&at(10, 99, 99).id()) && ids.contains(&at(10, 102, 101).id()));
    }

    #[test]
    fn fallbacks_go_no_higher_than_the_min_zoom() {
        let predicted = predictions(&[at(5, 3, 3)], None, 4);
        assert_eq!(predicted[0], (Tier::Fallback, at(5, 3, 3).ancestor(1).id()));
        assert!(
            predictions(&[at(4, 3, 3)], None, 4)
                .iter()
                .all(|&(tier, _)| tier == Tier::Ring)
        );
    }

    #[test]
    fn the_ring_leaves_out_tiles_whose_parent_or_children_are_in_view() {
        // In a pitched view, a far tile a level up next to the near ones.
        let near = at(10, 100, 100);
        let far = at(9, 50, 49);
        let predicted = predictions(&[near, far], None, 10);
        let ring: HashSet<TileId> = predicted.iter().map(|&(_, id)| id).collect();
        // Above `near` is a child of `far`, so in view.
        assert!(!ring.contains(&at(10, 100, 99).id()));
        // Below `far` is the parent of `near`: in view.
        assert!(!ring.contains(&at(9, 50, 50).id()));
        assert!(ring.contains(&at(10, 99, 101).id()));
    }

    #[test]
    fn the_ring_stops_at_the_poles_and_wraps_around_the_world() {
        let predicted = predictions(&[at(2, 0, 0)], None, 2);
        let ring: HashSet<TileId> = predicted.iter().map(|&(_, id)| id).collect();
        assert_eq!(ring.len(), 5);
        assert!(ring.contains(&at(2, 3, 0).id()) && ring.contains(&at(2, 3, 1).id()));
    }

    fn entries(entries: Vec<(Placement, CacheEntry<()>)>) -> HashMap<TileId, CacheEntry<()>> {
        entries.into_iter().map(|(p, e)| (p.id(), e)).collect()
    }

    fn ready_bytes(bytes: usize) -> CacheEntry<()> {
        CacheEntry::Ready {
            tile: (),
            features: None,
            last_used: 0,
            zoom: 0,
            stale: false,
            requested: false,
            bytes,
        }
    }

    #[test]
    fn selection_stops_at_the_memory_allowance_guessing_unloaded_tiles_average() {
        let [a, b, c, d] = at(1, 0, 0).children();
        let [e, ..] = at(1, 1, 1).children();
        let caches = vec![entries(vec![
            (a, ready_bytes(300)),
            (b, ready_bytes(100)),
            (c, CacheEntry::Empty),
            (d, CacheEntry::Loading),
        ])];
        let candidates: Vec<SourceTile> = [a, b, c, d, e].iter().map(|p| (0, p.id())).collect();
        // `a` 300, `b` 100, `c` nothing (left out), `d` and `e` the average 200.
        assert_eq!(select(&caches, &candidates, 600, 100), vec![0, 1, 3]);
        assert_eq!(select(&caches, &candidates, 800, 100), vec![0, 1, 3, 4]);
        // Stopping at the first that doesn't fit, so they're kept in order.
        assert_eq!(select(&caches, &candidates, 350, 100), vec![0]);
        assert!(select(&caches, &candidates, 0, 100).is_empty());
    }

    #[test]
    fn selection_stops_at_the_most_tiles() {
        let tiles = at(2, 0, 0).children();
        let caches = vec![HashMap::<TileId, CacheEntry<()>>::new()];
        let candidates: Vec<SourceTile> = tiles.iter().map(|p| (0, p.id())).collect();
        // Before any tile has loaded, each is guessed at GUESSED_TILE_BYTES.
        assert_eq!(select(&caches, &candidates, usize::MAX, 3), vec![0, 1, 2]);
        assert_eq!(
            select(&caches, &candidates, GUESSED_TILE_BYTES * 2, 10),
            vec![0, 1]
        );
    }

    #[test]
    fn nothing_is_dropped_within_the_budget() {
        let [a, b, ..] = at(1, 0, 0).children();
        let mut caches = vec![sized(&[(a, 1, 400), (b, 2, 400)])];
        assert!(evict(&mut caches, &[HashSet::new()], &HashMap::new(), 800).is_empty());
        assert_eq!(caches[0].len(), 2);
    }
}
