//! Typed style property values backed by `maplibre_expr` expressions.

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;

use maplibre_expr::{EvaluationContext, Expr, FormatArg, InterpKind, Type, Value};
use serde_json::Value as Json;

use super::StyleWarning;
use super::deps::Dependencies;

/// A Rust type a style property evaluates to. The impls live in [`super::values`].
pub trait PropertyType: Clone + Default + fmt::Debug + Send + Sync + 'static {
    /// Whether the property is string typed (not merely enum typed), in which case type
    /// checking coerces the expression's result to a string instead of asserting it.
    const COERCE_TO_STRING: bool = false;

    /// The type the expression is checked against, which also decides the coercions
    /// type checking inserts (a string becomes a color for a color property, ...).
    fn expr_type() -> Type;

    fn from_value(value: &Value) -> Option<Self>;

    /// The value `t` of the way from `from` to `to`, for transitions. Types the spec
    /// doesn't transition switch to `to`.
    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        let _ = (from, t);
        to.clone()
    }
}

/// How a property's value changes with zoom, when it does: the outermost `interpolate` or
/// `step` over `["zoom"]`, which the spec requires zoom-dependent expressions to have.
#[derive(Debug, Clone, Copy)]
pub enum ZoomCurve {
    Interpolate(InterpKind),
    Step,
}

impl ZoomCurve {
    fn of(expr: &Expr) -> Option<Self> {
        match expr {
            Expr::Assert(_, inner) | Expr::Coerce(_, inner) => Self::of(inner),
            Expr::Let { body, .. } => Self::of(body),
            Expr::Interpolate { kind, input, .. } if is_zoom(input) => {
                Some(Self::Interpolate(*kind))
            }
            Expr::Step { input, .. } if is_zoom(input) => Some(Self::Step),
            Expr::Call { op, args } if op == "coalesce" => args.iter().find_map(Self::of),
            _ => None,
        }
    }

    /// How far `zoom` is from `lower` to `upper` along this curve, as MapLibre's
    /// `interpolationFactor`: `0` at `lower`, `1` at `upper`, and always `0` for steps.
    /// Not clamped.
    pub fn factor(self, zoom: f64, lower: f64, upper: f64) -> f64 {
        match self {
            Self::Step => 0.0,
            Self::Interpolate(InterpKind::Linear) => exponential_factor(zoom, 1.0, lower, upper),
            Self::Interpolate(InterpKind::Exponential(base)) => {
                exponential_factor(zoom, base, lower, upper)
            }
            Self::Interpolate(InterpKind::CubicBezier(x1, y1, x2, y2)) => {
                let t = exponential_factor(zoom, 1.0, lower, upper);
                UnitBezier::new(x1, y1, x2, y2).solve(t, 1e-6)
            }
        }
    }
}

fn is_zoom(expr: &Expr) -> bool {
    match expr {
        Expr::Assert(_, inner) | Expr::Coerce(_, inner) => is_zoom(inner),
        Expr::Call { op, args } => op == "zoom" && args.is_empty(),
        _ => false,
    }
}

fn exponential_factor(input: f64, base: f64, lower: f64, upper: f64) -> f64 {
    let difference = upper - lower;
    let progress = input - lower;
    if difference == 0.0 {
        0.0
    } else if base == 1.0 {
        progress / difference
    } else {
        (base.powf(progress) - 1.0) / (base.powf(difference) - 1.0)
    }
}

/// The cubic Bézier easing curve of `["cubic-bezier", x1, y1, x2, y2]`, as in MapLibre.
struct UnitBezier {
    c: [f64; 3],
    b: [f64; 3],
    a: [f64; 3],
}

