//! The view onto the map: a perspective camera over Web Mercator or the globe, as
//! MapLibre's transform.
//!
//! World coordinates run `0..1` across the map in both directions, `y` downwards. The map
//! repeats horizontally, so `x` outside `0..1` shows another copy of the world. The camera
//! looks at `center` from `camera_to_center` logical pixels away, tilted by `pitch` and
//! turned by `bearing`; matrices map "world pixels" (world coordinates times the world's
//! size in logical pixels at the camera's zoom, `z` up) to clip space.
//!
//! With terrain, the center is lifted to the ground's elevation there, so the camera
//! orbits the point it looks at. On the globe (see [`sparkle::projection`]), positions are
//! taken relative to the center and blended between the plane and the sphere by `globe`.

use std::f64::consts::PI;

use sparkle::TileId;
use sparkle::projection::{Globe, GlobeTile, TileProjection};

/// MapLibre's tile size, in logical pixels.
pub const TILE_SIZE: f64 = 512.0;
pub const MAX_ZOOM: f64 = 22.0;
pub const MAX_PITCH: f64 = 85.0;
/// MapLibre's default vertical field of view, in radians.
pub const FOV: f64 = 0.643_501_108_793_284_4;
/// Upper bound on covering tiles, for views that look far over the horizon.
const MAX_COVERING_TILES: usize = 400;

/// A column-major 4x4 matrix, as WGSL expects.
pub type Mat4 = [[f64; 4]; 4];

#[derive(Debug, Clone)]
pub struct Camera {
    pub center: [f64; 2],
    pub zoom: f64,
    /// In degrees, clockwise from north.
    pub bearing: f64,
    /// In degrees, from looking straight down.
    pub pitch: f64,
    /// In physical pixels.
    pub viewport: [f64; 2],
    /// Physical pixels per logical pixel.
    pub pixel_ratio: f64,
    /// From Mercator (0) to the globe (1), as the style's projection has it at this zoom.
    pub globe: f64,
    /// The terrain's elevation at the center in meters, exaggeration included.
    pub center_elevation: f64,
}

/// Space at the edges of the viewport to keep clear, such as under a sheet, in logical
/// pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Padding {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl std::ops::Add for Padding {
    type Output = Self;

    /// Both kept clear: say, the app's sheet and room ahead of a followed puck.
    fn add(self, other: Self) -> Self {
        Self {
            top: self.top + other.top,
            right: self.right + other.right,
            bottom: self.bottom + other.bottom,
            left: self.left + other.left,
        }
    }
}

/// What a camera shows and how: `target` (world coordinates) in the middle of the
/// viewport less some padding, at a zoom, bearing and pitch. Eases and the follow camera
/// move between framings rather than between camera centers, so the target stays where
/// the padding puts it on screen however the pitch and bearing change on the way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Framing {
    pub target: [f64; 2],
    pub zoom: f64,
    /// In degrees, clockwise from north.
    pub bearing: f64,
    /// In degrees, from looking straight down.
    pub pitch: f64,
}

impl Framing {
    /// `self` turned into `to` by `k` (0 to 1): the zoom and pitch evenly, the bearing the
    /// short way round, and the target in a straight line on the map the short way
    /// around the world.
    pub fn toward(&self, to: &Self, k: f64) -> Self {
        let mut end = to.target;
        end[0] += (self.target[0] - end[0]).round();
        let turn = (to.bearing - self.bearing + 180.0).rem_euclid(360.0) - 180.0;
        let lerp = |a: f64, b: f64| a + (b - a) * k;
        Self {
            target: [lerp(self.target[0], end[0]), lerp(self.target[1], end[1])],
            zoom: lerp(self.zoom, to.zoom),
            bearing: self.bearing + turn * k,
            pitch: lerp(self.pitch, to.pitch),
        }
    }
}

/// The lowest and highest elevation in a tile, in logical pixels at the camera's zoom.
pub type ElevationRange<'a> = &'a dyn Fn(TileId) -> Option<(f64, f64)>;

/// A tile placed somewhere on the map. `x` is not wrapped, so tiles of different world
/// copies have different positions but can share the same [`TileId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Placement {
    pub z: u8,
    pub x: i64,
    pub y: u32,
}

impl Placement {
    pub fn id(self) -> TileId {
        TileId::new(self.z, self.x.rem_euclid(1 << self.z) as u32, self.y)
    }

    /// The tile `levels` zoom levels up that contains this one.
    pub fn ancestor(self, levels: u8) -> Self {
        Self {
            z: self.z - levels,
            x: self.x.div_euclid(1 << levels),
            y: self.y >> levels,
        }
    }

    pub fn children(self) -> [Self; 4] {
        let (z, x, y) = (self.z + 1, self.x * 2, self.y * 2);
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| Self {
            z,
            x: x + dx,
            y: y + dy,
        })
    }
}

impl Camera {
    pub fn new(viewport: [f64; 2], pixel_ratio: f64) -> Self {
        Self {
            center: [0.5, 0.5],
            zoom: 0.0,
            bearing: 0.0,
            pitch: 0.0,
            viewport,
            pixel_ratio,
            globe: 0.0,
            center_elevation: 0.0,
        }
    }

    pub fn look_at(&mut self, lng: f64, lat: f64, zoom: f64) {
        self.center = lng_lat_to_world(lng, lat);
        self.zoom = zoom.clamp(0.0, MAX_ZOOM);
    }

    pub fn set_pitch(&mut self, pitch: f64) {
        self.pitch = pitch.clamp(0.0, MAX_PITCH);
    }

