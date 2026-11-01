//! `color-relief` layers, drawn from `raster-dem` tiles: each pixel colored by its
//! elevation through `color-relief-color`, baked into a [`ColorRamp`] once per layer.

use std::sync::Arc;

use maplibre_expr::{EvaluationContext, Expr};

use super::TileId;
use crate::dem::DemData;
use crate::style::{Color, PropertyValue};

#[derive(Debug, Clone)]
pub struct ColorReliefBucket {
    pub tile: TileId,
    pub dem: Arc<DemData>,
}

pub(crate) fn bucket(tile: TileId, dem: Arc<DemData>) -> ColorReliefBucket {
    ColorReliefBucket { tile, dem }
}

/// How many colors [`ColorRamp::bake`] samples.
pub const RAMP_SIZE: usize = 512;

/// `color-relief-color` sampled at evenly spaced elevations, for a renderer to look up
/// with linear filtering. Elevations outside `min..=max` take the end colors, as they do
/// with the `interpolate` and `step` expressions the property is written with.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorRamp {
    /// The elevation of the first color, in meters.
    pub min: f32,
    /// The elevation of the last color, in meters.
    pub max: f32,
    /// Premultiplied RGBA.
    pub colors: Vec<[u8; 4]>,
}

impl ColorRamp {
    /// Sample `color` from a little below the lowest to a little above the highest stop of
    /// the `interpolate` or `step` over `["elevation"]` in it, so the end colors of a
    /// `step` are in the ramp. An unset color is transparent everywhere.
    pub fn bake(color: &PropertyValue<Option<Color>>) -> Self {
        let (min, max) = match color {
            PropertyValue::Expression { expr, .. } => {
                let mut stops = Vec::new();
                elevation_stops(expr, &mut stops);
                stops
                    .iter()
                    .fold(None, |range: Option<(f64, f64)>, &s| {
                        Some(range.map_or((s, s), |(lo, hi)| (lo.min(s), hi.max(s))))
                    })
                    .unwrap_or((0.0, 0.0))
            }
            PropertyValue::Constant(_) => (0.0, 0.0),
        };
        let pad = (max - min) * 0.01 + 1.0;
        let (min, max) = (min - pad, max + pad);
        let mut ctx = EvaluationContext::new();
        let colors = Color::ramp(RAMP_SIZE, [min, max], |elevation| {
            ctx.elevation = Some(elevation);
            color.evaluate(&ctx).unwrap_or(Color::TRANSPARENT)
        });
        Self {
            min: min as f32,
            max: max as f32,
            colors,
        }
    }

    /// The color at `elevation`, interpolated between samples as a linear texture lookup
    /// does.
    pub fn color_at(&self, elevation: f32) -> [u8; 4] {
        let span = self.max - self.min;
        let t = if span > 0.0 {
            ((elevation - self.min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = t * (self.colors.len() - 1) as f32;
        let (i, f) = (x.floor() as usize, x.fract());
        let (a, b) = (
            self.colors[i],
            self.colors[(i + 1).min(self.colors.len() - 1)],
        );
        std::array::from_fn(|k| {
            (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * f).round() as u8
        })
    }
}

/// The stop inputs of every `interpolate` and `step` over `["elevation"]` in `expr`.
fn elevation_stops(expr: &Expr, out: &mut Vec<f64>) {
    match expr {
        Expr::Interpolate { input, stops, .. } | Expr::Step { input, stops, .. } => {
            if is_elevation(input) {
                out.extend(stops.iter().map(|(s, _)| *s));
            }
            stops.iter().for_each(|(_, e)| elevation_stops(e, out));
            if let Expr::Step { output0, .. } = expr {
                elevation_stops(output0, out);
            }
        }
        Expr::Assert(_, inner) | Expr::Coerce(_, inner) => elevation_stops(inner, out),
        Expr::Let { bindings, body } => {
            bindings.iter().for_each(|(_, e)| elevation_stops(e, out));
            elevation_stops(body, out);
        }
        Expr::Call { args, .. } => args.iter().for_each(|e| elevation_stops(e, out)),
        Expr::Match {
            input,
            arms,
            default,
        } => {
            elevation_stops(input, out);
            arms.iter().for_each(|(_, e)| elevation_stops(e, out));
            elevation_stops(default, out);
        }
        _ => {}
    }
}

fn is_elevation(expr: &Expr) -> bool {
    match expr {
        Expr::Assert(_, inner) | Expr::Coerce(_, inner) => is_elevation(inner),
        Expr::Call { op, args } => op == "elevation" && args.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::style::LayerKind;

    fn color(value: serde_json::Value) -> PropertyValue<Option<Color>> {
        let style = crate::Style::from_json(&json!({
            "version": 8,
            "sources": {"dem": {"type": "raster-dem"}},
            "layers": [{"id": "c", "type": "color-relief", "source": "dem",
                        "paint": {"color-relief-color": value}}]
        }))
        .unwrap();
        assert!(style.warnings.is_empty(), "{:?}", style.warnings);
        match &style.layers[0].kind {
            LayerKind::ColorRelief(l) => l.paint.color.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn interpolate_spans_its_stops() {
        let ramp = ColorRamp::bake(&color(json!([
            "interpolate",
            ["linear"],
            ["elevation"],
            0,
            "#000000",
            100,
            "#ff0000",
            400,
            "#ffffff"
        ])));
        assert!(ramp.min < 0.0 && ramp.max > 400.0, "{ramp:?}");
        assert_eq!(ramp.colors.len(), RAMP_SIZE);
        assert_eq!(ramp.color_at(-50.0), [0, 0, 0, 255]);
        let red = ramp.color_at(100.0);
        assert!(red[0] >= 250 && red[1] <= 3, "{red:?}");
        let mid = ramp.color_at(50.0);
        assert!((126..=129).contains(&mid[0]) && mid[1] == 0, "{mid:?}");
        assert_eq!(ramp.color_at(1000.0), [255, 255, 255, 255]);
    }

    #[test]
    fn step_keeps_its_colors() {
        let ramp = ColorRamp::bake(&color(json!([
            "step",
            ["elevation"],
            "#0000ff",
            10,
            "#00ff00",
            20,
            "#ff0000"
        ])));
        assert!(ramp.min < 10.0 && ramp.max > 20.0, "{ramp:?}");
        assert_eq!(ramp.color_at(0.0), [0, 0, 255, 255]);
        assert_eq!(ramp.color_at(15.0), [0, 255, 0, 255]);
        assert_eq!(ramp.color_at(25.0), [255, 0, 0, 255]);
    }

    #[test]
    fn colors_are_premultiplied() {
        let ramp = ColorRamp::bake(&color(json!([
            "interpolate",
            ["linear"],
            ["elevation"],
            0,
            "rgba(255, 0, 0, 0.5)",
            1,
            "red"
        ])));
        assert_eq!(ramp.color_at(0.0), [128, 0, 0, 128]);
    }

    #[test]
    fn unset_is_transparent() {
        let ramp = ColorRamp::bake(&PropertyValue::Constant(None));
        assert!(ramp.colors.iter().all(|&c| c == [0; 4]));
    }
}