impl UnitBezier {
    fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        let coefficients = |p1: f64, p2: f64| {
            let c = 3.0 * p1;
            let b = 3.0 * (p2 - p1) - c;
            (c, b, 1.0 - c - b)
        };
        let (cx, bx, ax) = coefficients(x1, x2);
        let (cy, by, ay) = coefficients(y1, y2);
        Self {
            c: [cx, cy, 0.0],
            b: [bx, by, 0.0],
            a: [ax, ay, 0.0],
        }
    }

    fn sample(&self, axis: usize, t: f64) -> f64 {
        ((self.a[axis] * t + self.b[axis]) * t + self.c[axis]) * t
    }

    fn solve_x(&self, x: f64, epsilon: f64) -> f64 {
        let mut t = x;
        // Newton's method first, which converges quickly where the slope allows it.
        for _ in 0..8 {
            let error = self.sample(0, t) - x;
            if error.abs() < epsilon {
                return t;
            }
            let slope = (3.0 * self.a[0] * t + 2.0 * self.b[0]) * t + self.c[0];
            if slope.abs() < 1e-6 {
                break;
            }
            t -= error / slope;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x.clamp(0.0, 1.0);
        for _ in 0..64 {
            let value = self.sample(0, t);
            if (value - x).abs() < epsilon {
                break;
            }
            if x > value {
                lo = t;
            } else {
                hi = t;
            }
            t = (hi - lo) / 2.0 + lo;
        }
        t
    }

    fn solve(&self, x: f64, epsilon: f64) -> f64 {
        self.sample(1, self.solve_x(x, epsilon))
    }
}

/// The value of one paint or layout property.
///
/// Expressions that read neither the zoom nor the feature are folded to a
/// [`PropertyValue::Constant`] when the style is parsed.
#[derive(Debug, Clone)]
pub enum PropertyValue<T> {
    Constant(T),
    Expression {
        expr: Expr,
        deps: Dependencies,
        /// Set when the value depends on the zoom.
        curve: Option<ZoomCurve>,
        /// What evaluation falls back to when the expression fails or produces the
        /// wrong type, as MapLibre falls back to the property's spec default.
        default: T,
    },
}

impl<T: PropertyType> Default for PropertyValue<T> {
    fn default() -> Self {
        Self::Constant(T::default())
    }
}

impl<T: PropertyType> PropertyValue<T> {
    /// Parse the property `name` from a layer's `paint` or `layout` object. When the style
    /// doesn't set it, or sets it to something invalid (with a warning), this is the spec
    /// default from the reference embedded in `maplibre_expr`, or `T::default()` for
    /// properties without one (which is `None` for the `Option` types used for those).
    pub(crate) fn parse(
        name: &str,
        section: Option<&Json>,
        layer_id: &str,
        warnings: &mut Vec<StyleWarning>,
    ) -> Self {
        let default = || Self::spec_default(name);
        Self::parse_or(name, section, layer_id, warnings, default)
    }

    /// Like [`Self::parse`] for properties the spec reference doesn't cover (the root
    /// `light`, `sky` and `terrain`), with the default given as style JSON.
    pub(crate) fn parse_with_default(
        name: &str,
        section: Option<&Json>,
        default: Json,
        context: &str,
        warnings: &mut Vec<StyleWarning>,
    ) -> Self {
        let default = || Self::from_json(name, &default, T::default()).unwrap_or_default();
        Self::parse_or(name, section, context, warnings, default)
    }

