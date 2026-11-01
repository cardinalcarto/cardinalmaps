//! Paint properties on the GPU: which values are uniforms, which are per-vertex attributes,
//! and how zoom-and-feature ("composite") expressions are interpolated.
//!
//! Every layer type lists the paint properties its shaders read, by the name the shader
//! knows them under (see [`FillPaint::attributes`](crate::style::FillPaint::attributes)).
//! Each of them gets one of three [`Binding`]s, decided by what its expression reads:
//!
//! - [`Binding::Uniform`]: the same for every feature (constant, or depending on the zoom
//!   only). The renderer evaluates it at the camera zoom every frame and passes it in
//!   [`PaintUniforms::values`].
//! - [`Binding::Source`]: differs per feature but not with zoom. Tile processing evaluates
//!   it per feature and repeats the value for each of the feature's vertices in
//!   [`PaintArrays::data`].
//! - [`Binding::Composite`]: differs per feature *and* with zoom. As in MapLibre, tile
//!   processing evaluates it per feature at the two zoom levels of
//!   [`PaintArrays::zoom_range`] (the tile's zoom and the next), and stores both values per
//!   vertex. The renderer computes one interpolation factor per property and frame from the
//!   expression's zoom curve ([`PaintArrays::uniforms`] puts it in
//!   [`PaintUniforms::factors`]), and the vertex shader mixes the two values with it.
//!
//! So a bucket of any layer type has its geometry vertices plus one [`PaintArrays`] whose
//! layout ([`PaintArrays::attributes`]) depends on the bindings. A renderer builds its vertex
//! buffer layout and shader from that layout; the harness generates the WGSL accessors
//! (`paint_color(p)`, ...) from it, so layer shaders never see the difference between the
//! three cases.
//!
//! To use it from a new layer type:
//!
//! 1. Add `attributes()` to the layer type's paint struct, listing `(wgsl_name, &property)`.
//! 2. In its bucket builder, create `PaintArrays::new(&paint.attributes(), zoom)` and call
//!    [`PaintArrays::populate`] after adding each feature's vertices.
//! 3. In the renderer, get the per-frame uniforms with [`PaintArrays::uniforms`].

use std::collections::BTreeMap;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use maplibre_expr::{EvaluationContext, Feature, Value};

use crate::style::{Color, Padding, PropertyType, PropertyValue, ZoomCurve};

/// How many paint properties one layer type may expose to its shaders.
pub const MAX_PAINT_ATTRIBUTES: usize = 12;

/// How a paint property reaches the shader. See the [module docs](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    Uniform,
    Source,
    Composite,
}

/// A property type that can be passed to a shader as up to four floats.
pub trait AttributeValue: PropertyType {
    /// 1, 2 or 4.
    const COMPONENTS: usize;

    fn write(&self, out: &mut [f32]);
}

impl AttributeValue for f32 {
    const COMPONENTS: usize = 1;

    fn write(&self, out: &mut [f32]) {
        out[0] = *self;
    }
}

impl AttributeValue for Color {
    const COMPONENTS: usize = 4;

    fn write(&self, out: &mut [f32]) {
        out.copy_from_slice(&self.0);
    }
}

impl AttributeValue for [f32; 2] {
    const COMPONENTS: usize = 2;

    fn write(&self, out: &mut [f32]) {
        out.copy_from_slice(self);
    }
}

impl AttributeValue for [f32; 4] {
    const COMPONENTS: usize = 4;

    fn write(&self, out: &mut [f32]) {
        out.copy_from_slice(self);
    }
}

impl AttributeValue for Padding {
    const COMPONENTS: usize = 4;

    fn write(&self, out: &mut [f32]) {
        out.copy_from_slice(&self.0);
    }
}

/// Unset is all zeros; shaders that care (`fill-outline-color`) check for that themselves
/// or use another property.
impl<T: AttributeValue> AttributeValue for Option<T> {
    const COMPONENTS: usize = T::COMPONENTS;

