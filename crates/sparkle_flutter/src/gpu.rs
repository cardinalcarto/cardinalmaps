//! The GPU as Flutter's GPU API (`package:flutter_gpu`) offers it, driven from here: every
//! resource, pipeline and draw becomes a command in a stream that the Dart side replays
//! once per frame (`lib/src/executor.dart`).
//!
//! The stream is a list of 32-bit words: an opcode ([`Op`]), then its operands. Floats
//! are stored as their bits; strings as a byte length followed by the bytes, padded to a
//! whole word. Bulk data goes into three byte arrays next to the stream, and commands
//! refer to it by offset and length: the per-draw uniforms, vertices that change every
//! frame, and the contents of new buffers and textures, which the Dart side copies into
//! them. Uniforms start at multiples of the device's uniform alignment, so they can be
//! bound where they are; as every draw's are the same size, a draw's block is at the
//! same offset from one frame to the next, so the Dart side can keep what binds it.
//!
//! Resources are numbered here and live on the Dart side until their handles here are
//! dropped, which queues a free command. Texture 0 is the frame's own color target.
//!
//! Pipeline state that Flutter GPU sets per render pass (blending, depth and stencil
//! tests, primitive type) travels with each [`Pipeline`] and is set when it's bound;
//! a pipeline proper is only a pair of shaders and a vertex layout.
//!
//! Every method takes `&self`, so layer renderers can create resources while drawing, as
//! with wgpu; the stream sits behind a mutex that is only ever taken briefly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Opcodes. Mirrored in `executor.dart`.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
enum Op {
    /// `id, vertex name, fragment name`: shaders from the bundle.
    DefineProgram = 1,
    /// `id, buffer count`, then per buffer `stride, step (0 vertex, 1 instance), attribute
    /// count`, then per attribute `format, offset, name`.
    DefineLayout = 2,
    /// `id, upload offset, upload length`.
    CreateBuffer = 3,
    /// `id`.
    FreeBuffer = 4,
    /// `id, width, height, format, sample count, flags (1 render target, 2 sampled), data
    /// offset, upload length` (length 0: no contents).
    CreateTexture = 5,
    /// `id, upload offset, upload length`: replace the whole contents.
    WriteTexture = 6,
    /// `id`.
    FreeTexture = 7,
    /// `color, resolve (or NONE), color load, color store, clear r, g, b, a, depth-stencil
    /// (or NONE), depth load, depth store, clear depth, stencil load, stencil store, clear
    /// stencil`.
    BeginPass = 8,
    EndPass = 9,
    /// `program, layout`.
    SetPipeline = 10,
    /// `blend, depth compare, depth write, stencil compare, stencil pass op, stencil read
    /// mask, stencil write mask, primitive`.
    SetState = 11,
    /// `reference`.
    SetStencilReference = 12,
    /// `slot, buffer (or NONE for frame data), offset, length`.
    SetVertexBuffer = 13,
    /// `buffer (or NONE for frame data), offset, length, index type (0 u16, 1 u32)`.
    SetIndexBuffer = 14,
    /// `uniforms offset, length`: the `Draw` uniform block of both stages.
    SetUniforms = 15,
    /// `name, texture, filter (0 nearest, 1 linear), address u, address v (0 clamp, 1
    /// repeat)`. Shaders without a sampler of that name ignore it.
    SetTexture = 16,
    /// `vertex count, instance count`.
    Draw = 17,
    /// `index count, instance count`.
    DrawIndexed = 18,
    /// `name, buffer (or NONE for frame data), offset, length`: a uniform block other than
    /// `Draw`, of the stages that have it.
    SetUniformBlock = 19,
}

/// Stands for "no resource" (or frame data) in operands.
const NONE: u32 = u32::MAX;

/// The texture the frame is drawn into.
pub const SCREEN: u32 = 0;

/// What the device can do, as the Dart side found out.
#[derive(Debug, Clone, Copy)]
pub struct GpuInfo {
    /// Offsets of uniform blocks in a buffer must be multiples of this.
    pub uniform_alignment: u32,
    /// 4 with multisampling, else 1.
    pub sample_count: u32,
    /// Whether `Rgba16Float` textures can be rendered to and blended.
    pub half_float: bool,
}

