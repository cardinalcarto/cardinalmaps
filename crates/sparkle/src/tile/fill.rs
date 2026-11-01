//! Polygon fills: triangles for the interior, line segments for the outline.

use bytemuck::{Pod, Zeroable};
use earcut::Earcut;
use mlt_core::geo_types::{Coord, Geometry, LineString, Polygon};

use super::sort_key::SortKeys;
use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::pattern::FeatureImage;
use crate::sprite::ImageAtlas;
use crate::style::FillLayer;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FillVertex {
    /// In tile coordinates, `0..extent` across the tile.
    pub position: [f32; 2],
}

#[derive(Debug, Clone, Default)]
pub struct FillBucket {
    pub vertices: Vec<FillVertex>,
    /// Triangle list indices into [`Self::vertices`].
    pub triangles: Vec<u32>,
    /// Line list indices into [`Self::vertices`], one pair per ring edge, for
    /// `fill-antialias` and `fill-outline-color`.
    pub outlines: Vec<u32>,
    /// Per-vertex values of [`FillPaint::attributes`](crate::style::FillPaint::attributes).
    pub paint: PaintArrays,
    /// Per vertex, when `fill-pattern` differs per feature: its sprite image (see
    /// [`FeatureImage::pattern`]). Empty when the renderer evaluates it for the layer.
    pub images: Vec<FeatureImage>,
}

/// Triangles stored in the tile for one polygon feature.
#[derive(Clone, Copy)]
pub(crate) struct PreTessellated<'a> {
    /// Indices into the layer's vertex sequence.
    pub indices: &'a [u32],
    /// The layer vertex the feature's own vertices start at.
    pub first_vertex: u32,
    pub vertex_count: usize,
}

pub(crate) fn builder<'a>(
    layer: &'a FillLayer,
    cx: &BuildContext<'a>,
    scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    Some(Box::new(FillBuilder {
        layer,
        bucket: FillBucket {
            paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
            ..Default::default()
        },
        tessellator: scratch.take(),
        patterns: layer.paint.pattern.is_feature_dependent(),
        atlas: cx.resources.images,
        sort_keys: SortKeys::new(&layer.layout.sort_key),
    }))
}

struct FillBuilder<'a> {
    layer: &'a FillLayer,
    bucket: FillBucket,
    tessellator: FillTessellator,
    /// Whether `fill-pattern` is resolved per feature.
    patterns: bool,
    atlas: Option<&'a ImageAtlas>,
    sort_keys: Option<SortKeys>,
}

impl LayerBuilder for FillBuilder<'_> {
    fn wants_triangles(&self) -> bool {
        true
    }

    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let bucket = &mut self.bucket;
        let (first_triangle, first_outline) = (bucket.triangles.len(), bucket.outlines.len());
        if !self
            .tessellator
            .add_feature(bucket, feature.geometry, feature.pre_tessellated)
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
        if self.patterns {
            let name = paint.pattern.evaluate(feature.ctx);
            let image = name.and_then(|n| self.atlas?.resolve(&n));
            bucket
                .images
                .resize(bucket.vertices.len(), FeatureImage::pattern(image));
        }
        if let Some(keys) = &mut self.sort_keys {
            let key = self.layer.layout.sort_key.evaluate(feature.ctx);
            keys.push(
                key,
                [
                    first_triangle..bucket.triangles.len(),
                    first_outline..bucket.outlines.len(),
                ],
            );
        }
    }

    fn finish(mut self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry> {
        scratch.put(self.tessellator);
        if let Some(keys) = self.sort_keys {
            let bucket = &mut self.bucket;
            keys.apply([&mut bucket.triangles, &mut bucket.outlines]);
        }
        (!self.bucket.vertices.is_empty()).then_some(BucketGeometry::Fill(self.bucket))
    }
}

#[derive(Default)]
pub(crate) struct FillTessellator {
    earcut: Earcut<f64>,
    holes: Vec<u32>,
    triangles: Vec<u32>,
}

impl FillTessellator {
    /// Add a polygon or multi-polygon; anything else is ignored. Returns whether vertices
    /// were added.
    pub fn add_feature(
        &mut self,
        bucket: &mut FillBucket,
        geometry: &Geometry<i32>,
        pre_tessellated: Option<PreTessellated<'_>>,
    ) -> bool {
        let polygons = match geometry {
            Geometry::Polygon(p) => std::slice::from_ref(p),
            Geometry::MultiPolygon(mp) => mp.0.as_slice(),
            _ => return false,
        };
        let first_vertex = bucket.vertices.len();
        let first_outline = bucket.outlines.len();

        let used_stored = match pre_tessellated {
            Some(pre) => self.add_pre_tessellated(bucket, polygons, pre),
            None => false,
        };
        if !used_stored {
            bucket.vertices.truncate(first_vertex);
            bucket.outlines.truncate(first_outline);
            for polygon in polygons {
                self.add_polygon(bucket, polygon);
            }
        }
        bucket.vertices.len() > first_vertex
    }