    pub fn set_bearing(&mut self, bearing: f64) {
        self.bearing = (bearing + 180.0).rem_euclid(360.0) - 180.0;
    }

    /// Fit `[west, south, east, north]` in the viewport, looking straight down.
    pub fn fit_bounds(&mut self, bounds: [f64; 4]) {
        let [w, s, e, n] = bounds;
        let (a, b) = (lng_lat_to_world(w, n), lng_lat_to_world(e, s));
        self.center = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let [lw, lh] = self.logical_size();
        let span = ((b[0] - a[0]) / lw).max((b[1] - a[1]) / lh);
        self.zoom = (1.0 / (span * TILE_SIZE)).log2().clamp(0.0, MAX_ZOOM);
    }

    /// The zoom that fits `[west, south, east, north]` in the viewport less `padding`,
    /// looking straight down, and the bounds' center in world coordinates. Bounds with
    /// `west > east` cross the antimeridian.
    pub fn camera_for_bounds(&self, bounds: [f64; 4], padding: Padding) -> ([f64; 2], f64) {
        let [w, s, e, n] = bounds;
        let (a, mut b) = (lng_lat_to_world(w, n), lng_lat_to_world(e, s));
        if b[0] < a[0] {
            b[0] += 1.0;
        }
        let center = [((a[0] + b[0]) / 2.0).rem_euclid(1.0), (a[1] + b[1]) / 2.0];
        let [lw, lh] = self.logical_size();
        let width = (lw - padding.left - padding.right).max(1.0);
        let height = (lh - padding.top - padding.bottom).max(1.0);
        let span = ((b[0] - a[0]) / width).max((b[1] - a[1]) / height);
        let zoom = if span > 0.0 {
            (1.0 / (span * TILE_SIZE)).log2().clamp(0.0, MAX_ZOOM)
        } else {
            MAX_ZOOM
        };
        (center, zoom)
    }

    /// Where the center must be for `target` (world coordinates) to show in the middle of
    /// the viewport less `padding` at `zoom`, with the current bearing and looking
    /// straight down.
    pub fn center_for(&self, target: [f64; 2], zoom: f64, padding: Padding) -> [f64; 2] {
        // From the padded area's middle to the viewport's, in logical pixels.
        let dx = (padding.right - padding.left) / 2.0;
        let dy = (padding.bottom - padding.top) / 2.0;
        let (sin, cos) = self.bearing.to_radians().sin_cos();
        let size = TILE_SIZE * zoom.exp2();
        [
            target[0] + (dx * cos - dy * sin) / size,
            (target[1] + (dx * sin + dy * cos) / size).clamp(0.0, 1.0),
        ]
    }

    /// The world point shown in the middle of the viewport less `padding`, looking
    /// straight down: the inverse of [`Camera::center_for`] at the camera's zoom.
    pub fn padded_center(&self, padding: Padding) -> [f64; 2] {
        let dx = (padding.right - padding.left) / 2.0;
        let dy = (padding.bottom - padding.top) / 2.0;
        let (sin, cos) = self.bearing.to_radians().sin_cos();
        let size = self.world_size();
        [
            self.center[0] - (dx * cos - dy * sin) / size,
            (self.center[1] - (dx * sin + dy * cos) / size).clamp(0.0, 1.0),
        ]
    }

    /// The middle of the viewport less `padding`, in physical pixels.
    pub fn anchor(&self, padding: Padding) -> [f64; 2] {
        let [w, h] = self.logical_size();
        let x = padding.left + (w - padding.left - padding.right) / 2.0;
        let y = padding.top + (h - padding.top - padding.bottom) / 2.0;
        [x * self.pixel_ratio, y * self.pixel_ratio]
    }

    /// What the camera shows in the middle of the viewport less `padding`: the ground
    /// there as seen at the camera's pitch, which is [`Self::padded_center`] only when
    /// looking straight down. Above the horizon, the padded center.
    pub fn framing(&self, padding: Padding) -> Framing {
        Framing {
            target: self
                .screen_to_world(self.anchor(padding))
                .unwrap_or_else(|| self.padded_center(padding)),
            zoom: self.zoom,
            bearing: self.bearing,
            pitch: self.pitch,
        }
    }

    /// Show `framing`: its zoom, bearing and pitch, with its target on the ground in the
    /// middle of the viewport less `padding`. Unlike [`Self::center_for`], this holds at
    /// any pitch, where the padding's offset on screen is foreshortened on the ground.
    pub fn show(&mut self, framing: &Framing, padding: Padding) {
        self.zoom = framing.zoom.clamp(0.0, MAX_ZOOM);
        self.set_bearing(framing.bearing);
        self.set_pitch(framing.pitch);
        let target = framing.target;
        let anchor = self.anchor(padding);
        // On the plane the ground under a screen point moves with the center, so one
        // step lands it; the globe bends, so a few more close in.
        self.center = target;
        for _ in 0..4 {
            let Some(shown) = self.screen_to_world(anchor) else {
                self.center = self.center_for(target, self.zoom, padding);
                break;
            };
            let off = [target[0] - shown[0], target[1] - shown[1]];
            self.center = [
                self.center[0] + off[0],
                (self.center[1] + off[1]).clamp(0.0, 1.0),
            ];
            if off[0].hypot(off[1]) * self.world_size() < 1e-3 {
                break;
            }
        }
        self.center[0] = self.center[0].rem_euclid(1.0);
    }

    /// The viewport in logical pixels.
    pub fn logical_size(&self) -> [f64; 2] {
        self.viewport.map(|v| v / self.pixel_ratio)
    }

