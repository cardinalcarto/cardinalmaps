//! Static analysis of what an expression reads.
//!
//! `maplibre_expr` evaluates against an owned [`EvaluationContext`], so every feature
//! property it might look at has to be copied out of the tile first. Knowing ahead of
//! time which properties an expression can reach lets tile processing copy only those,
//! and lets expressions that read nothing from the feature be evaluated once per tile
//! (or once per style, when they don't read the zoom either).
//!
//! [`EvaluationContext`]: maplibre_expr::EvaluationContext

use std::collections::BTreeSet;

use maplibre_expr::{Expr, Value};

/// Which feature properties an expression reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PropertyDeps {
    /// No feature properties are read.
    #[default]
    None,
    /// Only these named properties are read.
    Keys(BTreeSet<String>),
    /// Properties are read by computed name or as a whole (`["properties"]`), so every
    /// property has to be made available.
    All,
}

impl PropertyDeps {
    pub(crate) fn insert(&mut self, key: &str) {
        match self {
            Self::None => *self = Self::Keys(BTreeSet::from([key.to_owned()])),
            Self::Keys(keys) => {
                keys.insert(key.to_owned());
            }
            Self::All => {}
        }
    }

    fn merge(&mut self, other: &Self) {
        match other {
            Self::None => {}
            Self::All => *self = Self::All,
            Self::Keys(keys) => keys.iter().for_each(|k| self.insert(k)),
        }
    }
}

/// Everything outside of its own literals that an expression can read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dependencies {
    /// `["zoom"]`
    pub zoom: bool,
    /// `["get", ...]`, `["has", ...]`, `["properties"]`
    pub properties: PropertyDeps,
    /// `["id"]`
    pub id: bool,
    /// `["geometry-type"]`
    pub geometry_type: bool,
    /// `["within", ...]`, `["distance", ...]`
    pub geometry: bool,
    /// `["feature-state", ...]`. Feature state lives in the renderer, so tile
    /// processing evaluates these against an empty state.
    pub feature_state: bool,
    /// `["global-state", ...]`
    pub global_state: bool,
    /// `["image", ...]`: whether an image exists is only known where the sprites are, when
    /// processing a tile, so these are evaluated per feature there (with
    /// [`EvaluationContext::available_images`](maplibre_expr::EvaluationContext)) rather
    /// than folded to a constant or evaluated by the renderer.
    pub images: bool,
    /// `["heatmap-density"]`, `["line-progress"]` or `["elevation"]`: inputs only the
    /// renderer has, which the layer type evaluates in its own way (a color ramp texture,
    /// say).
    pub special_input: bool,
}

impl Dependencies {
    pub fn of(expr: &Expr) -> Self {
        let mut deps = Self::default();
        deps.visit(expr);
        deps
    }

    /// Whether the expression can give different results for different features.
    pub fn is_feature_dependent(&self) -> bool {
        self.properties != PropertyDeps::None
            || self.id
            || self.geometry_type
            || self.geometry
            || self.feature_state
            || self.images
    }

    /// Whether the expression has the same result everywhere, so it can be evaluated once.
    pub fn is_constant(&self) -> bool {
        !self.zoom && !self.global_state && !self.special_input && !self.is_feature_dependent()
    }

    pub fn merge(&mut self, other: &Self) {
        self.zoom |= other.zoom;
        self.properties.merge(&other.properties);
        self.id |= other.id;
        self.geometry_type |= other.geometry_type;
        self.geometry |= other.geometry;
        self.feature_state |= other.feature_state;
        self.images |= other.images;
        self.global_state |= other.global_state;
        self.special_input |= other.special_input;
    }

