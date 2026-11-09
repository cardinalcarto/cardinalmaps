//! Lines as triangles that a vertex shader widens to the current line width.
//!
//! Each vertex sits on the line's centerline and carries an `extrude` vector in units of
//! half the line width. The shader moves it to `position + extrude * half_width`, with the
//! half width converted from pixels to tile units for the current zoom. Because widths are
//! only applied on the GPU, zooming never requires tessellating again.
//!
//! `line-offset` works the same way, along a separate `offset` vector: both sides of the
//! line move the same way, while `extrude` pushes them apart. `line-gap-width` needs no
//! geometry of its own: the shader extrudes to the outer edge of the two lines, and the
//! fragment shader leaves out the gap between them, since a fragment's distance from the
//! centerline is the length of its interpolated extrude.

use std::f32::consts::{FRAC_PI_8, PI};

use bytemuck::{Pod, Zeroable};
use maplibre_expr::EvaluationContext;
use mlt_core::geo_types::{Coord, Geometry, LineString};

use super::sort_key::SortKeys;
use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::pattern::{DashAtlas, FeatureImage};
use crate::sprite::ImageAtlas;
use crate::style::{LineCap, LineJoin, LineLayer, LineLayout, Source};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    /// On the centerline, in tile coordinates.
    pub position: [f32; 2],
    /// Offset from `position` in half line widths: a unit normal on the sides of a
    /// straight line, longer at miter joins, `[0, 0]` at the center of round joins and caps.
    pub extrude: [f32; 2],
    /// Where one pixel of `line-offset` moves the vertex: the unit normal of its segment,
    /// pointing to the right of the line's direction, or a miter at joins.
    pub offset: [f32; 2],
    /// Distance along the line from its start in tile units, for dashes and patterns.
    pub distance: f32,
    /// How far `extrude` reaches along the line, past `distance`, in half widths. Only
    /// caps have one, so that dashes and patterns continue around them.
    pub along: f32,
    /// How far along the whole line the vertex is, from 0 to 1, for `line-gradient`. Only
    /// set for sources with `lineMetrics`.
    pub progress: f32,
    /// 1 at the outer corners of round caps and joins, else 0. They're triangles around
    /// the round part that the fragment shader cuts to it where the interpolated extrude
    /// reaches past a length of 1: renderers that clamp extrudes to that length (for
    /// miters) must leave these as they are.
    pub round: f32,
}

/// Triangles for every line of a style layer in one tile.
///
/// Triangles have no consistent winding and overlap on the inside of bends, so draw them
/// without back-face culling, and with a stencil or depth test if translucent lines must
/// not darken where they overlap themselves.
#[derive(Debug, Clone, Default)]
pub struct LineBucket {
    pub vertices: Vec<LineVertex>,
    /// Triangle list indices into [`Self::vertices`].
    pub triangles: Vec<u32>,
    /// Per-vertex values of [`LinePaint::attributes`](crate::style::LinePaint::attributes).
    pub paint: PaintArrays,
    /// Per vertex, when `line-pattern` differs per feature (a sprite image, see
    /// [`FeatureImage::pattern`]) or else `line-dasharray` does ([`FeatureImage::dash`]).
    /// Empty when both are the same for every feature, and the renderer evaluates them.
    pub images: Vec<FeatureImage>,
    /// The dash patterns of a feature-dependent `line-dasharray`, which `images` refer to.
    pub dashes: Option<DashAtlas>,
}

/// Joins sharper than this miter length (in half widths) of a bend this slight are drawn
/// as miters whatever `line-join` says, since a bevel or round join would look the same.
const STRAIGHT_MITER: f32 = 1.05;

/// Largest angle one triangle of a round join spans, when it's tessellated as a fan.
const ROUND_STEP: f32 = FRAC_PI_8;

/// Largest angle one kite of a round join spans, when the fragment shader rounds it off:
/// its outer corner reaches at most twice the line's half width.
const ROUND_KITE: f32 = 2.0 * PI / 3.0;

