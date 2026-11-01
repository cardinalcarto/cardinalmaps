//! The Rust types style properties evaluate to, one [`PropertyType`] impl each.
//!
//! Spec `enum` properties become Rust enums through [`string_enum!`]; the other types
//! mirror the spec's value types (`padding`, `formatted`, `resolvedImage`, ...).

use maplibre_expr::{Type, Value};

use super::property::PropertyType;

/// An RGBA color with premultiplied alpha, channels in `0.0..=1.0` (sRGB, not linear).
///
/// This is the representation MapLibre uses internally and the one blending expects, so it
/// can be handed to a shader as is.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Color(pub [f32; 4]);

impl Color {
    pub const BLACK: Self = Self([0.0, 0.0, 0.0, 1.0]);
    pub const WHITE: Self = Self([1.0; 4]);
    pub const TRANSPARENT: Self = Self([0.0; 4]);

    /// Parse a CSS color string, as styles write colors outside of expressions.
    pub fn parse(css: &str) -> Option<Self> {
        maplibre_expr::Color::parse(css).map(Into::into)
    }

    /// As 8-bit RGBA (still premultiplied), for textures and vertex colors.
    pub fn to_rgba8(self) -> [u8; 4] {
        self.0.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    /// `n` colors from `color(input)` at inputs evenly spaced from `range[0]` to
    /// `range[1]`, as RGBA8: the ramp texture of a color expression over a special input
    /// such as `["heatmap-density"]`, `["line-progress"]` or `["elevation"]`.
    pub fn ramp(n: usize, range: [f64; 2], mut color: impl FnMut(f64) -> Color) -> Vec<[u8; 4]> {
        let [min, max] = range;
        let last = n.max(2) - 1;
        (0..n)
            .map(|i| color(min + (max - min) * i as f64 / last as f64).to_rgba8())
            .collect()
    }
}

impl From<maplibre_expr::Color> for Color {
    fn from(c: maplibre_expr::Color) -> Self {
        Self(c.to_rgba_unit().map(|v| v as f32))
    }
}

/// A type that can be an element of an array-typed property (`array<number>`,
/// `array<string>`, `array<enum>`).
pub trait ArrayItem: Sized {
    fn item_type() -> Type;
    fn from_item(value: &Value) -> Option<Self>;
}

impl ArrayItem for f32 {
    fn item_type() -> Type {
        Type::Number
    }

    fn from_item(value: &Value) -> Option<Self> {
        value.as_number().map(|n| n as f32)
    }
}

impl ArrayItem for String {
    fn item_type() -> Type {
        Type::String
    }

