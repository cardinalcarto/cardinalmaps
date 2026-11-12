//! Index buffers of 16-bit indices in batches where they fit (see
//! [`sparkle::tile::batch_indices`]), otherwise of 32-bit ones in one batch.

use sparkle::tile::{IndexBatch, batch_indices};

use crate::render::gpu::Gpu;

pub struct GpuIndices {
    buffer: wgpu::Buffer,
    format: wgpu::IndexFormat,
    batches: Vec<IndexBatch>,
}

impl GpuIndices {
    /// `indices` of whole primitives of `primitive` vertices (3 for triangles, 2 for
    /// lines).
    pub fn new(gpu: &Gpu, label: &str, indices: &[u32], primitive: usize) -> Self {
        let usage = wgpu::BufferUsages::INDEX;
        match batch_indices(indices, primitive) {
            Some((short, batches)) => Self {
                // wgpu wants buffers a multiple of 4 bytes long.
                buffer: gpu.buffer(label, bytemuck::cast_slice(&padded(short)), usage),
                format: wgpu::IndexFormat::Uint16,
                batches,
            },
            None => Self {
                buffer: gpu.buffer(label, bytemuck::cast_slice(indices), usage),
                format: wgpu::IndexFormat::Uint32,
                batches: vec![IndexBatch {
                    first_vertex: 0,
                    first_index: 0,
                    index_count: indices.len() as u32,
                }],
            },
        }
    }

    /// Draw every batch, with the vertex buffers bound as they are: each batch's indices
    /// count from its first vertex.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_index_buffer(self.buffer.slice(..), self.format);
        for b in &self.batches {
            let range = b.first_index..b.first_index + b.index_count;
            pass.draw_indexed(range, b.first_vertex as i32, 0..1);
        }
    }
}

fn padded(mut indices: Vec<u16>) -> Vec<u16> {
    if indices.len() % 2 == 1 {
        indices.push(0);
    }
    indices
}