    fn parse_or(
        name: &str,
        section: Option<&Json>,
        layer_id: &str,
        warnings: &mut Vec<StyleWarning>,
        default: impl FnOnce() -> Self,
    ) -> Self {
        let Some(json) = section.and_then(|s| s.get(name)) else {
            return default();
        };
        let default = default();
        let fallback = match &default {
            Self::Constant(v) => v.clone(),
            Self::Expression { .. } => T::default(),
        };
        match Self::from_json(name, json, fallback) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(StyleWarning::new(layer_id, name, err));
                default
            }
        }
    }

    /// The spec default of the property `name`.
    pub fn spec_default(name: &str) -> Self {
        maplibre_expr::migrate::property_spec(name)
            .and_then(|spec| spec.get("default"))
            .and_then(|json| Self::from_json(name, json, T::default()).ok())
            .unwrap_or_default()
    }

    /// Parse a style value (literal, legacy function or expression) for the property
    /// `name`. `default` is what evaluation falls back to when an expression fails.
    pub fn from_json(name: &str, json: &Json, default: T) -> Result<Self, String> {
        // A bare array is a literal (`[0, 0]`, `["Open Sans Regular"]`) unless its head
        // names an operator, as MapLibre reads property values.
        let wrapped;
        let json = if json.is_array() && !maplibre_expr::is_expression(json) {
            wrapped = Json::Array(vec!["literal".into(), json.clone()]);
            &wrapped
        } else {
            json
        };
        let expr = maplibre_expr::parse_property(name, json)
            .and_then(|expr| {
                maplibre_expr::typecheck(&expr, Some(&T::expr_type()), T::COERCE_TO_STRING)
            })
            .map_err(|e| e.to_string())?;
        Self::from_checked(expr, default)
    }

    /// A type checked expression, folded to a constant if it reads nothing.
    fn from_checked(expr: Expr, default: T) -> Result<Self, String> {
        let deps = Dependencies::of(&expr);
        if deps.is_constant() {
            let value = maplibre_expr::evaluate(&expr, &EvaluationContext::new())
                .map_err(|e| e.to_string())?;
            return T::from_value(&value)
                .map(Self::Constant)
                .ok_or_else(|| format!("invalid value {value:?}"));
        }
        let curve = if deps.zoom {
            ZoomCurve::of(&expr)
        } else {
            None
        };
        Ok(Self::Expression {
            expr,
            deps,
            curve,
            default,
        })
    }

    /// Replace `["global-state", name]` with the value `state` has for `name` (null when it
    /// has none) and fold what becomes constant. [`Dependencies::global_state`] stays set
    /// on what remains an expression, to tell that the value changes with the state.
    pub fn bake_global_state(&mut self, state: &BTreeMap<String, Value>) {
        let Self::Expression {
            expr,
            deps,
            default,
            ..
        } = self
        else {
            return;
        };
        if !deps.global_state {
            return;
        }
        let default = default.clone();
        let baked = substitute_global_state(expr, state);
        // An expression that fails with this state falls back to the default, as it would
        // when evaluated.
        *self = Self::from_checked(baked, default.clone()).unwrap_or(Self::Constant(default));
        if let Self::Expression { deps, .. } = self {
            deps.global_state = true;
        }
    }

    pub fn evaluate(&self, ctx: &EvaluationContext) -> T {
        match self {
            Self::Constant(value) => value.clone(),
            Self::Expression { expr, default, .. } => maplibre_expr::evaluate(expr, ctx)
                .ok()
                .and_then(|v| T::from_value(&v))
                .unwrap_or_else(|| default.clone()),
        }
    }

    /// Evaluate with nothing but a zoom level, as a renderer does for properties that
    /// are not [feature dependent](Self::is_feature_dependent) on every frame.
    pub fn evaluate_at_zoom(&self, zoom: f64) -> T {
        match self {
            Self::Constant(value) => value.clone(),
            Self::Expression { .. } => self.evaluate(&EvaluationContext::new().with_zoom(zoom)),
        }
    }

    /// The value when it is the same everywhere.
    pub fn as_constant(&self) -> Option<&T> {
        match self {
            Self::Constant(value) => Some(value),
            Self::Expression { .. } => None,
        }
    }

    pub fn dependencies(&self) -> Option<&Dependencies> {
        match self {
            Self::Constant(_) => None,
            Self::Expression { deps, .. } => Some(deps),
        }
    }

    /// The zoom curve of a zoom-dependent value.
    pub fn zoom_curve(&self) -> Option<ZoomCurve> {
        match self {
            Self::Constant(_) => None,
            Self::Expression { curve, .. } => *curve,
        }
    }

    /// Whether the value differs between features, so tile processing has to evaluate it
    /// per feature rather than leaving it to the renderer.
    pub fn is_feature_dependent(&self) -> bool {
        self.dependencies()
            .is_some_and(Dependencies::is_feature_dependent)
    }

    pub fn is_zoom_dependent(&self) -> bool {
        self.dependencies().is_some_and(|d| d.zoom)
    }
}

