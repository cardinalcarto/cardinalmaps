//! The navigation puck (see [`sparkle_view::puck`]): one draw of a small rectangle on
//! screen, which the fragment shader cuts the chevron and the disk under it out of. It's
//! drawn over the map rather than as a tile layer, so moving it every frame costs nothing
//! but the draw: no tiles are cut or uploaded. The device is GPU-bound, so it's kept to
//! one draw of four vertices with no textures, over a few thousand pixels: the disk shares
//! the chevron's rectangle rather than taking a draw of its own.
//!
//! The chevron lies flat on the ground and the disk faces the screen, so the rectangle is
//! drawn in screen space, around both, and each fragment finds where it is in each: its
//! offset from the puck on screen for the disk, and the point of the ground under it for
//! the chevron. A ground quad around both instead would need to grow with the tilt to
//! hold the upright disk, toward ten times as long at pitch 85, and could reach behind
//! the camera; a second draw would double the per-draw cost.

use bytemuck::Zeroable;
use sparkle_view::camera::{Camera, Mat4, invert, mul, transform};
use sparkle_view::puck::{Pose, PuckStyle};

use super::Gpu;
use super::view::{DrawUniforms, View};
use crate::gpu::{Pass, Pipeline, PipelineSpec, Topology};

/// Room around the chevron and the disk, in logical pixels, for their antialiased edges.
const MARGIN: f64 = 2.0;

/// The puck to draw this frame.
pub struct PuckDraw<'a> {
    pub pose: Pose,
    pub style: &'a PuckStyle,
    /// The index of the style layer it's drawn under (the style's layer count for over
    /// all of them).
    pub before: usize,
    /// The ground's height under it, in logical pixels (0 without terrain).
    pub height: f64,
}

pub struct PuckRenderer {
    pipeline: Pipeline,
}

impl PuckRenderer {
    pub fn new(gpu: &Gpu) -> Self {
        Self {
            pipeline: gpu.pipeline(&PipelineSpec {
                program: "Puck",
                vertex_buffers: vec![Gpu::quad_layout()],
                topology: Topology::TriangleStrip,
                // Over every tile, whatever the stencil holds, and over the terrain
                // rather than into it.
                clip: false,
                ..Default::default()
            }),
        }
    }

    /// Draw `puck` into `pass`.
    pub fn draw(&self, gpu: &Gpu, pass: &mut Pass<'_>, view: &View, puck: &PuckDraw<'_>) {
        let Some(place) = placement(&view.camera, puck) else {
            return;
        };
        let style = puck.style;
        let premultiplied = |[r, g, b, a]: [f32; 4]| [r * a, g * a, b * a, a];
        let mut u = DrawUniforms::zeroed();
        u.matrix = place.ground.map(|c| c.map(|v| v as f32));
        u.pixel_ratio = view.camera.pixel_ratio as f32;
        u.extra[0] = premultiplied(style.fill);
        u.extra[1] = premultiplied(style.outline);
        u.extra[2] = [
            0.0,
            style.size,
            style.outline_width,
            style.disk_radius.max(0.0),
        ];
        u.extra[3] = premultiplied(style.disk_fill);
        u.extra[4] = premultiplied(style.disk_outline);
        u.extra[5] = [style.disk_outline_width, 0.0, 0.0, 0.0];
        u.extra[6] = place.clip.map(|v| v as f32);
        u.extra[7] = place.screen.map(|v| v as f32);
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, gpu.quad.slice());
        pass.set_uniforms(bytemuck::bytes_of(&u));
        pass.draw(4, 1);
    }
}

/// Where the puck is drawn this frame.
#[derive(Debug, Clone, Copy)]
struct Placement {
    /// The rectangle on screen in clip space (`w` 1): left, top, right, bottom.
    clip: [f64; 4],
    /// The same rectangle in logical pixels from the puck's middle on screen, `y` down:
    /// left, top, right, bottom. Where the disk is measured.
    screen: [f64; 4],
    /// Clip space (`x, y, 0, 1`) to the ground around the puck in logical pixels from
    /// its middle (`x` right, `y` back from the chevron's tip, at the puck's bearing),
    /// up to scale: divided by its `w`, it's the point of the ground drawn there. Where
    /// the chevron is measured. The map is projective, so the vertex shader can take it
    /// at the corners and the fragment shader divide, exactly.
    ground: Mat4,
}

