//! Index buffers of 16-bit indices in batches where they fit (see
//! [`sparkle::tile::batch_indices`]), otherwise of 32-bit ones in one batch.

use sparkle::tile::{IndexBatch, batch_indices};

use crate::gpu::{Buffer, Gpu, Pass, Slice};

pub struct GpuIndices {
    buffer: Buffer,
    u16: bool,
    batches: Vec<IndexBatch>,
}

impl GpuIndices {
    /// `indices` of whole primitives of `primitive` vertices (3 for triangles, 2 for
    /// lines).
    pub fn new(gpu: &Gpu, indices: &[u32], primitive: usize) -> Self {
        match batch_indices(indices, primitive) {
            Some((short, batches)) => Self {
                buffer: gpu.buffer(bytemuck::cast_slice(&short)),
                u16: true,
                batches,
            },
            None => Self {
                buffer: gpu.buffer(bytemuck::cast_slice(indices)),
                u16: false,
                batches: vec![IndexBatch {
                    first_vertex: 0,
                    first_index: 0,
                    index_count: indices.len() as u32,
                }],
            },
        }
    }

    /// Draw every batch, after `bind` has bound the per-vertex buffers from the batch's
    /// first vertex on (see [`Buffer::slice_from`]). Everything else stays bound.
    pub fn draw<'p>(&self, pass: &mut Pass<'p>, mut bind: impl FnMut(&mut Pass<'p>, u32)) {
        let size = if self.u16 { 2 } else { 4 };
        for b in &self.batches {
            bind(pass, b.first_vertex);
            let slice = Slice::Buffer(&self.buffer, b.first_index * size, b.index_count * size);
            pass.set_index_buffer(slice, self.u16);
            pass.draw_indexed(b.index_count, 1);
        }
    }
}
