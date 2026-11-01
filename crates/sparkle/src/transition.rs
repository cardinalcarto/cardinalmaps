//! Paint property transitions: when a style change (an edit, a new global state, a
//! reloaded style) changes a paint property, the drawn value eases from the old value to
//! the new over the property's `*-transition` (or the style's `transition`), as in MapLibre.
//!
//! Renderers draw a *shown* copy of the style. [`Transitions::start`] compares it with
//! the new target style when that changes; every frame, [`Transitions::apply`] writes each
//! running transition's current value into the shown style as a constant. Like MapLibre,
//! only properties the spec interpolates transition, and only between values that are the
//! same for every feature; per-feature values switch at once.

use std::collections::HashMap;

use crate::style::{AnyProperty, Style, StyleLayer, Transition};

struct Running {
    layer: String,
    property: &'static str,
    /// What was shown when the transition started.
    from: Box<dyn AnyProperty>,
    /// In milliseconds, on the clock passed to [`Transitions::start`].
    start: f64,
    timing: Transition,
}

/// The paint transitions running on a style.
#[derive(Default)]
pub struct Transitions {
    running: Vec<Running>,
}

impl Transitions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start transitions for the paint properties that differ between `shown` (the style
    /// as drawn last, transitions included) and `target` (the style just changed to), at
    /// time `now` in milliseconds. Afterwards, replace `shown` with a copy of `target` and
    /// call [`Self::apply`] every frame.
    pub fn start(&mut self, shown: &Style, target: &Style, now: f64) {
        let shown_layers: HashMap<&str, &StyleLayer> =
            shown.layers.iter().map(|l| (l.id.as_str(), l)).collect();
        for layer in &target.layers {
            let Some(old) = shown_layers.get(layer.id.as_str()) else {
                self.running.retain(|r| r.layer != layer.id);
                continue;
            };
            if old.kind.type_name() != layer.kind.type_name() {
                self.running.retain(|r| r.layer != layer.id);
                continue;
            }
            let mut before = Vec::new();
            old.kind.for_each_paint_property(&mut |name, p| {
                before.push((name, format!("{p:?}"), p.clone_box()))
            });
            let mut i = 0;
            layer.kind.for_each_paint_property(&mut |name, p| {
                let (_, fingerprint, from) = &before[i];
                i += 1;
                if *fingerprint == format!("{p:?}") {
                    return;
                }
                // A new value replaces whatever transition the property had.
                self.running
                    .retain(|r| !(r.layer == layer.id && r.property == name));
                let timing = layer
                    .transitions
                    .get(name)
                    .copied()
                    .unwrap_or(target.transition);
                let per_feature = from.is_feature_dependent() || p.is_feature_dependent();
                if timing.duration + timing.delay <= 0.0 || per_feature || !interpolates(name) {
                    return;
                }
                self.running.push(Running {
                    layer: layer.id.clone(),
                    property: name,
                    from: from.clone_box(),
                    start: now,
                    timing,
                });
            });
        }
    }

    /// Write the current value of every running transition into `shown`, whose layers
    /// must be `target`'s (a copy of it, possibly with transitions applied), with paint
    /// evaluated at `zoom`. Finished transitions are dropped and their property set back to
    /// `target`'s. Returns whether any transition is still running, so another frame
    /// should be drawn.
    pub fn apply(&mut self, target: &Style, shown: &mut Style, now: f64, zoom: f64) -> bool {
        self.running.retain(|r| {
            let (Some(i), Some(j)) = (target.layer_index(&r.layer), shown.layer_index(&r.layer))
            else {
                return false;
            };
            let t = progress(r, now);
            let mut to = None;
            target.layers[i]
                .kind
                .for_each_paint_property(&mut |name, p| {
                    if name == r.property {
                        to = Some(p.clone_box());
                    }
                });
            let Some(to) = to else { return false };
            let mut ok = false;
            shown.layers[j]
                .kind
                .for_each_paint_property_mut(&mut |name, p| {
                    if name != r.property {
                        return;
                    }
                    ok = if t >= 1.0 {
                        p.assign(&*to);
                        false
                    } else {
                        p.set_interpolated(&*r.from, &*to, ease_cubic_in_out(t), zoom)
                    };
                });
            ok
        });
        !self.running.is_empty()
    }

    pub fn is_running(&self) -> bool {
        !self.running.is_empty()
    }
}