    /// Logical pixels the world spans at the camera's zoom.
    pub fn world_size(&self) -> f64 {
        TILE_SIZE * self.zoom.exp2()
    }

    /// Distance from the camera to the center of the view, in logical pixels.
    pub fn camera_to_center(&self) -> f64 {
        0.5 / (FOV / 2.0).tan() * self.logical_size()[1]
    }

    /// Logical pixels per meter on the ground at the center: for extrusion heights and
    /// terrain elevations, on the plane and the globe alike.
    pub fn pixels_per_meter(&self) -> f64 {
        sparkle::terrain::pixels_per_meter(self.world_size(), self.center[1])
    }

    /// The elevation of the center, in logical pixels.
    pub fn center_height(&self) -> f64 {
        self.center_elevation * self.pixels_per_meter()
    }

    /// How far the ground the view shows reaches from the camera, along its direction:
    /// the clip `w` where the top of a pitched view meets sea level (which terrain lifts
    /// the camera above), or the far side of the globe.
    pub fn furthest(&self) -> f64 {
        let pitch = self.pitch.to_radians();
        let distance = self.camera_to_center();
        let height = distance * pitch.cos() + self.center_height().max(0.0);
        // The top edge of the view, from straight down; level with the horizon at 90°.
        let top = (pitch + FOV / 2.0).min(PI / 2.0 - 0.01);
        let flat = height / top.cos() * (FOV / 2.0).cos();
        if self.globe <= 0.0 {
            return flat;
        }
        // Nothing on the globe is more than its diameter behind the center.
        let globe = distance + 2.0 * self.globe_frame().radius;
        flat + (globe - flat) * self.globe
    }

    /// Positions relative to the center of the view on the ground (logical pixels, `x`
    /// east, `y` south, `z` up) to clip space, for wgpu's `0..1` depth.
    pub fn view_matrix(&self) -> Mat4 {
        let [width, height] = self.logical_size();
        let pitch = self.pitch.to_radians();
        let distance = self.camera_to_center();
        let far = self.furthest() * 1.01;
        let near = height / 50.0;
        let mut m = perspective(FOV, width / height, near, far);
        m = mul(&m, &scale(1.0, -1.0, 1.0));
        m = mul(&m, &translate(0.0, 0.0, -distance));
        m = mul(&m, &rotate_x(pitch));
        m = mul(&m, &rotate_z(-self.bearing.to_radians()));
        mul(&m, &translate(0.0, 0.0, -self.center_height()))
    }

    /// World pixels to clip space on the Mercator plane, as MapLibre's `projMatrix`.
    pub fn view_projection(&self) -> Mat4 {
        let size = self.world_size();
        mul(
            &self.view_matrix(),
            &translate(-self.center[0] * size, -self.center[1] * size, 0.0),
        )
    }

    /// The globe around the center of the view.
    pub fn globe_frame(&self) -> Globe {
        Globe::new(self.center, self.world_size())
    }

    /// Where the camera is, in [`Self::view_matrix`]'s frame.
    pub fn eye(&self) -> [f64; 3] {
        let (sin_p, cos_p) = self.pitch.to_radians().sin_cos();
        let (sin_b, cos_b) = self.bearing.to_radians().sin_cos();
        let d = self.camera_to_center();
        [
            -sin_b * d * sin_p,
            cos_b * d * sin_p,
            self.center_height() + d * cos_p,
        ]
    }

    /// `world` at `height` logical pixels above sea level, in [`Self::view_matrix`]'s
    /// frame: on the plane, the globe, or between them.
    pub fn position(&self, world: [f64; 2], height: f64) -> [f64; 3] {
        let size = self.world_size();
        let flat = [
            (world[0] - self.center[0]) * size,
            (world[1] - self.center[1]) * size,
            height,
        ];
        if self.globe <= 0.0 {
            return flat;
        }
        let sphere = self.globe_frame().position(world, height);
        std::array::from_fn(|i| flat[i] + (sphere[i] - flat[i]) * self.globe)
    }

    /// Whether `position` (from [`Self::position`]) is on the side of the globe that
    /// faces the camera. Everything does on the plane and halfway to it.
    pub fn faces_camera(&self, position: [f64; 3]) -> bool {
        self.globe < 0.5 || self.globe_frame().faces(position, self.eye())
    }

    /// How [`sparkle::symbol::placement`] projects points of `tile`, as the shaders do.
    pub fn tile_projection(
        &self,
        matrix: [[f32; 4]; 4],
        tile: Placement,
        extent: u32,
        elevation: Option<sparkle::terrain::ElevationSampler>,
    ) -> TileProjection {
        TileProjection {
            matrix,
            elevation,
            globe: (self.globe > 0.0).then(|| GlobeTile {
                globe: self.globe_frame(),
                t: self.globe,
                view: self.view_matrix(),
                camera: self.eye(),
                tile: (tile.z, tile.x, tile.y),
                extent: extent as f32,
            }),
        }
    }

    /// Tile coordinates (`0..extent`, z in world pixels) of `tile` to clip space.
    pub fn tile_matrix(
        &self,
        view_projection: &Mat4,
        tile: Placement,
        extent: u32,
    ) -> [[f32; 4]; 4] {
        let s = self.tile_world_size(tile.z);
        let m = mul(
            view_projection,
            &translate(tile.x as f64 * s, f64::from(tile.y) * s, 0.0),
        );
        let m = mul(
            &m,
            &scale(s / f64::from(extent), s / f64::from(extent), 1.0),
        );
        m.map(|c| c.map(|v| v as f32))
    }

