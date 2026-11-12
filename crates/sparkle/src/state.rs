//! Feature state: values set at runtime per feature (hover, selection, ...) that paint
//! expressions read with `["feature-state", key]`.
//!
//! Tile processing knows nothing of feature state. Instead, a bucket whose per-vertex
//! paint reads feature (or global) state [retains](Bucket::retains_features) what its
//! expressions read of each feature, so when the state changes only those values are
//! evaluated again ([`Bucket::update_feature_state`]) and the renderer uploads the new
//! paint attributes, without tessellating anything again. Global state is handled by the
//! style instead (see [`Style::set_global_state`](crate::Style::set_global_state)); its
//! per-feature effects use [`Bucket::rebuild_paint`].

use std::collections::{BTreeMap, HashMap};

use maplibre_expr::Value;

use crate::paint::{PaintArrays, PaintProperties};
use crate::style::{LayerKind, StyleLayer};
use crate::tile::{Bucket, BucketGeometry};

/// One feature's state.
pub type StateMap = BTreeMap<String, Value>;

/// The id feature state and queries know a feature by, from an id or `promoteId` value:
/// a non-negative integer as it is (also when written as a string), anything else (such
/// as a non-numeric string) hashed, so it is stable but can't be told from a number.
pub fn feature_id(value: &Value) -> Option<u64> {
    match value {
        Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n < u64::MAX as f64 => {
            Some(*n as u64)
        }
        Value::String(s) => Some(s.parse().unwrap_or_else(|_| fnv1a(s.as_bytes()))),
        Value::Null => None,
        other => Some(fnv1a(other.to_string().as_bytes())),
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Feature state of every source, by source id, source layer (empty for sources without
/// layers, such as GeoJSON) and feature id.
#[derive(Debug, Clone, Default)]
pub struct FeatureStates {
    sources: HashMap<String, HashMap<String, HashMap<u64, StateMap>>>,
}

impl FeatureStates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge `state` into a feature's state, like MapLibre's `setFeatureState`. Returns
    /// whether anything changed.
    pub fn set(
        &mut self,
        source: &str,
        source_layer: Option<&str>,
        id: u64,
        state: impl IntoIterator<Item = (String, Value)>,
    ) -> bool {
        let current = self
            .sources
            .entry(source.to_owned())
            .or_default()
            .entry(source_layer.unwrap_or_default().to_owned())
            .or_default()
            .entry(id)
            .or_default();
        let mut changed = false;
        for (key, value) in state {
            if current.get(&key) != Some(&value) {
                current.insert(key, value);
                changed = true;
            }
        }
        changed
    }

    /// Remove one key of a feature's state, a feature's whole state (`key` `None`), or the
    /// state of every feature of the source layer (`id` `None`), like MapLibre's
    /// `removeFeatureState`. Returns the ids whose state changed.
    pub fn remove(
        &mut self,
        source: &str,
        source_layer: Option<&str>,
        id: Option<u64>,
        key: Option<&str>,
    ) -> Vec<u64> {
        let Some(layer) = self
            .sources
            .get_mut(source)
            .and_then(|s| s.get_mut(source_layer.unwrap_or_default()))
        else {
            return Vec::new();
        };
        let mut changed = Vec::new();
        let mut remove_from = |id: u64, state: &mut StateMap| match key {
            Some(key) => {
                if state.remove(key).is_some() {
                    changed.push(id);
                }
            }
            None => {
                if !state.is_empty() {
                    state.clear();
                    changed.push(id);
                }
            }
        };
        match id {
            Some(id) => {
                if let Some(state) = layer.get_mut(&id) {
                    remove_from(id, state);
                }
            }
            None => layer.iter_mut().for_each(|(&id, s)| remove_from(id, s)),
        }
        layer.retain(|_, s| !s.is_empty());
        changed
    }

    /// A feature's state, if it has any.
    pub fn get(&self, source: &str, source_layer: Option<&str>, id: u64) -> Option<&StateMap> {
        self.layer(source, source_layer)?.get(&id)
    }

    /// The state of every feature of a source layer that has some.
    pub fn layer(
        &self,
        source: &str,
        source_layer: Option<&str>,
    ) -> Option<&HashMap<u64, StateMap>> {
        self.sources
            .get(source)?
            .get(source_layer.unwrap_or_default())
    }

    /// Whether any feature of `source` has state.
    pub fn has_source(&self, source: &str) -> bool {
        self.sources
            .get(source)
            .is_some_and(|layers| layers.values().any(|l| !l.is_empty()))
    }
}

impl Bucket {
    /// Whether the bucket kept what it needs to evaluate its paint attributes again,
    /// because some of them read feature or global state.
    pub fn retains_features(&self) -> bool {
        self.geometry.paint().iter().any(|p| p.retains_features())
    }

    /// Evaluate again the paint of the features with an id in `ids` (all features for
    /// `None`) of this bucket of `layer`, with their state in `states`. Returns whether
    /// any paint attribute changed, in which case the renderer should upload them again.
    pub fn update_feature_state(
        &mut self,
        layer: &StyleLayer,
        states: &FeatureStates,
        ids: Option<&[u64]>,
    ) -> bool {
        let Some(source) = &layer.source else {
            return false;
        };
        let lookup = states.layer(source, layer.source_layer.as_deref());
        let state = |id: u64| lookup.and_then(|l| l.get(&id));
        let mut changed = false;
        self.geometry
            .for_each_paint_mut(&layer.kind, &mut |paint, properties| {
                changed |= paint.update_state(properties, ids, &state);
            });
        changed
    }

    /// Lay out and evaluate the paint attributes again for `layer` as it is now, after a
    /// style change listed it in [`StyleDiff::repaint`](crate::style::StyleDiff::repaint).
    /// Returns false, changing nothing, if the bucket doesn't
    /// [retain its features](Self::retains_features); process the tile again then.
    pub fn rebuild_paint(&mut self, layer: &StyleLayer, states: &FeatureStates) -> bool {
        if !self.retains_features() {
            return false;
        }
        let lookup = layer
            .source
            .as_deref()
            .and_then(|source| states.layer(source, layer.source_layer.as_deref()));
        let state = |id: u64| lookup.and_then(|l| l.get(&id));
        let mut rebuilt = true;
        self.geometry
            .for_each_paint_mut(&layer.kind, &mut |paint, properties| match paint
                .rebuild(properties, &state)
            {
                Some(new) => *paint = new,
                None => rebuilt = false,
            });
        rebuilt
    }
}

impl BucketGeometry {
    /// The bucket's paint attributes. Layer types with several sets of them (symbol icons
    /// and text, extrusion roofs and walls) list each.
    pub fn paint(&self) -> Vec<&PaintArrays> {
        match self {
            Self::Fill(b) => vec![&b.paint],
            Self::Line(b) => vec![&b.paint],
            Self::Circle(b) => vec![&b.paint],
            Self::Heatmap(b) => vec![&b.paint],
            Self::FillExtrusion(b) => vec![&b.roofs.paint, &b.walls.paint],
            Self::Symbol(b) => vec![&b.text.paint, &b.icons.paint],
            // Raster-based layer types have no per-feature paint attributes.
            _ => Vec::new(),
        }
    }

    /// Call `f` with each set of paint attributes and the properties of `kind` they were
    /// built from.
    pub(crate) fn for_each_paint_mut(
        &mut self,
        kind: &LayerKind,
        f: &mut dyn FnMut(&mut PaintArrays, &PaintProperties<'_>),
    ) {
        match (self, kind) {
            (Self::Fill(b), LayerKind::Fill(l)) => f(&mut b.paint, &l.paint.attributes()),
            (Self::Line(b), LayerKind::Line(l)) => f(&mut b.paint, &l.paint.attributes()),
            (Self::Circle(b), LayerKind::Circle(l)) => f(&mut b.paint, &l.paint.attributes()),
            (Self::Heatmap(b), LayerKind::Heatmap(l)) => f(&mut b.paint, &l.paint.attributes()),
            (Self::FillExtrusion(b), LayerKind::FillExtrusion(l)) => {
                f(&mut b.roofs.paint, &l.paint.roof_attributes());
                f(&mut b.walls.paint, &l.paint.attributes());
            }
            (Self::Symbol(b), LayerKind::Symbol(l)) => {
                f(&mut b.text.paint, &l.text_attributes());
                f(&mut b.icons.paint, &l.icon_attributes());
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(feature_id(&Value::Number(42.0)), Some(42));
        assert_eq!(feature_id(&Value::String("42".into())), Some(42));
        let hashed = feature_id(&Value::String("way/1".into())).unwrap();
        assert_eq!(feature_id(&Value::String("way/1".into())), Some(hashed));
        assert_ne!(feature_id(&Value::Number(-1.0)), Some(1));
        assert_eq!(feature_id(&Value::Null), None);
    }

    #[test]
    fn set_and_remove() {
        let mut states = FeatureStates::new();
        let hover = || [("hover".to_owned(), Value::Bool(true))];
        assert!(states.set("s", Some("roads"), 1, hover()));
        assert!(!states.set("s", Some("roads"), 1, hover()), "unchanged");
        assert!(states.set("s", Some("roads"), 2, hover()));
        assert!(states.get("s", Some("roads"), 1).is_some());
        assert!(states.get("s", None, 1).is_none());
        assert!(states.has_source("s"));

        assert_eq!(
            states.remove("s", Some("roads"), Some(1), Some("hover")),
            [1]
        );
        assert!(states.get("s", Some("roads"), 1).is_none());
        let mut removed = states.remove("s", Some("roads"), None, None);
        removed.sort_unstable();
        assert_eq!(removed, [2]);
        assert!(!states.has_source("s"));
    }
}
