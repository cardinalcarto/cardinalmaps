//! Where on screen a point of the map goes: on the Web Mercator plane, on the globe
//! (`vertical-perspective`), or partway between the two while the style's `projection`
//! transitions, optionally lifted by terrain.
//!
//! The globe is a sphere seen from the same camera as the plane. Positions on both are in
//! logical pixels relative to the point at the center of the view, `x` east, `y` south
//! and `z` up, so one view matrix projects either and a transition is a blend of the two
//! positions. The sphere's radius makes its scale at the center match Mercator's there,
//! so zooming means the same on both and the blend doesn't jump. Renderers do the same
//! math in their vertex shaders; this is the CPU's copy, for symbol placement, picking and
//! tile selection.

use std::f64::consts::PI;

use crate::style::{ProjectionDefinition, PropertyValue};
use crate::terrain::ElevationSampler;

/// The equatorial radius Web Mercator is based on, in meters.
pub const EARTH_RADIUS: f64 = 6_378_137.0;
/// Web Mercator's latitude limit, in degrees.
pub const MAX_LATITUDE: f64 = 85.051_128_779_806_59;

/// A column-major 4x4 matrix.
pub type Mat4 = [[f32; 4]; 4];

/// How far `projection` is along the way from Mercator (0) to the globe (1) at `zoom`.
///
/// `globe` is MapLibre's shorthand for the globe at low zooms turning into Mercator from
/// zoom 10 to 12, `["interpolate", ["linear"], ["zoom"], 10, "vertical-perspective", 12,
/// "mercator"]`.
pub fn globe_factor(projection: &PropertyValue<ProjectionDefinition>, zoom: f64) -> f64 {
    let named = |name: &str| match name {
        "vertical-perspective" => 1.0,
        "globe" => ((12.0 - zoom) / 2.0).clamp(0.0, 1.0),
        _ => 0.0,
    };
    match projection.evaluate_at_zoom(zoom) {
        ProjectionDefinition::Named(name) => named(&name),
        ProjectionDefinition::Transition { from, to, t } => {
            let (a, b) = (named(&from), named(&to));
            a + (b - a) * f64::from(t).clamp(0.0, 1.0)
        }
    }
}

/// World coordinates (`0..1` across the map, `y` down) to longitude and latitude in
/// radians.
pub fn world_to_lng_lat([x, y]: [f64; 2]) -> [f64; 2] {
    [x * 2.0 * PI - PI, (PI - 2.0 * PI * y).sinh().atan()]
}

/// Longitude and latitude in radians to world coordinates.
pub fn lng_lat_to_world([lng, lat]: [f64; 2]) -> [f64; 2] {
    let lat = lat.clamp(-MAX_LATITUDE.to_radians(), MAX_LATITUDE.to_radians());
    [
        (lng + PI) / (2.0 * PI),
        (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0,
    ]
}

/// The globe as seen around the center of the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Globe {
    /// Of the center of the view, in radians.
    pub center: [f64; 2],
    /// In logical pixels.
    pub radius: f64,
}

impl Globe {
    /// The globe for a view centered on `center` (world coordinates) of a world `world_size`
    /// logical pixels wide.
    pub fn new(center: [f64; 2], world_size: f64) -> Self {
        let center = world_to_lng_lat(center);
        Self {
            radius: globe_radius(world_size, center[1]),
            center,
        }
    }

    /// Where `world` at `height` logical pixels above the ground is, relative to the
    /// center of the view on the ground.
    pub fn position(&self, world: [f64; 2], height: f64) -> [f64; 3] {
        let [lng, lat] = world_to_lng_lat(world);
        let [lng0, lat0] = self.center;
        let (sin_lat, cos_lat) = lat.sin_cos();
        let (sin_lat0, cos_lat0) = lat0.sin_cos();
        let (sin_dl, cos_dl) = (lng - lng0).sin_cos();
        let east = cos_lat * sin_dl;
        let north = sin_lat * cos_lat0 - cos_lat * sin_lat0 * cos_dl;
        let up = sin_lat * sin_lat0 + cos_lat * cos_lat0 * cos_dl;
        let r = self.radius + height;
        [r * east, -r * north, r * up - self.radius]
    }

