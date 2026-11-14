//! The navigation puck (see [`sparkle_view::puck`]): one draw of a small square on the
//! ground, which the fragment shader cuts the chevron and the disk under it out of. It's drawn over the map
//! rather than as a tile layer, so moving it every frame costs nothing but the draw: no
//! tiles are cut or uploaded. The device is GPU-bound, so it's kept to one draw of four
//! vertices with no textures, over a few thousand pixels: the disk shares the chevron's
//! square rather than taking a draw of its own.

use bytemuck::Zeroable;
use sparkle_view::camera::{Camera, Mat4, mul, transform};
use sparkle_view::puck::{Pose, PuckStyle};

use super::Gpu;
use super::view::{DrawUniforms, View};
use crate::gpu::{Pass, Pipeline, PipelineSpec, Topology};

/// Room around the chevron or disk in the square, in logical pixels, for its
/// antialiased edge.
const MARGIN: f64 = 2.0;

/// Half the square's side, in logical pixels: enough for the chevron, and for the disk,
/// which then fits whichever way the square is turned to the bearing (a circle inside
/// the square stays inside it however it turns).
fn half_side(style: &PuckStyle) -> f64 {
    (f64::from(style.size) / 2.0).max(f64::from(style.disk_radius.max(0.0))) + MARGIN
}

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
        let Some(matrix) = square(&view.camera, puck) else {
            return;
        };
        let style = puck.style;
        let premultiplied = |[r, g, b, a]: [f32; 4]| [r * a, g * a, b * a, a];
        let mut u = DrawUniforms::zeroed();
        u.matrix = matrix;
        u.pixel_ratio = view.camera.pixel_ratio as f32;
        u.extra[0] = premultiplied(style.fill);
        u.extra[1] = premultiplied(style.outline);
        u.extra[2] = [
            half_side(style) as f32,
            style.size,
            style.outline_width,
            style.disk_radius.max(0.0),
        ];
        u.extra[3] = premultiplied(style.disk_fill);
        u.extra[4] = premultiplied(style.disk_outline);
        u.extra[5] = [style.disk_outline_width, 0.0, 0.0, 0.0];
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, gpu.quad.slice());
        pass.set_uniforms(bytemuck::bytes_of(&u));
        pass.draw(4, 1);
    }
}

/// The unit square to clip space: centered on the puck, its side the chevron's length or
/// the disk's diameter, whichever is more, plus [`MARGIN`] on each side, turned to its
/// bearing, on the ground (or the globe) at
/// its height. `None` behind the camera or on the far side of the globe.
///
/// The square is scaled with its distance from the camera so that it's drawn the same
/// size on screen however far off it is, as navigation pucks are: tilted, it still lies
/// flat and is foreshortened, but a puck low on a pitched view under a sheet doesn't
/// shrink toward the horizon with the map.
fn square(camera: &Camera, puck: &PuckDraw<'_>) -> Option<[[f32; 4]; 4]> {
    let half = half_side(puck.style);
    let mut world = puck.pose.world;
    // The copy of the world the view is on.
    world[0] += (camera.center[0] - world[0]).round();
    let (sin, cos) = puck.pose.bearing.to_radians().sin_cos();
    // A logical pixel of the ground is drawn a logical pixel across at the camera's
    // distance from the center, and smaller in proportion further off: so at the
    // puck's distance (clip w), that many times bigger on the ground.
    let middle = camera.position(world, puck.height);
    let distance = transform(
        &camera.view_matrix(),
        [middle[0], middle[1], middle[2], 1.0],
    )[3];
    if distance <= 0.0 {
        return None;
    }
    let per_pixel = distance / camera.camera_to_center() / camera.world_size();
    // Logical pixels from the puck (x right, y back from its tip) to a point of the
    // ground in the view's frame: turned clockwise by the bearing, as `y` points south.
    let corner = |x: f64, y: f64| {
        let (east, south) = (x * cos - y * sin, x * sin + y * cos);
        let at = [world[0] + east * per_pixel, world[1] + south * per_pixel];
        camera.position(at, puck.height)
    };
    let origin = corner(-half, -half);
    if !camera.faces_camera(origin) {
        return None;
    }
    let right = corner(half, -half);
    let down = corner(-half, half);
    let axis = |p: [f64; 3]| [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2], 0.0];
    let place: Mat4 = [
        axis(right),
        axis(down),
        [0.0, 0.0, 1.0, 0.0],
        [origin[0], origin[1], origin[2], 1.0],
    ];
    Some(mul(&camera.view_matrix(), &place).map(|c| c.map(|v| v as f32)))
}

