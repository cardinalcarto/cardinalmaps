//! Changing a style at runtime, as MapLibre's `setPaintProperty`, `setLayoutProperty`,
//! `setFilter`, `setGlobalStateProperty` and `setStyle` do, and telling a renderer what
//! each change invalidates.
//!
//! Edits are applied to the style's JSON and the edited layer is parsed again, so values
//! are checked and defaulted exactly as when loading. Each edit returns a [`StyleDiff`];
//! [`Style::diff`] compares two whole styles the same way, for reloading a style file.
//! Paint transitions are left to [`crate::transition`].

use std::collections::BTreeSet;

use serde_json::Value as Json;

use super::{AnyProperty, LayerKind, Style, StyleLayer};
use crate::Error;

/// What a style change means for tiles that were processed and uploaded with the old
/// style. Layer indices are into the new style.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleDiff {
    /// Sources, sprites or the list of layers changed, so buckets of the old style (which
    /// refer to layers by index) can't be used with the new one: process every tile again.
    pub reload_all: bool,
    /// Layers whose filter, layout, zoom range, visibility or source changed: the tiles
    /// they draw from must be processed again.
    pub reprocess: BTreeSet<usize>,
    /// Layers where a paint property that differs per feature changed. Buckets that
    /// [retain their features](crate::Bucket::retains_features) can
    /// [rebuild their paint attributes](crate::Bucket::rebuild_paint); others must be
    /// processed again.
    pub repaint: BTreeSet<usize>,
    /// Layers where only paint properties that are the same for every feature changed:
    /// drawing with the new style is enough.
    pub restyle: BTreeSet<usize>,
}

impl StyleDiff {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn merge(&mut self, other: Self) {
        self.reload_all |= other.reload_all;
        self.reprocess.extend(other.reprocess);
        self.repaint.extend(other.repaint);
        self.restyle.extend(other.restyle);
    }

    /// The sources of the layers in [`Self::reprocess`], in `style` (the new style).
    pub fn sources_to_reprocess<'a>(&self, style: &'a Style) -> BTreeSet<&'a str> {
        self.reprocess
            .iter()
            .filter_map(|&i| style.layers.get(i)?.source.as_deref())
            .collect()
    }

    fn all() -> Self {
        Self {
            reload_all: true,
            ..Self::default()
        }
    }
}

impl Style {
    /// The style document, with the edits made so far.
    pub fn json(&self) -> &Json {
        &self.json
    }

    /// The current global state as expression values.
    pub fn global_state_values(&self) -> std::collections::BTreeMap<String, maplibre_expr::Value> {
        self.state
            .iter()
            .map(|(k, v)| (k.clone(), maplibre_expr::Value::from_json(v)))
            .collect()
    }

    /// The index of the layer with id `id`.
    pub fn layer_index(&self, id: &str) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Set (or with `None`, unset) a paint property of a layer, like MapLibre's
    /// `setPaintProperty`. `name` may also be a `*-transition`.
    pub fn set_paint_property(
        &mut self,
        layer: &str,
        name: &str,
        value: Option<Json>,
    ) -> Result<StyleDiff, Error> {
        self.edit_section(layer, "paint", name, value)
    }

    /// Set (or with `None`, unset) a layout property of a layer, like MapLibre's
    /// `setLayoutProperty`. This includes `visibility`.
    pub fn set_layout_property(
        &mut self,
        layer: &str,
        name: &str,
        value: Option<Json>,
    ) -> Result<StyleDiff, Error> {
        self.edit_section(layer, "layout", name, value)
    }

    /// Set (or with `None`, remove) a layer's filter.
    pub fn set_filter(&mut self, layer: &str, filter: Option<Json>) -> Result<StyleDiff, Error> {
        let index = self.find_layer(layer)?;
        let mut json = self.json["layers"][index].clone();
        let object = json.as_object_mut().expect("parsed layers are objects");
        match filter {
            Some(filter) => object.insert("filter".into(), filter),
            None => object.remove("filter"),
        };
        self.replace_layer(index, json)
    }

    /// Set a layer's zoom range, like MapLibre's `setLayerZoomRange`.
    pub fn set_layer_zoom_range(
        &mut self,
        layer: &str,
        min_zoom: f32,
        max_zoom: f32,
    ) -> Result<StyleDiff, Error> {
        let index = self.find_layer(layer)?;
        let mut json = self.json["layers"][index].clone();
        json["minzoom"] = min_zoom.into();
        json["maxzoom"] = max_zoom.into();
        self.replace_layer(index, json)
    }