/// Texture formats, as the Dart side maps them to Flutter GPU's.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// The frame's color format.
    Screen = 0,
    Rgba8 = 1,
    /// Stored as `Rgba8` with the value in red: Impeller's GLES backend accepts `R8`
    /// textures but they sample as zero on some drivers (Adreno 610).
    R8 = 2,
    Rgba16Float = 3,
    /// The device's default depth-stencil format.
    DepthStencil = 4,
}

/// One-channel pixels as RGBA8 ones with the value in red. See [`TextureFormat::R8`].
fn widen_r8(pixels: &[u8]) -> Vec<u8> {
    pixels.iter().flat_map(|&r| [r, 0, 0, 0]).collect()
}

/// Vertex attribute formats. Flutter GPU only takes 32-bit ones, and shaders must declare
/// exactly the type an attribute has.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VertexFormat {
    Float32 = 0,
    Float32x2 = 1,
    // Unused for now, but the Dart side takes formats by these numbers.
    #[allow(dead_code)]
    Float32x3 = 2,
    Float32x4 = 3,
}

impl VertexFormat {
    pub fn size(self) -> u32 {
        4 * (self as u32 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepMode {
    Vertex = 0,
    Instance = 1,
}

/// One vertex buffer slot of a [`VertexLayout`]; slots are numbered by their order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VertexBufferLayout {
    pub stride: u32,
    pub step: StepMode,
    /// `(shader input name, format, byte offset)`.
    pub attributes: Vec<(String, VertexFormat, u32)>,
}

impl VertexBufferLayout {
    /// A per-vertex buffer of `attributes` packed one after another.
    pub fn packed(attributes: &[(&str, VertexFormat)]) -> Self {
        let mut offset = 0;
        let attributes = attributes
            .iter()
            .map(|&(name, format)| {
                let a = (name.to_owned(), format, offset);
                offset += format.size();
                a
            })
            .collect();
        Self {
            stride: offset,
            step: StepMode::Vertex,
            attributes,
        }
    }
}

pub type VertexLayout = Vec<VertexBufferLayout>;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// Overwrite.
    Replace = 0,
    /// Premultiplied source over destination.
    #[default]
    Premultiplied = 1,
    /// Source plus destination.
    Add = 2,
    /// Leave the color as it is: draws that only write depth or stencil.
    NoColor = 3,
}

/// As Flutter GPU's `CompareFunction`, all of whose values are listed so that the
/// numbering matches.
#[allow(dead_code)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compare {
    Never = 0,
    Always = 1,
    Less = 2,
    Equal = 3,
    LessEqual = 4,
    Greater = 5,
    NotEqual = 6,
    GreaterEqual = 7,
}

/// As Flutter GPU's `StencilOperation` (the first values, numbered as there).
#[allow(dead_code)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StencilOp {
    Keep = 0,
    Zero = 1,
    Replace = 2,
}

/// As Flutter GPU's `PrimitiveType`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Topology {
    #[default]
    TriangleList = 0,
    TriangleStrip = 1,
    LineList = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StencilState {
    pub compare: Compare,
    pub pass_op: StencilOp,
    pub read_mask: u32,
    pub write_mask: u32,
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

/// The state Flutter GPU sets per pass rather than per pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DrawState {
    blend: Blend,
    depth_compare: Compare,
    depth_write: bool,
    stencil: StencilState,
    topology: Topology,
}

/// A pipeline for [`Gpu::pipeline`]. Start from `Default` and fill in what differs.
pub struct PipelineSpec<'a> {
    /// The shaders `<Name>Vertex` and `<Name>Fragment` of the bundle.
    pub program: &'a str,
    pub vertex_buffers: VertexLayout,
    pub topology: Topology,
    /// Only draw inside the tile's clip mask (stencil equal to the tile's reference).
    pub clip: bool,
    pub depth: DepthMode,
    pub blend: Blend,
    /// Overrides the stencil state `clip` implies.
    pub stencil: Option<StencilState>,
}

impl Default for PipelineSpec<'_> {
    fn default() -> Self {
        Self {
            program: "",
            vertex_buffers: Vec::new(),
            topology: Topology::TriangleList,
            clip: true,
            depth: DepthMode::Ignore,
            blend: Blend::Premultiplied,
            stencil: None,
        }
    }
}

