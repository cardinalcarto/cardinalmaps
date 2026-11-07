//! Command line arguments.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

pub const USAGE: &str = "\
usage: sparkle_harness | sparkle_example [tiles.mbtiles | TileJSON URL] [options]

  --tiles [source=]path.mbtiles  tiles of a style source (repeatable); without `source=`, the
                                 style's first vector source (or --source). An http(s) URL
                                 is a tile server's TileJSON, e.g. a Martin tileset's
                                 https://host/name. An image file for an `image` source. A
                                 .geojson or .json file replaces the data of a geojson
                                 source. GeoJSON sources whose data is inline or a local path
                                 need no --tiles.
  --cache dir                    where tiles from servers are cached (default:
                                 $XDG_CACHE_HOME/sparkle or ~/.cache/sparkle, else in memory)
  --cache-size MB                the most the cache holds before dropping the least recently
                                 used tiles (default 100)
  --tile-memory MB               the most loaded tiles may take in memory, GPU included, before
                                 dropping the least recently drawn (default 256); tiles on
                                 screen are always kept
  --style style.json             the style (default: the built-in MapTiler Basic)
  --source name                  the source a bare tiles path stands in for
  --sprite [id=]path-prefix      a local sprite: reads prefix.json/.png (or @2x) (repeatable);
                                 without `id=`, the style's first sprite id or \"default\"
  --fonts dir                    fonts to rasterize glyphs from, searched before the system's
  --view lng,lat,zoom[,bearing,pitch]
  --screenshot out.png           save the first frame in which every visible tile has loaded, then exit
  --headless                     render offscreen without a window (needs --screenshot;
                                 sparkle_harness only)
  --size WxH                     physical pixels (default 1024x768)
  --pixel-ratio r                physical pixels per logical pixel (default: the window's, or 1)
  --feature-state source[/layer]:id:{json}
                                 set feature state before drawing (repeatable)
  --global-state name=json       set a global state value (repeatable)
  --hover x,y                    with --headless, hover the point (physical pixels) as the mouse
                                 would: the topmost feature there gets feature state hover=true
  --next-style style.json        with --headless, switch to this style once loaded, letting
                                 paint transitions run for --transition-ms (default 0) before
                                 the screenshot

A style's `terrain` drapes the map over its raster-dem source (give its tiles with
--tiles id=dem.mbtiles); its `projection` and `sky` are drawn too, e.g. styles/terrain-3d.json
and styles/globe.json. sparkle/tests/fixtures/raster has a synthetic DEM around Seattle.

Drag to pan; right-drag or ctrl+drag to rotate and pitch; scroll or +/- to zoom; arrows
pan, shift+arrows rotate and pitch; N resets bearing and pitch; Esc quits. The feature
under the mouse gets feature state hover=true, and clicking toggles selected=true. A
--style file is reloaded when it changes, with paint transitions.";

/// `lng, lat, zoom, bearing, pitch`
pub type ViewArg = (f64, f64, f64, f64, f64);

pub struct Args {
    /// `(source id, path)`; `None` maps to the default vector source.
    pub tiles: Vec<(Option<String>, PathBuf)>,
    pub style: Option<PathBuf>,
    pub source: Option<String>,
    /// `(sprite id, path prefix)`
    pub sprites: Vec<(Option<String>, PathBuf)>,
    pub fonts: Option<PathBuf>,
    pub cache: Option<PathBuf>,
    /// Bytes.
    pub cache_size: u64,
    /// Bytes.
    pub tile_memory: usize,
    pub view: Option<ViewArg>,
    pub screenshot: Option<PathBuf>,
    pub headless: bool,
    pub size: Option<[u32; 2]>,
    pub pixel_ratio: Option<f64>,
    pub feature_states: Vec<FeatureStateArg>,
    /// `(name, value)`
    pub global_state: Vec<(String, serde_json::Value)>,
    pub hover: Option<[f64; 2]>,
    pub next_style: Option<PathBuf>,
    pub transition_ms: f64,
}

/// `--feature-state source[/layer]:id:{json}`
pub struct FeatureStateArg {
    pub source: String,
    pub source_layer: Option<String>,
    pub id: u64,
    pub state: serde_json::Map<String, serde_json::Value>,
}

impl FeatureStateArg {
    fn parse(arg: &str) -> Result<Self> {
        let usage = || format!("--feature-state needs source[/layer]:id:{{json}}, not {arg:?}");
        let (source, rest) = arg.split_once(':').with_context(usage)?;
        let (id, json) = rest.split_once(':').with_context(usage)?;
        let (source, source_layer) = match source.split_once('/') {
            Some((s, l)) => (s, Some(l.to_owned())),
            None => (source, None),
        };
        let state = match serde_json::from_str(json).with_context(usage)? {
            serde_json::Value::Object(state) => state,
            _ => bail!(usage()),
        };
        Ok(Self {
            source: source.to_owned(),
            source_layer,
            id: id.parse().with_context(usage)?,
            state,
        })
    }
}

