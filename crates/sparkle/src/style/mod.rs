//! A MapLibre style, parsed into typed values.
//!
//! Every layer type of the spec is represented with all of its layout and paint
//! properties (see [`layers`]); which of them tile processing and the renderer act on is up
//! to those stages. Problems with individual values become [`StyleWarning`]s and the spec
//! default is used instead, as MapLibre does.

mod deps;
mod edit;
pub mod layers;
mod property;
mod root;
mod values;

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde_json::Value as Json;

pub use deps::{Dependencies, PropertyDeps};
pub use edit::StyleDiff;
pub use layers::*;
pub use property::{AnyProperty, Filter, PropertyType, PropertyValue, ZoomCurve};
pub use root::{
    DemEncoding, GeoJsonSource, ImageSource, Light, RasterDemSource, RasterSource, Sky, Source,
    SpriteSource, Terrain, TileScheme, TileSourceInfo, Transition, VectorEncoding, VectorSource,
    VideoSource,
};
pub use values::*;

use crate::Error;

/// A problem with one property of one layer (or of the style root, with an empty `layer`)
/// that was worked around rather than failing the whole style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleWarning {
    pub layer: String,
    pub property: String,
    pub message: String,
}

impl StyleWarning {
    pub(crate) fn new(layer: &str, property: &str, message: impl fmt::Display) -> Self {
        Self {
            layer: layer.to_owned(),
            property: property.to_owned(),
            message: message.to_string(),
        }
    }
}

impl fmt::Display for StyleWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.layer.is_empty() {
            write!(f, "{}: {}", self.property, self.message)
        } else {
            write!(
                f,
                "layer {:?}, {}: {}",
                self.layer, self.property, self.message
            )
        }
    }
}

#[derive(Debug, Clone)]
pub struct Style {
    pub name: Option<String>,
    /// The default camera: `[lng, lat]`.
    pub center: Option<[f64; 2]>,
    pub zoom: Option<f64>,
    /// In degrees, clockwise from north.
    pub bearing: f64,
    /// In degrees, from looking straight down.
    pub pitch: f64,
    /// By source id.
    pub sources: BTreeMap<String, Source>,
    pub sprite: Vec<SpriteSource>,
    /// The glyph URL template. sparkle rasterizes glyphs from local fonts instead, but
    /// keeps this for completeness.
    pub glyphs: Option<String>,
    pub light: Light,
    pub sky: Option<Sky>,
    pub terrain: Option<Terrain>,
    pub projection: PropertyValue<ProjectionDefinition>,
    /// The default transition of paint properties.
    pub transition: Transition,
    /// The current `["global-state", ...]` values by name, starting from the style's
    /// defaults. They are baked into layer properties (see
    /// [`PropertyValue::bake_global_state`]), so change them with
    /// [`Style::set_global_state`] rather than here.
    pub state: BTreeMap<String, Json>,
    /// Layers in draw order, bottom first.
    pub layers: Vec<StyleLayer>,
    pub warnings: Vec<StyleWarning>,
    /// The document this was parsed from, with runtime edits applied; edits re-parse
    /// layers from it.
    json: Json,
}

impl Style {
    pub fn from_json(json: &Json) -> Result<Self, Error> {
        let layers = json
            .get("layers")
            .and_then(Json::as_array)
            .ok_or_else(|| Error::InvalidStyle("`layers` must be an array".into()))?;
        let mut warnings = Vec::new();
        // Read anyway, as MapLibre's validation would only report it.
        if json.get("version").and_then(Json::as_u64) != Some(8) {
            warnings.push(StyleWarning::new("", "version", "must be 8"));
        }
        let root = root::Root::parse(json, &mut warnings);
        let layers = layers
            .iter()
            .map(|layer| StyleLayer::parse(layer, &root.transition, &mut warnings))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name: root.name,
            center: root.center,
            zoom: root.zoom,
            bearing: root.bearing,
            pitch: root.pitch,
            sources: root.sources,
            sprite: root.sprite,
            glyphs: root.glyphs,
            light: root.light,
            sky: root.sky,
            terrain: root.terrain,
            projection: root.projection,
            transition: root.transition,
            state: root.state,
            layers,
            warnings,
            json: json.clone(),
        }
        .with_global_state_baked())
    }

    fn with_global_state_baked(mut self) -> Self {
        let state = self.global_state_values();
        for layer in &mut self.layers {
            layer.bake_global_state(&state);
        }
        self
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.sources.get(id)
    }

    /// The id of the first `vector` source, in id order.
    pub fn first_vector_source(&self) -> Option<&str> {
        self.sources
            .iter()
            .find(|(_, s)| matches!(s, Source::Vector(_)))
            .map(|(id, _)| id.as_str())
    }
}

