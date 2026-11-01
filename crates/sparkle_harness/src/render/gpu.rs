//! What layer renderers build on: the device, per-draw uniforms, and a pipeline helper that
//! fills in the parts every pipeline shares (shader prelude, paint attributes, multisampling,
//! depth-stencil state for tile clipping).

use std::borrow::Cow;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use sparkle::light::ExtrusionLight;
use sparkle::paint::{PaintArrays, PaintUniforms};
use sparkle::sky::SkyColors;
use sparkle::style::{Light, Sky};
use wgpu::util::DeviceExt;

use super::paint;
use crate::camera::{Camera, Mat4};

pub const DEPTH_STENCIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// The first vertex shader location of paint attributes; layer geometry uses `0..4` by
/// default (see [`PipelineSpec::paint_location`]).
pub const PAINT_LOCATION: u32 = 4;

/// Per-draw values, at a dynamic offset into a uniform buffer bound at group 0. Matches
/// `Draw` in `common.wgsl`.
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
    /// Logical pixels per meter of terrain (0 without), the tile's DEM layer in the
    /// atlas, and the DEM's pixels per side without its border.
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
/// [`DrawUniforms`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TileGeo {
    pub tile: [f32; 4],
    pub terrain: [f32; 4],
    pub dem: [f32; 4],
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

/// A sprite atlas on the GPU, for layer types that draw icons or patterns.
pub struct ImageTexture {
    pub view: wgpu::TextureView,
    /// In pixels.
    pub size: [u32; 2],
    /// Where each image is, for properties the renderer resolves itself; shared with the
    /// tile loader.
    pub atlas: std::sync::Arc<sparkle::sprite::ImageAtlas>,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// The color target's format.
    pub format: wgpu::TextureFormat,
    pub sample_count: u32,
    /// Group 0 of every pipeline: [`DrawUniforms`] at a dynamic offset.
    pub draw_layout: wgpu::BindGroupLayout,
    pub linear_sampler: wgpu::Sampler,
    pub nearest_sampler: wgpu::Sampler,
    /// The style's sprites, if any were loaded.
    pub images: Option<ImageTexture>,
}

/// Whether and how a pipeline uses the depth buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DepthMode {
    /// Ignore it: flat layers drawn in style order.
    #[default]
    Ignore,
    /// Test against it without writing.
    Test,
    /// Test and write, as 3D geometry does.
    TestWrite,
}

/// A render pipeline for [`Gpu::create_pipeline`]. Start from `Default` and fill in what
/// differs.
pub struct PipelineSpec<'a> {
    pub label: &'a str,
    /// The layer's WGSL. `common.wgsl` (with `Draw`, `draw` and helpers) and, with `paint`,
    /// the generated `PaintIn`/`paint_*` code are prepended. Entry points: `vs_main` and
    /// `fs_main`.
    pub shader: &'a str,
    /// The geometry vertex buffers; the paint buffer, if any, is appended after them.
    pub vertex_buffers: &'a [wgpu::VertexBufferLayout<'a>],
    /// The layout of the buckets this pipeline draws, if it reads paint attributes.
    pub paint: Option<&'a PaintArrays>,
    pub paint_location: u32,
    pub topology: wgpu::PrimitiveTopology,
    /// Only draw inside the tile's clip mask (stencil equal to the tile's reference).
    pub clip: bool,
    pub depth: DepthMode,
    pub blend: Option<wgpu::BlendState>,
    pub write_mask: wgpu::ColorWrites,
    /// Bind groups 1 and up.
    pub bind_group_layouts: &'a [&'a wgpu::BindGroupLayout],
    /// Overrides the stencil state `clip` implies.
    pub stencil: Option<wgpu::StencilState>,
    /// Overrides the color target format, for offscreen passes.
    pub target_format: Option<wgpu::TextureFormat>,
    /// Overrides the sample count, for offscreen passes.
    pub sample_count: Option<u32>,
    /// Offscreen passes without a depth-stencil attachment set this.
    pub no_depth_stencil: bool,
}

impl Default for PipelineSpec<'_> {
    fn default() -> Self {
        Self {
            label: "",
            shader: "",
            vertex_buffers: &[],
            paint: None,
            paint_location: PAINT_LOCATION,
            topology: wgpu::PrimitiveTopology::TriangleList,
            clip: true,
            depth: DepthMode::Ignore,
            blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
            bind_group_layouts: &[],
            stencil: None,
            target_format: None,
            sample_count: None,
            no_depth_stencil: false,
        }
    }
}