/// A property of any value type, for code that handles properties by name: style
/// diffing, transitions, and baking in global state. Implemented by every
/// [`PropertyValue`].
pub trait AnyProperty: fmt::Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;

    fn dependencies(&self) -> Option<&Dependencies>;

    fn is_feature_dependent(&self) -> bool {
        self.dependencies()
            .is_some_and(Dependencies::is_feature_dependent)
    }

    fn clone_box(&self) -> Box<dyn AnyProperty>;

    /// Become a copy of `other`, if it's a property of the same type. Returns whether it was.
    fn assign(&mut self, other: &dyn AnyProperty) -> bool;

    /// Become the constant `t` of the way from `from`'s value to `to`'s (both of this
    /// type) at `zoom`. Returns false, changing nothing, if either differs per feature or
    /// isn't of this type.
    fn set_interpolated(
        &mut self,
        from: &dyn AnyProperty,
        to: &dyn AnyProperty,
        t: f32,
        zoom: f64,
    ) -> bool;

    fn bake_global_state(&mut self, state: &BTreeMap<String, Value>);
}

impl<T: PropertyType> AnyProperty for PropertyValue<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dependencies(&self) -> Option<&Dependencies> {
        PropertyValue::dependencies(self)
    }

    fn clone_box(&self) -> Box<dyn AnyProperty> {
        Box::new(self.clone())
    }

    fn assign(&mut self, other: &dyn AnyProperty) -> bool {
        match other.as_any().downcast_ref::<Self>() {
            Some(other) => {
                self.clone_from(other);
                true
            }
            None => false,
        }
    }

    fn set_interpolated(
        &mut self,
        from: &dyn AnyProperty,
        to: &dyn AnyProperty,
        t: f32,
        zoom: f64,
    ) -> bool {
        let (Some(from), Some(to)) = (
            from.as_any().downcast_ref::<Self>(),
            to.as_any().downcast_ref::<Self>(),
        ) else {
            return false;
        };
        if from.is_feature_dependent() || to.is_feature_dependent() {
            return false;
        }
        let value = T::interpolate(&from.evaluate_at_zoom(zoom), &to.evaluate_at_zoom(zoom), t);
        *self = Self::Constant(value);
        true
    }

    fn bake_global_state(&mut self, state: &BTreeMap<String, Value>) {
        PropertyValue::bake_global_state(self, state);
    }
}