#[derive(Clone, Copy)]
struct Shape {
    cap: LineCap,
    join: LineJoin,
    miter_limit: f32,
    round_limit: f32,
    /// Whether the line may be offset, which needs joins filled on both sides.
    offset: bool,
    /// Whether to fill in [`LineVertex::progress`].
    line_metrics: bool,
    /// Where this piece of the line starts and ends along the whole line, as fractions of
    /// its length: GeoJSON tiling cuts lines into pieces, and `line-progress` spans the
    /// whole line.
    clip: [f32; 2],
}

impl Shape {
    fn evaluate(
        layer: &LineLayer,
        ctx: &EvaluationContext,
        line_metrics: bool,
        clip: Option<[f64; 2]>,
    ) -> Self {
        let layout: &LineLayout = &layer.layout;
        Self {
            cap: layout.cap.evaluate(ctx),
            join: layout.join.evaluate(ctx),
            miter_limit: layout.miter_limit.evaluate(ctx),
            round_limit: layout.round_limit.evaluate(ctx),
            offset: layer.paint.offset.as_constant() != Some(&0.0),
            line_metrics,
            clip: clip.map_or([0.0, 1.0], |c| c.map(|f| f as f32)),
        }
    }
}

/// Which per-feature images a bucket stores in [`LineBucket::images`].
#[derive(Clone, Copy, PartialEq)]
enum Images {
    None,
    Pattern,
    Dash,
}

pub(crate) fn builder<'a>(
    layer: &'a LineLayer,
    cx: &BuildContext<'a>,
    scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    let paint = &layer.paint;
    let images = if paint.pattern.is_feature_dependent() {
        Images::Pattern
    } else if matches!(paint.pattern.as_constant(), Some(None))
        && paint.dasharray.is_feature_dependent()
    {
        Images::Dash
    } else {
        Images::None
    };
    Some(Box::new(LineBuilder {
        layer,
        bucket: LineBucket {
            paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
            dashes: (images == Images::Dash).then(DashAtlas::new),
            ..Default::default()
        },
        tessellator: scratch.take(),
        // `line-gradient` needs `lineMetrics`, which only GeoJSON sources have.
        line_metrics: matches!(cx.source, Some(Source::GeoJson(s)) if s.line_metrics),
        images,
        atlas: cx.resources.images,
        sort_keys: SortKeys::new(&layer.layout.sort_key),
    }))
}

struct LineBuilder<'a> {
    layer: &'a LineLayer,
    bucket: LineBucket,
    tessellator: LineTessellator,
    line_metrics: bool,
    images: Images,
    atlas: Option<&'a ImageAtlas>,
    sort_keys: Option<SortKeys>,
}

impl LayerBuilder for LineBuilder<'_> {
    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let shape = Shape::evaluate(
            self.layer,
            feature.ctx,
            self.line_metrics,
            feature.line_clip,
        );
        let bucket = &mut self.bucket;
        let first_triangle = bucket.triangles.len();
        if !self
            .tessellator
            .add_feature(bucket, shape, feature.geometry)
        {
            return;
        }
        let paint = &self.layer.paint;
        bucket.paint.populate(
            &paint.attributes(),
            feature.ctx,
            feature.id,
            bucket.vertices.len(),
        );
        let image = match self.images {
            Images::None => None,
            Images::Pattern => {
                let name = paint.pattern.evaluate(feature.ctx);
                let image = name.and_then(|n| self.atlas?.resolve(&n));
                Some(FeatureImage::pattern(image))
            }
            Images::Dash => {
                let dashes = bucket.dashes.get_or_insert_default();
                let round = shape.cap == LineCap::Round;
                let pattern = paint
                    .dasharray
                    .evaluate(feature.ctx)
                    .and_then(|d| dashes.add(&d, round));
                Some(FeatureImage::dash(pattern))
            }
        };
        if let Some(image) = image {
            bucket.images.resize(bucket.vertices.len(), image);
        }
        if let Some(keys) = &mut self.sort_keys {
            let key = self.layer.layout.sort_key.evaluate(feature.ctx);
            keys.push(key, [first_triangle..bucket.triangles.len(), 0..0]);
        }
    }

    fn finish(mut self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry> {
        scratch.put(self.tessellator);
        if let Some(keys) = self.sort_keys {
            keys.apply([&mut self.bucket.triangles, &mut Vec::new()]);
        }
        (!self.bucket.vertices.is_empty()).then_some(BucketGeometry::Line(self.bucket))
    }
}