/// Shaders, a vertex layout and the state to draw with: what [`Pass::set_pipeline`] binds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pipeline {
    program: u32,
    layout: u32,
    state: DrawState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Buffer,
    Texture,
}

/// What resource handles share with the [`Gpu`]: ids and the frees not yet sent.
#[derive(Default)]
struct Registry {
    next_id: u32,
    freed: Vec<(Kind, u32)>,
}

struct Handle {
    id: u32,
    kind: Kind,
    registry: Arc<Mutex<Registry>>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Ok(mut r) = self.registry.lock() {
            r.freed.push((self.kind, self.id));
        }
    }
}

/// A buffer on the Dart side, freed when the last clone is dropped.
#[derive(Clone)]
pub struct Buffer {
    handle: Arc<Handle>,
    size: u32,
}

impl Buffer {
    /// The whole buffer, for binding.
    pub fn slice(&self) -> Slice<'_> {
        Slice::Buffer(self, 0, self.size)
    }

    /// The buffer from vertex `first` on, for vertices of `stride` bytes: for drawing an
    /// index batch (see [`crate::render::indices::GpuIndices::draw`]).
    pub fn slice_from(&self, first: u32, stride: usize) -> Slice<'_> {
        let offset = (first * stride as u32).min(self.size);
        Slice::Buffer(self, offset, self.size - offset)
    }
}

/// A texture on the Dart side, freed when the last clone is dropped (except the frame's
/// target, which isn't).
#[derive(Clone)]
pub struct Texture {
    handle: Option<Arc<Handle>>,
    pub width: u32,
    pub height: u32,
    /// An [`TextureFormat::R8`] texture, whose contents are widened to RGBA8.
    r8: bool,
}

impl Texture {
    fn id(&self) -> u32 {
        self.handle.as_ref().map_or(SCREEN, |h| h.id)
    }

    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }
}

impl std::fmt::Debug for Texture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Texture({} {}x{})", self.id(), self.width, self.height)
    }
}

/// Vertex or index data to bind: part of a buffer, or bytes for this frame only.
#[derive(Clone, Copy)]
pub enum Slice<'a> {
    /// `(buffer, byte offset, byte length)`.
    Buffer(&'a Buffer, u32, u32),
    Bytes(&'a [u8]),
    /// Bytes already in this frame's data (see [`Pass::frame_data`]), `(offset, length)`.
    Frame(u32, u32),
}

/// How a texture is sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sampler {
    pub linear: bool,
    pub repeat_u: bool,
    pub repeat_v: bool,
}

impl Sampler {
    pub const LINEAR: Self = Self {
        linear: true,
        repeat_u: false,
        repeat_v: false,
    };
    pub const NEAREST: Self = Self {
        linear: false,
        repeat_u: false,
        repeat_v: false,
    };
}

/// As Flutter GPU's `LoadAction`.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Load<T> {
    Clear(T),
    Load,
    DontCare,
}

impl<T> Load<T> {
    fn code(&self) -> u32 {
        match self {
            Load::Clear(_) => 0,
            Load::Load => 1,
            Load::DontCare => 2,
        }
    }
}

/// As Flutter GPU's `StoreAction`.
#[allow(clippy::enum_variant_names)]
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Store = 0,
    DontCare = 1,
    /// Resolve into the pass's resolve texture, discarding the multisampled one.
    Resolve = 2,
    /// Resolve, and keep the multisampled image for later passes to load.
    StoreAndResolve = 3,
}

/// A render pass for [`Gpu::begin_pass`].
pub struct PassDesc<'a> {
    pub color: &'a Texture,
    pub resolve: Option<&'a Texture>,
    pub color_load: Load<[f32; 4]>,
    pub color_store: Store,
    pub depth_stencil: Option<DepthStencilDesc<'a>>,
}

pub struct DepthStencilDesc<'a> {
    pub texture: &'a Texture,
    pub depth_load: Load<f32>,
    pub depth_store: Store,
    pub stencil_load: Load<u32>,
    pub stencil_store: Store,
}

/// The most a stream keeps allocated for uploads once they're done with. A frame takes
/// in whole tiles, and a dense one is tens of megabytes: kept, that would stay allocated
/// in every stream that ever carried one.
const KEPT_UPLOAD_CAPACITY: usize = 4 << 20;

