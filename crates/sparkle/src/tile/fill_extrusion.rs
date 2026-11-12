//! `fill-extrusion` layers: polygons raised into prisms, with a roof at
//! `fill-extrusion-height` and walls down to `fill-extrusion-base`.
//!
//! Heights are per-feature paint values that can change with zoom, so vertices don't carry
//! a height: the vertex shader puts each at the feature's height or base. Normals are
//! computed here as MapLibre computes them, so that the shader's lighting (see
//! [`crate::light`]) matches MapLibre's.
//!
//! Roofs are triangles, as `fill` polygons are, with only a position per vertex. Walls are
//! a quad per ring edge, four [`WallVertex`]es that differ only in position and `top`.
//! Drawing walls instanced, from one point per ring vertex, would take a fraction of the
//! memory, but hangs the Adreno 610 (a moto g play 2024) at the instance counts of dense
//! cities.

use bytemuck::{Pod, Zeroable};
use mlt_core::geo_types::{Coord, Geometry, LineString};

use super::fill::{FillBucket, FillTessellator, PreTessellated};
use super::{BucketGeometry, BuildContext, FeatureInput, LayerBuilder, Scratch};
use crate::paint::PaintArrays;
use crate::sprite::ImageAtlas;
use crate::style::FillExtrusionLayer;

/// A corner of a wall.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct WallVertex {
    /// In tile coordinates.
    pub position: [f32; 2],
    /// The wall's unit normal, horizontal in tile coordinates (y down).
    ///
    /// As in MapLibre, wall normals point *into* the building (MapLibre takes
    /// `(p1 - p0).perp()` for an edge from `p0` to `p1` of a correctly wound ring). Its light
    /// direction is mirrored the same way, so keep both as they are.
    pub normal: [f32; 2],
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
    pub roofs: ExtrusionRoofs,
    pub walls: ExtrusionWalls,
}

/// The roofs of a [`FillExtrusionBucket`]: flat, at each feature's height.
#[derive(Debug, Clone, Default)]
pub struct ExtrusionRoofs {
    /// In tile coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Triangle list indices into [`Self::vertices`]. Triangles aren't consistently wound,
    /// so draw them without back-face culling.
    pub triangles: Vec<u32>,
    /// One per vertex when the layer has a `fill-extrusion-pattern` and there are sprites
    /// to take it from; empty otherwise.
    pub patterns: Vec<FillExtrusionPattern>,
    /// Per-vertex values of
    /// [`FillExtrusionPaint::roof_attributes`](crate::style::FillExtrusionPaint::roof_attributes):
    /// roofs are at the feature's height, whatever its base.
    pub paint: PaintArrays,
}