    /// The world coordinates of a point on the globe's surface, from [`Self::position`]'s
    /// frame.
    pub fn world_at(&self, p: [f64; 3]) -> [f64; 2] {
        let [x, y, z] = p;
        let r = (x * x + y * y + (z + self.radius).powi(2)).sqrt().max(1e-9);
        let (east, north, up) = (x / r, -y / r, (z + self.radius) / r);
        let [lng0, lat0] = self.center;
        let (sin_lat0, cos_lat0) = lat0.sin_cos();
        let sin_lat = (north * cos_lat0 + up * sin_lat0).clamp(-1.0, 1.0);
        let lat = sin_lat.asin();
        // The component toward the center's meridian in the equatorial plane.
        let toward = up * cos_lat0 - north * sin_lat0;
        let lng = lng0 + east.atan2(toward);
        let mut world = lng_lat_to_world([lng, lat]);
        world[0] = world[0].rem_euclid(1.0);
        world
    }

    /// Whether a point of the globe's surface (or above it) faces `camera`, both in
    /// [`Self::position`]'s frame.
    pub fn faces(&self, p: [f64; 3], camera: [f64; 3]) -> bool {
        let normal = [p[0], p[1], p[2] + self.radius];
        let to_camera = [camera[0] - p[0], camera[1] - p[1], camera[2] - p[2]];
        let dot: f64 = (0..3).map(|i| normal[i] * to_camera[i]).sum();
        // A little past the horizon, so things standing on it don't pop.
        dot >= -1e-3 * self.radius * self.radius
    }
}

/// The radius at which a globe's scale at `latitude` (radians) matches a Mercator world
/// `world_size` logical pixels wide, as MapLibre's `getGlobeRadiusPixels`.
pub fn globe_radius(world_size: f64, latitude: f64) -> f64 {
    world_size / (2.0 * PI) / latitude.cos().max(1e-6)
}

/// How one tile's points reach clip space: [`Self::project`] mirrors the vertex shaders,
/// so what the CPU places (labels, collision boxes) lines up with what the GPU draws.
#[derive(Debug, Clone, PartialEq)]
pub struct TileProjection {
    /// Tile units (`z` in logical pixels) to clip space, on the Mercator plane.
    pub matrix: Mat4,
    /// Terrain elevation under the tile, if any.
    pub elevation: Option<ElevationSampler>,
    /// The globe, while the view isn't plain Mercator.
    pub globe: Option<GlobeTile>,
}

/// What projecting a tile onto the globe takes.
#[derive(Debug, Clone, PartialEq)]
pub struct GlobeTile {
    pub globe: Globe,
    /// From Mercator (0) to the globe (1).
    pub t: f64,
    /// Positions relative to the center of the view (see [`Globe::position`]) to clip
    /// space.
    pub view: [[f64; 4]; 4],
    /// The camera in the same frame, to hide what's behind the globe.
    pub camera: [f64; 3],
    /// The tile: zoom, column (not wrapped) and row.
    pub tile: (u8, i64, u32),
    pub extent: f32,
}

impl TileProjection {
    /// Mercator without terrain: `matrix` alone.
    pub fn flat(matrix: Mat4) -> Self {
        Self {
            matrix,
            elevation: None,
            globe: None,
        }
    }