    fn write(&self, out: &mut [f32]) {
        match self {
            Some(v) => v.write(out),
            None => out.fill(0.0),
        }
    }
}

/// A paint property seen as a GPU attribute, whatever its Rust type. Implemented by
/// [`PropertyValue`] of every [`AttributeValue`] type.
pub trait PaintProperty {
    /// Floats per value: 1, 2 or 4.
    fn components(&self) -> usize;

    fn binding(&self) -> Binding;

    /// Evaluate for the feature in `ctx` (at `ctx.zoom`) into `out`, which has
    /// [`Self::components`] floats.
    fn write(&self, ctx: &EvaluationContext, out: &mut [f32]);

    /// The value at `zoom` when the binding is [`Binding::Uniform`], padded with zeros.
    fn uniform_value(&self, zoom: f64) -> [f32; 4];

    fn zoom_curve(&self) -> Option<ZoomCurve>;

    /// Whether the value depends on feature state or global state, which can change after
    /// the tile was processed.
    fn reads_state(&self) -> bool {
        false
    }
}

impl<T: AttributeValue> PaintProperty for PropertyValue<T> {
    fn components(&self) -> usize {
        T::COMPONENTS
    }

    fn binding(&self) -> Binding {
        if !self.is_feature_dependent() {
            Binding::Uniform
        } else if self.zoom_curve().is_some() {
            Binding::Composite
        } else {
            // Including a zoom-dependent expression without a recognizable curve, which
            // type checking should have rejected; it's evaluated at the tile's zoom.
            Binding::Source
        }
    }

    fn write(&self, ctx: &EvaluationContext, out: &mut [f32]) {
        self.evaluate(ctx).write(out);
    }

    fn uniform_value(&self, zoom: f64) -> [f32; 4] {
        let mut out = [0.0; 4];
        self.evaluate_at_zoom(zoom).write(&mut out[..T::COMPONENTS]);
        out
    }

    fn zoom_curve(&self) -> Option<ZoomCurve> {
        PropertyValue::zoom_curve(self)
    }

    fn reads_state(&self) -> bool {
        self.dependencies()
            .is_some_and(|d| d.feature_state || d.global_state)
    }
}

/// The properties a layer type exposes to its shaders, by their WGSL names.
pub type PaintProperties<'a> = [(&'static str, &'a dyn PaintProperty)];

/// Where one paint property is in a bucket's [`PaintArrays`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaintAttribute {
    /// The name shaders know the property by.
    pub name: &'static str,
    /// Floats per value: 1, 2 or 4.
    pub components: u32,
    pub binding: Binding,
    /// In floats from the start of a vertex's values; unused for [`Binding::Uniform`].
    /// A composite property's two values follow each other, `components` floats apart.
    pub offset: u32,
}

/// Which vertices one feature contributed to a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureVertices {
    pub id: Option<u64>,
    pub vertices: Range<u32>,
}

/// The per-vertex paint values of one bucket, interleaved. See the [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct PaintArrays {
    /// One per property the layer type lists, in that order, whatever the binding.
    pub attributes: Vec<PaintAttribute>,
    /// Floats per vertex in [`Self::data`]; zero when every property is a uniform.
    pub stride: u32,
    /// `stride` floats per vertex.
    pub data: Vec<f32>,
    /// The zoom levels composite properties were evaluated at.
    pub zoom_range: [f32; 2],
    /// Each feature's vertices, in order.
    pub features: Vec<FeatureVertices>,
    vertex_count: u32,
    /// What expressions read of each feature in [`Self::features`], kept when a
    /// per-vertex property depends on feature or global state, to evaluate it again when
    /// the state changes (see [`Self::update_state`]).
    retained: Vec<Feature>,
    retain: bool,
}

/// The feature state of a feature, by its id, for [`PaintArrays::update_state`].
pub type StateLookup<'a> = dyn Fn(u64) -> Option<&'a BTreeMap<String, Value>> + 'a;