    fn from_item(value: &Value) -> Option<Self> {
        value.as_str().map(str::to_owned)
    }
}

/// The numbers of an array value, whichever representation evaluation produced.
fn numbers(value: &Value) -> Option<Vec<f32>> {
    match value {
        Value::NumberArray(v) => Some(v.iter().map(|&n| n as f32).collect()),
        Value::Array(items) => items.iter().map(f32::from_item).collect(),
        _ => None,
    }
}

/// Declares a Rust enum for a spec `enum` property, with its spec strings.
macro_rules! string_enum {
    ($(
        $(#[$meta:meta])*
        pub enum $name:ident { $($(#[$vmeta:meta])* $variant:ident = $s:literal),+ $(,)? }
    )*) => {$(
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The spelling the style spec uses.
            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $s),+
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = ();

            fn from_str(s: &str) -> Result<Self, ()> {
                match s {
                    $($s => Ok(Self::$variant),)+
                    _ => Err(()),
                }
            }
        }

        impl Default for $name {
            /// The first variant; properties fall back to the spec's default, not this.
            fn default() -> Self {
                Self::ALL[0]
            }
        }

        impl PropertyType for $name {
            fn expr_type() -> Type {
                Type::String
            }

            fn from_value(value: &Value) -> Option<Self> {
                value.as_str()?.parse().ok()
            }
        }

        impl ArrayItem for $name {
            fn item_type() -> Type {
                Type::String
            }

            fn from_item(value: &Value) -> Option<Self> {
                value.as_str()?.parse().ok()
            }
        }
    )*};
}

string_enum! {
    /// `layout.visibility`.
    pub enum Visibility {
        Visible = "visible",
        None = "none",
    }

    /// How the ends of a line are drawn.
    pub enum LineCap {
        Butt = "butt",
        Round = "round",
        Square = "square",
    }

    /// How the corners of a line are drawn.
    pub enum LineJoin {
        Bevel = "bevel",
        Round = "round",
        Miter = "miter",
    }

    /// What `*-translate` offsets are relative to: the map (rotating with it) or the
    /// viewport.
    pub enum TranslateAnchor {
        Map = "map",
        Viewport = "viewport",
    }

    /// `circle-pitch-scale`: whether circles shrink with distance in pitched views.
    pub enum CirclePitchScale {
        Map = "map",
        Viewport = "viewport",
    }

    /// `circle-pitch-alignment`: whether circles lie on the map plane or face the viewer.
    pub enum CirclePitchAlignment {
        Map = "map",
        Viewport = "viewport",
    }

    /// `hillshade-illumination-anchor` and the light's `anchor`.
    pub enum IlluminationAnchor {
        Map = "map",
        Viewport = "viewport",
    }

    pub enum SymbolPlacement {
        Point = "point",
        Line = "line",
        LineCenter = "line-center",
    }

    pub enum SymbolZOrder {
        Auto = "auto",
        ViewportY = "viewport-y",
        Source = "source",
    }

    /// `icon-overlap` and `text-overlap`.
    pub enum Overlap {
        Never = "never",
        Always = "always",
        Cooperative = "cooperative",
    }

    /// `icon-rotation-alignment`, `icon-pitch-alignment` and `text-pitch-alignment`.
    pub enum Alignment {
        Map = "map",
        Viewport = "viewport",
        Auto = "auto",
    }

    pub enum TextRotationAlignment {
        Map = "map",
        Viewport = "viewport",
        ViewportGlyph = "viewport-glyph",
        Auto = "auto",
    }

    pub enum IconTextFit {
        None = "none",
        Width = "width",
        Height = "height",
        Both = "both",
    }

    /// `icon-anchor`, `text-anchor` and the entries of `text-variable-anchor`.
    pub enum SymbolAnchor {
        Center = "center",
        Left = "left",
        Right = "right",
        Top = "top",
        Bottom = "bottom",
        TopLeft = "top-left",
        TopRight = "top-right",
        BottomLeft = "bottom-left",
        BottomRight = "bottom-right",
    }

    pub enum TextJustify {
        Auto = "auto",
        Left = "left",
        Center = "center",
        Right = "right",
    }

    pub enum TextTransform {
        None = "none",
        Uppercase = "uppercase",
        Lowercase = "lowercase",
    }

    pub enum WritingMode {
        Horizontal = "horizontal",
        Vertical = "vertical",
    }

    /// `raster-resampling` and `resampling`.
    pub enum Resampling {
        Linear = "linear",
        Nearest = "nearest",
    }

    pub enum HillshadeMethod {
        Standard = "standard",
        Basic = "basic",
        Combined = "combined",
        Igor = "igor",
        Multidirectional = "multidirectional",
    }
}

impl PropertyType for f32 {
    fn expr_type() -> Type {
        Type::Number
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_number().map(|n| n as f32)
    }

    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        lerp(*from, *to, t)
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Element-wise, or a switch to `to` when the lengths differ.
fn lerp_slices<T: Clone>(from: &[T], to: &[T], t: f32, lerp: impl Fn(&T, &T, f32) -> T) -> Vec<T> {
    if from.len() == to.len() {
        from.iter().zip(to).map(|(a, b)| lerp(a, b, t)).collect()
    } else {
        to.to_vec()
    }
}

impl PropertyType for bool {
    fn expr_type() -> Type {
        Type::Boolean
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_bool()
    }
}

impl PropertyType for String {
    const COERCE_TO_STRING: bool = true;

    fn expr_type() -> Type {
        Type::String
    }

    fn from_value(value: &Value) -> Option<Self> {
        value.as_str().map(str::to_owned)
    }
}

impl PropertyType for Color {
    fn expr_type() -> Type {
        Type::Color
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Color(c) => Some((*c).into()),
            Value::String(s) => Self::parse(s),
            _ => None,
        }
    }

    /// Channel by channel, premultiplied, as MapLibre transitions colors.
    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        Self(std::array::from_fn(|i| lerp(from.0[i], to.0[i], t)))
    }
}

/// Fixed-length number arrays: `*-translate`, `*-offset` (2), the light's `position` (3),
/// `icon-text-fit-padding` (4).
macro_rules! fixed_number_array {
    ($($n:literal),*) => {$(
        impl PropertyType for [f32; $n] {
            fn expr_type() -> Type {
                Type::array(Type::Number, Some($n))
            }

            fn from_value(value: &Value) -> Option<Self> {
                numbers(value)?.try_into().ok()
            }

            fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
                std::array::from_fn(|i| lerp(from[i], to[i], t))
            }
        }
    )*};
}

fixed_number_array!(2, 3, 4);

/// Variable-length arrays: `line-dasharray`, `text-font`, `text-variable-anchor`,
/// `text-writing-mode`.
impl<T: ArrayItem + Clone + std::fmt::Debug + Send + Sync + 'static> PropertyType for Vec<T> {
    fn expr_type() -> Type {
        Type::array(T::item_type(), None)
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Array(items) => items.iter().map(T::from_item).collect(),
            Value::NumberArray(items) => items
                .iter()
                .map(|&n| T::from_item(&Value::Number(n)))
                .collect(),
            _ => None,
        }
    }
}