/// Where to draw the puck, or `None` behind the camera or on the far side of the globe.
///
/// The ground around it is scaled with its distance from the camera so that the chevron
/// is drawn the same size on screen however far off it is, as navigation pucks are:
/// tilted, it still lies flat and is foreshortened, but a puck low on a pitched view
/// under a sheet doesn't shrink toward the horizon with the map. The disk is a circle on
/// screen around the puck's middle, so a tilt doesn't squash it: what it's for, hiding
/// the end of the traveled line along the direction of travel, is just what a tilt
/// foreshortens most.
fn placement(camera: &Camera, puck: &PuckDraw<'_>) -> Option<Placement> {
    let style = puck.style;
    let mut world = puck.pose.world;
    // The copy of the world the view is on.
    world[0] += (camera.center[0] - world[0]).round();
    let (sin, cos) = puck.pose.bearing.to_radians().sin_cos();
    // A logical pixel of the ground is drawn a logical pixel across at the camera's
    // distance from the center, and smaller in proportion further off: so at the
    // puck's distance (clip w), that many times bigger on the ground.
    let view = camera.view_matrix();
    let middle = camera.position(world, puck.height);
    let distance = transform(&view, [middle[0], middle[1], middle[2], 1.0])[3];
    if distance <= 0.0 || !camera.faces_camera(middle) {
        return None;
    }
    let per_pixel = distance / camera.camera_to_center() / camera.world_size();
    // Logical pixels from the puck (x right, y back from its tip) to a point of the
    // ground in the view's frame: turned clockwise by the bearing, as `y` points south.
    let ground = |x: f64, y: f64| {
        let (east, south) = (x * cos - y * sin, x * sin + y * cos);
        let at = [world[0] + east * per_pixel, world[1] + south * per_pixel];
        camera.position(at, puck.height)
    };
    // The plane of the ground there, through the chevron's square's edges (the globe
    // curves little across it), from its pixels to clip space.
    let half = f64::from(style.size) / 2.0 + MARGIN;
    let axis = |a: [f64; 3], b: [f64; 3]| {
        let d: [f64; 3] = std::array::from_fn(|i| (b[i] - a[i]) / (2.0 * half));
        [d[0], d[1], d[2], 0.0]
    };
    let place: Mat4 = [
        axis(ground(-half, 0.0), ground(half, 0.0)),
        axis(ground(0.0, -half), ground(0.0, half)),
        [0.0, 0.0, 1.0, 0.0],
        [middle[0], middle[1], middle[2], 1.0],
    ];
    let mut to_clip = mul(&view, &place);
    // Only the ground's `x, y, 1` to clip `x, y, w` matter; keeping `z` out makes the
    // matrix invertible and its inverse leave `z` 0.
    for c in &mut to_clip {
        c[2] = 0.0;
    }
    to_clip[2] = [0.0, 0.0, 1.0, 0.0];
    let ground_from_clip = invert(&to_clip)?;

    // The rectangle: round the chevron's square, at any bearing, and the disk.
    let [width, height] = camera.logical_size();
    let on_screen = |x: f64, y: f64| {
        let c = transform(&to_clip, [x, y, 0.0, 1.0]);
        (c[3] > 0.0).then(|| {
            [
                (c[0] / c[3] + 1.0) / 2.0 * width,
                (1.0 - c[1] / c[3]) / 2.0 * height,
            ]
        })
    };
    let center = on_screen(0.0, 0.0)?;
    let disk = f64::from(style.disk_radius.max(0.0)) + MARGIN;
    let mut rect = [-disk, -disk, disk, disk];
    for (x, y) in [(-half, -half), (half, -half), (-half, half), (half, half)] {
        let p = on_screen(x, y)?;
        let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
        rect = [
            rect[0].min(dx),
            rect[1].min(dy),
            rect[2].max(dx),
            rect[3].max(dy),
        ];
    }
    let clip_x = |x: f64| (center[0] + x) / width * 2.0 - 1.0;
    let clip_y = |y: f64| 1.0 - (center[1] + y) / height * 2.0;
    Some(Placement {
        clip: [
            clip_x(rect[0]),
            clip_y(rect[1]),
            clip_x(rect[2]),
            clip_y(rect[3]),
        ],
        screen: rect,
        ground: ground_from_clip,
    })
}