/// `id=value` or a bare `value`.
fn keyed(arg: String) -> (Option<String>, PathBuf) {
    match arg.split_once('=') {
        Some((id, path)) if !id.is_empty() && !id.contains('/') => {
            (Some(id.to_owned()), path.into())
        }
        _ => (None, arg.into()),
    }
}

/// Parse `arguments`, which exclude the program's name.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut args = Args {
        tiles: Vec::new(),
        style: None,
        source: None,
        sprites: Vec::new(),
        fonts: None,
        cache: None,
        cache_size: 100 << 20,
        tile_memory: 256 << 20,
        view: None,
        screenshot: None,
        headless: false,
        size: None,
        pixel_ratio: None,
        feature_states: Vec::new(),
        global_state: Vec::new(),
        hover: None,
        next_style: None,
        transition_ms: 0.0,
    };
    let mut it = arguments.into_iter();
    let value = |it: &mut dyn Iterator<Item = String>, flag: &str| {
        it.next()
            .with_context(|| format!("{flag} needs a value\n{USAGE}"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tiles" => args.tiles.push(keyed(value(&mut it, "--tiles")?)),
            "--style" => args.style = Some(value(&mut it, "--style")?.into()),
            "--source" => args.source = Some(value(&mut it, "--source")?),
            "--sprite" => args.sprites.push(keyed(value(&mut it, "--sprite")?)),
            "--fonts" => args.fonts = Some(value(&mut it, "--fonts")?.into()),
            "--cache" => args.cache = Some(value(&mut it, "--cache")?.into()),
            "--cache-size" => {
                let mb: f64 = value(&mut it, "--cache-size")?
                    .parse()
                    .context("--cache-size needs megabytes")?;
                args.cache_size = (mb * 1048576.0) as u64;
            }
            "--tile-memory" => {
                let mb: f64 = value(&mut it, "--tile-memory")?
                    .parse()
                    .context("--tile-memory needs megabytes")?;
                args.tile_memory = (mb * 1048576.0) as usize;
            }
            "--screenshot" => args.screenshot = Some(value(&mut it, "--screenshot")?.into()),
            "--headless" => args.headless = true,
            "--view" => {
                let v = value(&mut it, "--view")?;
                let parts: Vec<f64> = v
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()
                    .context("--view needs numbers")?;
                args.view = Some(match *parts.as_slice() {
                    [lng, lat, zoom] => (lng, lat, zoom, 0.0, 0.0),
                    [lng, lat, zoom, bearing] => (lng, lat, zoom, bearing, 0.0),
                    [lng, lat, zoom, bearing, pitch] => (lng, lat, zoom, bearing, pitch),
                    _ => bail!("--view needs lng,lat,zoom[,bearing,pitch]"),
                });
            }
            "--size" => {
                let v = value(&mut it, "--size")?;
                let (w, h) = v.split_once('x').context("--size needs WxH")?;
                args.size = Some([w.parse()?, h.parse()?]);
            }
            "--pixel-ratio" => args.pixel_ratio = Some(value(&mut it, "--pixel-ratio")?.parse()?),
            "--feature-state" => args
                .feature_states
                .push(FeatureStateArg::parse(&value(&mut it, "--feature-state")?)?),
            "--global-state" => {
                let v = value(&mut it, "--global-state")?;
                let (name, json) = v
                    .split_once('=')
                    .context("--global-state needs name=json")?;
                let json = serde_json::from_str(json)
                    .with_context(|| format!("--global-state {name}: invalid JSON"))?;
                args.global_state.push((name.to_owned(), json));
            }
            "--hover" => {
                let v = value(&mut it, "--hover")?;
                let (x, y) = v.split_once(',').context("--hover needs x,y")?;
                args.hover = Some([x.parse()?, y.parse()?]);
            }
            "--next-style" => args.next_style = Some(value(&mut it, "--next-style")?.into()),
            "--transition-ms" => args.transition_ms = value(&mut it, "--transition-ms")?.parse()?,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            flag if flag.starts_with("--") => bail!("unknown option {flag}\n{USAGE}"),
            _ => args.tiles.push((None, arg.into())),
        }
    }
    if args.tiles.is_empty() && args.style.is_none() {
        bail!("no tiles given\n{USAGE}");
    }
    if args.headless && args.screenshot.is_none() {
        bail!("--headless needs --screenshot");
    }
    Ok(args)
}