    /// Set a `["global-state", name]` value, like MapLibre's `setGlobalStateProperty`.
    /// `None` goes back to the style's default.
    pub fn set_global_state(&mut self, name: &str, value: Option<Json>) -> StyleDiff {
        let value = value.unwrap_or_else(|| {
            self.json
                .pointer(&format!("/state/{}/default", escape_pointer(name)))
                .cloned()
                .unwrap_or(Json::Null)
        });
        if self.state.get(name) == Some(&value) {
            return StyleDiff::default();
        }
        self.state.insert(name.to_owned(), value);
        let state = self.global_state_values();
        let mut diff = StyleDiff::default();
        for index in 0..self.layers.len() {
            let json = &self.json["layers"][index];
            if !mentions_global_state(json) {
                continue;
            }
            // Parsing again gives the expressions as written, to bake the new state into.
            let Ok(mut layer) = StyleLayer::parse(json, &self.transition, &mut Vec::new()) else {
                continue;
            };
            layer.bake_global_state(&state);
            diff.merge(diff_layer(index, &self.layers[index], &layer));
            self.layers[index] = layer;
        }
        diff
    }

    /// What changes from this style to `new`, as when a style is reloaded.
    pub fn diff(&self, new: &Style) -> StyleDiff {
        let same_layers = self.layers.len() == new.layers.len()
            && self
                .layers
                .iter()
                .zip(&new.layers)
                .all(|(a, b)| a.id == b.id && a.kind.type_name() == b.kind.type_name());
        if !same_layers || self.sources != new.sources || self.sprite != new.sprite {
            return StyleDiff::all();
        }
        let mut diff = StyleDiff::default();
        for (index, (old, layer)) in self.layers.iter().zip(&new.layers).enumerate() {
            diff.merge(diff_layer(index, old, layer));
        }
        diff
    }

    fn find_layer(&self, id: &str) -> Result<usize, Error> {
        self.layer_index(id)
            .ok_or_else(|| Error::InvalidStyle(format!("no layer {id:?}")))
    }

    fn edit_section(
        &mut self,
        layer: &str,
        section: &str,
        name: &str,
        value: Option<Json>,
    ) -> Result<StyleDiff, Error> {
        let index = self.find_layer(layer)?;
        let ty = self.layers[index].kind.type_name();
        let [layout, paint] = LayerKind::properties_of(ty)
            .ok_or_else(|| Error::InvalidStyle(format!("{ty} layers have no properties")))?;
        let known = match section {
            "paint" => paint.contains(&name.strip_suffix("-transition").unwrap_or(name)),
            _ => name == "visibility" || layout.contains(&name),
        };
        if !known {
            return Err(Error::InvalidStyle(format!(
                "{name} is not a {section} property of {ty} layers"
            )));
        }
        let mut json = self.json["layers"][index].clone();
        let object = json.as_object_mut().expect("parsed layers are objects");
        let entries = object
            .entry(section)
            .or_insert_with(|| Json::Object(Default::default()));
        let Some(entries) = entries.as_object_mut() else {
            return Err(Error::InvalidStyle(format!(
                "{section} of {layer:?} is not an object"
            )));
        };
        match value {
            Some(value) => entries.insert(name.to_owned(), value),
            None => entries.remove(name),
        };
        self.replace_layer(index, json)
    }

    /// Parse `json` as the new layer at `index`, replacing its warnings.
    fn replace_layer(&mut self, index: usize, json: Json) -> Result<StyleDiff, Error> {
        let mut warnings = Vec::new();
        let mut layer = StyleLayer::parse(&json, &self.transition, &mut warnings)?;
        layer.bake_global_state(&self.global_state_values());
        let diff = diff_layer(index, &self.layers[index], &layer);
        let old = std::mem::replace(&mut self.layers[index], layer);
        self.json["layers"][index] = json;
        self.warnings.retain(|w| w.layer != old.id);
        self.warnings.extend(warnings);
        Ok(diff)
    }
}

