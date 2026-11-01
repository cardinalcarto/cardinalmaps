//! `heatmap` layers: one quad per point, which the renderer scales to the point's kernel
//! (`heatmap-radius`) and splats additively into a density texture.
//!
//! As in MapLibre, points outside the tile itself (in its buffer) are dropped: the density
//! pass isn't clipped to tiles, so a buffered point would be counted once by each tile that
//! holds it, and its kernel can spill over tile edges anyway.

use bytemuck::{Pod, Zeroable};
use mlt_core::geo_types::{Geometry, Point};

use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::style::HeatmapLayer;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct HeatmapVertex {
    /// The point, in tile coordinates (`0..extent`).
    pub position: [f32; 2],
    /// This corner of the point's quad: each component is -1 or 1. The vertex shader
    /// scales it by the kernel's extent in tile units.
    pub extrude: [f32; 2],
}

#[derive(Debug, Clone, Default)]
pub struct HeatmapBucket {
    /// Four per point.
    pub vertices: Vec<HeatmapVertex>,
    /// Triangle list indices into [`Self::vertices`], two triangles per point.
    pub indices: Vec<u32>,
    /// Per-vertex values of [`HeatmapPaint::attributes`](crate::style::HeatmapPaint::attributes).
    pub paint: PaintArrays,
}

impl HeatmapBucket {
    /// Add a quad for every point of `geometry` that lies inside the tile; other geometry
    /// types are ignored, as in MapLibre. Returns whether anything was added.
    pub fn add_points(&mut self, geometry: &Geometry<i32>, extent: u32) -> bool {
        let points = match geometry {
            Geometry::Point(p) => std::slice::from_ref(p),
            Geometry::MultiPoint(mp) => mp.0.as_slice(),
            _ => return false,
        };
        let before = self.vertices.len();
        let inside = |p: &&Point<i32>| {
            let extent = extent as i32;
            (0..extent).contains(&p.x()) && (0..extent).contains(&p.y())
        };
        for p in points.iter().filter(inside) {
            let base = self.vertices.len() as u32;
            let position = [p.x() as f32, p.y() as f32];
            self.vertices.extend(
                [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]]
                    .map(|extrude| HeatmapVertex { position, extrude }),
            );
            self.indices.extend([0, 1, 2, 2, 1, 3].map(|i| base + i));
        }
        self.vertices.len() > before
    }
}

pub(crate) fn builder<'a>(
    layer: &'a HeatmapLayer,
    cx: &BuildContext<'a>,
    _scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    Some(Box::new(HeatmapBuilder {
        layer,
        extent: cx.extent,
        bucket: HeatmapBucket {
            paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
            ..Default::default()
        },
    }))
}

struct HeatmapBuilder<'a> {
    layer: &'a HeatmapLayer,
    extent: u32,
    bucket: HeatmapBucket,
}

impl LayerBuilder for HeatmapBuilder<'_> {
    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let bucket = &mut self.bucket;
        if bucket.add_points(feature.geometry, self.extent) {
            bucket.paint.populate(
                &self.layer.paint.attributes(),
                feature.ctx,
                feature.id,
                bucket.vertices.len(),
            );
        }
    }

    fn finish(self: Box<Self>, _scratch: &mut Scratch) -> Option<BucketGeometry> {
        (!self.bucket.vertices.is_empty()).then_some(BucketGeometry::Heatmap(self.bucket))
    }
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::{LineString, MultiPoint};

    use super::*;

    #[test]
    fn a_quad_per_point_inside_the_tile() {
        let mut bucket = HeatmapBucket::default();
        let points = MultiPoint(vec![
            Point::new(10, 20),
            Point::new(-5, 20),
            Point::new(4095, 0),
            Point::new(4096, 0),
        ]);
        assert!(bucket.add_points(&Geometry::MultiPoint(points), 4096));
        assert_eq!(bucket.vertices.len(), 8, "only the two points inside");
        assert_eq!(bucket.indices, [0, 1, 2, 2, 1, 3, 4, 5, 6, 6, 5, 7]);
        assert!(
            bucket.vertices[..4]
                .iter()
                .all(|v| v.position == [10.0, 20.0])
        );
        assert_eq!(bucket.vertices[7].position, [4095.0, 0.0]);
        let corners: Vec<_> = bucket.vertices[..4].iter().map(|v| v.extrude).collect();
        assert_eq!(
            corners,
            [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]]
        );
    }

    #[test]
    fn other_geometry_and_buffered_points_add_nothing() {
        let mut bucket = HeatmapBucket::default();
        let line = Geometry::LineString(LineString::from(vec![(0, 0), (10, 10)]));
        assert!(!bucket.add_points(&line, 4096));
        assert!(!bucket.add_points(&Geometry::Point(Point::new(-1, 3)), 4096));
        assert!(bucket.vertices.is_empty());
    }
}