impl PaintArrays {
    /// Lay out `properties` for a tile processed at `zoom`.
    pub fn new(properties: &PaintProperties<'_>, zoom: f32) -> Self {
        assert!(
            properties.len() <= MAX_PAINT_ATTRIBUTES,
            "a layer type may expose at most {MAX_PAINT_ATTRIBUTES} paint attributes"
        );
        let mut stride = 0;
        let mut retain = false;
        let attributes = properties
            .iter()
            .map(|&(name, property)| {
                let components = property.components() as u32;
                let binding = property.binding();
                retain |= binding != Binding::Uniform && property.reads_state();
                let offset = stride;
                stride += match binding {
                    Binding::Uniform => 0,
                    Binding::Source => components,
                    Binding::Composite => 2 * components,
                };
                PaintAttribute {
                    name,
                    components,
                    binding,
                    offset,
                }
            })
            .collect();
        Self {
            attributes,
            stride,
            data: Vec::new(),
            zoom_range: [zoom, zoom + 1.0],
            features: Vec::new(),
            vertex_count: 0,
            retained: Vec::new(),
            retain,
        }
    }

    /// A copy without the per-vertex data, for a renderer to keep the layout of what it
    /// uploaded.
    pub fn layout(&self) -> Self {
        Self {
            attributes: self.attributes.clone(),
            stride: self.stride,
            zoom_range: self.zoom_range,
            vertex_count: self.vertex_count,
            retain: self.retain,
            ..Self::default()
        }
    }

    /// Whether the features' values can be evaluated again with [`Self::update_state`]
    /// and [`Self::rebuild`], which is the case when some per-vertex property reads
    /// feature or global state.
    pub fn retains_features(&self) -> bool {
        self.retain
    }

    /// Evaluate the feature in `ctx` and give its values to every vertex added since the
    /// last call, up to `vertex_count` in total. `ctx.zoom` is changed while evaluating
    /// composite properties and restored afterwards.
    ///
    /// `properties` must be the list this was created with.
    pub fn populate(
        &mut self,
        properties: &PaintProperties<'_>,
        ctx: &mut EvaluationContext,
        id: Option<u64>,
        vertex_count: usize,
    ) {
        let vertex_count = vertex_count as u32;
        let added = vertex_count.saturating_sub(self.vertex_count);
        if added == 0 {
            return;
        }
        self.features.push(FeatureVertices {
            id,
            vertices: self.vertex_count..vertex_count,
        });
        self.vertex_count = vertex_count;
        if self.stride == 0 {
            return;
        }
        if self.retain {
            let mut feature = ctx.feature.clone();
            feature.state.clear();
            self.retained.push(feature);
        }

        let start = self.data.len();
        let stride = self.stride as usize;
        let mut values = std::mem::take(&mut self.data);
        values.resize(start + stride, 0.0);
        self.evaluate(properties, ctx, &mut values[start..]);
        // Every vertex of the feature gets the same values.
        for _ in 1..added {
            values.extend_from_within(start..start + stride);
        }
        self.data = values;
    }

    /// One vertex's values for the feature in `ctx`.
    fn evaluate(
        &self,
        properties: &PaintProperties<'_>,
        ctx: &mut EvaluationContext,
        out: &mut [f32],
    ) {
        let zoom = ctx.zoom;
        for (attribute, &(_, property)) in self.attributes.iter().zip(properties) {
            let at = attribute.offset as usize;
            let n = attribute.components as usize;
            match attribute.binding {
                Binding::Uniform => {}
                Binding::Source => property.write(ctx, &mut out[at..at + n]),
                Binding::Composite => {
                    for (i, z) in self.zoom_range.into_iter().enumerate() {
                        ctx.zoom = Some(f64::from(z));
                        let at = at + i * n;
                        property.write(ctx, &mut out[at..at + n]);
                    }
                    ctx.zoom = zoom;
                }
            }
        }
    }