/// `expr` with every `["global-state", name]` replaced by its value in `state`.
fn substitute_global_state(expr: &Expr, state: &BTreeMap<String, Value>) -> Expr {
    let sub = |e: &Expr| substitute_global_state(e, state);
    let boxed = |e: &Expr| Box::new(sub(e));
    let opt = |e: &Option<Box<Expr>>| e.as_deref().map(boxed);
    match expr {
        Expr::Call { op, args } if op == "global-state" => match args.as_slice() {
            [Expr::Literal(Value::String(name))] => {
                Expr::Literal(state.get(name).cloned().unwrap_or(Value::Null))
            }
            _ => expr.clone(),
        },
        Expr::Call { op, args } => Expr::Call {
            op: op.clone(),
            args: args.iter().map(sub).collect(),
        },
        Expr::Let { bindings, body } => Expr::Let {
            bindings: bindings.iter().map(|(n, e)| (n.clone(), sub(e))).collect(),
            body: boxed(body),
        },
        Expr::Match {
            input,
            arms,
            default,
        } => Expr::Match {
            input: boxed(input),
            arms: arms.iter().map(|(l, e)| (l.clone(), sub(e))).collect(),
            default: boxed(default),
        },
        Expr::Step {
            input,
            output0,
            stops,
        } => Expr::Step {
            input: boxed(input),
            output0: boxed(output0),
            stops: stops.iter().map(|(s, e)| (*s, sub(e))).collect(),
        },
        Expr::Interpolate {
            kind,
            space,
            input,
            stops,
            projection,
        } => Expr::Interpolate {
            kind: *kind,
            space: *space,
            input: boxed(input),
            stops: stops.iter().map(|(s, e)| (*s, sub(e))).collect(),
            projection: *projection,
        },
        Expr::Format(sections) => Expr::Format(
            sections
                .iter()
                .map(|s| FormatArg {
                    content: sub(&s.content),
                    scale: s.scale.as_ref().map(sub),
                    font: s.font.as_ref().map(sub),
                    text_color: s.text_color.as_ref().map(sub),
                    vertical_align: s.vertical_align.as_ref().map(sub),
                })
                .collect(),
        ),
        Expr::Collator {
            case_sensitive,
            diacritic_sensitive,
            locale,
        } => Expr::Collator {
            case_sensitive: opt(case_sensitive),
            diacritic_sensitive: opt(diacritic_sensitive),
            locale: opt(locale),
        },
        Expr::NumberFormat {
            value,
            locale,
            currency,
            min_fraction_digits,
            max_fraction_digits,
            unit,
        } => Expr::NumberFormat {
            value: boxed(value),
            locale: opt(locale),
            currency: opt(currency),
            min_fraction_digits: opt(min_fraction_digits),
            max_fraction_digits: opt(max_fraction_digits),
            unit: opt(unit),
        },
        Expr::Assert(t, inner) => Expr::Assert(t.clone(), boxed(inner)),
        Expr::Coerce(t, inner) => Expr::Coerce(t.clone(), boxed(inner)),
        Expr::Literal(_) | Expr::Var(_) | Expr::Within(_) | Expr::Distance(_) => expr.clone(),
    }
}

/// A layer's `filter`.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// `None` when the layer has no filter, which lets every feature through.
    expr: Option<(Expr, Dependencies)>,
}

impl Filter {
    pub(crate) fn parse(
        json: Option<&serde_json::Value>,
        layer_id: &str,
        warnings: &mut Vec<StyleWarning>,
    ) -> Self {
        let Some(json) = json else {
            return Self::default();
        };
        let parsed = maplibre_expr::parse_filter(json)
            .map_err(|e| e.to_string())
            .and_then(|expr| {
                maplibre_expr::typecheck(&expr, Some(&Type::Boolean), false)
                    .map_err(|e| e.to_string())
            });
        match parsed {
            Ok(expr) => {
                let deps = Dependencies::of(&expr);
                Self {
                    expr: Some((expr, deps)),
                }
            }
            Err(err) => {
                // MapLibre rejects a layer with an invalid filter, so match that by
                // letting nothing through rather than everything.
                warnings.push(StyleWarning::new(layer_id, "filter", err));
                Self {
                    expr: Some((Expr::Literal(Value::Bool(false)), Dependencies::default())),
                }
            }
        }
    }

    /// Whether the feature in `ctx` passes. Like MapLibre, anything other than `true`
    /// (including an evaluation error) rejects it.
    pub fn matches(&self, ctx: &EvaluationContext) -> bool {
        match &self.expr {
            None => true,
            Some((expr, _)) => matches!(maplibre_expr::evaluate(expr, ctx), Ok(Value::Bool(true))),
        }
    }

    pub fn dependencies(&self) -> Option<&Dependencies> {
        self.expr.as_ref().map(|(_, deps)| deps)
    }