/// Commands and their data, as sent to the Dart side.
#[derive(Default)]
pub struct Stream {
    pub words: Vec<u32>,
    /// The draws' uniform blocks.
    pub uniforms: Vec<u8>,
    /// Vertices this frame's draws bind.
    pub data: Vec<u8>,
    /// The contents of new buffers and textures.
    pub uploads: Vec<u8>,
}

impl Stream {
    fn op(&mut self, op: Op, operands: &[u32]) {
        self.words.push(op as u32);
        self.words.extend_from_slice(operands);
    }

    fn string(&mut self, s: &str) {
        self.words.push(s.len() as u32);
        for chunk in s.as_bytes().chunks(4) {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            self.words.push(u32::from_le_bytes(word));
        }
    }

    /// Append `bytes` to the data at a multiple of `alignment`; returns `(offset, length)`.
    fn push(&mut self, bytes: &[u8], alignment: usize) -> [u32; 2] {
        push(&mut self.data, bytes, alignment)
    }

    /// Append resource contents to the uploads; returns `(offset, length)`.
    fn upload(&mut self, bytes: &[u8]) -> [u32; 2] {
        push(&mut self.uploads, bytes, 16)
    }

    /// Empty, keeping the allocations, except what the uploads took past
    /// [`KEPT_UPLOAD_CAPACITY`].
    fn clear(&mut self) {
        self.words.clear();
        self.uniforms.clear();
        self.data.clear();
        self.uploads.clear();
        self.uploads.shrink_to(KEPT_UPLOAD_CAPACITY);
    }
}

fn push(to: &mut Vec<u8>, bytes: &[u8], alignment: usize) -> [u32; 2] {
    let offset = to.len().next_multiple_of(alignment);
    to.resize(offset, 0);
    to.extend_from_slice(bytes);
    [offset as u32, bytes.len() as u32]
}

pub struct Gpu {
    pub info: GpuInfo,
    stream: Mutex<Stream>,
    registry: Arc<Mutex<Registry>>,
    programs: Mutex<HashMap<String, u32>>,
    layouts: Mutex<HashMap<VertexLayout, u32>>,
    /// The frame's color target.
    screen: Mutex<Texture>,
    /// Bound as `dem` to draws that sample no terrain.
    no_dem: Texture,
    /// Bytes of every buffer and texture created so far, freed or not.
    created: AtomicUsize,
}

impl Gpu {
    pub fn new(info: GpuInfo) -> Self {
        let registry = Arc::new(Mutex::new(Registry {
            // Texture 0 is the screen.
            next_id: 1,
            freed: Vec::new(),
        }));
        let mut gpu = Self {
            info,
            stream: Mutex::new(Stream::default()),
            registry,
            programs: Mutex::new(HashMap::new()),
            layouts: Mutex::new(HashMap::new()),
            screen: Mutex::new(Texture {
                handle: None,
                width: 1,
                height: 1,
                r8: false,
            }),
            no_dem: Texture {
                handle: None,
                width: 1,
                height: 1,
                r8: false,
            },
            created: AtomicUsize::new(0),
        };
        gpu.no_dem = gpu.rgba_texture(1, 1, &[0; 4]);
        gpu
    }