    /// Evaluate the features with an id in `ids` (or every feature, for `None`) again,
    /// with their current feature state from `state`, and overwrite their values.
    /// Returns whether any values changed. Does nothing unless the features are
    /// [retained](Self::retains_features).
    ///
    /// `properties` must bind as the ones this was created with, as they do when only the
    /// state changed; after a style change, [`Self::rebuild`] instead.
    pub fn update_state(
        &mut self,
        properties: &PaintProperties<'_>,
        ids: Option<&[u64]>,
        state: &StateLookup<'_>,
    ) -> bool {
        if !self.retain || self.stride == 0 {
            return false;
        }
        let stride = self.stride as usize;
        let mut ctx = EvaluationContext::new().with_zoom(f64::from(self.zoom_range[0]));
        let mut values = vec![0.0; stride];
        let mut changed = false;
        let mut retained = std::mem::take(&mut self.retained);
        for (f, feature) in self.features.iter().zip(&mut retained) {
            if ids.is_some_and(|ids| f.id.is_none_or(|id| !ids.contains(&id))) {
                continue;
            }
            std::mem::swap(&mut ctx.feature, feature);
            if let Some(s) = f.id.and_then(state) {
                ctx.feature.state.clone_from(s);
            }
            self.evaluate(properties, &mut ctx, &mut values);
            ctx.feature.state.clear();
            std::mem::swap(&mut ctx.feature, feature);

            let range = f.vertices.start as usize * stride..f.vertices.end as usize * stride;
            let slots = &mut self.data[range];
            if slots[..stride] != values[..] {
                changed = true;
                for vertex in slots.chunks_exact_mut(stride) {
                    vertex.copy_from_slice(&values);
                }
            }
        }
        self.retained = retained;
        changed
    }

    /// The same features evaluated again for `properties`, which may bind differently
    /// than the ones this was made with (after a style change), with feature state from
    /// `state`. `None` if the features weren't [retained](Self::retains_features).
    pub fn rebuild(
        &self,
        properties: &PaintProperties<'_>,
        state: &StateLookup<'_>,
    ) -> Option<Self> {
        if !self.retain {
            return None;
        }
        let mut out = Self::new(properties, self.zoom_range[0]);
        let mut ctx = EvaluationContext::new().with_zoom(f64::from(self.zoom_range[0]));
        for (f, feature) in self.features.iter().zip(&self.retained) {
            ctx.feature.clone_from(feature);
            if let Some(s) = f.id.and_then(state) {
                ctx.feature.state.clone_from(s);
            }
            out.populate(properties, &mut ctx, f.id, f.vertices.end as usize);
        }
        Some(out)
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count as usize
    }

    /// Identifies the bindings, for caching shaders and pipelines per combination.
    pub fn signature(&self) -> u32 {
        self.attributes
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let bits = match a.binding {
                    Binding::Uniform => 0,
                    Binding::Source => 1,
                    Binding::Composite => 2,
                };
                bits << (2 * i)
            })
            .sum()
    }

    /// The uniforms for drawing this bucket with the camera at `zoom`: uniform values
    /// and composite interpolation factors (clamped to `0..=1`).
    pub fn uniforms(&self, properties: &PaintProperties<'_>, zoom: f64) -> PaintUniforms {
        let mut u = PaintUniforms::zeroed();
        let [lower, upper] = self.zoom_range.map(f64::from);
        for (i, (attribute, &(_, property))) in self.attributes.iter().zip(properties).enumerate() {
            match attribute.binding {
                Binding::Uniform => u.values[i] = property.uniform_value(zoom),
                Binding::Source => {}
                Binding::Composite => {
                    let factor = property
                        .zoom_curve()
                        .map_or(0.0, |curve| curve.factor(zoom, lower, upper));
                    u.factors[i / 4][i % 4] = factor.clamp(0.0, 1.0) as f32;
                }
            }
        }
        u
    }

    /// The value of the attribute `name` at `vertex`: what a shader would compute for the
    /// camera at the tile's zoom (the first composite value). For tests and debugging.
    pub fn value(&self, properties: &PaintProperties<'_>, name: &str, vertex: usize) -> Vec<f32> {
        let (i, attribute) = self
            .attributes
            .iter()
            .enumerate()
            .find(|(_, a)| a.name == name)
            .unwrap_or_else(|| panic!("no paint attribute {name}"));
        let n = attribute.components as usize;
        match attribute.binding {
            Binding::Uniform => {
                properties[i].1.uniform_value(f64::from(self.zoom_range[0]))[..n].to_vec()
            }
            Binding::Source | Binding::Composite => {
                let at = vertex * self.stride as usize + attribute.offset as usize;
                self.data[at..at + n].to_vec()
            }
        }
    }
}