    fn visit(&mut self, expr: &Expr) {
        match expr {
            Expr::Literal(_) | Expr::Var(_) => {}
            Expr::Call { op, args } => {
                match (op.as_str(), args.as_slice()) {
                    ("zoom", _) => self.zoom = true,
                    ("id", _) => self.id = true,
                    ("geometry-type", _) => self.geometry_type = true,
                    ("properties", _) => self.properties = PropertyDeps::All,
                    ("feature-state", _) => self.feature_state = true,
                    ("global-state", _) => self.global_state = true,
                    ("image", _) => self.images = true,
                    ("heatmap-density" | "line-progress" | "elevation", _) => {
                        self.special_input = true
                    }
                    // The two-argument forms read from an object argument, not the feature.
                    ("get" | "has", [key]) => match literal_string(key) {
                        Some(key) => self.properties.insert(key),
                        None => self.properties = PropertyDeps::All,
                    },
                    _ => {}
                }
                args.iter().for_each(|arg| self.visit(arg));
            }
            Expr::Let { bindings, body } => {
                bindings.iter().for_each(|(_, value)| self.visit(value));
                self.visit(body);
            }
            Expr::Match {
                input,
                arms,
                default,
            } => {
                self.visit(input);
                arms.iter().for_each(|(_, output)| self.visit(output));
                self.visit(default);
            }
            Expr::Step {
                input,
                output0,
                stops,
            } => {
                self.visit(input);
                self.visit(output0);
                stops.iter().for_each(|(_, output)| self.visit(output));
            }
            Expr::Interpolate { input, stops, .. } => {
                self.visit(input);
                stops.iter().for_each(|(_, output)| self.visit(output));
            }
            Expr::Format(sections) => {
                for section in sections {
                    self.visit(&section.content);
                    [
                        &section.scale,
                        &section.font,
                        &section.text_color,
                        &section.vertical_align,
                    ]
                    .into_iter()
                    .flatten()
                    .for_each(|e| self.visit(e));
                }
            }
            Expr::Within(_) | Expr::Distance(_) => {
                self.geometry = true;
                self.geometry_type = true;
            }
            Expr::Collator {
                case_sensitive,
                diacritic_sensitive,
                locale,
            } => [case_sensitive, diacritic_sensitive, locale]
                .into_iter()
                .flatten()
                .for_each(|e| self.visit(e)),
            Expr::NumberFormat {
                value,
                locale,
                currency,
                min_fraction_digits,
                max_fraction_digits,
                unit,
            } => {
                self.visit(value);
                [
                    locale,
                    currency,
                    min_fraction_digits,
                    max_fraction_digits,
                    unit,
                ]
                .into_iter()
                .flatten()
                .for_each(|e| self.visit(e));
            }
            Expr::Assert(_, inner) | Expr::Coerce(_, inner) => self.visit(inner),
        }
    }
}

/// The string a key expression always evaluates to, seeing through type checking wrappers.
fn literal_string(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Literal(Value::String(s)) => Some(s),
        Expr::Assert(_, inner) | Expr::Coerce(_, inner) => literal_string(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use maplibre_expr::parse;
    use serde_json::json;

    use super::*;

    fn deps(json: serde_json::Value) -> Dependencies {
        Dependencies::of(&parse(&json).unwrap())
    }

    #[test]
    fn literal_is_constant() {
        assert!(deps(json!(["+", 1, 2])).is_constant());
    }

    #[test]
    fn zoom_only() {
        let d = deps(json!(["interpolate", ["linear"], ["zoom"], 5, 1, 10, 4]));
        assert!(d.zoom);
        assert!(!d.is_feature_dependent());
        assert!(!d.is_constant());
    }

    #[test]
    fn collects_property_keys() {
        let d = deps(json!([
            "case",
            ["has", "a"],
            ["get", "b"],
            ["match", ["get", "c"], "x", 1, 0]
        ]));
        let expected = ["a", "b", "c"].map(String::from).into();
        assert_eq!(d.properties, PropertyDeps::Keys(expected));
        assert!(d.is_feature_dependent());
    }

    #[test]
    fn object_get_is_not_a_feature_read() {
        let d = deps(json!(["get", "a", ["literal", {"a": 1}]]));
        assert_eq!(d.properties, PropertyDeps::None);
    }

    #[test]
    fn computed_key_reads_everything() {
        let d = deps(json!(["get", ["concat", "name_", "en"]]));
        assert_eq!(d.properties, PropertyDeps::All);
    }

    #[test]
    fn image_availability_is_known_per_tile() {
        let d = deps(json!(["coalesce", ["image", "a"], ["image", "b"]]));
        assert!(d.images && d.is_feature_dependent() && !d.is_constant());
        // A plain image name stays a constant.
        let plain = crate::style::PropertyValue::<Option<crate::style::ResolvedImage>>::from_json(
            "fill-pattern",
            &json!("a"),
            None,
        )
        .unwrap();
        assert!(plain.as_constant().is_some());
    }

    #[test]
    fn geometry_type_and_id() {
        let d = deps(json!([
            "all",
            ["==", ["geometry-type"], "Polygon"],
            ["==", ["id"], 1]
        ]));
        assert!(d.geometry_type && d.id);
    }
}