    fn add_polygon(&mut self, bucket: &mut FillBucket, polygon: &Polygon<i32>) {
        let base = bucket.vertices.len();
        self.holes.clear();
        for (i, ring) in rings(polygon).enumerate() {
            let ring = open_ring(ring);
            if ring.len() < 3 {
                // A degenerate exterior means there is no polygon; a degenerate hole can be dropped.
                if i == 0 {
                    return;
                }
                continue;
            }
            if i > 0 {
                self.holes.push((bucket.vertices.len() - base) as u32);
            }
            push_ring(bucket, ring);
        }

        let vertices = &bucket.vertices[base..];
        let data = vertices.iter().map(|v| v.position.map(f64::from));
        self.earcut.earcut(data, &self.holes, &mut self.triangles);
        let base = base as u32;
        bucket
            .triangles
            .extend(self.triangles.iter().map(|&i| i + base));
    }

    /// Use the triangles the tile stores, if they fit the feature's geometry; returns
    /// whether they did. Vertices and outlines are added either way.
    fn add_pre_tessellated(
        &mut self,
        bucket: &mut FillBucket,
        polygons: &[Polygon<i32>],
        pre: PreTessellated<'_>,
    ) -> bool {
        let base = bucket.vertices.len();
        // The stored indices count every ring vertex but the closing one, in ring order.
        for ring in polygons.iter().flat_map(rings) {
            push_ring(bucket, open_ring(ring));
        }
        let count = bucket.vertices.len() - base;
        if count != pre.vertex_count {
            return false;
        }
        let in_range = |&i: &u32| {
            i.checked_sub(pre.first_vertex)
                .is_some_and(|i| (i as usize) < count)
        };
        if !pre.indices.iter().all(in_range) {
            return false;
        }
        let shift = base as u32;
        bucket
            .triangles
            .extend(pre.indices.iter().map(|&i| i - pre.first_vertex + shift));
        true
    }
}

fn rings(p: &Polygon<i32>) -> impl Iterator<Item = &LineString<i32>> {
    std::iter::once(p.exterior()).chain(p.interiors())
}

/// The ring's vertices without the closing repeat of the first.
fn open_ring(ring: &LineString<i32>) -> &[Coord<i32>] {
    match ring.0.as_slice() {
        [first, rest @ .., last] if first == last => &ring.0[..rest.len() + 1],
        all => all,
    }
}

fn push_ring(bucket: &mut FillBucket, ring: &[Coord<i32>]) {
    let start = bucket.vertices.len() as u32;
    let n = ring.len() as u32;
    bucket.vertices.extend(ring.iter().map(|c| FillVertex {
        position: [c.x as f32, c.y as f32],
    }));
    if n >= 2 {
        bucket
            .outlines
            .extend((0..n).flat_map(|i| [start + i, start + (i + 1) % n]));
    }
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::{MultiPolygon, polygon};

    use super::*;

    fn area(bucket: &FillBucket) -> f32 {
        bucket
            .triangles
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| bucket.vertices[t[i] as usize].position);
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum()
    }

    fn square_with_hole() -> Polygon<i32> {
        polygon!(
            exterior: [(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)],
            interiors: [[(x: 2, y: 2), (x: 4, y: 2), (x: 4, y: 4), (x: 2, y: 4), (x: 2, y: 2)]],
        )
    }

    #[test]
    fn polygon_with_hole() {
        let mut bucket = FillBucket::default();
        let geometry = Geometry::Polygon(square_with_hole());
        assert!(FillTessellator::default().add_feature(&mut bucket, &geometry, None));
        assert_eq!(bucket.vertices.len(), 8, "closing vertices are dropped");
        assert_eq!(area(&bucket), 96.0);
        assert_eq!(bucket.outlines.len(), 16, "one pair per edge");
    }

    #[test]
    fn multipolygon_offsets_indices() {
        let mut bucket = FillBucket::default();
        let a = polygon![(x: 0, y: 0), (x: 1, y: 0), (x: 1, y: 1)];
        let b = polygon![(x: 5, y: 5), (x: 7, y: 5), (x: 7, y: 7)];
        let geometry = Geometry::MultiPolygon(MultiPolygon(vec![a, b]));
        FillTessellator::default().add_feature(&mut bucket, &geometry, None);
        assert_eq!(bucket.triangles.len(), 6);
        assert_eq!(area(&bucket), 0.5 + 2.0);
        assert!(bucket.triangles[3..].iter().all(|&i| i >= 3));
    }

    #[test]
    fn mismatched_pre_tessellation_falls_back() {
        let mut bucket = FillBucket::default();
        let geometry = Geometry::Polygon(square_with_hole());
        let pre = PreTessellated {
            indices: &[0, 1, 2],
            first_vertex: 100,
            vertex_count: 8,
        };
        FillTessellator::default().add_feature(&mut bucket, &geometry, Some(pre));
        assert_eq!(bucket.vertices.len(), 8);
        assert_eq!(area(&bucket), 96.0);
    }

    #[test]
    fn points_are_ignored() {
        let mut bucket = FillBucket::default();
        let geometry = Geometry::Point((1, 2).into());
        assert!(!FillTessellator::default().add_feature(&mut bucket, &geometry, None));
    }
}