    /// Logical pixels a tile of zoom `z` spans at the camera's zoom.
    pub fn tile_world_size(&self, z: u8) -> f64 {
        self.world_size() / f64::from(z).exp2()
    }

    /// The ray under a point of the screen in physical pixels, from the near plane to the
    /// far plane, in [`Self::view_matrix`]'s frame.
    fn ray(&self, p: [f64; 2]) -> Option<([f64; 3], [f64; 3])> {
        let inverse = invert(&self.view_matrix())?;
        let ndc = [
            2.0 * p[0] / self.viewport[0] - 1.0,
            1.0 - 2.0 * p[1] / self.viewport[1],
        ];
        let unproject = |z: f64| {
            let v = transform(&inverse, [ndc[0], ndc[1], z, 1.0]);
            [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
        };
        Some((unproject(0.0), unproject(1.0)))
    }

    /// The world point (`0..1`) under a point of the screen in physical pixels, on the
    /// ground at the center's elevation, or `None` above the horizon. This is what
    /// dragging and zooming hold in place.
    pub fn screen_to_world(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        let (a, b) = self.ray(p)?;
        let height = self.center_height();
        if self.globe >= 0.5 {
            // Where the ray meets a sphere at the center's height.
            let globe = self.globe_frame();
            let o = [0.0, 0.0, -globe.radius];
            let r = globe.radius + height;
            let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let f = [a[0] - o[0], a[1] - o[1], a[2] - o[2]];
            let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
            let (qa, qb, qc) = (dot(d, d), 2.0 * dot(f, d), dot(f, f) - r * r);
            let disc = qb * qb - 4.0 * qa * qc;
            if disc < 0.0 {
                return None;
            }
            let t = (-qb - disc.sqrt()) / (2.0 * qa);
            if !(0.0..=1.0).contains(&t) {
                return None;
            }
            return Some(globe.world_at([a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t]));
        }
        let t = (a[2] - height) / (a[2] - b[2]);
        if !(0.0..=1.0).contains(&t) || !t.is_finite() {
            return None;
        }
        let size = self.world_size();
        Some([
            self.center[0] + (a[0] + (b[0] - a[0]) * t) / size,
            self.center[1] + (a[1] + (b[1] - a[1]) * t) / size,
        ])
    }

    /// The world point on the terrain under a point of the screen, the first place the
    /// ray dips below `ground` (elevations in meters, exaggerated): what is drawn there,
    /// for queries. Without terrain, or on the globe, the same as [`Self::screen_to_world`].
    pub fn pick(&self, p: [f64; 2], ground: &dyn Fn([f64; 2]) -> Option<f64>) -> Option<[f64; 2]> {
        let flat = self.screen_to_world(p);
        if self.globe >= 0.5 {
            return flat;
        }
        let (a, b) = self.ray(p)?;
        let (size, ppm) = (self.world_size(), self.pixels_per_meter());
        let world_at = |t: f64| {
            [
                self.center[0] + (a[0] + (b[0] - a[0]) * t) / size,
                self.center[1] + (a[1] + (b[1] - a[1]) * t) / size,
            ]
        };
        let below = |t: f64| {
            let z = a[2] + (b[2] - a[2]) * t;
            ground(world_at(t)).is_some_and(|e| z <= e * ppm)
        };
        // March in steps that grow with distance, then bisect the step that went under.
        const STEPS: usize = 256;
        let mut prev = 0.0;
        for i in 1..=STEPS {
            let t = (i as f64 / STEPS as f64).powi(2);
            if below(t) {
                let (mut lo, mut hi) = (prev, t);
                for _ in 0..24 {
                    let mid = (lo + hi) / 2.0;
                    if below(mid) { hi = mid } else { lo = mid }
                }
                return Some(world_at(hi));
            }
            prev = t;
        }
        flat
    }

    /// Move the map so that the ground point under `from` ends up under `to` (both in
    /// physical pixels).
    pub fn drag(&mut self, from: [f64; 2], to: [f64; 2]) {
        if let (Some(a), Some(b)) = (self.screen_to_world(from), self.screen_to_world(to)) {
            self.move_by(a, b);
        }
    }

    /// Move the center by the difference between two world points: on the globe by
    /// their longitudes and latitudes, so the globe turns under the cursor.
    fn move_by(&mut self, a: [f64; 2], b: [f64; 2]) {
        if self.globe < 0.5 {
            self.move_center([a[0] - b[0], a[1] - b[1]]);
            return;
        }
        let [a, b, c] = [a, b, self.center].map(sparkle::projection::world_to_lng_lat);
        let dlng = (a[0] - b[0] + PI).rem_euclid(2.0 * PI) - PI;
        let world = sparkle::projection::lng_lat_to_world([c[0] + dlng, c[1] + a[1] - b[1]]);
        self.center = [world[0].rem_euclid(1.0), world[1].clamp(0.0, 1.0)];
    }

    /// Lower the pitch until the camera is above `ground` (elevations in meters,
    /// exaggerated) under it, so it never looks out from inside a hill.
    pub fn avoid_terrain(&mut self, ground: &dyn Fn([f64; 2]) -> Option<f64>) {
        if self.globe >= 0.5 {
            return;
        }
        let ppm = self.pixels_per_meter();
        let size = self.world_size();
        // A few meters of clearance, more when zoomed out.
        let clearance = 10.0 * ppm + self.camera_to_center() * 0.01;
        while self.pitch > 0.0 {
            let eye = self.eye();
            let under = [
                self.center[0] + eye[0] / size,
                self.center[1] + eye[1] / size,
            ];
            let Some(e) = ground(under) else { return };
            if eye[2] >= e * ppm + clearance {
                return;
            }
            self.pitch = (self.pitch - 1.0).max(0.0);
        }
    }

    /// Pan by a screen distance in physical pixels, as arrow keys do.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let c = [self.viewport[0] / 2.0, self.viewport[1] / 2.0];
        self.drag(c, [c[0] + dx, c[1] + dy]);
    }