/// The per-draw paint uniforms, laid out to match `PaintUniforms` in the harness's
/// `common.wgsl`: one `vec4` per attribute (scalars in `.x`, pairs in `.xy`), then the
/// composite factors packed four to a `vec4`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct PaintUniforms {
    pub values: [[f32; 4]; MAX_PAINT_ATTRIBUTES],
    pub factors: [[f32; 4]; MAX_PAINT_ATTRIBUTES / 4],
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn width(json: serde_json::Value) -> PropertyValue<f32> {
        PropertyValue::from_json("line-width", &json, 1.0).unwrap()
    }

    #[test]
    fn bindings_follow_dependencies() {
        let constant = width(json!(2));
        let zoom = width(json!(["interpolate", ["linear"], ["zoom"], 0, 1, 10, 5]));
        let source = width(json!(["get", "w"]));
        let composite = width(json!([
            "interpolate",
            ["linear"],
            ["zoom"],
            10,
            ["get", "w"],
            12,
            9
        ]));
        let props: [(&str, &dyn PaintProperty); 4] = [
            ("a", &constant),
            ("b", &zoom),
            ("c", &source),
            ("d", &composite),
        ];
        let arrays = PaintArrays::new(&props, 10.0);
        let bindings: Vec<_> = arrays.attributes.iter().map(|a| a.binding).collect();
        assert_eq!(
            bindings,
            [
                Binding::Uniform,
                Binding::Uniform,
                Binding::Source,
                Binding::Composite
            ]
        );
        assert_eq!(arrays.stride, 3);
        assert_eq!(arrays.attributes[3].offset, 1);
        assert_eq!(arrays.signature(), (1 << 4) | (2 << 6));
    }

    #[test]
    fn composite_values_and_factor() {
        let composite = width(json!([
            "interpolate",
            ["linear"],
            ["zoom"],
            10,
            ["get", "w"],
            12,
            9
        ]));
        let color =
            PropertyValue::<Color>::from_json("line-color", &json!("red"), Color::BLACK).unwrap();
        let props: [(&str, &dyn PaintProperty); 2] = [("width", &composite), ("color", &color)];
        let mut arrays = PaintArrays::new(&props, 10.0);
        let mut ctx = EvaluationContext::new().with_zoom(10.0);
        ctx.feature
            .properties
            .insert("w".into(), maplibre_expr::Value::Number(1.0));
        arrays.populate(&props, &mut ctx, Some(3), 2);
        assert_eq!(ctx.zoom, Some(10.0), "zoom is restored");
        // At zoom 10 the width is the feature's 1, at 11 halfway to 9.
        assert_eq!(arrays.data, [1.0, 5.0, 1.0, 5.0]);
        assert_eq!(arrays.features[0].vertices, 0..2);

        ctx.feature
            .properties
            .insert("w".into(), maplibre_expr::Value::Number(3.0));
        arrays.populate(&props, &mut ctx, None, 3);
        assert_eq!(&arrays.data[4..], [3.0, 6.0]);
        assert_eq!(arrays.value(&props, "width", 2), [3.0]);
        assert_eq!(arrays.value(&props, "color", 2), [1.0, 0.0, 0.0, 1.0]);

        let u = arrays.uniforms(&props, 10.25);
        assert_eq!(u.factors[0][0], 0.25);
        assert_eq!(u.values[1], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(arrays.uniforms(&props, 14.0).factors[0][0], 1.0);
        assert_eq!(size_of::<PaintUniforms>() % 16, 0);
    }
}