const COMMON_WGSL: &str = include_str!("../shaders/common.wgsl");

impl Gpu {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw uniforms"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(size_of::<DrawUniforms>() as u64),
                    },
                    count: None,
                },
                // The terrain's DEM atlas, which vertex shaders read elevations from.
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        Self {
            linear_sampler: sampler(wgpu::FilterMode::Linear),
            nearest_sampler: sampler(wgpu::FilterMode::Nearest),
            device,
            queue,
            format,
            sample_count,
            draw_layout,
            images: None,
        }
    }

    pub fn buffer(&self, label: &str, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
    }

    /// A bucket's paint vertex buffer, or `None` when every attribute is a uniform. It can
    /// be overwritten with [`Self::update_paint_buffer`].
    pub fn paint_buffer(&self, paint: &PaintArrays) -> Option<wgpu::Buffer> {
        (paint.stride > 0 && !paint.data.is_empty()).then(|| {
            self.buffer(
                "paint attributes",
                bytemuck::cast_slice(&paint.data),
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            )
        })
    }

    /// Overwrite a buffer from [`Self::paint_buffer`] with new values of the same layout
    /// (`layout`, the one it was made with). Returns false if `paint` doesn't fit it.
    pub fn update_paint_buffer(
        &self,
        buffer: Option<&wgpu::Buffer>,
        layout: &PaintArrays,
        paint: &PaintArrays,
    ) -> bool {
        if layout.signature() != paint.signature() || layout.stride != paint.stride {
            return false;
        }
        match buffer {
            Some(buffer) if buffer.size() == (paint.data.len() * 4) as u64 => {
                self.queue
                    .write_buffer(buffer, 0, bytemuck::cast_slice(&paint.data));
                true
            }
            Some(_) => false,
            None => paint.data.is_empty() || paint.stride == 0,
        }
    }

    /// An RGBA8 texture with premultiplied alpha, such as a decoded image.
    pub fn rgba_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> wgpu::Texture {
        self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            pixels,
        )
    }

    /// A bind group layout of `textures` 2D float textures (bindings `0..textures`) and a
    /// filtering sampler after them: what most layer types bind as group 1.
    pub fn texture_layout(&self, label: &str, textures: u32) -> wgpu::BindGroupLayout {
        let mut entries: Vec<_> = (0..textures)
            .map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                // Vertex shaders may read a texture's size, as symbols do.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            })
            .collect();
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: textures,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
        self.device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &entries,
            })
    }

    /// A bind group for a [`Self::texture_layout`] of `textures.len()` textures.
    pub fn bind_textures(
        &self,
        layout: &wgpu::BindGroupLayout,
        textures: &[&wgpu::TextureView],
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        let mut entries: Vec<_> = textures
            .iter()
            .enumerate()
            .map(|(i, view)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            })
            .collect();
        entries.push(wgpu::BindGroupEntry {
            binding: textures.len() as u32,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        })
    }

    pub fn create_pipeline(&self, spec: &PipelineSpec<'_>) -> wgpu::RenderPipeline {
        let paint_code = spec
            .paint
            .map(|p| paint::wgsl(p, spec.paint_location))
            .unwrap_or_default();
        let source = format!("{COMMON_WGSL}\n{paint_code}\n{}", spec.shader);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(spec.label),
                source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
            });

        let paint_attributes = spec
            .paint
            .map(|p| paint::vertex_attributes(p, spec.paint_location))
            .unwrap_or_default();
        let mut buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> =
            spec.vertex_buffers.iter().cloned().map(Some).collect();
        if let Some(p) = spec.paint
            && p.stride > 0
        {
            buffers.push(Some(wgpu::VertexBufferLayout {
                array_stride: u64::from(p.stride) * 4,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &paint_attributes,
            }));
        }

        let mut groups = vec![Some(&self.draw_layout)];
        groups.extend(spec.bind_group_layouts.iter().map(|&l| Some(l)));
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(spec.label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });

        let stencil = spec.stencil.clone().unwrap_or_else(|| {
            let face = wgpu::StencilFaceState {
                compare: if spec.clip {
                    wgpu::CompareFunction::Equal
                } else {
                    wgpu::CompareFunction::Always
                },
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op: wgpu::StencilOperation::Keep,
            };
            wgpu::StencilState {
                front: face,
                back: face,
                read_mask: 0xff,
                write_mask: 0,
            }
        });
        let (depth_write, depth_compare) = match spec.depth {
            DepthMode::Ignore => (false, wgpu::CompareFunction::Always),
            DepthMode::Test => (false, wgpu::CompareFunction::LessEqual),
            DepthMode::TestWrite => (true, wgpu::CompareFunction::LessEqual),
        };
        let depth_stencil = (!spec.no_depth_stencil).then(|| wgpu::DepthStencilState {
            format: DEPTH_STENCIL_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil,
            bias: Default::default(),
        });

        self.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(spec.label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: spec.topology,
                    ..Default::default()
                },
                depth_stencil,
                multisample: wgpu::MultisampleState {
                    count: spec.sample_count.unwrap_or(self.sample_count),
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: spec.target_format.unwrap_or(self.format),
                        blend: spec.blend,
                        write_mask: spec.write_mask,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
    }
}