    fn move_center(&mut self, delta: [f64; 2]) {
        self.center[0] = (self.center[0] + delta[0]).rem_euclid(1.0);
        self.center[1] = (self.center[1] + delta[1]).clamp(0.0, 1.0);
    }

    /// Zoom by `delta` levels, keeping the ground point under `anchor` (in physical pixels)
    /// in place.
    pub fn zoom_around(&mut self, delta: f64, anchor: [f64; 2]) {
        let before = self.screen_to_world(anchor);
        self.zoom = (self.zoom + delta).clamp(0.0, MAX_ZOOM);
        if let (Some(before), Some(after)) = (before, self.screen_to_world(anchor)) {
            self.move_by(before, after);
        }
    }

    /// Turn the bearing by `delta` degrees, keeping the ground point under `anchor` (in
    /// physical pixels) in place.
    pub fn rotate_around(&mut self, delta: f64, anchor: [f64; 2]) {
        let before = self.screen_to_world(anchor);
        self.set_bearing(self.bearing + delta);
        if let (Some(before), Some(after)) = (before, self.screen_to_world(anchor)) {
            self.move_by(before, after);
        }
    }

    /// The tile zoom level to draw near the center for tiles of `tile_size` logical pixels:
    /// the view's zoom rounded down, limited to what the source has. Past its max zoom, the
    /// deepest tiles are drawn magnified.
    pub fn tile_zoom(&self, min: u8, max: u8, tile_size: u32) -> u8 {
        let zoom = self.zoom + (TILE_SIZE / f64::from(tile_size)).log2();
        (zoom.floor().max(0.0) as u8).clamp(min, max)
    }

    /// Tiles of zoom `min..=max` that cover the view, nearest the center first. Tiles far
    /// from the camera in pitched views are taken from lower zoom levels, so each tile
    /// covers roughly the same screen area.
    pub fn covering_tiles(&self, min: u8, max: u8, tile_size: u32) -> Vec<Placement> {
        self.covering_tiles_with(min, max, tile_size, &|_| None)
    }

    /// [`Self::covering_tiles`] over terrain, whose tiles stand as high as `range` says
    /// (in meters, exaggerated).
    pub fn covering_tiles_with(
        &self,
        min: u8,
        max: u8,
        tile_size: u32,
        range: ElevationRange<'_>,
    ) -> Vec<Placement> {
        let max = self.tile_zoom(min, max, tile_size);
        let view = self.view_matrix();
        let distance = self.camera_to_center();
        let ppm = self.pixels_per_meter();
        let mut stack: Vec<Placement> = (-1..=1).map(|x| Placement { z: 0, x, y: 0 }).collect();
        let mut out = Vec::new();
        while let Some(tile) = stack.pop() {
            let n = f64::from(tile.z).exp2();
            let (x0, y0) = (tile.x as f64 / n, f64::from(tile.y) / n);
            // The globe has one copy of the world: the one around the center.
            if self.globe > 0.0
                && (x0 >= self.center[0] + 0.5 || x0 + 1.0 / n <= self.center[0] - 0.5)
            {
                continue;
            }
            let (lo, hi) = range(tile.id()).map_or((0.0, 0.0), |(lo, hi)| (lo * ppm, hi * ppm));
            // A grid of points at both heights, as the globe bulges between corners; finer
            // for big tiles, which can surround the view without a point in it.
            let k = if self.globe > 0.0 && tile.z < 5 { 8 } else { 2 };
            let mut points = Vec::with_capacity((k + 1) * (k + 1) * 2);
            for j in 0..=k {
                for i in 0..=k {
                    let world = [x0 + i as f64 / k as f64 / n, y0 + j as f64 / k as f64 / n];
                    for h in [lo, hi] {
                        points.push(self.position(world, h));
                    }
                }
            }
            let contains_center = (x0..x0 + 1.0 / n).contains(&self.center[0])
                && (y0..y0 + 1.0 / n).contains(&self.center[1]);
            if !contains_center && !points.iter().any(|&p| self.faces_camera(p)) {
                continue;
            }
            let clip: Vec<[f64; 4]> = points
                .iter()
                .map(|p| transform(&view, [p[0], p[1], p[2], 1.0]))
                .collect();
            if !contains_center && outside_frustum(&clip) {
                continue;
            }
            // Clip w is the distance along the view direction, which sets how big the tile
            // appears; split while the tile would be drawn at more than twice its size.
            let nearest = clip.iter().map(|c| c[3]).fold(f64::INFINITY, f64::min);
            let side = |a: usize, b: usize| {
                let (p, q) = (points[a], points[b]);
                ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
            };
            // The corners at the low height.
            let row = 2 * (k + 1);
            let (tl, tr, bl) = (0, 2 * k, row * k);
            let br = bl + 2 * k;
            let size = side(tl, tr)
                .max(side(bl, br))
                .max(side(tl, bl))
                .max(side(tr, br));
            let projected = if nearest <= 1e-6 {
                f64::INFINITY
            } else {
                size * distance / nearest
            };
            let split = tile.z < min || projected >= 2.0 * f64::from(tile_size);
            // On the globe, of the world copies overlapping the view, only the one whose
            // tile center is nearest the view's center is drawn.
            let copy = self.globe <= 0.0 || {
                let cx = x0 + 0.5 / n;
                (self.center[0] - 0.5..self.center[0] + 0.5).contains(&cx)
            };
            if tile.z < max && split {
                stack.extend(tile.children());
            } else if tile.z >= min && copy {
                out.push(tile);
            }
            if out.len() > MAX_COVERING_TILES {
                log::warn!("view needs more than {MAX_COVERING_TILES} tiles; dropping the rest");
                break;
            }
        }
        let distance = |t: &Placement| {
            let n = f64::from(t.z).exp2();
            let p = self.position([(t.x as f64 + 0.5) / n, (f64::from(t.y) + 0.5) / n], 0.0);
            p[0] * p[0] + p[1] * p[1] + p[2] * p[2]
        };
        out.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        out
    }
}

