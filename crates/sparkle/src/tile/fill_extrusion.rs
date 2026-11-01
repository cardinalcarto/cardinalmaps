//! `fill-extrusion` layers: polygons raised into prisms, with a roof at
//! `fill-extrusion-height` and walls down to `fill-extrusion-base`.
//!
//! Heights are per-feature paint values that can change with zoom, so vertices don't carry
//! a height: each has a `top` flag, and the vertex shader puts it at the feature's height or
//! base. Normals are computed here as MapLibre computes them, so that the shader's lighting
//! (see [`crate::light`]) matches MapLibre's.

use bytemuck::{Pod, Zeroable};
use mlt_core::geo_types::{Coord, Geometry, LineString};

use super::fill::{FillBucket, FillTessellator, PreTessellated};
use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::sprite::ImageAtlas;
use crate::style::FillExtrusionLayer;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FillExtrusionVertex {
    /// In tile coordinates.
    pub position: [f32; 2],
    /// For walls, the distance along the ring from where its wall starts, in tile units;
    /// the horizontal coordinate of `fill-extrusion-pattern` on walls. Zero on roofs.
    pub edge_distance: f32,
    /// A unit normal in tile coordinates (y down): `[0, 0, 1]` on roofs, horizontal on
    /// walls.
    ///
    /// As in MapLibre, wall normals point *into* the building (MapLibre takes
    /// `(p1 - p0).perp()` for an edge from `p0` to `p1` of a correctly wound ring). Its light
    /// direction is mirrored the same way, so keep both as they are.
    pub normal: [f32; 3],
    /// 1 for vertices at the feature's height, 0 for those at its base.
    pub top: f32,
}

/// Where a feature's `fill-extrusion-pattern` image is, repeated for each of its vertices.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default, Pod, Zeroable)]
pub struct FillExtrusionPattern {
    /// `[x, y, width, height]` in sprite atlas pixels; all zeros where the feature has no
    /// pattern (or its image is missing), which draws it with `fill-extrusion-color`.
    pub rect: [f32; 4],
    /// The image's size in logical pixels: how far apart its repeats are.
    pub size: [f32; 2],
}

/// Roofs and walls of every extruded feature of a style layer in one tile.
#[derive(Debug, Clone, Default)]
pub struct FillExtrusionBucket {
    pub vertices: Vec<FillExtrusionVertex>,
    /// Triangle list indices into [`Self::vertices`]. Triangles aren't consistently wound,
    /// so draw them without back-face culling.
    pub triangles: Vec<u32>,
    /// One per vertex when the layer has a `fill-extrusion-pattern` and there are sprites
    /// to take it from; empty otherwise.
    pub patterns: Vec<FillExtrusionPattern>,
    /// Per-vertex values of
    /// [`FillExtrusionPaint::attributes`](crate::style::FillExtrusionPaint::attributes).
    pub paint: PaintArrays,
}

pub(crate) fn builder<'a>(
    layer: &'a FillExtrusionLayer,
    cx: &BuildContext<'a>,
    scratch: &mut Scratch,
) -> Option<Box<dyn LayerBuilder + 'a>> {
    let has_pattern = !matches!(layer.paint.pattern.as_constant(), Some(None));
    Some(Box::new(FillExtrusionBuilder {
        layer,
        bucket: FillExtrusionBucket {
            paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
            ..Default::default()
        },
        extent: cx.extent as i32,
        images: cx.resources.images.filter(|_| has_pattern),
        roofs: scratch.take(),
    }))
}

struct FillExtrusionBuilder<'a> {
    layer: &'a FillExtrusionLayer,
    bucket: FillExtrusionBucket,
    extent: i32,
    /// Where patterns come from, when the layer has one.
    images: Option<&'a ImageAtlas>,
    roofs: RoofScratch,
}

/// Roofs are tessellated as `fill` polygons are, into a scratch fill bucket.
#[derive(Default)]
struct RoofScratch {
    tessellator: FillTessellator,
    fill: FillBucket,
}