/// `/` and `~` escaped for a JSON pointer.
fn escape_pointer(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn mentions_global_state(json: &Json) -> bool {
    match json {
        Json::Array(items) => {
            items.first().is_some_and(|op| op == "global-state")
                || items.iter().any(mentions_global_state)
        }
        Json::Object(entries) => entries.values().any(mentions_global_state),
        _ => false,
    }
}

/// Properties are compared by their parsed form, so a change of global state that alters
/// a baked-in value counts as a change too.
fn fingerprint(p: &dyn AnyProperty) -> String {
    format!("{p:?}")
}

fn diff_layer(index: usize, old: &StyleLayer, new: &StyleLayer) -> StyleDiff {
    let mut diff = StyleDiff::default();
    if old.kind.type_name() != new.kind.type_name() {
        diff.reload_all = true;
        return diff;
    }
    let layout = |l: &StyleLayer| {
        let mut out = Vec::new();
        l.kind
            .for_each_layout_property(&mut |_, p| out.push(fingerprint(p)));
        out
    };
    let structural = old.source != new.source
        || old.source_layer != new.source_layer
        || old.min_zoom != new.min_zoom
        || old.max_zoom != new.max_zoom
        || old.visible != new.visible
        || format!("{:?}", old.filter) != format!("{:?}", new.filter)
        || layout(old) != layout(new);
    if structural {
        diff.reprocess.insert(index);
    }
    let mut old_paint = Vec::new();
    old.kind.for_each_paint_property(&mut |_, p| {
        old_paint.push((fingerprint(p), p.is_feature_dependent()))
    });
    let mut i = 0;
    new.kind.for_each_paint_property(&mut |_, p| {
        let (before, was_per_feature) = &old_paint[i];
        i += 1;
        if *before != fingerprint(p) {
            if *was_per_feature || p.is_feature_dependent() {
                diff.repaint.insert(index);
            } else {
                diff.restyle.insert(index);
            }
        }
    });
    if old.transitions != new.transitions {
        diff.restyle.insert(index);
    }
    diff
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::Color;

    fn style() -> Style {
        Style::from_json(&json!({
            "version": 8,
            "state": {"highlight": {"default": "none"}},
            "sources": {"s": {"type": "vector"}},
            "layers": [
                {"id": "water", "type": "fill", "source": "s", "source-layer": "water",
                 "paint": {"fill-color": "blue"}},
                {"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                 "paint": {"line-color": ["case",
                     ["==", ["get", "class"], ["global-state", "highlight"]], "red", "gray"]}},
                {"id": "parks", "type": "fill", "source": "s", "source-layer": "landuse",
                 "filter": ["==", ["get", "class"], ["global-state", "highlight"]]},
            ]
        }))
        .unwrap()
    }

    #[test]
    fn paint_edits() {
        let mut style = style();
        let diff = style
            .set_paint_property("water", "fill-color", Some(json!("green")))
            .unwrap();
        assert_eq!(diff.restyle, [0].into());
        assert!(diff.reprocess.is_empty() && diff.repaint.is_empty());
        let LayerKind::Fill(fill) = &style.layers[0].kind else {
            panic!()
        };
        assert_eq!(
            fill.paint.color.as_constant(),
            Some(&Color([0.0, 128.0 / 255.0, 0.0, 1.0]))
        );
        assert_eq!(style.json()["layers"][0]["paint"]["fill-color"], "green");

        let diff = style
            .set_paint_property("water", "fill-color", Some(json!(["get", "c"])))
            .unwrap();
        assert_eq!(diff.repaint, [0].into());
        assert!(
            style
                .set_paint_property("water", "line-width", None)
                .is_err()
        );
        assert!(
            style
                .set_paint_property("nope", "fill-color", None)
                .is_err()
        );

        let diff = style
            .set_paint_property(
                "water",
                "fill-color-transition",
                Some(json!({"duration": 0})),
            )
            .unwrap();
        assert_eq!(style.layers[0].transitions["fill-color"].duration, 0.0);
        assert_eq!(diff.restyle, [0].into());
    }

    #[test]
    fn layout_and_filter_edits_reprocess() {
        let mut style = style();
        let diff = style
            .set_layout_property("roads", "visibility", Some(json!("none")))
            .unwrap();
        assert_eq!(diff.reprocess, [1].into());
        assert!(!style.layers[1].visible);
        let diff = style
            .set_filter("water", Some(json!(["has", "x"])))
            .unwrap();
        assert_eq!(diff.reprocess, [0].into());
        assert_eq!(diff.sources_to_reprocess(&style), ["s"].into());
    }

    #[test]
    fn global_state() {
        let mut style = style();
        let diff = style.set_global_state("highlight", Some(json!("park")));
        assert_eq!(diff.reprocess, [2].into(), "the filter reads it");
        assert_eq!(diff.repaint, [1].into(), "per-feature paint reads it");
        assert!(
            style
                .set_global_state("highlight", Some(json!("park")))
                .is_empty()
        );
        let mut ctx = maplibre_expr::EvaluationContext::new();
        ctx.feature
            .properties
            .insert("class".into(), maplibre_expr::Value::String("park".into()));
        assert!(style.layers[2].filter.matches(&ctx));
        let LayerKind::Line(line) = &style.layers[1].kind else {
            panic!()
        };
        assert_eq!(line.paint.color.evaluate(&ctx), Color([1.0, 0.0, 0.0, 1.0]));
        assert!(line.paint.color.dependencies().unwrap().global_state);

        style.set_global_state("highlight", None);
        assert_eq!(style.state["highlight"], "none");
        assert!(!style.layers[2].filter.matches(&ctx));
    }

    #[test]
    fn whole_style_diff() {
        let old = style();
        let mut json = old.json().clone();
        json["layers"][0]["paint"]["fill-color"] = json!("red");
        json["layers"][2]["minzoom"] = json!(5);
        let new = Style::from_json(&json).unwrap();
        let diff = old.diff(&new);
        assert_eq!(diff.restyle, [0].into());
        assert_eq!(diff.reprocess, [2].into());
        assert!(!diff.reload_all);

        json["layers"].as_array_mut().unwrap().pop();
        assert!(old.diff(&Style::from_json(&json).unwrap()).reload_all);
        assert!(old.diff(&old.clone()).is_empty());
    }
}