    fn stream(&self) -> std::sync::MutexGuard<'_, Stream> {
        self.stream
            .lock()
            .expect("the GPU stream is never poisoned")
    }

    fn next_id(&self, kind: Kind) -> Arc<Handle> {
        let mut r = self.registry.lock().expect("never poisoned");
        let id = r.next_id;
        r.next_id += 1;
        Arc::new(Handle {
            id,
            kind,
            registry: self.registry.clone(),
        })
    }

    /// The frame's color target, `size` physical pixels.
    pub fn screen(&self) -> Texture {
        self.screen.lock().expect("never poisoned").clone()
    }

    pub fn set_screen_size(&self, [width, height]: [u32; 2]) {
        let mut screen = self.screen.lock().expect("never poisoned");
        screen.width = width.max(1);
        screen.height = height.max(1);
    }

    /// A 1x1 texture for draws that read no terrain.
    pub fn no_dem(&self) -> &Texture {
        &self.no_dem
    }

    /// Everything recorded since the last call, then frees of dropped resources.
    ///
    /// `recycled`, a stream taken before that the Dart side is done with, is reused for
    /// what's recorded next, saving its allocations.
    pub fn take(&self, mut recycled: Stream) -> Stream {
        recycled.clear();
        // Frees go last: a resource dropped while a frame was recorded may still be used
        // by that frame's earlier commands. Taking them under the stream's lock means
        // every freed resource's creation is in this stream or an earlier one.
        let mut stream = self.stream();
        let freed = std::mem::take(&mut self.registry.lock().expect("never poisoned").freed);
        let mut out = std::mem::replace(&mut *stream, recycled);
        drop(stream);
        for (kind, id) in freed {
            let op = match kind {
                Kind::Buffer => Op::FreeBuffer,
                Kind::Texture => Op::FreeTexture,
            };
            out.op(op, &[id]);
        }
        out
    }

    /// Bytes of resource contents recorded since the last [`Self::take`].
    pub fn uploaded(&self) -> usize {
        self.stream().uploads.len()
    }

    /// Bytes of every buffer and texture created so far, freed or not: what something
    /// creates is the difference from before to after.
    pub fn created_bytes(&self) -> usize {
        self.created.load(Ordering::Relaxed)
    }

    pub fn buffer(&self, contents: &[u8]) -> Buffer {
        self.created.fetch_add(contents.len(), Ordering::Relaxed);
        let handle = self.next_id(Kind::Buffer);
        let mut s = self.stream();
        let [offset, len] = s.upload(contents);
        s.op(Op::CreateBuffer, &[handle.id, offset, len]);
        Buffer {
            handle,
            size: contents.len() as u32,
        }
    }

    /// A texture, with `contents` if given (tightly packed rows).
    pub fn texture(
        &self,
        [width, height]: [u32; 2],
        format: TextureFormat,
        sample_count: u32,
        render_target: bool,
        contents: Option<&[u8]>,
    ) -> Texture {
        let handle = self.next_id(Kind::Texture);
        let (width, height) = (width.max(1), height.max(1));
        let r8 = format == TextureFormat::R8;
        let format = if r8 { TextureFormat::Rgba8 } else { format };
        let texel = match format {
            TextureFormat::Rgba16Float => 8,
            // The screen's format and the depth-stencil one are 4 bytes on every device
            // we know of.
            TextureFormat::Screen
            | TextureFormat::Rgba8
            | TextureFormat::R8
            | TextureFormat::DepthStencil => 4,
        };
        self.created.fetch_add(
            (width * height * sample_count) as usize * texel,
            Ordering::Relaxed,
        );
        let mut s = self.stream();
        let [offset, len] = contents.map_or([0, 0], |c| {
            if r8 {
                s.upload(&widen_r8(c))
            } else {
                s.upload(c)
            }
        });
        // Multisampled textures can't be sampled.
        let flags = u32::from(render_target) | if sample_count == 1 { 2 } else { 0 };
        s.op(
            Op::CreateTexture,
            &[
                handle.id,
                width,
                height,
                format as u32,
                sample_count,
                flags,
                offset,
                len,
            ],
        );
        Texture {
            handle: Some(handle),
            width,
            height,
            r8,
        }
    }

    /// An RGBA8 texture with premultiplied alpha, such as a decoded image.
    pub fn rgba_texture(&self, width: u32, height: u32, pixels: &[u8]) -> Texture {
        self.texture(
            [width, height],
            TextureFormat::Rgba8,
            1,
            false,
            Some(pixels),
        )
    }

    /// An offscreen color target of the frame's format (or `format`) that can be sampled
    /// once resolved.
    pub fn target(&self, size: [u32; 2], format: TextureFormat, sample_count: u32) -> Texture {
        self.texture(size, format, sample_count, true, None)
    }

    /// Replace the whole contents of `texture`.
    pub fn write_texture(&self, texture: &Texture, contents: &[u8]) {
        let mut s = self.stream();
        let [offset, len] = if texture.r8 {
            s.upload(&widen_r8(contents))
        } else {
            s.upload(contents)
        };
        s.op(Op::WriteTexture, &[texture.id(), offset, len]);
    }

    pub fn pipeline(&self, spec: &PipelineSpec<'_>) -> Pipeline {
        let program = {
            let mut programs = self.programs.lock().expect("never poisoned");
            match programs.get(spec.program) {
                Some(&id) => id,
                None => {
                    let id = programs.len() as u32;
                    programs.insert(spec.program.to_owned(), id);
                    let mut s = self.stream();
                    s.op(Op::DefineProgram, &[id]);
                    s.string(&format!("{}Vertex", spec.program));
                    s.string(&format!("{}Fragment", spec.program));
                    id
                }
            }
        };
        let layout = {
            let mut layouts = self.layouts.lock().expect("never poisoned");
            match layouts.get(&spec.vertex_buffers) {
                Some(&id) => id,
                None => {
                    let id = layouts.len() as u32;
                    layouts.insert(spec.vertex_buffers.clone(), id);
                    let mut s = self.stream();
                    s.op(Op::DefineLayout, &[id, spec.vertex_buffers.len() as u32]);
                    for b in &spec.vertex_buffers {
                        s.words
                            .extend([b.stride, b.step as u32, b.attributes.len() as u32]);
                        for (name, format, offset) in &b.attributes {
                            s.words.extend([*format as u32, *offset]);
                            s.string(name);
                        }
                    }
                    id
                }
            }
        };
        let stencil = spec.stencil.unwrap_or(StencilState {
            compare: if spec.clip {
                Compare::Equal
            } else {
                Compare::Always
            },
            pass_op: StencilOp::Keep,
            read_mask: 0xff,
            write_mask: 0,
        });
        let (depth_write, depth_compare) = match spec.depth {
            DepthMode::Ignore => (false, Compare::Always),
            DepthMode::Test => (false, Compare::LessEqual),
            DepthMode::TestWrite => (true, Compare::LessEqual),
        };
        Pipeline {
            program,
            layout,
            state: DrawState {
                blend: spec.blend,
                depth_compare,
                depth_write,
                stencil,
                topology: spec.topology,
            },
        }
    }

    /// Start a render pass; it ends when the returned [`Pass`] is dropped.
    pub fn begin_pass(&self, desc: &PassDesc<'_>) -> Pass<'_> {
        let mut words = vec![
            desc.color.id(),
            desc.resolve.map_or(NONE, Texture::id),
            desc.color_load.code(),
            desc.color_store as u32,
        ];
        let clear = match desc.color_load {
            Load::Clear(c) => c,
            _ => [0.0; 4],
        };
        words.extend(clear.map(f32::to_bits));
        match &desc.depth_stencil {
            Some(ds) => {
                let depth = match ds.depth_load {
                    Load::Clear(d) => d,
                    _ => 1.0,
                };
                let stencil = match ds.stencil_load {
                    Load::Clear(s) => s,
                    _ => 0,
                };
                words.extend([
                    ds.texture.id(),
                    ds.depth_load.code(),
                    ds.depth_store as u32,
                    depth.to_bits(),
                    ds.stencil_load.code(),
                    ds.stencil_store as u32,
                    stencil,
                ]);
            }
            None => words.extend([NONE, 2, 1, 1f32.to_bits(), 2, 1, 0]),
        }
        self.stream().op(Op::BeginPass, &words);
        Pass {
            gpu: self,
            pipeline: None,
        }
    }
}