impl LayerBuilder for FillExtrusionBuilder<'_> {
    fn wants_triangles(&self) -> bool {
        true
    }

    fn add_feature(&mut self, feature: &mut FeatureInput<'_, '_>) {
        let bucket = &mut self.bucket;
        let first_vertex = bucket.vertices.len();
        add_extrusion(
            bucket,
            &mut self.roofs,
            feature.geometry,
            feature.pre_tessellated,
            self.extent,
        );
        if bucket.vertices.len() == first_vertex {
            return;
        }
        if let Some(images) = self.images {
            let pattern = self
                .layer
                .paint
                .pattern
                .evaluate(feature.ctx)
                .and_then(|name| images.resolve(&name))
                .map(|image| FillExtrusionPattern {
                    rect: image.rect.map(|v| v as f32),
                    size: image.logical_size(),
                })
                .unwrap_or_default();
            bucket.patterns.resize(bucket.vertices.len(), pattern);
        }
        bucket.paint.populate(
            &self.layer.paint.attributes(),
            feature.ctx,
            feature.id,
            bucket.vertices.len(),
        );
    }

    fn finish(self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry> {
        scratch.put(self.roofs);
        (!self.bucket.vertices.is_empty()).then_some(BucketGeometry::FillExtrusion(self.bucket))
    }
}

/// Add the roof and walls of a polygon or multi-polygon; anything else is ignored.
fn add_extrusion(
    bucket: &mut FillExtrusionBucket,
    roofs: &mut RoofScratch,
    geometry: &Geometry<i32>,
    pre_tessellated: Option<PreTessellated<'_>>,
    extent: i32,
) {
    let polygons = match geometry {
        Geometry::Polygon(p) => std::slice::from_ref(p),
        Geometry::MultiPolygon(mp) => mp.0.as_slice(),
        _ => return,
    };

    let fill = &mut roofs.fill;
    fill.vertices.clear();
    fill.triangles.clear();
    fill.outlines.clear();
    if !roofs
        .tessellator
        .add_feature(fill, geometry, pre_tessellated)
    {
        return;
    }
    let base = bucket.vertices.len() as u32;
    bucket
        .vertices
        .extend(fill.vertices.iter().map(|v| FillExtrusionVertex {
            position: v.position,
            edge_distance: 0.0,
            normal: [0.0, 0.0, 1.0],
            top: 1.0,
        }));
    bucket
        .triangles
        .extend(fill.triangles.iter().map(|&i| i + base));

    for polygon in polygons {
        add_walls(bucket, polygon.exterior(), false, extent);
        for hole in polygon.interiors() {
            add_walls(bucket, hole, true, extent);
        }
    }
}

/// A quad per ring edge, except for edges along the tile's clipped border: the neighbouring
/// tile's copy of the polygon continues there, so the wall would be inside the building.
fn add_walls(bucket: &mut FillExtrusionBucket, ring: &LineString<i32>, hole: bool, extent: i32) {
    // Rings may or may not repeat their first vertex at the end; walk them as closed.
    let points = match ring.0.as_slice() {
        [first, rest @ .., last] if first == last => &ring.0[..rest.len() + 1],
        all => all,
    };
    if points.len() < 2 {
        return;
    }
    let edges = || points.iter().zip(points.iter().cycle().skip(1));
    // `perp` points into a ring wound as tiles should wind exteriors (positive area with y
    // down) and out of one wound the other way. Normals point into the building, as
    // MapLibre's do for correctly wound tiles, whichever way this ring is wound.
    let area: i64 = edges()
        .map(|(a, b)| i64::from(a.x) * i64::from(b.y) - i64::from(b.x) * i64::from(a.y))
        .sum();
    let sign = if (area > 0) != hole { 1.0 } else { -1.0 };
    let mut edge_distance = 0.0;
    for (&p0, &p1) in edges() {
        if p0 == p1 || is_boundary_edge(p0, p1, extent) {
            continue;
        }
        let d = [(p1.x - p0.x) as f32, (p1.y - p0.y) as f32];
        let length = d[0].hypot(d[1]);
        let normal = [-d[1] / length * sign, d[0] / length * sign, 0.0];
        let first = bucket.vertices.len() as u32;
        for (p, distance) in [(p1, edge_distance), (p0, edge_distance + length)] {
            for top in [0.0, 1.0] {
                bucket.vertices.push(FillExtrusionVertex {
                    position: [p.x as f32, p.y as f32],
                    edge_distance: distance,
                    normal,
                    top,
                });
            }
        }
        edge_distance += length;
        bucket
            .triangles
            .extend([0, 2, 1, 1, 2, 3].map(|i| first + i));
    }
}

/// Whether the edge runs along a side of the tile, outside its `0..extent` area (where
/// polygons are clipped to the tile's buffer).
fn is_boundary_edge(a: Coord<i32>, b: Coord<i32>, extent: i32) -> bool {
    let outside = |v: i32| v < 0 || v > extent;
    (a.x == b.x && outside(a.x)) || (a.y == b.y && outside(a.y))
}

#[cfg(test)]
mod tests {
    use mlt_core::geo_types::{MultiPolygon, Polygon, polygon};

    use super::*;