#[derive(Default)]
pub(crate) struct LineTessellator {
    points: Vec<[f32; 2]>,
}

impl LineTessellator {
    /// Add a line feature. Polygons are drawn as their closed rings, as MapLibre does;
    /// points are ignored. Returns whether vertices were added.
    fn add_feature(
        &mut self,
        bucket: &mut LineBucket,
        shape: Shape,
        geometry: &Geometry<i32>,
    ) -> bool {
        let first_vertex = bucket.vertices.len();
        let mut add = |line: &LineString<i32>, closed| {
            self.add_line(bucket, &line.0, closed, shape);
        };
        match geometry {
            Geometry::LineString(l) => add(l, false),
            Geometry::MultiLineString(ml) => ml.0.iter().for_each(|l| add(l, false)),
            Geometry::Polygon(p) => {
                std::iter::once(p.exterior())
                    .chain(p.interiors())
                    .for_each(|r| add(r, true));
            }
            Geometry::MultiPolygon(mp) => {
                mp.0.iter()
                    .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                    .for_each(|r| add(r, true))
            }
            _ => {}
        }
        bucket.vertices.len() > first_vertex
    }

    fn add_line(
        &mut self,
        bucket: &mut LineBucket,
        coords: &[Coord<i32>],
        closed: bool,
        shape: Shape,
    ) {
        let pts = &mut self.points;
        pts.clear();
        for c in coords {
            let p = [c.x as f32, c.y as f32];
            if pts.last() != Some(&p) {
                pts.push(p);
            }
        }
        if closed && pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        let n = pts.len();
        if n < 2 || (closed && n < 3) {
            return;
        }

        let pts = &*pts;
        let dir = |i: usize| normalize(sub(pts[(i + 1) % n], pts[i]));
        let seg_len = |i: usize| length(sub(pts[(i + 1) % n], pts[i]));
        let first_vertex = bucket.vertices.len();
        let mut out = Emitter { bucket };

        let total = if closed {
            let (first_end, first_start) = out.join(pts[0], dir(n - 1), dir(0), 0.0, shape);
            let mut prev = first_start;
            let mut distance = 0.0;
            for (i, &p) in pts.iter().enumerate().skip(1) {
                distance += seg_len(i - 1);
                let (end, start) = out.join(p, dir(i - 1), dir(i), distance, shape);
                out.connect(prev, end);
                prev = start;
            }
            // Close the ring with a copy of the first join, so distance keeps increasing.
            let total = distance + seg_len(n - 1);
            let last = out.copy_pair(first_end, total);
            out.connect(prev, last);
            total
        } else {
            let mut prev = out.cap(pts[0], dir(0), 0.0, shape.cap, false);
            let mut distance = 0.0;
            for (i, &p) in pts.iter().enumerate().take(n - 1).skip(1) {
                distance += seg_len(i - 1);
                let (end, start) = out.join(p, dir(i - 1), dir(i), distance, shape);
                out.connect(prev, end);
                prev = start;
            }
            distance += seg_len(n - 2);
            let last = out.cap(pts[n - 1], dir(n - 2), distance, shape.cap, true);
            out.connect(prev, last);
            distance
        };

        if shape.line_metrics && total > 0.0 {
            for v in &mut bucket.vertices[first_vertex..] {
                let [start, end] = shape.clip;
                v.progress = start + (end - start) * v.distance / total;
            }
        }
    }
}

