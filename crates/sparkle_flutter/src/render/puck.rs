//! The navigation puck (see [`sparkle_view::puck`]): one draw of a small square on the
//! ground, which the fragment shader cuts the chevron out of. It's drawn over the map
//! rather than as a tile layer, so moving it every frame costs nothing but the draw: no
//! tiles are cut or uploaded. The device is GPU-bound, so it's kept to one draw of four
//! vertices with no textures, over a few hundred pixels.

use bytemuck::Zeroable;
use sparkle_view::camera::{Mat4, mul};
use sparkle_view::puck::{Pose, PuckStyle};

use super::Gpu;
use super::view::{DrawUniforms, View};
use crate::gpu::{Pass, Pipeline, PipelineSpec, Topology};

/// Room around the chevron in the square, in logical pixels, for its antialiased edge.
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
        let Some(matrix) = square(view, puck) else {
            return;
        };
        let style = puck.style;
        let premultiplied = |[r, g, b, a]: [f32; 4]| [r * a, g * a, b * a, a];
        let mut u = DrawUniforms::zeroed();
        u.matrix = matrix;
        u.pixel_ratio = view.camera.pixel_ratio as f32;
        u.extra[0] = premultiplied(style.fill);
        u.extra[1] = premultiplied(style.outline);
        let half = f64::from(style.size) / 2.0 + MARGIN;
        u.extra[2] = [half as f32, style.size, style.outline_width, 0.0];
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, gpu.quad.slice());
        pass.set_uniforms(bytemuck::bytes_of(&u));
        pass.draw(4, 1);
    }
}

/// The unit square to clip space: centered on the puck, its side the chevron's length
/// plus [`MARGIN`] on each side, turned to its bearing, on the ground (or the globe) at
/// its height. `None` on the far side of the globe.
fn square(view: &View, puck: &PuckDraw<'_>) -> Option<[[f32; 4]; 4]> {
    let camera = &view.camera;
    let half = f64::from(puck.style.size) / 2.0 + MARGIN;
    let mut world = puck.pose.world;
    // The copy of the world the view is on.
    world[0] += (camera.center[0] - world[0]).round();
    let (sin, cos) = puck.pose.bearing.to_radians().sin_cos();
    let per_pixel = 1.0 / camera.world_size();
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