#[cfg(test)]
mod tests {
    use sparkle_view::camera::lng_lat_to_world;

    use super::*;

    fn at(camera: &Camera, world: [f64; 2], bearing: f64, style: &PuckStyle) -> Placement {
        let draw = PuckDraw {
            pose: Pose { world, bearing },
            style,
            before: 0,
            height: 0.0,
        };
        placement(camera, &draw).unwrap()
    }

    /// Where the point `x, y` of the ground around the puck (logical pixels from it) is
    /// drawn, in logical pixels from the puck on screen, as the shaders invert it.
    fn drawn(camera: &Camera, place: &Placement, x: f64, y: f64) -> [f64; 2] {
        let to_clip = invert(&place.ground).unwrap();
        let c = transform(&to_clip, [x, y, 0.0, 1.0]);
        let [w, h] = camera.logical_size();
        let [l, t, ..] = place.clip;
        let [sl, st, ..] = place.screen;
        let screen =
            |v: f64, lo: f64, s_lo: f64, size: f64, sign: f64| s_lo + (v - lo) * sign / 2.0 * size;
        [
            screen(c[0] / c[3], l, sl, w, 1.0),
            screen(c[1] / c[3], t, st, h, -1.0),
        ]
    }

    /// The point of the ground the fragment shader finds at `screen` (logical pixels
    /// from the puck), or `None` above the horizon.
    fn ground_at(camera: &Camera, place: &Placement, screen: [f64; 2]) -> Option<[f64; 2]> {
        let [w, h] = camera.logical_size();
        let [l, t, ..] = place.clip;
        let [sl, st, ..] = place.screen;
        let clip = [
            l + (screen[0] - sl) / w * 2.0,
            t - (screen[1] - st) / h * 2.0,
        ];
        let g = transform(&place.ground, [clip[0], clip[1], 0.0, 1.0]);
        (g[3] > 0.0).then(|| [g[0] / g[3], g[1] / g[3]])
    }

    #[test]
    fn the_rectangle_holds_the_screen_facing_disk_at_any_pitch_and_bearing() {
        let style = PuckStyle::default();
        let disk = f64::from(style.disk_radius) + MARGIN;
        for pitch in [0.0, 30.0, 60.0, 75.0, 85.0] {
            let mut camera = Camera::new([412.0, 915.0], 2.625);
            camera.look_at(-122.33, 47.6, 17.0);
            camera.set_pitch(pitch);
            for bearing in [0.0, 37.0, 90.0, 180.0, 328.0] {
                for screen in [[206.0, 457.0], [206.0, 800.0], [60.0, 700.0]] {
                    let world = camera.screen_to_world(screen).unwrap();
                    let p = at(&camera, world, bearing, &style);
                    let [l, t, r, b] = p.screen;
                    let what = format!("pitch {pitch}, bearing {bearing}, at {screen:?}");
                    // A circle of the disk's radius, plus room to antialias, round the
                    // puck's middle on screen.
                    assert!(l <= -disk && t <= -disk && r >= disk && b >= disk, "{what}");
                    // And the chevron's square on the ground, which a tilt foreshortens.
                    let half = f64::from(style.size) / 2.0 + MARGIN;
                    for (x, y) in [(-half, -half), (half, -half), (-half, half), (half, half)] {
                        let [sx, sy] = drawn(&camera, &p, x, y);
                        let e = 1e-6;
                        assert!(sx >= l - e && sx <= r + e && sy >= t - e && sy <= b + e);
                    }
                    // The clip rectangle is the same one.
                    let [w, h] = camera.logical_size();
                    let [cl, ct, cr, cb] = p.clip;
                    assert!(((cr - cl) / 2.0 * w - (r - l)).abs() < 1e-6, "{what}");
                    assert!(((ct - cb) / 2.0 * h - (b - t)).abs() < 1e-6, "{what}");
                }
            }
        }
    }