/// A render pass being recorded. Mirrors what layer renderers used of `wgpu::RenderPass`.
pub struct Pass<'a> {
    gpu: &'a Gpu,
    /// The last pipeline bound, so binding it again sends nothing.
    pipeline: Option<Pipeline>,
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        self.gpu.stream().op(Op::EndPass, &[]);
    }
}

impl Pass<'_> {
    pub fn set_pipeline(&mut self, pipeline: &Pipeline) {
        if self.pipeline.as_ref() == Some(pipeline) {
            return;
        }
        let mut s = self.gpu.stream();
        if self.pipeline.map(|p| (p.program, p.layout)) != Some((pipeline.program, pipeline.layout))
        {
            s.op(Op::SetPipeline, &[pipeline.program, pipeline.layout]);
        }
        if self.pipeline.map(|p| p.state) != Some(pipeline.state) {
            let st = &pipeline.state;
            s.op(
                Op::SetState,
                &[
                    st.blend as u32,
                    st.depth_compare as u32,
                    u32::from(st.depth_write),
                    st.stencil.compare as u32,
                    st.stencil.pass_op as u32,
                    st.stencil.read_mask,
                    st.stencil.write_mask,
                    st.topology as u32,
                ],
            );
        }
        self.pipeline = Some(*pipeline);
    }

    pub fn set_stencil_reference(&mut self, reference: u32) {
        self.gpu.stream().op(Op::SetStencilReference, &[reference]);
    }

    fn slice_words(s: &mut Stream, slice: Slice<'_>) -> [u32; 3] {
        match slice {
            Slice::Buffer(b, offset, len) => [b.handle.id, offset, len],
            Slice::Bytes(bytes) => {
                let [offset, len] = s.push(bytes, 16);
                [NONE, offset, len]
            }
            Slice::Frame(offset, len) => [NONE, offset, len],
        }
    }

    /// Put `bytes` in this frame's data once, aligned for uniform blocks, for commands to
    /// bind as often as they like.
    pub fn frame_data(&mut self, bytes: &[u8]) -> Slice<'static> {
        let alignment = self.gpu.info.uniform_alignment.max(16) as usize;
        let [offset, len] = self.gpu.stream().push(bytes, alignment);
        Slice::Frame(offset, len)
    }

    /// Bind `slice` as the uniform block `name` of the bound shaders that have one; it
    /// must be aligned as [`Self::frame_data`] aligns.
    pub fn set_uniform_block(&mut self, name: &str, slice: Slice<'_>) {
        let mut s = self.gpu.stream();
        let [b, offset, len] = Self::slice_words(&mut s, slice);
        s.words.push(Op::SetUniformBlock as u32);
        s.string(name);
        s.words.extend([b, offset, len]);
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, slice: Slice<'_>) {
        let mut s = self.gpu.stream();
        let [b, offset, len] = Self::slice_words(&mut s, slice);
        s.op(Op::SetVertexBuffer, &[slot, b, offset, len]);
    }

    /// Bind 32-bit indices (or 16-bit with `u16`).
    pub fn set_index_buffer(&mut self, slice: Slice<'_>, u16: bool) {
        let mut s = self.gpu.stream();
        let [b, offset, len] = Self::slice_words(&mut s, slice);
        s.op(Op::SetIndexBuffer, &[b, offset, len, u32::from(!u16)]);
    }

    /// Store `uniforms` for the next draws as the `Draw` block of both shader stages.
    pub fn set_uniforms(&mut self, uniforms: &[u8]) {
        let alignment = self.gpu.info.uniform_alignment.max(16) as usize;
        let mut s = self.gpu.stream();
        let [offset, len] = push(&mut s.uniforms, uniforms, alignment);
        s.op(Op::SetUniforms, &[offset, len]);
    }

    /// Bind `texture` to the sampler `name` of the bound shaders.
    pub fn set_texture(&mut self, name: &str, texture: &Texture, sampler: Sampler) {
        let mut s = self.gpu.stream();
        s.words.push(Op::SetTexture as u32);
        s.string(name);
        s.words.extend([
            texture.id(),
            u32::from(sampler.linear),
            u32::from(sampler.repeat_u),
            u32::from(sampler.repeat_v),
        ]);
    }

    pub fn draw(&mut self, vertices: u32, instances: u32) {
        self.gpu.stream().op(Op::Draw, &[vertices, instances]);
    }

    pub fn draw_indexed(&mut self, indices: u32, instances: u32) {
        self.gpu.stream().op(Op::DrawIndexed, &[indices, instances]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_dont_keep_a_big_upload_allocated() {
        let gpu = Gpu::new(GpuInfo {
            uniform_alignment: 256,
            sample_count: 1,
            half_float: false,
        });
        let big = vec![0; 8 * KEPT_UPLOAD_CAPACITY];
        let _buffer = gpu.buffer(&big);
        let carried = gpu.take(Stream::default());
        assert!(carried.uploads.capacity() >= big.len());
        // Once Dart is done with it, it's recycled to record the next frame in.
        drop(gpu.take(carried));
        assert!(gpu.stream().uploads.capacity() <= KEPT_UPLOAD_CAPACITY);
    }
}