/// The walls of a [`FillExtrusionBucket`]: a quad per ring edge.
#[derive(Debug, Clone, Default)]
pub struct ExtrusionWalls {
    /// Four per wall: bottom and top at the edge's end, then at its start.
    pub vertices: Vec<WallVertex>,
    /// Triangle list indices into [`Self::vertices`], two triangles per wall. Triangles
    /// aren't consistently wound, so draw them without back-face culling.
    pub triangles: Vec<u32>,
    /// For each vertex, how far along its ring its wall starts or ends, in tile units,
    /// counting only the edges that have walls: the horizontal coordinate of
    /// `fill-extrusion-pattern` on walls. As in MapLibre, a wall's end has the distance
    /// where it starts, and its start the distance where it ends.
    ///
    /// Only with [`Self::patterns`]; empty otherwise, as nothing else reads them.
    pub edge_distances: Vec<f32>,
    /// One per vertex, as [`ExtrusionRoofs::patterns`].
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
            roofs: ExtrusionRoofs {
                paint: PaintArrays::new(&layer.paint.roof_attributes(), cx.zoom),
                ..Default::default()
            },
            walls: ExtrusionWalls {
                paint: PaintArrays::new(&layer.paint.attributes(), cx.zoom),
                ..Default::default()
            },
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
        let counts = |b: &FillExtrusionBucket| (b.roofs.vertices.len(), b.walls.vertices.len());
        let before = counts(bucket);
        add_extrusion(
            bucket,
            &mut self.roofs,
            feature.geometry,
            feature.pre_tessellated,
            self.extent,
            self.images.is_some(),
        );
        if counts(bucket) == before {
            return;
        }
        let (roofs, walls) = (&mut bucket.roofs, &mut bucket.walls);
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
            roofs.patterns.resize(roofs.vertices.len(), pattern);
            walls.patterns.resize(walls.vertices.len(), pattern);
        }
        let paint = &self.layer.paint;
        let (id, ctx) = (feature.id, &mut *feature.ctx);
        let count = roofs.vertices.len();
        roofs
            .paint
            .populate(&paint.roof_attributes(), ctx, id, count);
        let count = walls.vertices.len();
        walls.paint.populate(&paint.attributes(), ctx, id, count);
    }

    fn finish(self: Box<Self>, scratch: &mut Scratch) -> Option<BucketGeometry> {
        scratch.put(self.roofs);
        let b = self.bucket;
        (!b.roofs.vertices.is_empty() || !b.walls.vertices.is_empty())
            .then_some(BucketGeometry::FillExtrusion(b))
    }
}

/// Add the roof and walls of a polygon or multi-polygon; anything else is ignored. With
/// `edge_distances`, for patterns, the walls' distances along their rings too.
fn add_extrusion(
    bucket: &mut FillExtrusionBucket,
    roofs: &mut RoofScratch,
    geometry: &Geometry<i32>,
    pre_tessellated: Option<PreTessellated<'_>>,
    extent: i32,
    edge_distances: bool,
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
    let roof = &mut bucket.roofs;
    let base = roof.vertices.len() as u32;
    roof.vertices
        .extend(fill.vertices.iter().map(|v| v.position));
    roof.triangles
        .extend(fill.triangles.iter().map(|&i| i + base));

    let walls = &mut bucket.walls;
    for polygon in polygons {
        add_walls(walls, polygon.exterior(), false, extent, edge_distances);
        for hole in polygon.interiors() {
            add_walls(walls, hole, true, extent, edge_distances);
        }
    }
}

