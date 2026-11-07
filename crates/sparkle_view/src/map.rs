//! Which tiles to draw: per source, the tiles covering the view, with loaded ancestors or
//! children standing in for tiles that are still loading. Also what the map changes at
//! runtime: the style (tiles are processed again or repainted as the change requires),
//! the data of GeoJSON sources, feature state, and queries of what is under a point.
//!
//! The map keeps whatever a renderer made of each tile, through [`TileUploader`], so the
//! same logic serves every frontend.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use maplibre_expr::Value;
use sparkle::geojson::GeoJsonIndex;
use sparkle::query::{FeatureIndex, PointQuery, QueriedFeature};
use sparkle::state::FeatureStates;
use sparkle::style::{Source, StyleDiff};
use sparkle::terrain::TerrainTiles;
use sparkle::{ProcessedTile, Style, TileId};

use crate::camera::{Camera, MAX_ZOOM, Placement};
use crate::loader::{TileLoader, TileMessage};

/// How many levels up to look for a loaded tile to show while a tile is loading.
const MAX_FALLBACK_LEVELS: u8 = 8;
/// How many levels down to look for loaded tiles to show while a tile is loading.
const MAX_DESCENDANT_LEVELS: u8 = 2;

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
    caches: Vec<HashMap<TileId, CacheEntry<T>>>,
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

    /// Pick the tiles to draw for `camera`, requesting the ones that are missing.
    pub fn frame(&mut self, camera: &Camera) -> Frame {
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
            let cache = &mut self.caches[s];
            for p in &covering {
                let id = p.id();
                let zoom = zoom_for(id);
                wanted.push((s, id));
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
            let chosen = choose(cache, &covering, source.max_zoom);
            complete &= covering
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
        for (s, id) in evict(&mut self.caches, &drawn, self.memory_budget) {
            if Some(s) == self.terrain_source {
                self.terrain.remove(id);
            }
        }
        self.loader.set_wanted(wanted);
        let loading = self
            .caches
            .iter()
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
            TileMessage::Loaded(s, mut tile, generation) => {
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
                self.caches[s].insert(
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
            TileMessage::Empty((s, id)) => {
                self.caches[s].insert(id, CacheEntry::Empty);
            }
            TileMessage::Cancelled((s, id)) => {
                match self.caches[s].get_mut(&id) {
                    Some(CacheEntry::Loading) => {
                        self.caches[s].remove(&id);
                    }
                    Some(CacheEntry::Ready { requested, .. }) => *requested = false,
                    _ => {}
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
                self.min_generation[s] = generation;
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
            for entry in self.caches[s].values_mut() {
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

    /// Replace the data of the style's GeoJSON source `id` with `data`, a GeoJSON object.
    /// Its tiles are cut again. Unlike after a style change, the old ones aren't drawn
    /// meanwhile, even standing in for others: what they show is gone, and the new ones
    /// are cut from memory, so they don't keep the source blank for long.
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
        self.sources[s].max_zoom = index.max_zoom();
        let generation = self.loader.set_geojson(s, Arc::new(index));
        self.min_generation[s] = generation;
        self.caches[s].clear();
        Ok(())
    }

    /// Have the tiles of source `s` processed again, as of `generation`: loaded ones are
    /// drawn until then.
    fn reprocess(&mut self, s: usize, generation: u64) {
        self.min_generation[s] = generation;
        let cache = &mut self.caches[s];
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
        for entry in self.caches[s].values_mut() {
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

/// Drop the least recently drawn tiles, of any source, until the loaded ones take no more
/// than `budget` bytes, never ones drawn this frame (`drawn`, per source). Returns the
/// tiles dropped, as `(source index, tile)`.
fn evict<T>(
    caches: &mut [HashMap<TileId, CacheEntry<T>>],
    drawn: &[HashSet<TileId>],
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
                    droppable.push((*last_used, s, *id, *bytes));
                }
            }
        }
    }
    if total <= budget {
        return Vec::new();
    }
    droppable.sort_unstable_by_key(|&(last_used, ..)| last_used);
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
        let dropped = evict(&mut caches, &drawn, 900);
        assert_eq!(dropped, vec![(0, a.id()), (1, c.id())]);
        assert!(caches[0].contains_key(&b.id()) && caches[1].contains_key(&d.id()));
    }

    #[test]
    fn keeps_tiles_drawn_this_frame_even_over_the_budget() {
        let [a, b, ..] = at(1, 0, 0).children();
        let mut caches = vec![sized(&[(a, 1, 1000), (b, 2, 1000)])];
        let drawn = vec![HashSet::from([a.id()])];
        assert_eq!(evict(&mut caches, &drawn, 500), vec![(0, b.id())]);
        assert!(caches[0].contains_key(&a.id()));
    }

    #[test]
    fn nothing_is_dropped_within_the_budget() {
        let [a, b, ..] = at(1, 0, 0).children();
        let mut caches = vec![sized(&[(a, 1, 400), (b, 2, 400)])];
        assert!(evict(&mut caches, &[HashSet::new()], 800).is_empty());
        assert_eq!(caches[0].len(), 2);
    }
}
