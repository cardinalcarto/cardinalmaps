//! Everything in a style besides its layers: sources, sprites, glyphs, light, sky,
//! terrain, projection and the default camera.
//!
//! Source URLs are kept as written; sparkle never fetches anything, so whoever loads tiles
//! maps sources to local data themselves.

use std::collections::BTreeMap;

use serde_json::{Value as Json, json};

use super::{Color, IlluminationAnchor, ProjectionDefinition, PropertyValue, StyleWarning};

/// A `sources` entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Vector(VectorSource),
    Raster(RasterSource),
    RasterDem(RasterDemSource),
    GeoJson(GeoJsonSource),
    Image(ImageSource),
    Video(VideoSource),
    /// A source type this crate doesn't know, by its `type`.
    Unknown(String),
}

impl Source {
    /// The `type` in the style.
    pub fn type_name(&self) -> &str {
        match self {
            Self::Vector(_) => "vector",
            Self::Raster(_) => "raster",
            Self::RasterDem(_) => "raster-dem",
            Self::GeoJson(_) => "geojson",
            Self::Image(_) => "image",
            Self::Video(_) => "video",
            Self::Unknown(ty) => ty,
        }
    }

    /// The tiling of a tiled source (vector, raster, raster-dem).
    pub fn tiles(&self) -> Option<&TileSourceInfo> {
        match self {
            Self::Vector(s) => Some(&s.tiles),
            Self::Raster(s) => Some(&s.tiles),
            Self::RasterDem(s) => Some(&s.raster.tiles),
            _ => None,
        }
    }

    /// The property `promoteId` makes the feature id of `source_layer`'s features, if any.
    /// GeoJSON sources have a single unnamed layer, so their object form's only entry counts.
    pub fn promote_id(&self, source_layer: &str) -> Option<&str> {
        let promote = match self {
            Self::Vector(s) => s.promote_id.as_ref()?,
            Self::GeoJson(s) => s.promote_id.as_ref()?,
            _ => return None,
        };
        match promote {
            Json::String(key) => Some(key),
            Json::Object(by_layer) => match self {
                Self::GeoJson(_) => by_layer.values().next()?.as_str(),
                _ => by_layer.get(source_layer)?.as_str(),
            },
            _ => None,
        }
    }

    /// Logical pixels one tile spans on screen at its own zoom level.
    pub fn tile_size(&self) -> u32 {
        match self {
            Self::Raster(s) => s.tile_size,
            Self::RasterDem(s) => s.raster.tile_size,
            _ => 512,
        }
    }

