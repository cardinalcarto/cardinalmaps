//! `line`: stroked lines (and polygon outlines).

use maplibre_expr::EvaluationContext;

use super::{layer, properties};
use crate::paint::PaintProperty;
use crate::style::{Color, LineCap, LineJoin, ResolvedImage, TranslateAnchor};

properties! {
    pub struct LineLayout {
        cap: LineCap = "line-cap",
        join: LineJoin = "line-join",
        /// Miter joins longer than this many line widths become bevels.
        miter_limit: f32 = "line-miter-limit",
        /// Round joins shorter than this many line widths become miters.
        round_limit: f32 = "line-round-limit",
        sort_key: Option<f32> = "line-sort-key",
    }
}

properties! {
    pub struct LinePaint {
        opacity: f32 = "line-opacity",
        color: Color = "line-color",
        /// In pixels.
        translate: [f32; 2] = "line-translate",
        translate_anchor: TranslateAnchor = "line-translate-anchor",
        /// In pixels.
        width: f32 = "line-width",
        /// Draws two lines this far apart (inner edge to inner edge), in pixels.
        gap_width: f32 = "line-gap-width",
        /// In pixels, to the right of the line's direction.
        offset: f32 = "line-offset",
        /// In pixels.
        blur: f32 = "line-blur",
        /// In line widths.
        dasharray: Option<Vec<f32>> = "line-dasharray",
        pattern: Option<ResolvedImage> = "line-pattern",
        /// Reads `["line-progress"]`, so it needs the source's `lineMetrics`.
        gradient: Option<Color> = "line-gradient",
        layer_opacity: f32 = "line-layer-opacity",
    }
}

layer!(LineLayer = "line", LineLayout, LinePaint);

impl LinePaint {
    /// The paint properties a line shader reads, by their names in WGSL.
    ///
    /// `floorwidth` is `line-width` again, for the renderer to evaluate at the integer zoom
    /// below the camera: dashes and patterns are sized in widths of that zoom, as in
    /// MapLibre, so they keep their length while the width is interpolated.
    pub fn attributes(&self) -> [(&'static str, &dyn PaintProperty); 7] {
        [
            ("color", &self.color),
            ("opacity", &self.opacity),
            ("width", &self.width),
            ("blur", &self.blur),
            ("gap_width", &self.gap_width),
            ("offset", &self.offset),
            ("floorwidth", &self.width),
        ]
    }

    /// The index of `floorwidth` in [`Self::attributes`].
    pub const FLOORWIDTH_ATTRIBUTE: usize = 6;

    /// `line-gradient` at `n` evenly spaced line progress values from 0 to 1, as
    /// premultiplied RGBA8 for a texture. `None` without a gradient.
    pub fn gradient_ramp(&self, n: usize) -> Option<Vec<[u8; 4]>> {
        if matches!(self.gradient.as_constant(), Some(None)) {
            return None;
        }
        let mut ctx = EvaluationContext::new();
        Some(Color::ramp(n, [0.0, 1.0], |progress| {
            ctx.line_progress = Some(progress);
            self.gradient.evaluate(&ctx).unwrap_or(Color::TRANSPARENT)
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::Style;
    use crate::style::LayerKind;

    #[test]
    fn gradient_ramp_reads_line_progress() {
        let style: Style = r##"{"version": 8, "sources": {}, "layers": [{
            "id": "l", "type": "line", "source": "s",
            "paint": {"line-gradient": ["interpolate", ["linear"], ["line-progress"],
                0, "#ff0000", 1, "#0000ff"]}
        }, {"id": "plain", "type": "line", "source": "s"}]}"##
            .parse()
            .unwrap();
        let LayerKind::Line(line) = &style.layers[0].kind else {
            panic!()
        };
        let ramp = line.paint.gradient_ramp(3).unwrap();
        assert_eq!(ramp[0], [255, 0, 0, 255]);
        assert_eq!(ramp[1], [128, 0, 128, 255]);
        assert_eq!(ramp[2], [0, 0, 255, 255]);
        let LayerKind::Line(plain) = &style.layers[1].kind else {
            panic!()
        };
        assert!(plain.paint.gradient_ramp(3).is_none());
    }
}
