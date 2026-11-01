//! Setting a map up from its [`Args`]: the style, its sources (local files or tile servers)
//! and sprites, where the view
//! starts, and the feature state the command line asks for. Also marking the feature under
//! a point, as frontends do for hover and selection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sparkle::Style;
use sparkle::geojson::GeoJsonIndex;
use sparkle::sprite::{ImageAtlas, SpriteSheet};
use sparkle::style::{ImageSource, Source, SpriteSource};
use sparkle::video::VideoFrames;

use crate::args::Args;
use crate::cache::DiskCache;
use crate::camera::Camera;
use crate::loader::SourceConfig;
use crate::map::{MapSource, MapState, TileUploader};
use crate::mbtiles::{MbTiles, Metadata, TileFormat};
use crate::remote::{self, TileServer};

/// Used when `--style` isn't given.
pub const DEFAULT_STYLE: &str = include_str!("../style.json");

pub fn load_style(path: Option<&Path>, args: &Args) -> Result<Style> {
    let text = match path {
        Some(path) => {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => DEFAULT_STYLE.to_owned(),
    };
    let mut style: Style = text.parse()?;
    for warning in &style.warnings {
        log::warn!("style: {warning}");
    }
    for (name, value) in &args.global_state {
        style.set_global_state(name, Some(value.clone()));
    }
    Ok(style)
}

/// Where relative paths in the style (GeoJSON data, video) are resolved from.
pub fn style_dir(args: &Args) -> PathBuf {
    args.style
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_owned)
        .unwrap_or_default()
}

/// Where tile servers' responses are cached: `--cache`, else the user's cache directory.
/// `None` keeps them in memory.
pub fn cache_dir(args: &Args) -> Option<PathBuf> {
    if let Some(dir) = &args.cache {
        return Some(dir.clone());
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache")))?;
    Some(base.join("sparkle"))
}

pub fn is_geojson(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("geojson" | "json")
    )
}

