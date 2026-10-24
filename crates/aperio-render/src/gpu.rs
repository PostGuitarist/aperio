use aperio_core::EditDocument;

use crate::adjust::{apply_all, compile, AdjustOp, Skipped};
use crate::{Error, LinearImage, Result};

/// Pixels processed per GPU pass. 4M pixels is 64 MiB of RGBA f32, well under
/// the smallest storage-buffer limit wgpu guarantees (128 MiB).
const CHUNK_PIXELS: usize = 4 * 1024 * 1024;
const WORKGROUP: u32 = 256;
const MAX_GROUPS_X: u32 = 65535;

/// Applies an edit document to decoded images.
///
/// Uses the GPU when one is available and otherwise falls back to an
/// equivalent CPU path, so export still works on machines without a usable
/// GPU driver.
pub struct Renderer {
    gpu: Option<Gpu>,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    pixel_count: u32,
    op_count: u32,
}

// SAFETY: two u32s, no padding.
unsafe impl bytemuck::Zeroable for Params {}
unsafe impl bytemuck::Pod for Params {}

impl Renderer {
    /// Try to set up the GPU, falling back to the CPU if that fails.
    pub fn new() -> Self {
        match pollster::block_on(Gpu::new()) {
            Ok(gpu) => Self { gpu: Some(gpu) },
            Err(e) => {
                log::warn!("rendering on the CPU: {e}");
                Self::cpu()
            }
        }
    }

    /// A renderer that never touches the GPU.
    pub fn cpu() -> Self {
        Self { gpu: None }
    }

    /// Set up the GPU or fail. Mostly useful for tests.
    pub fn gpu() -> Result<Self> {
        Ok(Self {
            gpu: Some(pollster::block_on(Gpu::new())?),
        })
    }

    pub fn uses_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Render `doc` onto `img` in place. Returns the operations that this
    /// version cannot render yet.
    pub fn render(&self, doc: &EditDocument, img: &mut LinearImage) -> Vec<Skipped> {
        let (ops, skipped) = compile(doc);
        if !ops.is_empty() {
            match &self.gpu {
                Some(gpu) => gpu.run(&ops, &mut img.pixels),
                None => img
                    .pixels
                    .iter_mut()
                    .for_each(|px| *px = apply_all(&ops, *px)),
            }
        }
        skipped
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpu {
    async fn new() -> Result<Self> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .map_err(|e| Error::Gpu(e.to_string()))?;
        log::info!("GPU: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("aperio"),
                ..Default::default()
            })
            .await
            .map_err(|e| Error::Gpu(e.to_string()))?;
        let module = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("adjust"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Ok(Self {
            device,
            queue,
            pipeline,
        })
    }

    fn run(&self, ops: &[AdjustOp], pixels: &mut [[f32; 4]]) {
        use wgpu::util::DeviceExt;

        let ops_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ops"),
                contents: bytemuck::cast_slice(ops),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let chunk_bytes = (CHUNK_PIXELS.min(pixels.len()) * 16) as u64;
        let storage = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pixels"),
            size: chunk_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: chunk_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        for chunk in pixels.chunks_mut(CHUNK_PIXELS) {
            let count = chunk.len() as u32;
            let bytes = (chunk.len() * 16) as u64;
            let params = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("params"),
                    contents: bytemuck::bytes_of(&Params {
                        pixel_count: count,
                        op_count: ops.len() as u32,
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: ops_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: storage.as_entire_binding(),
                    },
                ],
            });

            self.queue
                .write_buffer(&storage, 0, bytemuck::cast_slice(chunk));
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                let groups = count.div_ceil(WORKGROUP);
                pass.dispatch_workgroups(
                    groups.min(MAX_GROUPS_X),
                    groups.div_ceil(MAX_GROUPS_X),
                    1,
                );
            }
            encoder.copy_buffer_to_buffer(&storage, 0, &readback, 0, bytes);
            self.queue.submit([encoder.finish()]);

            let slice = readback.slice(..bytes);
            slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback buffer"));
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("GPU device lost");
            {
                let view = slice.get_mapped_range().expect("read mapped buffer");
                chunk.copy_from_slice(bytemuck::cast_slice(&view));
            }
            readback.unmap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperio_core::{Operation, SourceRef};

    fn gradient(n: u32) -> LinearImage {
        let pixels = (0..n * n)
            .map(|i| {
                let t = i as f32 / (n * n) as f32;
                [t, 1.0 - t, (t * 7.0).fract(), 1.0]
            })
            .collect();
        LinearImage::new(n, n, pixels)
    }

    fn doc() -> EditDocument {
        let mut d = EditDocument::new(SourceRef {
            file_name: "t.jpg".into(),
            size_bytes: 0,
            sha256: None,
        });
        d.layers[0].operations = vec![
            Operation::WhiteBalance {
                temperature_k: 5000.0,
                tint: 0.1,
            },
            Operation::Exposure { ev: 0.7 },
            Operation::Contrast { amount: 0.3 },
            Operation::Color {
                saturation: 0.2,
                vibrance: 0.4,
            },
        ];
        d
    }

    #[test]
    fn gpu_matches_cpu() {
        // CI sets APERIO_REQUIRE_GPU (with a software Vulkan driver) so this
        // can't silently skip there.
        let gpu = match Renderer::gpu() {
            Ok(gpu) => gpu,
            Err(e) if std::env::var_os("APERIO_REQUIRE_GPU").is_none() => {
                eprintln!("skipping: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let mut a = gradient(64);
        let mut b = a.clone();
        gpu.render(&doc(), &mut a);
        Renderer::cpu().render(&doc(), &mut b);
        for (x, y) in a.pixels.iter().zip(&b.pixels) {
            for c in 0..4 {
                assert!((x[c] - y[c]).abs() < 1e-4, "{x:?} vs {y:?}");
            }
        }
    }

    #[test]
    fn empty_document_leaves_pixels_untouched() {
        let mut img = gradient(8);
        let before = img.clone();
        let mut d = doc();
        d.layers[0].operations.clear();
        Renderer::new().render(&d, &mut img);
        assert_eq!(img, before);
    }
}