    fn extrude(geometry: &Geometry<i32>) -> FillExtrusionBucket {
        let mut bucket = FillExtrusionBucket::default();
        add_extrusion(
            &mut bucket,
            &mut RoofScratch::default(),
            geometry,
            None,
            4096,
        );
        bucket
    }

    fn roof_vertices(b: &FillExtrusionBucket) -> usize {
        b.vertices
            .iter()
            .filter(|v| v.normal == [0.0, 0.0, 1.0])
            .count()
    }

    #[test]
    fn square_has_roof_and_four_walls() {
        let square =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)];
        let b = extrude(&Geometry::Polygon(square));
        assert_eq!(roof_vertices(&b), 4);
        assert_eq!(b.vertices.len(), 4 + 4 * 4);
        assert_eq!(b.triangles.len(), 2 * 3 + 4 * 6);
        assert!(b.triangles.iter().all(|&i| (i as usize) < b.vertices.len()));

        // The first edge runs east along the top (north) side; MapLibre's normal for it
        // points south, into the square.
        let wall = &b.vertices[4..8];
        assert!(wall.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
        assert_eq!(
            wall.iter().map(|v| v.top).collect::<Vec<_>>(),
            [0.0, 1.0, 0.0, 1.0]
        );
        // Edge distances accumulate around the ring.
        assert_eq!(wall[0].edge_distance, 0.0);
        assert_eq!(wall[2].edge_distance, 10.0);
        assert_eq!(b.vertices.last().unwrap().edge_distance, 40.0);
    }

    /// Each wall's normal and the midpoint of its bottom edge.
    fn walls(b: &FillExtrusionBucket) -> Vec<([f32; 3], [f32; 2])> {
        b.vertices
            .iter()
            .filter(|v| v.normal[2] == 0.0)
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|q| {
                let [a, c] = [q[0].position, q[2].position];
                (q[0].normal, [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0])
            })
            .collect()
    }

    #[test]
    fn normals_point_into_the_building_whatever_the_winding() {
        let ring = [(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)];
        let hole = [(3, 3), (3, 7), (7, 7), (7, 3), (3, 3)];
        let reversed = |r: &[(i32, i32)]| r.iter().rev().copied().collect::<Vec<_>>();
        for (exterior, interior) in [
            (ring.to_vec(), hole.to_vec()),
            (reversed(&ring), reversed(&hole)),
        ] {
            let polygon =
                Polygon::new(LineString::from(exterior), vec![LineString::from(interior)]);
            let b = extrude(&Geometry::Polygon(polygon));
            let walls = walls(&b);
            assert_eq!(walls.len(), 8);
            for (normal, [x, y]) in walls {
                // Into the building: towards the center from the outside walls, away from
                // it from the courtyard's.
                let towards_center = normal[0] * (5.0 - x) + normal[1] * (5.0 - y) > 0.0;
                let courtyard = (3.0..=7.0).contains(&x) && (3.0..=7.0).contains(&y);
                assert_eq!(towards_center, !courtyard, "{normal:?} at {x},{y}");
            }
        }
    }

    #[test]
    fn walls_along_clipped_tile_edges_are_skipped() {
        // Clipped at the buffer on the left (x = -64) and bottom (y = 4160); the right
        // side lies inside the tile and keeps its wall.
        let clipped = polygon![
            (x: -64, y: 4000), (x: 100, y: 4000), (x: 100, y: 4160), (x: -64, y: 4160), (x: -64, y: 4000)
        ];
        let b = extrude(&Geometry::Polygon(clipped));
        let walls = (b.vertices.len() - roof_vertices(&b)) / 4;
        assert_eq!(walls, 2);
        // An edge exactly on the tile border is a real wall, as in MapLibre.
        let on_border =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)];
        let b = extrude(&Geometry::Polygon(on_border));
        assert_eq!((b.vertices.len() - roof_vertices(&b)) / 4, 4);
    }

    #[test]
    fn holes_get_walls_and_points_are_ignored() {
        let with_hole = polygon!(
            exterior: [(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)],
            interiors: [[(x: 2, y: 2), (x: 4, y: 2), (x: 4, y: 4), (x: 2, y: 4), (x: 2, y: 2)]],
        );
        let two = Geometry::MultiPolygon(MultiPolygon(vec![
            with_hole,
            polygon![(x: 20, y: 20), (x: 30, y: 20), (x: 30, y: 30), (x: 20, y: 20)],
        ]));
        let b = extrude(&two);
        assert_eq!((b.vertices.len() - roof_vertices(&b)) / 4, 4 + 4 + 3);
        assert!(extrude(&Geometry::Point((1, 2).into())).vertices.is_empty());
    }
}