/// Each tiles argument and GeoJSON source as a loader source and a map source, plus the
/// first tileset's metadata for the initial view.
pub fn sources(
    args: &Args,
    style: &Style,
) -> Result<(Vec<SourceConfig>, Vec<MapSource>, Option<Metadata>)> {
    let mut configs: Vec<SourceConfig> = Vec::new();
    let mut map_sources = Vec::new();
    let mut first = None;
    // Opened for the first tile server.
    let mut http = None;
    for (id, path) in &args.tiles {
        let id = match id.as_ref().or(args.source.as_ref()) {
            Some(id) => id.clone(),
            None => style
                .first_vector_source()
                .context("the style has no vector source; name one with --tiles id=path")?
                .to_owned(),
        };
        if configs.iter().any(|c| c.id == id) {
            bail!("source {id:?} is given more than once");
        }
        let style_source = style.source(&id);
        let url = path.to_str().filter(|p| remote::is_url(p));
        if url.is_none() && is_geojson(path) {
            let Some(Source::GeoJson(source)) = style_source else {
                bail!(
                    "{} is GeoJSON, but the style's source {id:?} isn't",
                    path.display()
                );
            };
            let mut source = source.clone();
            source.data = path.to_string_lossy().into_owned().into();
            let index = GeoJsonIndex::load(&source, None)?;
            log::info!("{id}: {} ({} features)", path.display(), index.len());
            map_sources.push(geojson_map_source(&id, &index));
            configs.push(geojson_config(id, path.clone(), index));
            continue;
        }
        if style_source.is_none() {
            log::warn!(
                "the style has no source {id:?}; no layer will draw {}",
                path.display()
            );
        }
        if let Some(Source::Image(image)) = style_source {
            if url.is_some() {
                bail!("the style's source {id:?} is an image source, which takes a local file");
            }
            let (config, map_source) = image_source(&id, path, image);
            configs.push(config);
            map_sources.push(map_source);
            continue;
        }
        let (metadata, server) = match url {
            Some(url) => {
                let (agent, cache) = match &http {
                    Some(http) => http,
                    None => {
                        let dir = cache_dir(args);
                        let cache = DiskCache::open(dir.as_deref(), args.cache_size)?;
                        match &dir {
                            Some(dir) => log::info!("tile cache: {}", dir.display()),
                            None => log::info!("tile cache: in memory"),
                        }
                        http.insert((remote::agent(), Arc::new(cache)))
                    }
                };
                // TileJSON needn't say; MapLibre takes vector tiles to be MVT.
                let format = match style_source {
                    Some(Source::Raster(_) | Source::RasterDem(_)) => TileFormat::Image,
                    _ => TileFormat::Mvt,
                };
                let (server, metadata) =
                    TileServer::open(url, agent.clone(), cache.clone(), format)?;
                (metadata, Some(Arc::new(server)))
            }
            None => (MbTiles::open(path)?.metadata()?, None),
        };
        // The style can narrow what the tileset has.
        let tiles = style_source.and_then(Source::tiles);
        let (min_zoom, max_zoom) = match tiles {
            Some(t) => (
                metadata.min_zoom.max(t.min_zoom),
                metadata.max_zoom.min(t.max_zoom).max(metadata.min_zoom),
            ),
            None => (metadata.min_zoom, metadata.max_zoom),
        };
        let bounds = match (metadata.bounds, tiles.map(|t| t.bounds)) {
            (Some([w, s, e, n]), Some([sw, ss, se, sn])) => {
                Some([w.max(sw), s.max(ss), e.min(se), n.min(sn)])
            }
            (a, b) => a.or(b),
        };
        log::info!(
            "{id}: {} ({:?}, zoom {}-{})",
            path.display(),
            metadata.format,
            metadata.min_zoom,
            metadata.max_zoom
        );
        map_sources.push(MapSource {
            id: id.clone(),
            min_zoom,
            max_zoom,
            tile_size: style_source.map_or(512, |s| s.tile_size()),
            bounds,
        });
        configs.push(SourceConfig {
            id,
            path: path.clone(),
            format: metadata.format,
            image_tile: None,
            geojson: None,
            server,
        });
        first.get_or_insert(metadata);
    }
    let base = style_dir(args);
    for (id, source) in &style.sources {
        if configs.iter().any(|c| &c.id == id) {
            continue;
        }
        match source {
            Source::GeoJson(geojson) => match GeoJsonIndex::load(geojson, Some(&base)) {
                Ok(index) => {
                    log::info!("{id}: GeoJSON, {} features", index.len());
                    map_sources.push(geojson_map_source(id, &index));
                    let path = geojson.data.as_str().map(PathBuf::from).unwrap_or_default();
                    configs.push(geojson_config(id.clone(), path, index));
                }
                Err(err) => log::warn!("{id}: {err}"),
            },
            Source::Video(video) => {
                if let Err(err) = VideoFrames::open(video, Some(&base)) {
                    log::warn!("{id}: {err}");
                }
            }
            _ => log::info!("no tiles for {} source {id:?}", source.type_name()),
        }
    }
    // A style of only background layers needs no data.
    if configs.is_empty() && style.layers.iter().any(|l| l.source.is_some()) {
        bail!("no tiles given, and the style has no GeoJSON source");
    }
    Ok((configs, map_sources, first))
}

pub fn geojson_map_source(id: &str, index: &GeoJsonIndex) -> MapSource {
    MapSource {
        id: id.to_owned(),
        min_zoom: 0,
        max_zoom: index.max_zoom(),
        tile_size: 512,
        bounds: None,
    }
}

pub fn geojson_config(id: String, path: PathBuf, index: GeoJsonIndex) -> SourceConfig {
    SourceConfig {
        id,
        path,
        format: TileFormat::Mlt,
        image_tile: None,
        geojson: Some(Arc::new(index)),
        server: None,
    }
}

/// An image source drawn from a local image file: one tile, at the zoom of the deepest
/// tile that holds the whole image.
pub fn image_source(id: &str, path: &Path, image: &ImageSource) -> (SourceConfig, MapSource) {
    let tile = sparkle::tile::image_source_tile(image);
    log::info!(
        "{id}: {} (image, in tile {}/{}/{})",
        path.display(),
        tile.z,
        tile.x,
        tile.y
    );
    let config = SourceConfig {
        id: id.to_owned(),
        path: path.to_owned(),
        format: TileFormat::Image,
        image_tile: Some(tile),
        geojson: None,
        server: None,
    };
    let map_source = MapSource {
        id: id.to_owned(),
        min_zoom: tile.z,
        max_zoom: tile.z,
        tile_size: 512,
        bounds: None,
    };
    (config, map_source)
}