/// Bytes per uniform buffer; each holds many draws' [`DrawUniforms`].
const CHUNK_SIZE: usize = 1 << 16;

/// Hands out [`DrawUniforms`] slots for a frame, in as many buffers as it takes. Writes are
/// collected on the CPU and uploaded by [`Self::flush`] before the frame is submitted.
pub struct UniformArena {
    chunks: Vec<Chunk>,
    stride: usize,
    used: usize,
    /// The DEM atlas every chunk's bind group includes.
    dem_atlas: wgpu::TextureView,
}

struct Chunk {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    data: Vec<u8>,
}

impl UniformArena {
    pub fn new(gpu: &Gpu) -> Self {
        let alignment = gpu.device.limits().min_uniform_buffer_offset_alignment as usize;
        // Until there's terrain, a single texel no draw reads.
        let dem_atlas = gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no DEM"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
        Self {
            chunks: Vec::new(),
            stride: size_of::<DrawUniforms>().next_multiple_of(alignment),
            used: 0,
            dem_atlas,
        }
    }

    /// Bind `atlas` (a 2D array view) as the DEM atlas from now on.
    pub fn set_dem_atlas(&mut self, gpu: &Gpu, atlas: wgpu::TextureView) {
        self.dem_atlas = atlas;
        for c in &mut self.chunks {
            c.group = Self::group(gpu, &c.buffer, &self.dem_atlas);
        }
    }

    fn group(gpu: &Gpu, buffer: &wgpu::Buffer, dem_atlas: &wgpu::TextureView) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draw uniforms"),
            layout: &gpu.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: NonZeroU64::new(size_of::<DrawUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(dem_atlas),
                },
            ],
        })
    }

    /// Start a new frame, reusing the buffers.
    pub fn reset(&mut self) {
        self.used = 0;
        self.chunks.iter_mut().for_each(|c| c.data.clear());
    }

    /// Store `u` and return the bind group and dynamic offset to draw with it.
    pub fn push(&mut self, gpu: &Gpu, u: &DrawUniforms) -> (wgpu::BindGroup, u32) {
        let per_chunk = CHUNK_SIZE / self.stride;
        let (chunk, slot) = (self.used / per_chunk, self.used % per_chunk);
        self.used += 1;
        if chunk == self.chunks.len() {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("draw uniforms"),
                size: CHUNK_SIZE as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = Self::group(gpu, &buffer, &self.dem_atlas);
            self.chunks.push(Chunk {
                buffer,
                group,
                data: Vec::with_capacity(CHUNK_SIZE),
            });
        }
        let c = &mut self.chunks[chunk];
        let offset = slot * self.stride;
        c.data.resize(offset + self.stride, 0);
        c.data[offset..offset + size_of::<DrawUniforms>()].copy_from_slice(bytemuck::bytes_of(u));
        (c.group.clone(), offset as u32)
    }

    pub fn flush(&self, queue: &wgpu::Queue) {
        for c in &self.chunks {
            if !c.data.is_empty() {
                queue.write_buffer(&c.buffer, 0, &c.data);
            }
        }
    }
}