    fn parse(id: &str, json: &Json, warnings: &mut Vec<StyleWarning>) -> Self {
        let mut warn = |property: &str, message: &str| {
            warnings.push(StyleWarning::new(
                "",
                &format!("sources.{id}.{property}"),
                message,
            ));
        };
        match json.get("type").and_then(Json::as_str) {
            Some("vector") => Self::Vector(VectorSource {
                tiles: TileSourceInfo::parse(json),
                promote_id: json.get("promoteId").cloned(),
                encoding: match str_field(json, "encoding").as_deref() {
                    None | Some("mvt") => VectorEncoding::Mvt,
                    Some("mlt") => VectorEncoding::Mlt,
                    Some(other) => {
                        warn("encoding", &format!("unknown encoding {other:?}"));
                        VectorEncoding::Mvt
                    }
                },
            }),
            Some("raster") => Self::Raster(RasterSource::parse(json)),
            Some("raster-dem") => {
                let encoding = match str_field(json, "encoding").as_deref() {
                    None | Some("mapbox") => DemEncoding::Mapbox,
                    Some("terrarium") => DemEncoding::Terrarium,
                    Some("custom") => DemEncoding::Custom,
                    Some(other) => {
                        warn("encoding", &format!("unknown encoding {other:?}"));
                        DemEncoding::Mapbox
                    }
                };
                Self::RasterDem(RasterDemSource {
                    raster: RasterSource::parse(json),
                    encoding,
                    red_factor: num_field(json, "redFactor").unwrap_or(1.0),
                    green_factor: num_field(json, "greenFactor").unwrap_or(1.0),
                    blue_factor: num_field(json, "blueFactor").unwrap_or(1.0),
                    base_shift: num_field(json, "baseShift").unwrap_or(0.0),
                })
            }
            Some("geojson") => Self::GeoJson(GeoJsonSource {
                data: json.get("data").cloned().unwrap_or(Json::Null),
                max_zoom: num_field(json, "maxzoom").unwrap_or(18.0) as u8,
                attribution: str_field(json, "attribution"),
                buffer: num_field(json, "buffer").unwrap_or(128.0) as u32,
                filter: json.get("filter").cloned(),
                tolerance: num_field(json, "tolerance").unwrap_or(0.375),
                cluster: bool_field(json, "cluster").unwrap_or(false),
                cluster_radius: num_field(json, "clusterRadius").unwrap_or(50.0),
                cluster_max_zoom: num_field(json, "clusterMaxZoom").map(|z| z as u8),
                cluster_min_points: num_field(json, "clusterMinPoints").unwrap_or(2.0) as u32,
                cluster_properties: json.get("clusterProperties").cloned(),
                line_metrics: bool_field(json, "lineMetrics").unwrap_or(false),
                generate_id: bool_field(json, "generateId").unwrap_or(false),
                promote_id: json.get("promoteId").cloned(),
            }),
            Some("image") => Self::Image(ImageSource {
                url: str_field(json, "url").unwrap_or_default(),
                coordinates: coordinates(json),
            }),
            Some("video") => Self::Video(VideoSource {
                urls: json
                    .get("urls")
                    .and_then(Json::as_array)
                    .map(|urls| {
                        urls.iter()
                            .filter_map(|u| u.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                coordinates: coordinates(json),
            }),
            Some(other) => {
                warn("type", &format!("unknown source type {other:?}"));
                Self::Unknown(other.to_owned())
            }
            None => {
                warn("type", "missing");
                Self::Unknown(String::new())
            }
        }
    }
}

/// What every tiled source has.
#[derive(Debug, Clone, PartialEq)]
pub struct TileSourceInfo {
    /// A TileJSON URL.
    pub url: Option<String>,
    /// Tile URL templates.
    pub tiles: Vec<String>,
    /// `[west, south, east, north]`
    pub bounds: [f64; 4],
    pub scheme: TileScheme,
    pub min_zoom: u8,
    pub max_zoom: u8,
    pub attribution: Option<String>,
    pub volatile: bool,
}

impl TileSourceInfo {
    fn parse(json: &Json) -> Self {
        Self {
            url: str_field(json, "url"),
            tiles: json
                .get("tiles")
                .and_then(Json::as_array)
                .map(|t| {
                    t.iter()
                        .filter_map(|u| u.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            bounds: json
                .get("bounds")
                .and_then(Json::as_array)
                .and_then(|b| {
                    b.iter()
                        .map(Json::as_f64)
                        .collect::<Option<Vec<_>>>()?
                        .try_into()
                        .ok()
                })
                .unwrap_or([-180.0, -85.051129, 180.0, 85.051129]),
            scheme: match str_field(json, "scheme").as_deref() {
                Some("tms") => TileScheme::Tms,
                _ => TileScheme::Xyz,
            },
            min_zoom: num_field(json, "minzoom").unwrap_or(0.0) as u8,
            max_zoom: num_field(json, "maxzoom").unwrap_or(22.0) as u8,
            attribution: str_field(json, "attribution"),
            volatile: bool_field(json, "volatile").unwrap_or(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileScheme {
    Xyz,
    /// Rows counted from the bottom.
    Tms,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorEncoding {
    Mvt,
    Mlt,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorSource {
    pub tiles: TileSourceInfo,
    /// A property name to use as feature ids, or an object of them per source layer.
    pub promote_id: Option<Json>,
    pub encoding: VectorEncoding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterSource {
    pub tiles: TileSourceInfo,
    /// Logical pixels a tile spans at its own zoom level.
    pub tile_size: u32,
}

impl RasterSource {
    fn parse(json: &Json) -> Self {
        Self {
            tiles: TileSourceInfo::parse(json),
            tile_size: num_field(json, "tileSize").unwrap_or(512.0) as u32,
        }
    }
}

/// How elevations are packed into a DEM tile's RGB channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemEncoding {
    /// `-10000 + (r * 65536 + g * 256 + b) * 0.1`
    Mapbox,
    /// `r * 256 + g + b / 256 - 32768`
    Terrarium,
    /// `(r * red_factor + g * green_factor + b * blue_factor) - base_shift`
    Custom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterDemSource {
    pub raster: RasterSource,
    pub encoding: DemEncoding,
    pub red_factor: f64,
    pub green_factor: f64,
    pub blue_factor: f64,
    pub base_shift: f64,
}

impl RasterDemSource {
    /// Unpack an elevation in meters from a pixel's 8-bit channels.
    pub fn elevation(&self, [r, g, b]: [u8; 3]) -> f64 {
        let [r, g, b] = [r, g, b].map(f64::from);
        match self.encoding {
            DemEncoding::Mapbox => -10000.0 + (r * 65536.0 + g * 256.0 + b) * 0.1,
            DemEncoding::Terrarium => r * 256.0 + g + b / 256.0 - 32768.0,
            DemEncoding::Custom => {
                r * self.red_factor + g * self.green_factor + b * self.blue_factor - self.base_shift
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeoJsonSource {
    /// Inline GeoJSON, or a URL string.
    pub data: Json,
    pub max_zoom: u8,
    pub attribution: Option<String>,
    /// Tile buffer, in 1/512ths of a tile.
    pub buffer: u32,
    pub filter: Option<Json>,
    /// Douglas-Peucker simplification tolerance, in 1/512ths of a tile.
    pub tolerance: f64,
    pub cluster: bool,
    /// In 1/512ths of a tile.
    pub cluster_radius: f64,
    pub cluster_max_zoom: Option<u8>,
    pub cluster_min_points: u32,
    pub cluster_properties: Option<Json>,
    pub line_metrics: bool,
    pub generate_id: bool,
    pub promote_id: Option<Json>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageSource {
    pub url: String,
    /// `[lng, lat]` of the top left, top right, bottom right and bottom left corners.
    pub coordinates: [[f64; 2]; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoSource {
    pub urls: Vec<String>,
    /// As [`ImageSource::coordinates`].
    pub coordinates: [[f64; 2]; 4],
}

/// One sprite sheet. Images of a sheet other than `"default"` are named `"{id}:{name}"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpriteSource {
    pub id: String,
    /// The URL without `.json`/`.png` and `@2x`.
    pub url: String,
}

impl SpriteSource {
    pub const DEFAULT_ID: &'static str = "default";

    fn parse_all(json: Option<&Json>) -> Vec<Self> {
        match json {
            Some(Json::String(url)) => vec![Self {
                id: Self::DEFAULT_ID.into(),
                url: url.clone(),
            }],
            Some(Json::Array(sprites)) => sprites
                .iter()
                .filter_map(|s| {
                    Some(Self {
                        id: s.get("id")?.as_str()?.to_owned(),
                        url: s.get("url")?.as_str()?.to_owned(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// The root `light`, which shades fill extrusions.
#[derive(Debug, Clone)]
pub struct Light {
    pub anchor: PropertyValue<IlluminationAnchor>,
    /// `[radial, azimuthal, polar]`: distance from the center, degrees clockwise from
    /// north (or the viewport's top) and degrees from straight above.
    pub position: PropertyValue<[f32; 3]>,
    pub color: PropertyValue<Color>,
    pub intensity: PropertyValue<f32>,
}

impl Light {
    fn parse(json: Option<&Json>, warnings: &mut Vec<StyleWarning>) -> Self {
        Self {
            anchor: PropertyValue::parse_with_default(
                "anchor",
                json,
                json!("viewport"),
                "light",
                warnings,
            ),
            position: PropertyValue::parse_with_default(
                "position",
                json,
                json!([1.15, 210, 30]),
                "light",
                warnings,
            ),
            color: PropertyValue::parse_with_default(
                "color",
                json,
                json!("#ffffff"),
                "light",
                warnings,
            ),
            intensity: PropertyValue::parse_with_default(
                "intensity",
                json,
                json!(0.5),
                "light",
                warnings,
            ),
        }
    }
}

/// The root `sky`: colors above the horizon and the fog that blends the map into them.
#[derive(Debug, Clone)]
pub struct Sky {
    pub sky_color: PropertyValue<Color>,
    pub horizon_color: PropertyValue<Color>,
    pub fog_color: PropertyValue<Color>,
    pub fog_ground_blend: PropertyValue<f32>,
    pub horizon_fog_blend: PropertyValue<f32>,
    pub sky_horizon_blend: PropertyValue<f32>,
    pub atmosphere_blend: PropertyValue<f32>,
}

impl Sky {
    fn parse(json: &Json, warnings: &mut Vec<StyleWarning>) -> Self {
        let section = Some(json);
        Self {
            sky_color: PropertyValue::parse_with_default(
                "sky-color",
                section,
                json!("#88C6FC"),
                "sky",
                warnings,
            ),
            horizon_color: PropertyValue::parse_with_default(
                "horizon-color",
                section,
                json!("#ffffff"),
                "sky",
                warnings,
            ),
            fog_color: PropertyValue::parse_with_default(
                "fog-color",
                section,
                json!("#ffffff"),
                "sky",
                warnings,
            ),
            fog_ground_blend: PropertyValue::parse_with_default(
                "fog-ground-blend",
                section,
                json!(0.5),
                "sky",
                warnings,
            ),
            horizon_fog_blend: PropertyValue::parse_with_default(
                "horizon-fog-blend",
                section,
                json!(0.8),
                "sky",
                warnings,
            ),
            sky_horizon_blend: PropertyValue::parse_with_default(
                "sky-horizon-blend",
                section,
                json!(0.8),
                "sky",
                warnings,
            ),
            atmosphere_blend: PropertyValue::parse_with_default(
                "atmosphere-blend",
                section,
                json!(0.8),
                "sky",
                warnings,
            ),
        }
    }
}

/// The root `terrain`: elevation from a `raster-dem` source.
#[derive(Debug, Clone, PartialEq)]
pub struct Terrain {
    pub source: String,
    pub exaggeration: f64,
}

/// The root `transition`, and the `*-transition` of paint properties.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    /// In milliseconds.
    pub duration: f64,
    /// In milliseconds.
    pub delay: f64,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            duration: 300.0,
            delay: 0.0,
        }
    }
}

impl Transition {
    pub(crate) fn parse(json: &Json, base: Self) -> Self {
        Self {
            duration: num_field(json, "duration").unwrap_or(base.duration),
            delay: num_field(json, "delay").unwrap_or(base.delay),
        }
    }
}

/// The parts of the style root besides layers, parsed by [`super::Style::from_json`].
pub(crate) struct Root {
    pub name: Option<String>,
    pub center: Option<[f64; 2]>,
    pub zoom: Option<f64>,
    pub bearing: f64,
    pub pitch: f64,
    pub sources: BTreeMap<String, Source>,
    pub sprite: Vec<SpriteSource>,
    pub glyphs: Option<String>,
    pub light: Light,
    pub sky: Option<Sky>,
    pub terrain: Option<Terrain>,
    pub projection: PropertyValue<ProjectionDefinition>,
    pub transition: Transition,
    pub state: BTreeMap<String, Json>,
}

impl Root {
    pub fn parse(json: &Json, warnings: &mut Vec<StyleWarning>) -> Self {
        let sources = json
            .get("sources")
            .and_then(Json::as_object)
            .map(|sources| {
                sources
                    .iter()
                    .map(|(id, s)| (id.clone(), Source::parse(id, s, warnings)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            name: str_field(json, "name"),
            center: json
                .get("center")
                .and_then(Json::as_array)
                .and_then(|c| Some([c.first()?.as_f64()?, c.get(1)?.as_f64()?])),
            zoom: num_field(json, "zoom"),
            bearing: num_field(json, "bearing").unwrap_or(0.0),
            pitch: num_field(json, "pitch").unwrap_or(0.0),
            sources,
            sprite: SpriteSource::parse_all(json.get("sprite")),
            glyphs: str_field(json, "glyphs"),
            light: Light::parse(json.get("light"), warnings),
            sky: json.get("sky").map(|s| Sky::parse(s, warnings)),
            terrain: json.get("terrain").and_then(|t| {
                Some(Terrain {
                    source: t.get("source")?.as_str()?.to_owned(),
                    exaggeration: num_field(t, "exaggeration").unwrap_or(1.0),
                })
            }),
            projection: PropertyValue::parse_with_default(
                "type",
                json.get("projection"),
                json!("mercator"),
                "projection",
                warnings,
            ),
            transition: json
                .get("transition")
                .map_or_else(Transition::default, |t| {
                    Transition::parse(t, Transition::default())
                }),
            // Each entry is `{"default": value}`; keep the defaults.
            state: json
                .get("state")
                .and_then(Json::as_object)
                .map(|state| {
                    state
                        .iter()
                        .map(|(k, v)| (k.clone(), v.get("default").cloned().unwrap_or(Json::Null)))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

fn str_field(json: &Json, key: &str) -> Option<String> {
    json.get(key).and_then(Json::as_str).map(str::to_owned)
}

fn num_field(json: &Json, key: &str) -> Option<f64> {
    json.get(key).and_then(Json::as_f64)
}

fn bool_field(json: &Json, key: &str) -> Option<bool> {
    json.get(key).and_then(Json::as_bool)
}

fn coordinates(json: &Json) -> [[f64; 2]; 4] {
    json.get("coordinates")
        .and_then(Json::as_array)
        .and_then(|corners| {
            corners
                .iter()
                .map(|c| Some([c.get(0)?.as_f64()?, c.get(1)?.as_f64()?]))
                .collect::<Option<Vec<_>>>()?
                .try_into()
                .ok()
        })
        .unwrap_or_default()
}