    /// Replace `["global-state", name]` with its value in `state`, as
    /// [`PropertyValue::bake_global_state`] does.
    pub fn bake_global_state(&mut self, state: &BTreeMap<String, Value>) {
        if let Some((expr, deps)) = &mut self.expr
            && deps.global_state
        {
            *expr = substitute_global_state(expr, state);
            *deps = Dependencies::of(expr);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::values::{Color, LineCap, Padding, ResolvedImage};

    fn parse<T: PropertyType>(name: &str, paint: serde_json::Value) -> PropertyValue<T> {
        let mut warnings = Vec::new();
        let value = PropertyValue::parse(name, Some(&paint), "layer", &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        value
    }

    #[test]
    fn color_string_folds_to_constant() {
        let v: PropertyValue<Color> = parse("fill-color", json!({"fill-color": "#ff0000"}));
        assert!(matches!(
            v,
            PropertyValue::Constant(Color([1.0, 0.0, 0.0, 1.0]))
        ));
    }

    #[test]
    fn color_is_premultiplied() {
        let v: PropertyValue<Color> =
            parse("fill-color", json!({"fill-color": "rgba(255, 0, 0, 0.5)"}));
        assert_eq!(v.evaluate_at_zoom(0.0), Color([0.5, 0.0, 0.0, 0.5]));
    }

    #[test]
    fn missing_uses_spec_default() {
        let v: PropertyValue<f32> = parse("line-width", json!({}));
        assert!(matches!(v, PropertyValue::Constant(1.0)));
        let v: PropertyValue<f32> = parse("line-miter-limit", json!({}));
        assert!(matches!(v, PropertyValue::Constant(2.0)));
        let v: PropertyValue<Padding> = parse("icon-padding", json!({}));
        assert_eq!(v.as_constant(), Some(&Padding([2.0; 4])));
        let v: PropertyValue<Option<ResolvedImage>> = parse("fill-pattern", json!({}));
        assert_eq!(v.as_constant(), Some(&None));
        let v: PropertyValue<Vec<String>> = parse("text-font", json!({}));
        assert_eq!(v.as_constant().unwrap()[0], "Open Sans Regular");
    }

    #[test]
    fn literal_arrays() {
        let v: PropertyValue<[f32; 2]> = parse("fill-translate", json!({"fill-translate": [1, 2]}));
        assert_eq!(v.as_constant(), Some(&[1.0, 2.0]));
        let v: PropertyValue<Option<Vec<f32>>> =
            parse("line-dasharray", json!({"line-dasharray": [2, 1]}));
        assert_eq!(v.as_constant(), Some(&Some(vec![2.0, 1.0])));
    }

    #[test]
    fn legacy_stops_are_zoom_dependent() {
        let v: PropertyValue<f32> = parse(
            "line-width",
            json!({"line-width": {"stops": [[2, 1], [6, 5]]}}),
        );
        assert!(v.is_zoom_dependent());
        assert!(!v.is_feature_dependent());
        assert_eq!(v.evaluate_at_zoom(4.0), 3.0);
        assert!(matches!(v.zoom_curve(), Some(ZoomCurve::Interpolate(_))));
    }

    #[test]
    fn invalid_value_warns_and_falls_back() {
        let mut warnings = Vec::new();
        let paint = json!({"line-width": ["get"], "line-cap": "pointy"});
        let v = PropertyValue::<f32>::parse("line-width", Some(&paint), "roads", &mut warnings);
        assert!(matches!(v, PropertyValue::Constant(1.0)));
        let v = PropertyValue::<LineCap>::parse("line-cap", Some(&paint), "roads", &mut warnings);
        assert!(matches!(v, PropertyValue::Constant(LineCap::Butt)));
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].layer, "roads");
    }

    #[test]
    fn interpolation_factors() {
        let linear = ZoomCurve::Interpolate(InterpKind::Linear);
        assert_eq!(linear.factor(10.5, 10.0, 11.0), 0.5);
        let exp = ZoomCurve::Interpolate(InterpKind::Exponential(2.0));
        assert!((exp.factor(10.5, 10.0, 11.0) - (2f64.sqrt() - 1.0)).abs() < 1e-12);
        assert_eq!(ZoomCurve::Step.factor(10.5, 10.0, 11.0), 0.0);
        let ease = ZoomCurve::Interpolate(InterpKind::CubicBezier(0.0, 0.0, 1.0, 1.0));
        assert!((ease.factor(10.5, 10.0, 11.0) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn composite_curve_is_found() {
        let v: PropertyValue<f32> = parse(
            "line-width",
            json!({"line-width": ["interpolate", ["exponential", 1.5], ["zoom"],
                5, ["get", "a"], 10, ["*", 2, ["get", "a"]]]}),
        );
        assert!(v.is_feature_dependent() && v.is_zoom_dependent());
        assert!(matches!(
            v.zoom_curve(),
            Some(ZoomCurve::Interpolate(InterpKind::Exponential(1.5)))
        ));
    }

    #[test]
    fn global_state_is_baked_in() {
        let mut v: PropertyValue<Color> = parse(
            "fill-color",
            json!({"fill-color": ["case", ["global-state", "dark"], "black", "white"]}),
        );
        assert!(v.dependencies().unwrap().global_state);
        let dark = BTreeMap::from([("dark".to_owned(), Value::Bool(true))]);
        let mut baked = v.clone();
        baked.bake_global_state(&dark);
        assert_eq!(baked.as_constant(), Some(&Color::BLACK));
        let light = BTreeMap::from([("dark".to_owned(), Value::Bool(false))]);
        v.bake_global_state(&light);
        assert_eq!(v.as_constant(), Some(&Color::WHITE));

        let mut v: PropertyValue<f32> = parse(
            "line-width",
            json!({"line-width": ["*", ["get", "w"], ["global-state", "scale"]]}),
        );
        v.bake_global_state(&BTreeMap::from([("scale".to_owned(), Value::Number(3.0))]));
        assert!(v.dependencies().unwrap().global_state, "still marked");
        let mut ctx = EvaluationContext::new();
        ctx.feature
            .properties
            .insert("w".into(), Value::Number(2.0));
        assert_eq!(v.evaluate(&ctx), 6.0);
    }

    #[test]
    fn global_state_in_filter() {
        let mut warnings = Vec::new();
        let mut filter = Filter::parse(
            Some(&json!(["==", ["get", "class"], ["global-state", "class"]])),
            "l",
            &mut warnings,
        );
        assert!(warnings.is_empty());
        filter.bake_global_state(&BTreeMap::from([(
            "class".to_owned(),
            Value::String("park".into()),
        )]));
        let mut ctx = EvaluationContext::new();
        ctx.feature
            .properties
            .insert("class".into(), Value::String("park".into()));
        assert!(filter.matches(&ctx));
        assert!(!filter.dependencies().unwrap().global_state);
    }

    #[test]
    fn interpolates_through_any_property() {
        let from: PropertyValue<f32> = parse("line-width", json!({"line-width": 2}));
        let to: PropertyValue<f32> = parse(
            "line-width",
            json!({"line-width": ["interpolate", ["linear"], ["zoom"], 0, 4, 10, 14]}),
        );
        let mut shown = to.clone();
        assert!(AnyProperty::set_interpolated(
            &mut shown, &from, &to, 0.5, 5.0
        ));
        // Halfway from 2 to the 9 the expression gives at zoom 5.
        assert_eq!(shown.as_constant(), Some(&5.5));
        let data: PropertyValue<f32> = parse("line-width", json!({"line-width": ["get", "w"]}));
        assert!(!AnyProperty::set_interpolated(
            &mut shown, &from, &data, 0.5, 5.0
        ));
    }

    #[test]
    fn legacy_filter() {
        let mut warnings = Vec::new();
        let filter = Filter::parse(Some(&json!(["==", "$type", "Polygon"])), "l", &mut warnings);
        assert!(warnings.is_empty());
        let mut ctx = EvaluationContext::new();
        ctx.feature.geometry_type = Some("Polygon".into());
        assert!(filter.matches(&ctx));
        ctx.feature.geometry_type = Some("LineString".into());
        assert!(!filter.matches(&ctx));
    }
}