#[cfg(test)]
mod tests {
    use sparkle_view::camera::lng_lat_to_world;

    use super::*;

    /// Where the point `x, y` of the unit square is drawn, in logical pixels.
    fn drawn(camera: &Camera, matrix: &[[f32; 4]; 4], x: f64, y: f64) -> [f64; 2] {
        let m = matrix.map(|c| c.map(f64::from));
        let clip = transform(&m, [x, y, 0.0, 1.0]);
        let [w, h] = camera.logical_size();
        [
            (clip[0] / clip[3] + 1.0) / 2.0 * w,
            (1.0 - clip[1] / clip[3]) / 2.0 * h,
        ]
    }

    #[test]
    fn the_square_holds_the_chevron_and_the_disk() {
        let style = |size: f32, disk_radius: f32| PuckStyle {
            size,
            disk_radius,
            ..PuckStyle::default()
        };
        // A disk wider than the chevron sets the square, so no bearing clips it.
        assert_eq!(half_side(&style(48.0, 40.0)), 40.0 + MARGIN);
        // A smaller one, or none, leaves it to the chevron.
        assert_eq!(half_side(&style(48.0, 10.0)), 24.0 + MARGIN);
        assert_eq!(half_side(&style(48.0, 0.0)), 24.0 + MARGIN);
        assert_eq!(half_side(&style(48.0, -5.0)), 24.0 + MARGIN);
    }

    #[test]
    fn the_puck_is_the_same_size_on_screen_near_and_far() {
        let mut camera = Camera::new([800.0, 600.0], 1.0);
        camera.look_at(-122.33, 47.6, 16.0);
        camera.set_pitch(60.0);
        let style = PuckStyle::default();
        let side = 2.0 * half_side(&style);
        let width_at = |camera: &Camera, screen: [f64; 2]| {
            let pose = Pose {
                world: camera.screen_to_world(screen).unwrap(),
                bearing: 0.0,
            };
            let draw = PuckDraw {
                pose,
                style: &style,
                before: 0,
                height: 0.0,
            };
            let m = square(camera, &draw).unwrap();
            // Across its middle, which faces the camera square on with no bearing.
            let (l, r) = (drawn(camera, &m, 0.0, 0.5), drawn(camera, &m, 1.0, 0.5));
            (l[0] - r[0]).hypot(l[1] - r[1])
        };
        for screen in [
            [400.0, 300.0],
            [400.0, 560.0],
            [400.0, 60.0],
            [100.0, 450.0],
        ] {
            let width = width_at(&camera, screen);
            assert!((width - side).abs() < 0.05, "{screen:?}: {width}");
        }
        // Looking straight down it's as long as it's wide; tilted, it's foreshortened.
        let length = |camera: &Camera| {
            let pose = Pose {
                world: lng_lat_to_world(-122.33, 47.6),
                bearing: 0.0,
            };
            let draw = PuckDraw {
                pose,
                style: &style,
                before: 0,
                height: 0.0,
            };
            let m = square(camera, &draw).unwrap();
            let (t, b) = (drawn(camera, &m, 0.5, 0.0), drawn(camera, &m, 0.5, 1.0));
            (t[0] - b[0]).hypot(t[1] - b[1])
        };
        let tilted = length(&camera);
        assert!(tilted < 0.6 * side && tilted > 0.4 * side, "{tilted}");
        camera.set_pitch(0.0);
        assert!((length(&camera) - side).abs() < 1e-3);
    }
}