/// Whether every corner is outside the same clip plane (`-w <= x, y <= w`, `0 <= z <= w`).
/// A tile that only touches the edge of the view counts as outside.
fn outside_frustum(corners: &[[f64; 4]]) -> bool {
    let planes: [fn(&[f64; 4]) -> bool; 6] = [
        |c| c[0] <= -c[3],
        |c| c[0] >= c[3],
        |c| c[1] <= -c[3],
        |c| c[1] >= c[3],
        |c| c[2] < 0.0,
        |c| c[2] > c[3],
    ];
    planes.iter().any(|outside| corners.iter().all(outside))
}

pub fn lng_lat_to_world(lng: f64, lat: f64) -> [f64; 2] {
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    [
        (lng + 180.0) / 360.0,
        (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0,
    ]
}

pub fn identity() -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    (0..4).for_each(|i| m[i][i] = 1.0);
    m
}

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

pub fn transform(m: &Mat4, v: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0; 4];
    for (r, o) in out.iter_mut().enumerate() {
        *o = (0..4).map(|k| m[k][r] * v[k]).sum();
    }
    out
}

fn translate(x: f64, y: f64, z: f64) -> Mat4 {
    let mut m = identity();
    m[3] = [x, y, z, 1.0];
    m
}

fn scale(x: f64, y: f64, z: f64) -> Mat4 {
    let mut m = identity();
    m[0][0] = x;
    m[1][1] = y;
    m[2][2] = z;
    m
}

fn rotate_x(angle: f64) -> Mat4 {
    let (s, c) = angle.sin_cos();
    let mut m = identity();
    m[1] = [0.0, c, s, 0.0];
    m[2] = [0.0, -s, c, 0.0];
    m
}

fn rotate_z(angle: f64) -> Mat4 {
    let (s, c) = angle.sin_cos();
    let mut m = identity();
    m[0] = [c, s, 0.0, 0.0];
    m[1] = [-s, c, 0.0, 0.0];
    m
}

