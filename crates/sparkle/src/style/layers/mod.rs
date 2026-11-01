//! Every style layer type with all of its layout and paint properties, one module each.
//!
//! The property lists are declared with [`properties!`], which also generates parsing (with
//! spec defaults from the reference embedded in `maplibre_expr`), the list of property
//! names the completeness test checks against that reference, and dependency collection.
//! A layer type's GPU paint attributes (see [`crate::paint`]) are listed next to its paint
//! properties.

mod background;
mod circle;
mod color_relief;
mod fill;
mod fill_extrusion;
mod heatmap;
mod hillshade;
mod line;
mod raster;
mod symbol;

pub use background::{BackgroundLayer, BackgroundLayout, BackgroundPaint};
pub use circle::{CircleLayer, CircleLayout, CirclePaint};
pub use color_relief::{ColorReliefLayer, ColorReliefLayout, ColorReliefPaint};
pub use fill::{FillLayer, FillLayout, FillPaint};
pub use fill_extrusion::{FillExtrusionLayer, FillExtrusionLayout, FillExtrusionPaint};
pub use heatmap::{HeatmapLayer, HeatmapLayout, HeatmapPaint};
pub use hillshade::{HillshadeLayer, HillshadeLayout, HillshadePaint};
pub use line::{LineLayer, LineLayout, LinePaint};
pub use raster::{RasterLayer, RasterLayout, RasterPaint};
pub use symbol::{SymbolLayer, SymbolLayout, SymbolPaint};

/// Declares a struct of [`PropertyValue`](crate::style::PropertyValue) fields, each tied to
/// its spec property name:
///
/// ```ignore
/// properties! {
///     pub struct LinePaint {
///         /// In pixels.
///         width: f32 = "line-width",
///     }
/// }
/// ```
macro_rules! properties {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty = $prop:literal),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $crate::style::PropertyValue<$ty>,)*
        }

        impl $name {
            /// The spec names of the properties, in declaration order.
            pub const PROPERTIES: &'static [&'static str] = &[$($prop),*];

            // Layer types without layout properties don't use the arguments.
            #[allow(unused_variables, clippy::ptr_arg)]
            pub(crate) fn parse(
                section: Option<&serde_json::Value>,
                layer_id: &str,
                warnings: &mut Vec<$crate::style::StyleWarning>,
            ) -> Self {
                Self {
                    $($field: $crate::style::PropertyValue::parse($prop, section, layer_id, warnings),)*
                }
            }

            /// Merge what every property reads into `deps`.
            #[allow(unused_variables)]
            pub fn add_dependencies(&self, deps: &mut $crate::style::Dependencies) {
                $(if let Some(d) = self.$field.dependencies() {
                    deps.merge(d);
                })*
            }

            /// Call `f` with each property and its spec name, in declaration order.
            #[allow(unused_variables)]
            pub fn for_each_property(
                &self,
                f: &mut dyn FnMut(&'static str, &dyn $crate::style::AnyProperty),
            ) {
                $(f($prop, &self.$field);)*
            }

            /// [`Self::for_each_property`], mutably.
            #[allow(unused_variables)]
            pub fn for_each_property_mut(
                &mut self,
                f: &mut dyn FnMut(&'static str, &mut dyn $crate::style::AnyProperty),
            ) {
                $(f($prop, &mut self.$field);)*
            }
        }

        impl Default for $name {
            /// Every property at its spec default.
            fn default() -> Self {
                Self::parse(None, "", &mut Vec::new())
            }
        }
    };
}

/// Declares a layer type's struct of layout and paint properties.
macro_rules! layer {
    ($(#[$meta:meta])* $name:ident = $ty:literal, $layout:ident, $paint:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default)]
        pub struct $name {
            pub layout: $layout,
            pub paint: $paint,
        }

        impl $name {
            /// The layer `type` in the style.
            pub const TYPE: &'static str = $ty;
            /// The spec names of the layout properties (besides `visibility`).
            pub const LAYOUT_PROPERTIES: &'static [&'static str] = $layout::PROPERTIES;
            /// The spec names of the paint properties.
            pub const PAINT_PROPERTIES: &'static [&'static str] = $paint::PROPERTIES;

            pub(crate) fn parse(
                layer_id: &str,
                layout: Option<&serde_json::Value>,
                paint: Option<&serde_json::Value>,
                warnings: &mut Vec<$crate::style::StyleWarning>,
            ) -> Self {
                Self {
                    layout: $layout::parse(layout, layer_id, warnings),
                    paint: $paint::parse(paint, layer_id, warnings),
                }
            }

            pub fn add_dependencies(&self, deps: &mut $crate::style::Dependencies) {
                self.layout.add_dependencies(deps);
                self.paint.add_dependencies(deps);
            }
        }
    };
}