/// The vertices on the left (`+normal`) and right (`-normal`) of a point on the line.
type Pair = [u32; 2];

struct Emitter<'a> {
    bucket: &'a mut LineBucket,
}

impl Emitter<'_> {
    fn vertex(
        &mut self,
        position: [f32; 2],
        extrude: [f32; 2],
        offset: [f32; 2],
        distance: f32,
        along: f32,
    ) -> u32 {
        let index = self.bucket.vertices.len() as u32;
        self.bucket.vertices.push(LineVertex {
            position,
            extrude,
            offset,
            distance,
            along,
            progress: 0.0,
            round: 0.0,
        });
        index
    }

    /// A cross section of the line, offset along `normal`.
    fn pair(&mut self, p: [f32; 2], normal: [f32; 2], distance: f32) -> Pair {
        [
            self.vertex(p, normal, normal, distance, 0.0),
            self.vertex(p, scale(normal, -1.0), normal, distance, 0.0),
        ]
    }

    fn copy_pair(&mut self, pair: Pair, distance: f32) -> Pair {
        pair.map(|i| {
            let v = self.bucket.vertices[i as usize];
            let copy = self.vertex(v.position, v.extrude, v.offset, distance, v.along);
            self.bucket.vertices[copy as usize].round = v.round;
            copy
        })
    }

    /// The quad between two cross sections of the line.
    fn connect(&mut self, a: Pair, b: Pair) {
        self.bucket
            .triangles
            .extend([a[0], a[1], b[0], a[1], b[1], b[0]]);
    }

    /// A fan around `p` from vertex `from` (extruded by `from_extrude`) to vertex `to`,
    /// sweeping `angle` radians in `steps` triangles. Its vertices are offset along
    /// `offset(fraction of the sweep)`.
    #[allow(clippy::too_many_arguments)]
    fn fan(
        &mut self,
        p: [f32; 2],
        distance: f32,
        from: u32,
        from_extrude: [f32; 2],
        angle: f32,
        steps: u32,
        to: u32,
        center_offset: [f32; 2],
        offset: impl Fn(f32) -> [f32; 2],
    ) -> u32 {
        let center = self.vertex(p, [0.0, 0.0], center_offset, distance, 0.0);
        let mut prev = from;
        for k in 1..steps {
            let f = k as f32 / steps as f32;
            let extrude = rotate(from_extrude, angle * f);
            let next = self.vertex(p, extrude, offset(f), distance, 0.0);
            self.bucket.triangles.extend([center, prev, next]);
            prev = next;
        }
        self.bucket.triangles.extend([center, prev, to]);
        center
    }

    /// A round join around `p` from vertex `from` (extruded by `from_extrude`) to vertex
    /// `to`, sweeping `angle` radians: kites of two triangles whose outer corners reach
    /// past the arc, for the fragment shader to cut to it (see [`LineVertex::round`]).
    /// Its vertices are offset along `normal` turned with the sweep. Only for lines
    /// without `line-offset`, which would move the corner the arc is around.
    #[allow(clippy::too_many_arguments)]
    fn kites(
        &mut self,
        p: [f32; 2],
        distance: f32,
        from: u32,
        from_extrude: [f32; 2],
        angle: f32,
        to: u32,
        normal: [f32; 2],
    ) {
        let pieces = (angle.abs() / ROUND_KITE).ceil().max(1.0) as u32;
        let half = angle / pieces as f32 / 2.0;
        let reach = 1.0 / half.cos();
        let center = self.vertex(p, [0.0; 2], normal, distance, 0.0);
        let mut prev = from;
        for k in 0..pieces {
            let turn = |t: f32| (rotate(from_extrude, t), rotate(normal, t));
            let (extrude, offset) = turn(half * (2 * k + 1) as f32);
            let corner = self.vertex(p, scale(extrude, reach), offset, distance, 0.0);
            self.bucket.vertices[corner as usize].round = 1.0;
            let next = if k + 1 == pieces {
                to
            } else {
                let (extrude, offset) = turn(half * (2 * k + 2) as f32);
                self.vertex(p, extrude, offset, distance, 0.0)
            };
            self.bucket
                .triangles
                .extend([center, prev, corner, center, corner, next]);
            prev = next;
        }
    }

    /// The end of an open line, pointing along `dir` (`end`) or against it (start).
    fn cap(&mut self, p: [f32; 2], dir: [f32; 2], distance: f32, cap: LineCap, end: bool) -> Pair {
        let normal = perp(dir);
        let out = if end { dir } else { scale(dir, -1.0) };
        let along = if end { 1.0 } else { -1.0 };
        match cap {
            LineCap::Butt => self.pair(p, normal, distance),
            LineCap::Square => [
                self.vertex(p, add(normal, out), normal, distance, along),
                self.vertex(p, sub(out, normal), normal, distance, along),
            ],
            LineCap::Round => {
                // The smallest triangle past the end around the half circle, which the
                // fragment shader cuts to it (see `LineVertex::round`): one triangle
                // rather than a fan of them, which adds up for the many short lines of a
                // tile.
                let pair = self.pair(p, normal, distance);
                let reach = std::f32::consts::SQRT_2;
                let corners = [
                    scale(normal, reach),
                    scale(normal, -reach),
                    scale(out, reach),
                ]
                .map(|extrude| {
                    let corner = self.vertex(
                        p,
                        extrude,
                        normal,
                        distance,
                        along * dot(extrude, out).abs(),
                    );
                    self.bucket.vertices[corner as usize].round = 1.0;
                    corner
                });
                self.bucket.triangles.extend(corners);
                pair
            }
        }
    }

    /// The corner at `p` between a segment along `a` and one along `b`. Returns the cross
    /// sections that end the first segment and start the second, which are the same for a
    /// miter join.
    fn join(
        &mut self,
        p: [f32; 2],
        a: [f32; 2],
        b: [f32; 2],
        distance: f32,
        shape: Shape,
    ) -> (Pair, Pair) {
        let (na, nb) = (perp(a), perp(b));
        let sum = add(na, nb);
        let sum_len = length(sum);
        let reversal = sum_len < 1e-6;
        let miter = if reversal {
            [0.0; 2]
        } else {
            scale(sum, 1.0 / sum_len)
        };
        // How far the miter reaches, for unit normals: one over the cosine of half the turn,
        // and the sum of the normals is twice that cosine. Near a U-turn, this stays right
        // where the dot product of the miter and a normal would be lost to rounding.
        let miter_len = if reversal {
            f32::INFINITY
        } else {
            2.0 / sum_len
        };

        let limit = match shape.join {
            LineJoin::Miter => shape.miter_limit.max(STRAIGHT_MITER),
            LineJoin::Round => shape.round_limit.max(STRAIGHT_MITER),
            LineJoin::Bevel => STRAIGHT_MITER,
        };
        if miter_len <= limit {
            let pair = self.pair(p, scale(miter, miter_len), distance);
            return (pair, pair);
        }

        let end = self.pair(p, na, distance);
        let start = self.pair(p, nb, distance);
        // Fill the gap on the outside of the bend: the right side when turning left.
        let (side, u0, u1) = if cross(a, b) > 0.0 {
            (1, scale(na, -1.0), scale(nb, -1.0))
        } else {
            (0, na, nb)
        };
        // A full reversal has no outside; go around the front of the line, like an end cap.
        let angle = if reversal {
            -PI
        } else {
            cross(u0, u1).atan2(dot(u0, u1))
        };
        let steps = match shape.join {
            LineJoin::Round => round_steps(angle),
            LineJoin::Miter | LineJoin::Bevel => 1,
        };
        // Offset lines turn the corner around where their two sides would meet, as far
        // as the join's limit allows.
        let center_offset = scale(miter, miter_len.min(limit));
        if shape.join == LineJoin::Round && !shape.offset {
            self.kites(p, distance, end[side], u0, angle, start[side], na);
            return (end, start);
        }
        let center = self.fan(
            p,
            distance,
            end[side],
            u0,
            angle,
            steps,
            start[side],
            center_offset,
            |f| rotate(na, angle * f),
        );
        if shape.offset {
            // An offset can move the inside of the bend outwards too, where the two
            // segments no longer overlap.
            self.bucket
                .triangles
                .extend([center, end[1 - side], start[1 - side]]);
        }
        (end, start)
    }
}