impl FromStr for Style {
    type Err = Error;

    fn from_str(json: &str) -> Result<Self, Error> {
        Self::from_json(&serde_json::from_str(json)?)
    }
}

#[derive(Debug, Clone)]
pub struct StyleLayer {
    pub id: String,
    pub source: Option<String>,
    pub source_layer: Option<String>,
    pub min_zoom: f32,
    pub max_zoom: f32,
    /// `layout.visibility`. This is fixed at parse time, so hidden layers are never processed.
    pub visible: bool,
    pub filter: Filter,
    /// The `*-transition` of each paint property that has one, by property name (without
    /// the suffix), with the style's default transition filled in.
    pub transitions: BTreeMap<String, Transition>,
    pub kind: LayerKind,
}

impl StyleLayer {
    fn parse(
        json: &Json,
        default_transition: &Transition,
        warnings: &mut Vec<StyleWarning>,
    ) -> Result<Self, Error> {
        let id = json
            .get("id")
            .and_then(Json::as_str)
            .ok_or_else(|| Error::InvalidStyle("layer without a string `id`".into()))?
            .to_owned();
        let ty = json
            .get("type")
            .and_then(Json::as_str)
            .ok_or_else(|| Error::InvalidStyle(format!("layer {id:?} has no `type`")))?;
        let string = |key| json.get(key).and_then(Json::as_str).map(str::to_owned);
        let number = |key, default| {
            json.get(key)
                .and_then(Json::as_f64)
                .map_or(default, |v| v as f32)
        };
        let layout = json.get("layout");
        let paint = json.get("paint");

        let kind = LayerKind::parse(ty, &id, layout, paint, warnings);
        let mut transitions = BTreeMap::new();
        if let Some([known_layout, known_paint]) = LayerKind::properties_of(ty) {
            for (key, _) in layout.and_then(Json::as_object).into_iter().flatten() {
                if key != "visibility" && !known_layout.contains(&key.as_str()) {
                    warnings.push(StyleWarning::new(
                        &id,
                        key,
                        format!("not a layout property of {ty} layers"),
                    ));
                }
            }
            for (key, value) in paint.and_then(Json::as_object).into_iter().flatten() {
                match key.strip_suffix("-transition") {
                    Some(base) if known_paint.contains(&base) => {
                        transitions.insert(
                            base.to_owned(),
                            Transition::parse(value, *default_transition),
                        );
                    }
                    _ if known_paint.contains(&key.as_str()) => {}
                    _ => warnings.push(StyleWarning::new(
                        &id,
                        key,
                        format!("not a paint property of {ty} layers"),
                    )),
                }
            }
        }
        Ok(Self {
            source: string("source"),
            source_layer: string("source-layer"),
            min_zoom: number("minzoom", 0.0),
            max_zoom: number("maxzoom", 24.0),
            visible: layout
                .and_then(|l| l.get("visibility"))
                .and_then(Json::as_str)
                != Some("none"),
            filter: Filter::parse(json.get("filter"), &id, warnings),
            transitions,
            kind,
            id,
        })
    }

    /// Whether the layer draws anything at `zoom`. As in MapLibre, `maxzoom` is exclusive.
    pub fn is_visible_at(&self, zoom: f32) -> bool {
        self.visible && zoom >= self.min_zoom && zoom < self.max_zoom
    }

    /// Feature dependencies of everything tile processing evaluates for this layer: the
    /// filter and every layout and paint property.
    pub fn bake_global_state(&mut self, state: &BTreeMap<String, maplibre_expr::Value>) {
        self.filter.bake_global_state(state);
        self.kind.bake_global_state(state);
    }

    pub fn feature_dependencies(&self) -> Dependencies {
        let mut deps = Dependencies::default();
        if let Some(d) = self.filter.dependencies() {
            deps.merge(d);
        }
        self.kind.add_dependencies(&mut deps);
        deps
    }
}

/// A layer's type and its typed layout and paint properties.
// Styles have tens to hundreds of layers, parsed once, so boxing the larger variants
// would only cost indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum LayerKind {
    Background(BackgroundLayer),
    Fill(FillLayer),
    Line(LineLayer),
    Symbol(SymbolLayer),
    Circle(CircleLayer),
    Heatmap(HeatmapLayer),
    FillExtrusion(FillExtrusionLayer),
    Raster(RasterLayer),
    Hillshade(HillshadeLayer),
    ColorRelief(ColorReliefLayer),
    /// A layer type this crate doesn't know, by its `type`.
    Unsupported(String),
}