/// Load `--sprite` sheets into one atlas.
pub fn sprites(args: &Args, style: &Style, pixel_ratio: f64) -> Result<Option<ImageAtlas>> {
    if args.sprites.is_empty() {
        return Ok(None);
    }
    let default_id = style
        .sprite
        .first()
        .map_or(SpriteSource::DEFAULT_ID, |s| s.id.as_str());
    let sheets = args
        .sprites
        .iter()
        .map(|(id, prefix)| {
            let sheet = SpriteSheet::load(prefix, pixel_ratio as f32)
                .with_context(|| format!("loading sprite {}", prefix.display()))?;
            Ok((id.clone().unwrap_or_else(|| default_id.to_owned()), sheet))
        })
        .collect::<Result<Vec<_>>>()?;
    let atlas = ImageAtlas::build(sheets.iter().map(|(id, s)| (id.as_str(), s)));
    log::info!(
        "sprites: {} images in a {}x{} atlas",
        atlas.len(),
        atlas.width,
        atlas.height
    );
    Ok(Some(atlas))
}

/// Where the view starts: `--view`, else the style's camera, else the tileset's.
pub fn initial_camera(
    camera: &mut Camera,
    args: &Args,
    style: &Style,
    metadata: Option<&Metadata>,
) {
    if let Some((lng, lat, zoom, bearing, pitch)) = args.view {
        camera.look_at(lng, lat, zoom);
        camera.set_bearing(bearing);
        camera.set_pitch(pitch);
    } else if let (Some([lng, lat]), Some(zoom)) = (style.center, style.zoom) {
        camera.look_at(lng, lat, zoom);
        camera.set_bearing(style.bearing);
        camera.set_pitch(style.pitch);
    } else if let Some((lng, lat, zoom)) = metadata.and_then(|m| m.center) {
        camera.look_at(lng, lat, zoom);
    } else if let Some(bounds) = metadata.and_then(|m| m.bounds) {
        camera.fit_bounds(bounds);
    }
}

/// Apply `--feature-state` arguments.
pub fn initial_feature_state<R: TileUploader>(
    map: &mut MapState<R::Tile>,
    renderer: &mut R,
    args: &Args,
) {
    for f in &args.feature_states {
        let state = f
            .state
            .iter()
            .map(|(k, v)| (k.clone(), maplibre_expr::Value::from_json(v)));
        map.set_feature_state(renderer, &f.source, f.source_layer.as_deref(), f.id, state);
    }
}

/// Give the topmost feature with an id at `point` the state `key: true`, or toggle it,
/// removing it from `previous` (the last feature that had it). Returns the feature now
/// holding it, if any.
pub fn mark_feature<R: TileUploader>(
    map: &mut MapState<R::Tile>,
    renderer: &mut R,
    camera: &Camera,
    point: [f64; 2],
    key: &str,
    previous: Option<&Marked>,
    toggle: bool,
) -> (Option<Marked>, bool) {
    let found = map.query_point(camera, point);
    for f in &found {
        log::debug!(
            "at {point:?}: layer {:?}, feature {:?} {:?}",
            map.style.layers[f.layer].id,
            f.id,
            f.properties
        );
    }
    let hit = found
        .into_iter()
        .find_map(|f| Some((f.source, f.source_layer, f.id?)));
    if !toggle && hit.as_ref() == previous {
        return (hit, false);
    }
    let mut changed = false;
    if let Some((source, layer, id)) = previous {
        changed |=
            map.remove_feature_state(renderer, source, layer.as_deref(), Some(*id), Some(key));
        if toggle && hit.as_ref() == previous {
            return (None, changed);
        }
    }
    if let Some((source, layer, id)) = &hit {
        let state = [(key.to_owned(), maplibre_expr::Value::Bool(true))];
        changed |= map.set_feature_state(renderer, source, layer.as_deref(), *id, state);
        log::info!(
            "{key}: {source}/{} feature {id}",
            layer.as_deref().unwrap_or("-")
        );
    }
    (hit, changed)
}

/// A feature by `(source, source layer, id)`.
pub type Marked = (String, Option<String>, u64);