/// A wall per ring edge, except for edges along the tile's clipped border: the neighbouring
/// tile's copy of the polygon continues there, so the wall would be inside the building.
fn add_walls(
    walls: &mut ExtrusionWalls,
    ring: &LineString<i32>,
    hole: bool,
    extent: i32,
    edge_distances: bool,
) {
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
        let normal = [-d[1] / length * sign, d[0] / length * sign];
        let first = walls.vertices.len() as u32;
        for (p, distance) in [(p1, edge_distance), (p0, edge_distance + length)] {
            for top in [0.0, 1.0] {
                walls.vertices.push(WallVertex {
                    position: [p.x as f32, p.y as f32],
                    normal,
                    top,
                });
                if edge_distances {
                    walls.edge_distances.push(distance);
                }
            }
        }
        edge_distance += length;
        walls
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
            true,
        );
        bucket
    }

    /// A wall as the shaders draw it: its corners `(position, edge distance)`, bottom and
    /// top at the edge's end, then bottom and top at its start, and its normal.
    type Quad = ([([f32; 2], f32); 4], [f32; 2]);

    fn quads(b: &FillExtrusionBucket) -> Vec<Quad> {
        let walls = &b.walls;
        assert_eq!(walls.edge_distances.len(), walls.vertices.len());
        assert_eq!(walls.triangles.len(), walls.vertices.len() / 4 * 6);
        walls
            .vertices
            .chunks(4)
            .zip(walls.edge_distances.chunks(4))
            .enumerate()
            .map(|(i, (v, d))| {
                let first = 4 * i as u32;
                let triangles = &walls.triangles[6 * i..][..6];
                assert_eq!(triangles, [0, 2, 1, 1, 2, 3].map(|i| first + i));
                assert!(v.iter().all(|w| w.normal == v[0].normal));
                assert_eq!(
                    v.iter().map(|v| v.top).collect::<Vec<_>>(),
                    [0.0, 1.0, 0.0, 1.0]
                );
                let corners = [0, 1, 2, 3].map(|k| (v[k].position, d[k]));
                (corners, v[0].normal)
            })
            .collect()
    }

    #[test]
    fn square_has_roof_and_four_walls() {
        let square =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)];
        let b = extrude(&Geometry::Polygon(square));
        assert_eq!(b.roofs.vertices.len(), 4);
        assert_eq!(b.roofs.triangles.len(), 2 * 3);
        assert!(b.roofs.triangles.iter().all(|&i| (i as usize) < 4));

        // The first edge runs east along the top (north) side; MapLibre's normal for it
        // points south, into the square.
        let walls = quads(&b);
        assert_eq!(walls.len(), 4);
        let (corners, normal) = walls[0];
        assert_eq!(normal, [0.0, 1.0]);
        assert_eq!(corners[0], ([10.0, 0.0], 0.0));
        assert_eq!(corners[2], ([0.0, 0.0], 10.0));
        // Edge distances accumulate around the ring.
        assert_eq!(walls[3].0[2].1, 40.0);
    }

    /// Each wall's normal and the midpoint of its bottom edge.
    fn walls(b: &FillExtrusionBucket) -> Vec<([f32; 2], [f32; 2])> {
        quads(b)
            .into_iter()
            .map(|(q, normal)| {
                let [a, c] = [q[0].0, q[2].0];
                (normal, [(a[0] + c[0]) / 2.0, (a[1] + c[1]) / 2.0])
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
        let walls = quads(&b);
        assert_eq!(walls.len(), 2);
        // Skipped edges add nothing to the distance along the ring: 164 along the top,
        // then 160 down the right side.
        assert_eq!(walls[1].0[2].1, 324.0);
        // An edge exactly on the tile border is a real wall, as in MapLibre.
        let on_border =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)];
        assert_eq!(quads(&extrude(&Geometry::Polygon(on_border))).len(), 4);
        // A ring entirely along the border has a roof but no walls.
        let border_only = polygon![
            (x: -64, y: -64), (x: 4160, y: -64), (x: 4160, y: 4160), (x: -64, y: 4160), (x: -64, y: -64)
        ];
        let b = extrude(&Geometry::Polygon(border_only));
        assert_eq!(b.roofs.vertices.len(), 4);
        assert!(b.walls.vertices.is_empty());
    }

    #[test]
    fn repeated_points_have_no_wall() {
        let ring =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 0)];
        assert_eq!(quads(&extrude(&Geometry::Polygon(ring))).len(), 3);
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
        assert_eq!(quads(&extrude(&two)).len(), 4 + 4 + 3);
        let point = extrude(&Geometry::Point((1, 2).into()));
        assert!(point.walls.vertices.is_empty() && point.roofs.vertices.is_empty());
    }

    #[test]
    fn edge_distances_only_for_patterns() {
        let square =
            polygon![(x: 0, y: 0), (x: 10, y: 0), (x: 10, y: 10), (x: 0, y: 10), (x: 0, y: 0)];
        let mut b = FillExtrusionBucket::default();
        let geometry = Geometry::Polygon(square);
        add_extrusion(
            &mut b,
            &mut RoofScratch::default(),
            &geometry,
            None,
            4096,
            false,
        );
        assert_eq!(b.walls.vertices.len(), 16);
        assert!(b.walls.edge_distances.is_empty());
        // The same vertices as with them.
        assert_eq!(b.walls.vertices, extrude(&geometry).walls.vertices);
    }
}