/// A right-handed perspective projection to wgpu's clip space, depth `0..1`.
fn perspective(fovy: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
    let f = 1.0 / (fovy / 2.0).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

/// The inverse of `m`, by cofactor expansion.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    let a: Vec<f64> = m.iter().flatten().copied().collect();
    let mut inv = [0.0; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14]
        + a[13] * a[6] * a[11]
        - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14]
        - a[12] * a[6] * a[11]
        + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13]
        + a[12] * a[5] * a[11]
        - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13]
        - a[12] * a[5] * a[10]
        + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14]
        - a[13] * a[2] * a[11]
        + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14]
        + a[12] * a[2] * a[11]
        - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13]
        - a[12] * a[1] * a[11]
        + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13]
        + a[12] * a[1] * a[10]
        - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14]
        + a[13] * a[2] * a[7]
        - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14]
        - a[12] * a[2] * a[7]
        + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13]
        + a[12] * a[1] * a[7]
        - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13]
        - a[12] * a[1] * a[6]
        + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10]
        - a[9] * a[2] * a[7]
        + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10]
        + a[8] * a[2] * a[7]
        - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9]
        - a[8] * a[1] * a[7]
        + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9]
        + a[8] * a[1] * a[6]
        - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det.abs() < 1e-300 {
        return None;
    }
    let mut out = [[0.0; 4]; 4];
    for (i, v) in inv.iter().enumerate() {
        out[i / 4][i % 4] = v / det;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        let mut c = Camera::new([800.0, 600.0], 1.0);
        c.look_at(-122.33, 47.6, 12.0);
        c
    }

    fn close(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn bounds_fit_inside_the_padding() {
        let c = camera();
        let bounds = [-122.44, 47.50, -122.24, 47.73];
        let (_, open) = c.camera_for_bounds(bounds, Padding::default());
        let padding = Padding {
            bottom: 300.0,
            ..Padding::default()
        };
        let (center, zoom) = c.camera_for_bounds(bounds, padding);
        // Half the height left: the taller-than-wide bounds need a level less.
        assert!((open - zoom - 1.0).abs() < 1e-9, "{open} {zoom}");
        let mid = lng_lat_to_world(-122.34, 0.0)[0];
        assert!((center[0] - mid).abs() < 1e-9);
    }

    #[test]
    fn a_target_shows_in_the_middle_of_the_padding() {
        let mut c = camera();
        let target = lng_lat_to_world(-122.33, 47.6);
        let padding = Padding {
            bottom: 300.0,
            ..Padding::default()
        };
        c.center = c.center_for(target, c.zoom, padding);
        // The padded area is the top 300 of 600 pixels: its middle is at y = 150.
        let world = c.screen_to_world([400.0, 150.0]).unwrap();
        assert!(close(world, target), "{world:?} {target:?}");

        c.set_bearing(90.0);
        c.center = c.center_for(target, c.zoom, padding);
        let world = c.screen_to_world([400.0, 150.0]).unwrap();
        assert!(close(world, target), "{world:?} {target:?}");
    }

    #[test]
    fn the_padded_center_is_where_center_for_put_it() {
        let mut c = camera();
        c.set_bearing(30.0);
        let target = lng_lat_to_world(-122.3, 47.65);
        let padding = Padding {
            top: 40.0,
            bottom: 250.0,
            left: 10.0,
            right: 0.0,
        };
        c.center = c.center_for(target, c.zoom, padding);
        assert!(close(c.padded_center(padding), target));
    }

    #[test]
    fn a_framing_shows_its_target_in_the_middle_of_the_padding_at_any_pitch() {
        let mut c = camera();
        let padding = Padding {
            top: 300.0,
            right: 0.0,
            bottom: 100.0,
            left: 60.0,
        };
        let target = lng_lat_to_world(-122.31, 47.61);
        for (bearing, pitch) in [(0.0, 0.0), (45.0, 60.0), (-170.0, 80.0)] {
            let framing = Framing {
                target,
                zoom: 16.0,
                bearing,
                pitch,
            };
            c.show(&framing, padding);
            // The middle of what the padding leaves: x 60 + 740 / 2, y 300 + 200 / 2.
            let shown = c.screen_to_world([430.0, 400.0]).unwrap();
            assert!(close(shown, target), "{bearing} {pitch}: {shown:?}");
            let back = c.framing(padding);
            assert!(close(back.target, target) && back.pitch == pitch);
        }
    }

    #[test]
    fn looking_straight_down_a_framing_is_center_for() {
        let mut c = camera();
        c.set_bearing(30.0);
        let padding = Padding {
            top: 40.0,
            bottom: 250.0,
            left: 10.0,
            right: 0.0,
        };
        let target = lng_lat_to_world(-122.3, 47.65);
        let flat = c.center_for(target, 13.0, padding);
        c.show(
            &Framing {
                target,
                zoom: 13.0,
                bearing: 30.0,
                pitch: 0.0,
            },
            padding,
        );
        assert!(close(c.center, flat), "{:?} {flat:?}", c.center);
    }

    #[test]
    fn framings_turn_the_short_way_and_cross_the_antimeridian() {
        let a = Framing {
            target: [0.99, 0.5],
            zoom: 10.0,
            bearing: 170.0,
            pitch: 0.0,
        };
        let b = Framing {
            target: [0.01, 0.5],
            zoom: 12.0,
            bearing: -170.0,
            pitch: 60.0,
        };
        let half = a.toward(&b, 0.5);
        assert!(
            (half.target[0].rem_euclid(1.0) - 0.0).abs() < 1e-12,
            "{half:?}"
        );
        assert!((half.bearing - 180.0).abs() < 1e-12, "{half:?}");
        assert_eq!((half.zoom, half.pitch), (11.0, 30.0));
    }

    #[test]
    fn world_corners() {
        assert_eq!(lng_lat_to_world(0.0, 0.0), [0.5, 0.5]);
        let nw = lng_lat_to_world(-180.0, 85.051_128_78);
        assert!((nw[0]).abs() < 1e-9 && nw[1].abs() < 1e-6);
    }

    #[test]
    fn center_projects_to_screen_center() {
        for (bearing, pitch) in [(0.0, 0.0), (30.0, 60.0), (-120.0, 85.0)] {
            let mut c = camera();
            c.bearing = bearing;
            c.pitch = pitch;
            let w = c.screen_to_world([400.0, 300.0]).unwrap();
            assert!(close(w, c.center), "{bearing} {pitch}: {w:?}");
        }
    }

    #[test]
    fn flat_view_matches_2d_scale() {
        let c = camera();
        // 100 physical pixels right of center is 100 logical pixels of world east.
        let w = c.screen_to_world([500.0, 300.0]).unwrap();
        let expected = [c.center[0] + 100.0 / c.world_size(), c.center[1]];
        assert!(close(w, expected), "{w:?} vs {expected:?}");
    }

    #[test]
    fn bearing_rotates_clockwise() {
        let mut c = camera();
        c.bearing = 90.0;
        // Facing east, the top of the screen is east.
        let w = c.screen_to_world([400.0, 200.0]).unwrap();
        assert!(w[0] > c.center[0] && (w[1] - c.center[1]).abs() < 1e-9);
    }

    #[test]
    fn horizon_has_no_ground() {
        let mut c = camera();
        c.pitch = 85.0;
        assert!(c.screen_to_world([400.0, 0.0]).is_none());
        assert!(c.screen_to_world([400.0, 599.0]).is_some());
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut c = camera();
        c.pitch = 45.0;
        c.bearing = 20.0;
        let anchor = [100.0, 500.0];
        let before = c.screen_to_world(anchor).unwrap();
        c.zoom_around(1.5, anchor);
        let after = c.screen_to_world(anchor).unwrap();
        assert!(close(before, after));
    }

    #[test]
    fn rotate_keeps_anchor_fixed() {
        let mut c = camera();
        c.pitch = 30.0;
        let anchor = [150.0, 400.0];
        let before = c.screen_to_world(anchor).unwrap();
        c.rotate_around(-35.0, anchor);
        let after = c.screen_to_world(anchor).unwrap();
        assert!(close(before, after));
        assert_eq!(c.bearing, -35.0);
    }

    #[test]
    fn covering_tiles_flat() {
        let mut c = Camera::new([1024.0, 1024.0], 1.0);
        c.zoom = 1.0;
        let mut ids: Vec<_> = c
            .covering_tiles(0, 14, 512)
            .into_iter()
            .map(|t| (t.z, t.x, t.y))
            .collect();
        ids.sort();
        assert_eq!(ids, [(1, 0, 0), (1, 0, 1), (1, 1, 0), (1, 1, 1)]);
        let small = c.covering_tiles(0, 14, 256);
        assert!(
            small.iter().all(|t| t.z == 2),
            "256 pixel tiles come from one zoom up"
        );
    }

    #[test]
    fn pitched_cover_uses_lower_zoom_far_away() {
        let mut c = camera();
        c.pitch = 75.0;
        let tiles = c.covering_tiles(0, 14, 512);
        let zooms: Vec<_> = tiles.iter().map(|t| t.z).collect();
        assert_eq!(zooms[0], 12, "nearest the center is at the camera's zoom");
        assert!(zooms.iter().any(|&z| z < 11), "{zooms:?}");
        assert!(tiles.len() < 100, "{}", tiles.len());
    }

    #[test]
    fn wrapped_tiles_share_ids() {
        let t = Placement { z: 2, x: -1, y: 0 };
        assert_eq!(t.id(), TileId::new(2, 3, 0));
        assert_eq!(t.ancestor(2), Placement { z: 0, x: -1, y: 0 });
        assert_eq!(t.ancestor(2).id(), TileId::new(0, 0, 0));
    }

    #[test]
    fn inverse() {
        let c = camera();
        let m = c.view_projection();
        let p = mul(&m, &invert(&m).unwrap());
        for (i, col) in p.iter().enumerate() {
            for (j, v) in col.iter().enumerate() {
                assert!((v - f64::from(u8::from(i == j))).abs() < 1e-6, "{p:?}");
            }
        }
    }

    #[test]
    fn globe_center_projects_to_screen_center() {
        for (bearing, pitch) in [(0.0, 0.0), (40.0, 50.0)] {
            let mut c = camera();
            c.zoom = 3.0;
            c.globe = 1.0;
            c.bearing = bearing;
            c.pitch = pitch;
            let w = c.screen_to_world([400.0, 300.0]).unwrap();
            assert!(close(w, c.center), "{w:?} vs {:?}", c.center);
            // Off center too, the point found projects back to where it was picked.
            let w = c.screen_to_world([250.0, 400.0]).unwrap();
            let clip = transform(&c.view_matrix(), {
                let p = c.position(w, 0.0);
                [p[0], p[1], p[2], 1.0]
            });
            let screen = [
                (clip[0] / clip[3] + 1.0) / 2.0 * 800.0,
                (1.0 - clip[1] / clip[3]) / 2.0 * 600.0,
            ];
            assert!((screen[0] - 250.0).abs() < 1e-6 && (screen[1] - 400.0).abs() < 1e-6);
        }
    }

    #[test]
    fn globe_shows_only_its_near_side() {
        let mut c = Camera::new([1024.0, 1024.0], 1.0);
        c.zoom = 1.5;
        c.globe = 1.0;
        let tiles = c.covering_tiles(0, 14, 512);
        assert!(!tiles.is_empty());
        // Nothing on the far side (the antimeridian, half a world away) nor any second copy
        // of the world.
        for t in &tiles {
            let n = f64::from(t.z).exp2();
            let x = (t.x as f64 + 0.5) / n;
            assert!((x - 0.5).abs() < 0.4, "{t:?}");
        }
        let ids: std::collections::HashSet<_> = tiles.iter().map(|t| t.id()).collect();
        assert_eq!(ids.len(), tiles.len());
        // The space around the globe has no ground.
        assert!(c.screen_to_world([5.0, 5.0]).is_none());
    }

    #[test]
    fn dragging_turns_the_globe() {
        let mut c = camera();
        c.zoom = 2.0;
        c.globe = 1.0;
        let before = c.screen_to_world([500.0, 350.0]).unwrap();
        c.drag([500.0, 350.0], [400.0, 300.0]);
        let after = c.screen_to_world([400.0, 300.0]).unwrap();
        assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
    }

    #[test]
    fn terrain_lifts_the_center_and_the_camera() {
        let mut c = camera();
        c.pitch = 60.0;
        c.center_elevation = 500.0;
        // The center is still in the middle of the screen, now at its elevation.
        let w = c.screen_to_world([400.0, 300.0]).unwrap();
        assert!(close(w, c.center), "{w:?}");
        // A wall of terrain behind the center pushes the camera's pitch down.
        let center = c.center;
        let ground = move |p: [f64; 2]| Some(if p[1] > center[1] + 1e-6 { 9000.0 } else { 0.0 });
        c.avoid_terrain(&ground);
        assert!(c.pitch < 60.0, "{}", c.pitch);
        let eye = c.eye();
        assert!(eye[2] > 9000.0 * c.pixels_per_meter());
        // Picking finds the terrain where the ray first meets it.
        let mut c = camera();
        c.pitch = 45.0;
        let hill = |p: [f64; 2]| {
            Some(if p[1] < center[1] - 0.0001 {
                3000.0
            } else {
                0.0
            })
        };
        let flat = c.screen_to_world([400.0, 100.0]).unwrap();
        let on_hill = c.pick([400.0, 100.0], &hill).unwrap();
        assert!(on_hill[1] > flat[1], "the hill is hit before the plane");
    }
}