    /// `p` (tile units, on the ground) in clip space. Points on the far side of the globe
    /// come back with a negative `w`, like points behind the camera.
    pub fn project(&self, p: [f32; 2]) -> [f32; 4] {
        let z = self.elevation.as_ref().map_or(0.0, |e| e.at(p));
        let m = &self.matrix;
        let mut flat = [0.0; 4];
        for (r, o) in flat.iter_mut().enumerate() {
            *o = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * z + m[3][r];
        }
        let Some(g) = &self.globe else {
            return flat;
        };
        let (z_tile, x, y) = g.tile;
        let scale = f64::from(z_tile).exp2();
        let world = [
            (x as f64 + f64::from(p[0] / g.extent)) / scale,
            (f64::from(y) + f64::from(p[1] / g.extent)) / scale,
        ];
        let position = g.globe.position(world, f64::from(z));
        if g.t >= 0.5 && !g.globe.faces(position, g.camera) {
            return [0.0, 0.0, 0.0, -1.0];
        }
        let v = &g.view;
        let mut out = [0.0; 4];
        for (r, o) in out.iter_mut().enumerate() {
            let sphere =
                v[0][r] * position[0] + v[1][r] * position[1] + v[2][r] * position[2] + v[3][r];
            *o = flat[r] + ((sphere - f64::from(flat[r])) * g.t) as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn projection(json: serde_json::Value) -> PropertyValue<ProjectionDefinition> {
        let style = crate::Style::from_json(&json!({
            "version": 8, "sources": {}, "layers": [], "projection": {"type": json}
        }))
        .unwrap();
        style.projection
    }

    #[test]
    fn globe_shorthand_turns_into_mercator() {
        let globe = projection(json!("globe"));
        assert_eq!(globe_factor(&globe, 3.0), 1.0);
        assert_eq!(globe_factor(&globe, 11.0), 0.5);
        assert_eq!(globe_factor(&globe, 14.0), 0.0);
        assert_eq!(globe_factor(&projection(json!("mercator")), 3.0), 0.0);
        let vp = projection(json!("vertical-perspective"));
        assert_eq!(globe_factor(&vp, 18.0), 1.0);
    }

    #[test]
    fn transitions_by_zoom_expression() {
        let p = projection(json!([
            "interpolate",
            ["linear"],
            ["zoom"],
            4,
            "vertical-perspective",
            6,
            "mercator"
        ]));
        assert_eq!(globe_factor(&p, 3.0), 1.0);
        assert!((globe_factor(&p, 5.0) - 0.5).abs() < 1e-6);
        assert_eq!(globe_factor(&p, 7.0), 0.0);
        // A transition written as `[from, to, t]`.
        let p = projection(json!(["vertical-perspective", "mercator", 0.25]));
        assert!((globe_factor(&p, 0.0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn globe_matches_mercator_near_the_center() {
        let center = lng_lat_to_world([-2.13, 0.83]);
        let size = 512.0 * 2f64.powi(6);
        let globe = Globe::new(center, size);
        assert_eq!(globe.position(center, 0.0), [0.0, 0.0, 0.0]);
        // A few pixels away, the globe and the plane agree but for the curvature.
        let d = 3.0 / size;
        let p = globe.position([center[0] + d, center[1] - d], 0.0);
        assert!(
            (p[0] - 3.0).abs() < 5e-3 && (p[1] + 3.0).abs() < 5e-3,
            "{p:?}"
        );
        assert!(p[2] < 0.0 && p[2] > -5e-3);
        // Height goes up along the normal.
        let up = globe.position(center, 10.0);
        assert!((up[2] - 10.0).abs() < 1e-9);
    }

    #[test]
    fn world_round_trips_through_the_globe() {
        let globe = Globe::new([0.3, 0.4], 2048.0);
        for world in [[0.3, 0.4], [0.1, 0.2], [0.55, 0.7], [0.95, 0.5]] {
            let back = globe.world_at(globe.position(world, 0.0));
            assert!(
                (back[0] - world[0]).abs() < 1e-9 && (back[1] - world[1]).abs() < 1e-9,
                "{world:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn the_far_side_faces_away() {
        let globe = Globe::new([0.5, 0.5], 1024.0);
        let camera = [0.0, 0.0, 2000.0];
        assert!(globe.faces(globe.position([0.5, 0.5], 0.0), camera));
        assert!(globe.faces(globe.position([0.6, 0.45], 0.0), camera));
        assert!(!globe.faces(globe.position([0.0, 0.5], 0.0), camera));
    }

    #[test]
    fn tile_projection_blends_plane_and_globe() {
        let mut identity = [[0.0; 4]; 4];
        (0..4).for_each(|i| identity[i][i] = 1.0);
        let globe = Globe::new([0.5, 0.5], 512.0);
        let tile = |t| TileProjection {
            matrix: identity,
            elevation: None,
            globe: Some(GlobeTile {
                globe,
                t,
                view: identity.map(|c| c.map(f64::from)),
                camera: [0.0, 0.0, 1000.0],
                tile: (0, 0, 0),
                extent: 512.0,
            }),
        };
        let p = [300.0, 256.0];
        assert_eq!(tile(0.0).project(p), [300.0, 256.0, 0.0, 1.0]);
        let on_globe = tile(1.0).project(p);
        let expected = globe.position([300.0 / 512.0, 0.5], 0.0);
        assert!((f64::from(on_globe[0]) - expected[0]).abs() < 1e-3);
        let half = tile(0.5).project(p);
        assert!((half[0] - (300.0 + on_globe[0]) / 2.0).abs() < 1e-3);
    }
}