/// A property without a spec default, such as `fill-pattern` or `line-dasharray`: `None`
/// when the style doesn't set it (the [`Default`]).
impl<T: PropertyType> PropertyType for Option<T> {
    const COERCE_TO_STRING: bool = T::COERCE_TO_STRING;

    fn expr_type() -> Type {
        T::expr_type()
    }

    fn from_value(value: &Value) -> Option<Self> {
        T::from_value(value).map(Some)
    }

    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        match (from, to) {
            (Some(from), Some(to)) => Some(T::interpolate(from, to, t)),
            _ => to.clone(),
        }
    }
}

/// `icon-padding`: `[top, right, bottom, left]` in pixels, expanded from the one to four
/// number shorthand like CSS padding.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Padding(pub [f32; 4]);

impl PropertyType for Padding {
    fn expr_type() -> Type {
        Type::Padding
    }

    fn from_value(value: &Value) -> Option<Self> {
        let n = match value {
            Value::Padding(p) => return Some(Self(p.map(|v| v as f32))),
            Value::Number(n) => vec![*n as f32],
            other => numbers(other)?,
        };
        let p = match *n.as_slice() {
            [a] => [a; 4],
            [a, b] => [a, b, a, b],
            [a, b, c] => [a, b, c, b],
            [a, b, c, d] => [a, b, c, d],
            _ => return None,
        };
        Some(Self(p))
    }

    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        Self(std::array::from_fn(|i| lerp(from.0[i], to.0[i], t)))
    }
}

/// `numberArray`: one number or several, as `hillshade-illumination-direction` takes one
/// per light.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NumberArray(pub Vec<f32>);

impl PropertyType for NumberArray {
    fn expr_type() -> Type {
        Type::NumberArray
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(n) => Some(Self(vec![*n as f32])),
            other => numbers(other).map(Self),
        }
    }

    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        Self(lerp_slices(&from.0, &to.0, t, |a, b, t| lerp(*a, *b, t)))
    }
}

/// `colorArray`: one color or several, as `hillshade-shadow-color` takes one per light.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColorArray(pub Vec<Color>);

impl PropertyType for ColorArray {
    fn expr_type() -> Type {
        Type::ColorArray
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::ColorArray(colors) => Some(Self(colors.iter().map(|&c| c.into()).collect())),
            Value::Array(items) => items
                .iter()
                .map(Color::from_value)
                .collect::<Option<_>>()
                .map(Self),
            other => Color::from_value(other).map(|c| Self(vec![c])),
        }
    }

    fn interpolate(from: &Self, to: &Self, t: f32) -> Self {
        Self(lerp_slices(&from.0, &to.0, t, Color::interpolate))
    }
}

/// A `resolvedImage`: the name of an image in the style's sprites (or added at runtime).
/// Look it up with [`ImageAtlas::resolve`](crate::sprite::ImageAtlas::resolve).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ResolvedImage(pub String);

impl PropertyType for ResolvedImage {
    fn expr_type() -> Type {
        Type::ResolvedImage
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Image { name, .. } => Some(Self(name.clone())),
            Value::String(s) => Some(Self(s.clone())),
            _ => None,
        }
    }
}