pub(crate) use {layer, properties};

#[cfg(test)]
mod tests {
    use crate::style::LayerKind;

    /// Which layer type a reference property belongs to, by its prefix.
    fn layer_type(property: &str) -> Option<&'static [&'static str]> {
        const PREFIXES: &[(&str, &[&str])] = &[
            ("background-", &["background"]),
            ("fill-extrusion-", &["fill-extrusion"]),
            ("fill-", &["fill"]),
            ("line-", &["line"]),
            ("symbol-", &["symbol"]),
            ("icon-", &["symbol"]),
            ("text-", &["symbol"]),
            ("circle-", &["circle"]),
            ("heatmap-", &["heatmap"]),
            ("raster-", &["raster"]),
            ("hillshade-", &["hillshade"]),
            ("color-relief-", &["color-relief"]),
            ("resampling", &["raster", "hillshade", "color-relief"]),
        ];
        PREFIXES
            .iter()
            .find(|(prefix, _)| property.starts_with(prefix))
            .map(|(_, types)| *types)
    }

    /// Fails when the spec reference has a layout or paint property that the layer type it
    /// belongs to doesn't parse.
    #[test]
    fn every_reference_property_is_parsed() {
        let reference = maplibre_expr::migrate::reference().as_object().unwrap();
        let mut missing = Vec::new();
        for name in reference.keys() {
            // Handled by `StyleLayer` itself, for every layer type.
            if name == "visibility" {
                continue;
            }
            let types = layer_type(name).unwrap_or_else(|| panic!("unknown property {name}"));
            for ty in types {
                let [layout, paint] = LayerKind::properties_of(ty).unwrap();
                if !layout.contains(&name.as_str()) && !paint.contains(&name.as_str()) {
                    missing.push(format!("{ty}: {name}"));
                }
            }
        }
        assert!(missing.is_empty(), "not parsed: {missing:#?}");
    }

    /// And the other way around: every declared property exists in the reference, so
    /// typos don't silently get no default. Our own extensions are prefixed `cardinal-`.
    #[test]
    fn every_parsed_property_is_in_the_reference() {
        for ty in LayerKind::TYPES {
            for name in LayerKind::properties_of(ty).unwrap().concat() {
                if name.starts_with("cardinal-") {
                    continue;
                }
                assert!(
                    maplibre_expr::migrate::property_spec(name).is_some(),
                    "{ty}: {name} is not in the reference"
                );
            }
        }
    }

    /// Spec defaults must parse for every property, or layers silently get `T::default()`.
    #[test]
    fn spec_defaults_parse() {
        for ty in LayerKind::TYPES {
            let kind = LayerKind::parse(ty, "l", None, None, &mut Vec::new());
            assert!(!matches!(kind, LayerKind::Unsupported(_)), "{ty}");
        }
        let symbol = crate::style::SymbolLayer::default();
        assert_eq!(
            symbol.layout.icon_padding.as_constant(),
            Some(&crate::style::Padding([2.0; 4]))
        );
        let heatmap = crate::style::HeatmapLayer::default();
        assert!(heatmap.paint.color.dependencies().unwrap().special_input);
        let hillshade = crate::style::HillshadeLayer::default();
        assert_eq!(
            hillshade.paint.illumination_direction.as_constant(),
            Some(&crate::style::NumberArray(vec![335.0]))
        );
    }

    #[test]
    fn spec_default_values_are_typed() {
        let reference = maplibre_expr::migrate::reference().as_object().unwrap();
        // A default that fails to parse leaves `T::default()`, which for most types is a
        // zero; catch that by checking a sample of non-zero defaults.
        let line = crate::style::LineLayer::default();
        assert_eq!(line.layout.round_limit.as_constant(), Some(&1.05));
        let symbol = crate::style::SymbolLayer::default();
        assert_eq!(symbol.layout.symbol_spacing.as_constant(), Some(&250.0));
        assert_eq!(symbol.layout.text_line_height.as_constant(), Some(&1.2));
        assert!(reference.contains_key("text-keep-upright"));
        assert_eq!(symbol.layout.text_keep_upright.as_constant(), Some(&true));
    }
}
