//! `circle` layers: a quad per point, which the vertex shader sizes and the fragment shader
//! rounds into a disc.
//!
//! Each point becomes four vertices at the same position with a different `extrude`
//! corner. The shader moves each corner out by the circle's radius plus stroke width, in
//! screen or map pixels depending on `circle-pitch-alignment` and `circle-pitch-scale`, so
//! radii never need processing again when the camera moves.

use bytemuck::{Pod, Zeroable};
use mlt_core::geo_types::{Coord, Geometry, LineString};

use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::style::CircleLayer;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct CircleVertex {
    /// The circle's center, in tile coordinates.
    pub position: [f32; 2],
    /// The quad corner: each component is -1 or 1.
    pub extrude: [f32; 2],
}

/// Every circle of a style layer in one tile.
#[derive(Debug, Clone, Default)]
pub struct CircleBucket {
    /// Four per circle, one per corner.
    pub vertices: Vec<CircleVertex>,
    /// Triangle list indices into [`Self::vertices`], two triangles per circle, in draw
    /// order: by `circle-sort-key` when the layer has one, else in feature order.
    pub triangles: Vec<u32>,
    /// Per-vertex values of
    /// [`CirclePaint::attributes`](crate::style::CirclePaint::attributes).
    pub paint: PaintArrays,
}

pub(crate) fn builder<'a>(
    layer: &'a CircleLayer,
    cx: &BuildContext<'a>,
    _scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    let sorted = !matches!(layer.layout.sort_key.as_constant(), Some(None));
    Some(Box::new(CircleBuilder {
        layer,
        extent: cx.extent as i32,
        bucket: CircleBucket {
            paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
            ..Default::default()
        },
        sort_keys: sorted.then(Vec::new),
    }))
}

struct CircleBuilder<'a> {
    layer: &'a CircleLayer,
    extent: i32,
    bucket: CircleBucket,
    /// With `circle-sort-key`: each feature's key and the run of `triangles` it added.
    sort_keys: Option<Vec<(f32, std::ops::Range<usize>)>>,
}

impl LayerBuilder for CircleBuilder<'_> {
    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let bucket = &mut self.bucket;
        let first_index = bucket.triangles.len();
        if !add_circles(bucket, feature.geometry, self.extent) {
            return;
        }
        bucket.paint.populate(
            &self.layer.paint.attributes(),
            feature.ctx,
            feature.id,
            bucket.vertices.len(),
        );
        if let Some(keys) = &mut self.sort_keys {
            // Features whose key evaluates to nothing sort as 0.
            let key = self.layer.layout.sort_key.evaluate(feature.ctx);
            keys.push((key.unwrap_or(0.0), first_index..bucket.triangles.len()));
        }
    }

    fn finish(mut self: Box<Self>, _scratch: &mut Scratch) -> Option<BucketGeometry> {
        if self.bucket.vertices.is_empty() {
            return None;
        }
        if let Some(mut keys) = self.sort_keys.take() {
            // Stable, so features with equal keys keep their order in the tile.
            keys.sort_by(|a, b| a.0.total_cmp(&b.0));
            let old = &self.bucket.triangles;
            let sorted = keys.iter().flat_map(|(_, run)| &old[run.clone()]).copied();
            self.bucket.triangles = sorted.collect();
        }
        Some(BucketGeometry::Circle(self.bucket))
    }
}

/// Add a circle for every point of `geometry` inside the tile, `0..extent` on both axes.
/// Points in the buffer around the tile are left to the neighboring tile, so circles
/// overlapping a tile edge are drawn once, whole, without clipping. Lines and polygons get
/// a circle at each vertex, as in MapLibre (without repeating a ring's closing vertex).
/// Returns whether any circle was added.
fn add_circles(bucket: &mut CircleBucket, geometry: &Geometry<i32>, extent: i32) -> bool {
    let first_vertex = bucket.vertices.len();
    let mut add = |c: &Coord<i32>| {
        if (0..extent).contains(&c.x) && (0..extent).contains(&c.y) {
            push_quad(bucket, [c.x as f32, c.y as f32]);
        }
    };
    match geometry {
        Geometry::Point(p) => add(&p.0),
        Geometry::MultiPoint(mp) => mp.0.iter().for_each(|p| add(&p.0)),
        Geometry::LineString(l) => l.0.iter().for_each(add),
        Geometry::MultiLineString(ml) => ml.0.iter().flat_map(|l| &l.0).for_each(add),
        Geometry::Polygon(p) => std::iter::once(p.exterior())
            .chain(p.interiors())
            .flat_map(ring)
            .for_each(add),
        Geometry::MultiPolygon(mp) => {
            mp.0.iter()
                .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                .flat_map(ring)
                .for_each(add)
        }
        _ => {}
    }
    bucket.vertices.len() > first_vertex
}

/// The ring's vertices without the closing repeat of the first.
fn ring(r: &LineString<i32>) -> &[Coord<i32>] {
    match r.0.as_slice() {
        [first, rest @ .., last] if first == last => &r.0[..rest.len() + 1],
        all => all,
    }
}

fn push_quad(bucket: &mut CircleBucket, position: [f32; 2]) {
    let base = bucket.vertices.len() as u32;
    bucket.vertices.extend(
        [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
            .map(|extrude| CircleVertex { position, extrude }),
    );
    bucket
        .triangles
        .extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::{MultiPoint, Point, polygon};

    use super::*;

    #[test]
    fn point_becomes_quad() {
        let mut bucket = CircleBucket::default();
        assert!(add_circles(
            &mut bucket,
            &Geometry::Point(Point::new(5, 7)),
            4096
        ));
        assert_eq!(bucket.vertices.len(), 4);
        assert!(bucket.vertices.iter().all(|v| v.position == [5.0, 7.0]));
        let corners: Vec<_> = bucket.vertices.iter().map(|v| v.extrude).collect();
        assert_eq!(
            corners,
            [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
        );
        assert_eq!(bucket.triangles, [0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn points_outside_the_tile_are_skipped() {
        let mut bucket = CircleBucket::default();
        let points = MultiPoint(vec![
            Point::new(0, 0),
            Point::new(-1, 10),
            Point::new(10, 4096),
            Point::new(4095, 4095),
        ]);
        assert!(add_circles(
            &mut bucket,
            &Geometry::MultiPoint(points),
            4096
        ));
        assert_eq!(bucket.vertices.len(), 8);
        assert_eq!(&bucket.triangles[6..], [4, 5, 6, 4, 6, 7]);

        let outside = Geometry::Point(Point::new(-5, -5));
        assert!(!add_circles(&mut bucket, &outside, 4096));
    }

    #[test]
    fn polygon_vertices_without_closing_repeat() {
        let mut bucket = CircleBucket::default();
        let p = polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 0)];
        add_circles(&mut bucket, &Geometry::Polygon(p), 4096);
        assert_eq!(bucket.vertices.len(), 3 * 4);
    }
}
