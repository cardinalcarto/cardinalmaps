//! What every draw is given: the per-draw uniform block, and the camera and scene it's
//! drawn with. Mirrors `sparkle_harness`'s `render::gpu`.

use bytemuck::{Pod, Zeroable};
use sparkle::light::ExtrusionLight;
use sparkle::paint::PaintUniforms;
use sparkle::sky::SkyColors;
use sparkle::style::{Light, Sky};
use sparkle_view::camera::{Camera, Mat4};

use crate::gpu::Texture;

/// Per-draw values, bound as the `Draw` uniform block. Laid out as `std140` sees the
/// block in `shaders/common.glsl`, which declares the paint uniforms' two arrays in
/// place of [`PaintUniforms`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DrawUniforms {
    /// Tile coordinates (`0..extent`; z in logical pixels) to clip space.
    pub matrix: [[f32; 4]; 4],
    /// Tile units per logical pixel, at this tile's zoom and the camera's zoom.
    pub units_per_pixel: f32,
    pub pixel_ratio: f32,
    /// The camera's zoom.
    pub zoom: f32,
    pub extent: f32,
    /// In physical pixels.
    pub viewport: [f32; 2],
    /// In radians.
    pub bearing: f32,
    /// In radians.
    pub pitch: f32,
    /// Whatever a layer type needs besides its paint attributes.
    pub extra: [[f32; 4]; 8],
    pub paint: PaintUniforms,
    /// Positions relative to the center of the view to clip space ([`Camera::view_matrix`]),
    /// for the globe.
    pub view: [[f32; 4]; 4],
    /// The tile [`Self::matrix`] places: column, row, tiles per side at its zoom, extent.
    pub tile: [f32; 4],
    /// From Mercator to the globe (0..1), the globe's radius in logical pixels, and the
    /// longitude and latitude of the center in radians.
    pub globe: [f32; 4],
    /// Logical pixels per meter of terrain (0 without), unused, and the DEM's pixels per
    /// side without its border.
    pub terrain: [f32; 4],
    /// Where the tile is in its DEM tile: the offset of its top left corner and the
    /// fraction it spans (see `sparkle::terrain::ElevationSampler`).
    pub dem: [f32; 4],
    /// Fog: the clip `w` of the center and of the horizon, `fog-ground-blend`, and the
    /// opacity (0 for none).
    pub fog: [f32; 4],
    /// Premultiplied.
    pub fog_color: [f32; 4],
    /// The camera in [`Self::view`]'s frame.
    pub eye: [f32; 4],
}

/// Where one tile is, for draws that follow terrain and the globe: the per-tile parts of
/// [`DrawUniforms`], and the DEM texture they sample (bound as `dem`).
#[derive(Debug, Clone, Default)]
pub struct TileGeo {
    pub tile: [f32; 4],
    pub terrain: [f32; 4],
    pub dem: [f32; 4],
    pub dem_texture: Option<Texture>,
}

/// What a frame's draws need to know about the camera.
pub struct View {
    pub camera: Camera,
    pub view_projection: Mat4,
    /// The style's light at the camera's zoom and bearing, for 3D layers.
    pub light: ExtrusionLight,
    /// Terrain's logical pixels per meter, exaggeration included; 0 without terrain.
    pub z_per_meter: f32,
    /// The style's sky at the camera's zoom, if it has one.
    pub sky: Option<SkyColors>,
}

impl View {
    /// A view of the flat map.
    pub fn new(camera: &Camera, light: &Light) -> Self {
        Self::scene(camera, light, None, None)
    }

    /// A view with terrain exaggerated by `exaggeration` (if any) and the style's sky.
    pub fn scene(
        camera: &Camera,
        light: &Light,
        exaggeration: Option<f64>,
        sky: Option<&Sky>,
    ) -> Self {
        Self {
            camera: camera.clone(),
            view_projection: camera.view_projection(),
            light: ExtrusionLight::new(light, camera.zoom, camera.bearing),
            z_per_meter: exaggeration.map_or(0.0, |e| (e * camera.pixels_per_meter()) as f32),
            sky: sky.map(|s| SkyColors::evaluate(s, camera.zoom)),
        }
    }

    /// Whether anything is drawn in 3D: over terrain, on the globe or under a sky.
    pub fn is_3d(&self) -> bool {
        self.z_per_meter > 0.0 || self.camera.globe > 0.0 || self.sky.is_some()
    }

    /// The uniforms for a draw that isn't tied to a tile (`matrix` is the identity).
    pub fn uniforms(&self) -> DrawUniforms {
        let c = &self.camera;
        let mut u = DrawUniforms::zeroed();
        u.matrix = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        u.pixel_ratio = c.pixel_ratio as f32;
        u.zoom = c.zoom as f32;
        u.viewport = c.viewport.map(|v| v as f32);
        u.bearing = c.bearing.to_radians() as f32;
        u.pitch = c.pitch.to_radians() as f32;
        if c.globe > 0.0 {
            let globe = c.globe_frame();
            u.view = c.view_matrix().map(|col| col.map(|v| v as f32));
            u.globe = [
                c.globe as f32,
                globe.radius as f32,
                globe.center[0] as f32,
                globe.center[1] as f32,
            ];
        }
        let [x, y, z] = c.eye();
        u.eye = [x as f32, y as f32, z as f32, 1.0];
        if let Some(sky) = &self.sky
            && self.z_per_meter > 0.0
        {
            // Fog over terrain, fading out toward the globe whose horizon is near.
            let opacity = sparkle::sky::fog_opacity(c.pitch) * (1.0 - c.globe as f32);
            // Fog thickens toward the far end of the ground shown, which near the horizon
            // runs off toward infinity; past a few times the center's distance it's full.
            let center = c.camera_to_center();
            u.fog = [
                center as f32,
                c.furthest().min(center * 6.0) as f32,
                sky.fog_ground_blend,
                opacity,
            ];
            u.fog_color = sky.haze().0;
        }
        u
    }

    /// The per-tile uniforms of `geo`, on top of [`Self::uniforms`].
    pub fn tile_uniforms(&self, matrix: [[f32; 4]; 4], geo: &TileGeo) -> DrawUniforms {
        let mut u = self.uniforms();
        u.matrix = matrix;
        u.tile = geo.tile;
        u.terrain = geo.terrain;
        u.dem = geo.dem;
        u
    }
}