/// Calls `$m!(Variant, LayerStruct)` for every known layer type.
macro_rules! for_each_layer_type {
    ($m:ident) => {
        $m! {
            Background(BackgroundLayer),
            Fill(FillLayer),
            Line(LineLayer),
            Symbol(SymbolLayer),
            Circle(CircleLayer),
            Heatmap(HeatmapLayer),
            FillExtrusion(FillExtrusionLayer),
            Raster(RasterLayer),
            Hillshade(HillshadeLayer),
            ColorRelief(ColorReliefLayer),
        }
    };
}

macro_rules! layer_kind_impl {
    ($($variant:ident($ty:ident)),* $(,)?) => {
        impl LayerKind {
            /// Every layer `type` of the spec.
            pub const TYPES: &'static [&'static str] = &[$($ty::TYPE),*];

            pub(crate) fn parse(
                ty: &str,
                id: &str,
                layout: Option<&Json>,
                paint: Option<&Json>,
                warnings: &mut Vec<StyleWarning>,
            ) -> Self {
                match ty {
                    $($ty::TYPE => Self::$variant($ty::parse(id, layout, paint, warnings)),)*
                    other => Self::Unsupported(other.to_owned()),
                }
            }

            /// The layer `type` in the style.
            pub fn type_name(&self) -> &str {
                match self {
                    $(Self::$variant(_) => $ty::TYPE,)*
                    Self::Unsupported(ty) => ty,
                }
            }

            /// The layout and paint property names of a layer type, `None` if unknown.
            pub fn properties_of(ty: &str) -> Option<[&'static [&'static str]; 2]> {
                match ty {
                    $($ty::TYPE => Some([$ty::LAYOUT_PROPERTIES, $ty::PAINT_PROPERTIES]),)*
                    _ => None,
                }
            }

            pub fn add_dependencies(&self, deps: &mut Dependencies) {
                match self {
                    $(Self::$variant(layer) => layer.add_dependencies(deps),)*
                    Self::Unsupported(_) => {}
                }
            }

            /// Call `f` with each layout property and its spec name.
            pub fn for_each_layout_property(&self, f: &mut dyn FnMut(&'static str, &dyn AnyProperty)) {
                match self {
                    $(Self::$variant(layer) => layer.layout.for_each_property(f),)*
                    Self::Unsupported(_) => {}
                }
            }

            /// Call `f` with each paint property and its spec name.
            pub fn for_each_paint_property(&self, f: &mut dyn FnMut(&'static str, &dyn AnyProperty)) {
                match self {
                    $(Self::$variant(layer) => layer.paint.for_each_property(f),)*
                    Self::Unsupported(_) => {}
                }
            }

            /// [`Self::for_each_paint_property`], mutably.
            pub fn for_each_paint_property_mut(
                &mut self,
                f: &mut dyn FnMut(&'static str, &mut dyn AnyProperty),
            ) {
                match self {
                    $(Self::$variant(layer) => layer.paint.for_each_property_mut(f),)*
                    Self::Unsupported(_) => {}
                }
            }

            /// Bake `state` into every layout and paint property, see
            /// [`PropertyValue::bake_global_state`].
            pub fn bake_global_state(&mut self, state: &BTreeMap<String, maplibre_expr::Value>) {
                let mut bake = |_: &'static str, p: &mut dyn AnyProperty| p.bake_global_state(state);
                match self {
                    $(Self::$variant(layer) => {
                        layer.layout.for_each_property_mut(&mut bake);
                        layer.paint.for_each_property_mut(&mut bake);
                    })*
                    Self::Unsupported(_) => {}
                }
            }
        }

    };
}

for_each_layer_type!(layer_kind_impl);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_layers_in_order() {
        let style = Style::from_json(&json!({
            "version": 8,
            "layers": [
                {"id": "bg", "type": "background", "paint": {"background-color": "white"}},
                {"id": "labels", "type": "symbol", "layout": {"text-field": "{name}"}},
                {"id": "water", "type": "fill", "source": "s", "source-layer": "water",
                 "paint": {"fill-color": "#00f", "fill-color-transition": {"duration": 0}}},
                {"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                 "minzoom": 5, "layout": {"line-join": "round"},
                 "paint": {"line-width": ["get", "width"]}},
                {"id": "sky", "type": "sky"},
            ]
        }))
        .unwrap();
        assert!(style.warnings.is_empty(), "{:?}", style.warnings);
        assert_eq!(style.layers.len(), 5);
        let LayerKind::Background(bg) = &style.layers[0].kind else {
            panic!()
        };
        assert_eq!(bg.paint.color.evaluate_at_zoom(0.0), Color([1.0; 4]));
        let LayerKind::Symbol(labels) = &style.layers[1].kind else {
            panic!()
        };
        assert!(labels.layout.text_field.is_feature_dependent());
        assert_eq!(style.layers[2].transitions["fill-color"].duration, 0.0);
        assert!(matches!(&style.layers[4].kind, LayerKind::Unsupported(t) if t == "sky"));

        let roads = &style.layers[3];
        assert!(!roads.is_visible_at(4.0));
        assert!(roads.is_visible_at(5.0));
        let LayerKind::Line(line) = &roads.kind else {
            panic!()
        };
        assert!(matches!(
            line.layout.join,
            PropertyValue::Constant(LineJoin::Round)
        ));
        assert!(line.paint.width.is_feature_dependent());
        assert_eq!(
            roads.feature_dependencies().properties,
            PropertyDeps::Keys(["width".to_owned()].into())
        );
    }

    #[test]
    fn unknown_properties_warn() {
        let style = Style::from_json(&json!({"version": 8, "layers": [
            {"id": "a", "type": "fill", "paint": {"line-width": 2, "fill-colour": "red"}}
        ]}))
        .unwrap();
        let props: Vec<_> = style.warnings.iter().map(|w| w.property.as_str()).collect();
        assert_eq!(props, ["fill-colour", "line-width"]);
    }

    #[test]
    fn version_must_be_8() {
        let style = Style::from_json(&json!({"version": 7, "layers": []})).unwrap();
        let props: Vec<_> = style.warnings.iter().map(|w| w.property.as_str()).collect();
        assert_eq!(props, ["version"]);
        let style = Style::from_json(&json!({"version": 8, "layers": []})).unwrap();
        assert!(style.warnings.is_empty());
    }

    #[test]
    fn hidden_layer() {
        let style = Style::from_json(&json!({"layers": [
            {"id": "a", "type": "fill", "layout": {"visibility": "none"}}
        ]}))
        .unwrap();
        assert!(!style.layers[0].is_visible_at(10.0));
    }

    #[test]
    fn parses_root() {
        let style = Style::from_json(&json!({
            "version": 8,
            "center": [-122.3, 47.6], "zoom": 12, "bearing": 30, "pitch": 45,
            "sprite": [{"id": "default", "url": "a"}, {"id": "extra", "url": "b"}],
            "glyphs": "fonts/{fontstack}/{range}.pbf",
            "light": {"intensity": 0.8, "anchor": "map"},
            "sky": {"sky-color": "red"},
            "terrain": {"source": "dem", "exaggeration": 2},
            "projection": {"type": "globe"},
            "transition": {"duration": 100},
            "state": {"lang": {"default": "en"}},
            "sources": {
                "omt": {"type": "vector", "url": "x", "maxzoom": 14},
                "sat": {"type": "raster", "tiles": ["t/{z}/{x}/{y}.png"], "tileSize": 256},
                "dem": {"type": "raster-dem", "encoding": "terrarium"},
                "pts": {"type": "geojson", "data": {"type": "FeatureCollection", "features": []},
                        "cluster": true},
                "img": {"type": "image", "url": "i.png",
                        "coordinates": [[0, 1], [1, 1], [1, 0], [0, 0]]},
                "vid": {"type": "video", "urls": ["v.mp4"],
                        "coordinates": [[0, 1], [1, 1], [1, 0], [0, 0]]},
            },
            "layers": [],
        }))
        .unwrap();
        assert!(style.warnings.is_empty(), "{:?}", style.warnings);
        assert_eq!(style.center, Some([-122.3, 47.6]));
        assert_eq!(
            (style.zoom, style.bearing, style.pitch),
            (Some(12.0), 30.0, 45.0)
        );
        assert_eq!(style.sprite[1].id, "extra");
        assert_eq!(style.light.intensity.evaluate_at_zoom(0.0), 0.8);
        assert_eq!(
            style.light.position.evaluate_at_zoom(0.0),
            [1.15, 210.0, 30.0]
        );
        assert_eq!(
            style.sky.as_ref().unwrap().sky_color.evaluate_at_zoom(0.0),
            Color([1.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(style.terrain.as_ref().unwrap().exaggeration, 2.0);
        assert_eq!(
            style.projection.evaluate_at_zoom(0.0),
            ProjectionDefinition::Named("globe".into())
        );
        assert_eq!(style.transition.duration, 100.0);
        assert_eq!(style.state["lang"], "en");
        assert_eq!(style.first_vector_source(), Some("omt"));
        assert_eq!(style.sources["omt"].tiles().unwrap().max_zoom, 14);
        assert_eq!(style.sources["sat"].tile_size(), 256);
        let Source::RasterDem(dem) = &style.sources["dem"] else {
            panic!()
        };
        assert_eq!(dem.elevation([128, 0, 0]), 0.0);
        assert!(matches!(&style.sources["pts"], Source::GeoJson(g) if g.cluster));
        assert!(
            matches!(&style.sources["img"], Source::Image(i) if i.coordinates[0] == [0.0, 1.0])
        );
        assert_eq!(style.sources["vid"].type_name(), "video");
    }
}