    #[test]
    fn the_shader_finds_the_ground_the_chevron_lies_on() {
        let style = PuckStyle::default();
        let mut camera = Camera::new([800.0, 600.0], 1.0);
        camera.look_at(-122.33, 47.6, 16.0);
        camera.set_pitch(60.0);
        camera.set_bearing(20.0);
        let world = camera.screen_to_world([500.0, 450.0]).unwrap();
        let p = at(&camera, world, 135.0, &style);
        let size = camera.world_size();
        let center = camera.world_to_screen(world).unwrap();
        for (x, y) in [(0.0, -24.0), (10.0, 12.0), (-20.0, 20.0)] {
            // Where the ground point `x, y` from the puck really is drawn...
            let (sin, cos) = 135f64.to_radians().sin_cos();
            let (east, south) = (x * cos - y * sin, x * sin + y * cos);
            let middle = camera.position(world, 0.0);
            let distance = transform(
                &camera.view_matrix(),
                [middle[0], middle[1], middle[2], 1.0],
            )[3];
            let per_pixel = distance / camera.camera_to_center() / size;
            let point = [world[0] + east * per_pixel, world[1] + south * per_pixel];
            let s = camera.world_to_screen(point).unwrap();
            // ... is where the shader finds it.
            let g = ground_at(&camera, &p, [s[0] - center[0], s[1] - center[1]]).unwrap();
            assert!((g[0] - x).abs() < 1e-3 && (g[1] - y).abs() < 1e-3, "{g:?}");
        }
        // The disk is measured from the puck's middle on screen.
        let g = ground_at(&camera, &p, [0.0, 0.0]).unwrap();
        assert!(g[0].abs() < 1e-6 && g[1].abs() < 1e-6, "{g:?}");
    }

    #[test]
    fn the_chevron_is_the_same_size_on_screen_near_and_far() {
        let mut camera = Camera::new([800.0, 600.0], 1.0);
        camera.look_at(-122.33, 47.6, 16.0);
        camera.set_pitch(60.0);
        let style = PuckStyle::default();
        let length = f64::from(style.size);
        let width_at = |camera: &Camera, screen: [f64; 2]| {
            let world = camera.screen_to_world(screen).unwrap();
            let p = at(camera, world, 0.0, &style);
            // Across its middle, which faces the camera square on with no bearing.
            let (l, r) = (
                drawn(camera, &p, -length / 2.0, 0.0),
                drawn(camera, &p, length / 2.0, 0.0),
            );
            (l[0] - r[0]).hypot(l[1] - r[1])
        };
        for screen in [
            [400.0, 300.0],
            [400.0, 560.0],
            [400.0, 60.0],
            [100.0, 450.0],
        ] {
            let width = width_at(&camera, screen);
            assert!((width - length).abs() < 0.05, "{screen:?}: {width}");
        }
        // Looking straight down it's as long as it's wide; tilted, it's foreshortened,
        // while the disk stays round.
        let tip_to_tail = |camera: &Camera| {
            let p = at(camera, lng_lat_to_world(-122.33, 47.6), 0.0, &style);
            let (t, b) = (
                drawn(camera, &p, 0.0, -length / 2.0),
                drawn(camera, &p, 0.0, length / 2.0),
            );
            ((t[0] - b[0]).hypot(t[1] - b[1]), p.screen)
        };
        let (tilted, rect) = tip_to_tail(&camera);
        assert!(tilted < 0.6 * length && tilted > 0.4 * length, "{tilted}");
        let disk = f64::from(style.disk_radius) + MARGIN;
        assert_eq!(rect, [-disk, -disk, disk, disk]);
        camera.set_pitch(0.0);
        assert!((tip_to_tail(&camera).0 - length).abs() < 1e-3);
    }

    #[test]
    fn without_a_disk_the_rectangle_is_the_chevrons() {
        let style = PuckStyle {
            disk_radius: 0.0,
            ..PuckStyle::default()
        };
        let mut camera = Camera::new([800.0, 600.0], 1.0);
        camera.look_at(-122.33, 47.6, 16.0);
        let p = at(&camera, lng_lat_to_world(-122.33, 47.6), 0.0, &style);
        let half = f64::from(style.size) / 2.0 + MARGIN;
        for (v, want) in p.screen.iter().zip([-half, -half, half, half]) {
            assert!((v - want).abs() < 1e-6, "{:?}", p.screen);
        }
    }
}