fn round_steps(angle: f32) -> u32 {
    (angle.abs() / ROUND_STEP).ceil().max(1.0) as u32
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: [f32; 2], s: f32) -> [f32; 2] {
    [a[0] * s, a[1] * s]
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn length(a: [f32; 2]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 2]) -> [f32; 2] {
    scale(a, 1.0 / length(a))
}

/// `a` turned a quarter turn, from the x axis towards the y axis.
fn perp(a: [f32; 2]) -> [f32; 2] {
    [-a[1], a[0]]
}

fn rotate(a: [f32; 2], angle: f32) -> [f32; 2] {
    let (sin, cos) = angle.sin_cos();
    [a[0] * cos - a[1] * sin, a[0] * sin + a[1] * cos]
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::{line_string, polygon};

    use super::*;

    fn layer(cap: LineCap, join: LineJoin) -> Shape {
        Shape {
            cap,
            join,
            miter_limit: 2.0,
            round_limit: 1.05,
            offset: false,
            line_metrics: false,
            clip: [0.0, 1.0],
        }
    }

    fn tessellate(shape: &Shape, geometry: Geometry<i32>) -> LineBucket {
        let mut bucket = LineBucket::default();
        LineTessellator::default().add_feature(&mut bucket, *shape, &geometry);
        bucket
    }

    /// Where vertex `i` ends up with every extrude scaled by `half_width` and every
    /// offset by `offset`.
    fn place(bucket: &LineBucket, i: u32, half_width: f32, offset: f32) -> [f32; 2] {
        let v = bucket.vertices[i as usize];
        add(
            add(v.position, scale(v.extrude, half_width)),
            scale(v.offset, offset),
        )
    }

    /// Area covered when every extrude is scaled by `half_width`, counting overlaps twice.
    fn area(bucket: &LineBucket, half_width: f32) -> f32 {
        let at = |i: u32| place(bucket, i, half_width, 0.0);
        bucket
            .triangles
            .chunks(3)
            .map(|t| cross(sub(at(t[1]), at(t[0])), sub(at(t[2]), at(t[0]))).abs() / 2.0)
            .sum()
    }

    fn assert_close(actual: f32, expected: f32) {
        assert!((actual - expected).abs() < 1e-3, "{actual} != {expected}");
    }

    #[test]
    fn straight_butt_line() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let bucket = tessellate(
            &l,
            Geometry::LineString(line_string![(x: 0, y: 0), (x: 10, y: 0)]),
        );
        assert_eq!(bucket.vertices.len(), 4);
        assert_eq!(bucket.triangles.len(), 6);
        assert_close(area(&bucket, 1.0), 20.0);
        assert_eq!(bucket.vertices[3].distance, 10.0);
    }

    #[test]
    fn collinear_and_duplicate_points_are_mitered() {
        let l = layer(LineCap::Butt, LineJoin::Round);
        let line = line_string![(x: 0, y: 0), (x: 5, y: 0), (x: 5, y: 0), (x: 10, y: 0)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        assert_eq!(bucket.vertices.len(), 6);
        assert_close(area(&bucket, 1.0), 20.0);
    }

    #[test]
    fn square_caps_extend_the_line() {
        let l = layer(LineCap::Square, LineJoin::Miter);
        let bucket = tessellate(
            &l,
            Geometry::LineString(line_string![(x: 0, y: 0), (x: 10, y: 0)]),
        );
        assert_close(area(&bucket, 1.0), 24.0);
        let along: Vec<f32> = bucket.vertices.iter().map(|v| v.along).collect();
        assert_eq!(along, [-1.0, -1.0, 1.0, 1.0]);
    }

    #[test]
    fn round_caps_are_quads_to_round_off() {
        let l = layer(LineCap::Round, LineJoin::Miter);
        let bucket = tessellate(
            &l,
            Geometry::LineString(line_string![(x: 0, y: 0), (x: 100, y: 0)]),
        );
        // The line, and a triangle past each end around its half circle.
        assert_eq!(bucket.triangles.len(), 3 * 4);
        assert_close(area(&bucket, 1.0), 200.0 + 2.0 * 2.0);
        // Only the caps' corners are rounded, the tips reaching past the ends: backwards at
        // the start, forwards at the end.
        let corners: Vec<_> = bucket.vertices.iter().filter(|v| v.round == 1.0).collect();
        assert_eq!(corners.len(), 6);
        for v in &corners {
            assert_close(length(v.extrude), 2f32.sqrt());
        }
        let along: Vec<f32> = corners.iter().map(|v| v.along).collect();
        assert_eq!(along.iter().filter(|&&a| a == 0.0).count(), 4);
        assert_close(along.iter().copied().fold(0.0, f32::min), -2f32.sqrt());
        assert_close(along.iter().copied().fold(0.0, f32::max), 2f32.sqrt());
        for v in &bucket.vertices {
            assert_close(v.along, v.extrude[0]);
        }
    }

    #[test]
    fn right_angle_miter() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        assert_eq!(bucket.vertices.len(), 6);
        let corner = bucket.vertices[2].extrude;
        assert_close(length(corner), 2f32.sqrt());
        // Two 10 by 2 rectangles that overlap in a 1 by 1 square at the bend, plus the square
        // the miter adds on the outside.
        assert_close(area(&bucket, 1.0), 20.0 + 20.0);
    }

    #[test]
    fn sharp_miter_becomes_bevel() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 0, y: 1)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        assert!(
            bucket
                .vertices
                .iter()
                .all(|v| length(v.extrude) <= 1.0 + 1e-6)
        );
        // Two cross sections at the join plus the bevel's center vertex.
        assert_eq!(bucket.vertices.len(), 2 + 4 + 1 + 2);
    }

    #[test]
    fn round_join_is_a_kite_to_round_off() {
        let l = layer(LineCap::Butt, LineJoin::Round);
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        // A quarter turn: the center and one corner, at the miter.
        assert_eq!(bucket.vertices.len(), 2 + 4 + 2 + 2);
        assert_eq!(bucket.triangles.len(), 3 * (2 + 2 + 2));
        let corners: Vec<_> = bucket.vertices.iter().filter(|v| v.round == 1.0).collect();
        assert_eq!(corners.len(), 1);
        assert_close(length(corners[0].extrude), 2f32.sqrt());
        assert_close(place(&bucket, 6 + 1, 1.0, 0.0)[0], 11.0);
        assert_close(place(&bucket, 6 + 1, 1.0, 0.0)[1], -1.0);
        // Two 10 by 2 rectangles, plus the kite's 1 by 1 square on the outside.
        assert_close(area(&bucket, 1.0), 20.0 + 20.0 + 1.0);
    }

    #[test]
    fn sharp_round_joins_take_more_kites() {
        let l = layer(LineCap::Butt, LineJoin::Round);
        // Nearly turning back: more than a third of a turn takes two kites.
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 0, y: 1)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        let corners: Vec<_> = bucket.vertices.iter().filter(|v| v.round == 1.0).collect();
        assert_eq!(corners.len(), 2);
        assert!(corners.iter().all(|v| length(v.extrude) <= 2.0));
    }

    #[test]
    fn near_u_turns_stay_within_the_line() {
        // A shape where the normals nearly cancel, and rounding used to make the miter
        // negative and thousands of half widths long, which drew a line across the map.
        for (cap, join) in [
            (LineCap::Butt, LineJoin::Miter),
            (LineCap::Round, LineJoin::Round),
            (LineCap::Butt, LineJoin::Bevel),
        ] {
            let l = layer(cap, join);
            let line = line_string![(x: 0, y: 0), (x: 136, y: 379), (x: 122, y: 340)];
            let bucket = tessellate(&l, Geometry::LineString(line));
            assert!(
                bucket.vertices.iter().all(|v| length(v.extrude) <= 2.0),
                "{:?}",
                bucket
                    .vertices
                    .iter()
                    .map(|v| v.extrude)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn offsets_move_both_sides_the_same_way() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let bucket = tessellate(
            &l,
            Geometry::LineString(line_string![(x: 0, y: 0), (x: 10, y: 0)]),
        );
        // A positive offset moves an eastward line south (+y), to its right.
        let ys: Vec<f32> = (0..4).map(|i| place(&bucket, i, 1.0, 3.0)[1]).collect();
        assert_eq!(ys, [4.0, 2.0, 4.0, 2.0]);
        assert_close(area(&bucket, 1.0), 20.0);
    }

    #[test]
    fn offset_miter_corner_stays_on_the_offset_lines() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10)];
        let bucket = tessellate(&l, Geometry::LineString(line));
        // East then south: offsetting by 2 to the right (south, then west) meets at (8, 2).
        let corner = place(&bucket, 2, 0.0, 2.0);
        assert_close(corner[0], 8.0);
        assert_close(corner[1], 2.0);
    }

    #[test]
    fn offset_bevels_fill_both_sides() {
        let mut l = layer(LineCap::Butt, LineJoin::Bevel);
        let line = line_string![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10)];
        let plain = tessellate(&l, Geometry::LineString(line.clone()));
        l.offset = true;
        let offset = tessellate(&l, Geometry::LineString(line));
        assert_eq!(offset.triangles.len(), plain.triangles.len() + 3);
    }

    #[test]
    fn line_progress_needs_line_metrics() {
        let mut l = layer(LineCap::Round, LineJoin::Round);
        let line = Geometry::LineString(line_string![(x: 0, y: 0), (x: 30, y: 0), (x: 30, y: 10)]);
        assert!(
            tessellate(&l, line.clone())
                .vertices
                .iter()
                .all(|v| v.progress == 0.0)
        );
        l.line_metrics = true;
        let bucket = tessellate(&l, line);
        let progress: Vec<f32> = bucket.vertices.iter().map(|v| v.progress).collect();
        assert_eq!(progress.first(), Some(&0.0));
        assert_eq!(progress.last(), Some(&1.0));
        assert!(progress.contains(&0.75), "the join is three quarters along");

        // A piece of a longer line, as GeoJSON tiling cuts it.
        l.clip = [0.5, 0.7];
        let bucket = tessellate(
            &l,
            Geometry::LineString(line_string![(x: 0, y: 0), (x: 10, y: 0)]),
        );
        let progress: Vec<f32> = bucket.vertices.iter().map(|v| v.progress).collect();
        assert_eq!(progress.first(), Some(&0.5));
        assert!((progress.last().unwrap() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn polygon_rings_are_closed() {
        let l = layer(LineCap::Round, LineJoin::Miter);
        let square = polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10)];
        let bucket = tessellate(&l, Geometry::Polygon(square));
        // Four mitered corners plus the copy that closes the ring; caps don't apply.
        assert_eq!(bucket.vertices.len(), 2 * 5);
        assert_eq!(bucket.triangles.len(), 6 * 4);
        let last = bucket.vertices.last().unwrap();
        assert_eq!(last.position, [0.0, 0.0]);
        assert_eq!(last.distance, 40.0);
        // A 12 by 12 square with an 8 by 8 hole.
        assert_close(area(&bucket, 1.0), 144.0 - 64.0);
    }

    #[test]
    fn points_are_ignored() {
        let l = layer(LineCap::Butt, LineJoin::Miter);
        let bucket = tessellate(&l, Geometry::Point((1, 2).into()));
        assert!(bucket.vertices.is_empty());
    }
}