/// Whether the spec interpolates a property, which is what makes it transition.
fn interpolates(name: &str) -> bool {
    maplibre_expr::migrate::property_spec(name)
        .and_then(|spec| spec.pointer("/expression/interpolated"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// How far along a transition is at `now`, `0..=1`.
fn progress(r: &Running, now: f64) -> f32 {
    let elapsed = now - r.start - r.timing.delay;
    if elapsed < 0.0 {
        0.0
    } else if r.timing.duration <= 0.0 {
        1.0
    } else {
        (elapsed / r.timing.duration).min(1.0) as f32
    }
}

/// MapLibre's `easeCubicInOut`, which property transitions follow.
fn ease_cubic_in_out(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let (t2, t3) = (t * t, t * t * t);
    4.0 * if t < 0.5 {
        t3
    } else {
        3.0 * (t - t2) + t3 - 0.75
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::{Color, LayerKind};

    fn style(color: &str, width: serde_json::Value) -> Style {
        Style::from_json(&json!({
            "version": 8,
            "transition": {"duration": 400, "delay": 0},
            "sources": {"s": {"type": "vector"}},
            "layers": [
                {"id": "water", "type": "fill", "source": "s", "source-layer": "water",
                 "paint": {"fill-color": color, "fill-opacity-transition": {"duration": 0}}},
                {"id": "roads", "type": "line", "source": "s", "source-layer": "roads",
                 "paint": {"line-width": width, "line-width-transition": {"duration": 100, "delay": 100}}},
            ]
        }))
        .unwrap()
    }

    fn fill_color(style: &Style) -> Color {
        let LayerKind::Fill(fill) = &style.layers[0].kind else {
            panic!()
        };
        fill.paint.color.evaluate_at_zoom(10.0)
    }

    fn line_width(style: &Style) -> f32 {
        let LayerKind::Line(line) = &style.layers[1].kind else {
            panic!()
        };
        line.paint.width.evaluate_at_zoom(10.0)
    }

    #[test]
    fn eases_from_old_to_new() {
        let old = style("#000000", json!(2));
        let new = style("#ffffff", json!(4));
        let mut transitions = Transitions::new();
        transitions.start(&old, &new, 1000.0);
        let mut shown = new.clone();

        assert!(transitions.apply(&new, &mut shown, 1000.0, 10.0));
        assert_eq!(fill_color(&shown), Color::BLACK);
        assert_eq!(line_width(&shown), 2.0, "still delayed");

        transitions.apply(&new, &mut shown, 1200.0, 10.0);
        assert_eq!(fill_color(&shown), Color([0.5, 0.5, 0.5, 1.0]), "halfway");
        assert_eq!(line_width(&shown), 4.0, "the line's own timing ended");

        assert!(!transitions.apply(&new, &mut shown, 1400.0, 10.0));
        assert_eq!(fill_color(&shown), Color::WHITE);
        assert_eq!(format!("{shown:?}"), format!("{new:?}"));
    }

    #[test]
    fn restarts_from_the_shown_value() {
        let a = style("#000000", json!(2));
        let b = style("#ffffff", json!(2));
        let mut transitions = Transitions::new();
        transitions.start(&a, &b, 0.0);
        let mut shown = b.clone();
        transitions.apply(&b, &mut shown, 200.0, 10.0);
        // Back to black from mid-gray.
        let c = style("#000000", json!(2));
        transitions.start(&shown, &c, 200.0);
        let mut shown = c.clone();
        transitions.apply(&c, &mut shown, 200.0, 10.0);
        assert_eq!(fill_color(&shown), Color([0.5, 0.5, 0.5, 1.0]));
    }

    #[test]
    fn per_feature_values_switch_at_once() {
        let a = style("#000000", json!(2));
        let b = style("#000000", json!(["get", "w"]));
        let mut transitions = Transitions::new();
        transitions.start(&a, &b, 0.0);
        assert!(!transitions.is_running());
    }

    #[test]
    fn easing() {
        assert_eq!(ease_cubic_in_out(0.5), 0.5);
        assert!(ease_cubic_in_out(0.25) < 0.25);
        assert!(ease_cubic_in_out(0.75) > 0.75);
    }
}