/// `text-field`: text in sections that can each have their own font, size and color.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Formatted(pub Vec<FormattedSection>);

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FormattedSection {
    pub text: String,
    /// Set for an image section, whose `text` is empty.
    pub image: Option<ResolvedImage>,
    /// Multiplies `text-size`.
    pub scale: Option<f32>,
    /// Overrides `text-font`: the font names, comma separated.
    pub font_stack: Option<String>,
    pub text_color: Option<Color>,
    pub vertical_align: Option<String>,
}

impl Formatted {
    /// All sections' text run together.
    pub fn text(&self) -> String {
        self.0.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.0
            .iter()
            .all(|s| s.text.is_empty() && s.image.is_none())
    }
}

impl PropertyType for Formatted {
    fn expr_type() -> Type {
        Type::Formatted
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Formatted(sections) => Some(Self(
                sections
                    .iter()
                    .map(|s| FormattedSection {
                        text: s.text.clone(),
                        image: s
                            .image
                            .as_ref()
                            .map(|(name, _)| ResolvedImage(name.clone())),
                        scale: s.scale.map(|v| v as f32),
                        font_stack: s.font_stack.clone(),
                        text_color: s.text_color.map(Into::into),
                        vertical_align: s.vertical_align.clone(),
                    })
                    .collect(),
            )),
            Value::Null => Some(Self::default()),
            other => Some(Self(vec![FormattedSection {
                text: other.to_string(),
                ..Default::default()
            }])),
        }
    }
}

/// `text-variable-anchor-offset`: anchors to try in order, each with its text offset in ems.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VariableAnchorOffsetCollection(pub Vec<(SymbolAnchor, [f32; 2])>);

impl PropertyType for VariableAnchorOffsetCollection {
    fn expr_type() -> Type {
        Type::VariableAnchorOffsetCollection
    }

    fn from_value(value: &Value) -> Option<Self> {
        let Value::Array(items) = value else {
            return None;
        };
        if items.len() % 2 != 0 {
            return None;
        }
        items
            .chunks(2)
            .map(|pair| {
                Some((
                    SymbolAnchor::from_item(&pair[0])?,
                    <[f32; 2]>::from_value(&pair[1])?,
                ))
            })
            .collect::<Option<_>>()
            .map(Self)
    }
}

/// The root `projection.type`: a named projection, or a transition between two.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionDefinition {
    Named(String),
    Transition { from: String, to: String, t: f32 },
}

impl Default for ProjectionDefinition {
    fn default() -> Self {
        Self::Named("mercator".into())
    }
}

impl PropertyType for ProjectionDefinition {
    fn expr_type() -> Type {
        Type::ProjectionDefinition
    }

    fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Projection(maplibre_expr::Projection::Named(name)) | Value::String(name) => {
                Some(Self::Named(name.clone()))
            }
            Value::Projection(maplibre_expr::Projection::Transition {
                from,
                to,
                transition,
            }) => Some(Self::Transition {
                from: from.clone(),
                to: to.clone(),
                t: *transition as f32,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_shorthand() {
        let p = |v: Value| Padding::from_value(&v).unwrap().0;
        assert_eq!(p(Value::Number(2.0)), [2.0; 4]);
        let arr = |v: &[f64]| Value::Array(v.iter().map(|&n| Value::Number(n)).collect());
        assert_eq!(p(arr(&[1.0, 2.0])), [1.0, 2.0, 1.0, 2.0]);
        assert_eq!(p(arr(&[1.0, 2.0, 3.0])), [1.0, 2.0, 3.0, 2.0]);
        assert!(Padding::from_value(&arr(&[1.0; 5])).is_none());
    }

    #[test]
    fn enum_round_trip() {
        for &anchor in SymbolAnchor::ALL {
            assert_eq!(anchor.as_str().parse(), Ok(anchor));
        }
        assert_eq!(
            LineCap::from_value(&Value::String("round".into())),
            Some(LineCap::Round)
        );
        assert_eq!(LineCap::from_value(&Value::String("nope".into())), None);
    }

    #[test]
    fn variable_anchor_offsets() {
        let v = Value::Array(vec![
            Value::String("top".into()),
            Value::Array(vec![Value::Number(0.0), Value::Number(1.0)]),
        ]);
        let c = VariableAnchorOffsetCollection::from_value(&v).unwrap();
        assert_eq!(c.0, [(SymbolAnchor::Top, [0.0, 1.0])]);
    }
}
