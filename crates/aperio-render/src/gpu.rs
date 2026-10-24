use aperio_core::EditDocument;

use crate::adjust::{blend, compile, LayerProgram, Program, Skipped};
use crate::masks::rasterize;
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

/// Per-layer uniforms. Padded to 32 bytes to satisfy uniform layout rules.
#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    pixel_count: u32,
    op_start: u32,
    op_count: u32,
    has_mask: u32,
    opacity: f32,
    _pad: [f32; 3],
}

// SAFETY: 4-byte scalars only, no implicit padding.
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

    /// Render `doc` onto `img`: each layer's adjustments blended over the
    /// layers below through its mask, then the crop, which can change the
    /// image size. Returns the operations that this version cannot render yet.
    pub fn render(&self, doc: &EditDocument, img: &mut LinearImage) -> Vec<Skipped> {
        let mut program = compile(doc);
        // Rasterize masks at this image's size. Plugin masks (AI) can't be
        // generated yet, so their layers are left out and reported.
        let mut layers = Vec::new();
        for layer in std::mem::take(&mut program.layers) {
            let mask = match &layer.mask {
                None => None,
                Some(m) => match rasterize(m, img.width, img.height) {
                    Some(weights) => Some(weights),
                    None => {
                        program.skipped.push(Skipped {
                            layer: layer.index,
                            op: "plugin_mask",
                        });
                        continue;
                    }
                },
            };
            if layer.op_count > 0 && layer.opacity > 0.0 {
                layers.push((layer, mask));
            }
        }
        if !layers.is_empty() {
            match &self.gpu {
                Some(gpu) => gpu.run(&program, &layers, &mut img.pixels),
                None => {
                    for (layer, mask) in &layers {
                        for (i, px) in img.pixels.iter_mut().enumerate() {
                            let w = layer.opacity * mask.as_ref().map_or(1.0, |m| m[i]);
                            if w > 0.0 {
                                *px = blend(*px, program.apply_layer(layer, *px), w);
                            }
                        }
                    }
                }
            }
        }
        if let Some(rect) = program.crop {
            *img = img.crop(rect);
        }
        program.skipped
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

    fn run(
        &self,
        program: &Program,
        layers: &[(LayerProgram, Option<Vec<f32>>)],
        pixels: &mut [[f32; 4]],
    ) {
        use wgpu::util::DeviceExt;

        let storage_init = |label, contents: &[u8]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };
        // Bindings can't be empty, so give empty lists one dummy entry.
        let ops_buf = storage_init(
            "ops",
            if program.ops.is_empty() {
                &[0; 32]
            } else {
                bytemuck::cast_slice(&program.ops)
            },
        );
        let luts_buf = storage_init(
            "luts",
            bytemuck::cast_slice(if program.luts.is_empty() {
                &[0.0f32][..]
            } else {
                &program.luts
            }),
        );
        let no_mask = storage_init("no mask", bytemuck::cast_slice(&[0.0f32]));

        let chunk_len = CHUNK_PIXELS.min(pixels.len());
        let chunk_bytes = (chunk_len * 16) as u64;
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
        // One mask buffer per masked layer, refilled for every chunk.
        let mask_bufs: Vec<Option<wgpu::Buffer>> = layers
            .iter()
            .map(|(_, mask)| {
                mask.as_ref().map(|_| {
                    self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("mask"),
                        size: (chunk_len * 4) as u64,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
            })
            .collect();

        for (index, chunk) in pixels.chunks_mut(CHUNK_PIXELS).enumerate() {
            let offset = index * CHUNK_PIXELS;
            let count = chunk.len() as u32;
            let bytes = (chunk.len() * 16) as u64;
            self.queue
                .write_buffer(&storage, 0, bytemuck::cast_slice(chunk));

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            for ((layer, mask), mask_buf) in layers.iter().zip(&mask_bufs) {
                if let (Some(mask), Some(buf)) = (mask, mask_buf) {
                    let slice = &mask[offset..offset + chunk.len()];
                    self.queue.write_buffer(buf, 0, bytemuck::cast_slice(slice));
                }
                let params = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("params"),
                        contents: bytemuck::bytes_of(&Params {
                            pixel_count: count,
                            op_start: layer.op_start as u32,
                            op_count: layer.op_count as u32,
                            has_mask: mask.is_some() as u32,
                            opacity: layer.opacity,
                            _pad: [0.0; 3],
                        }),
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let entries = [
                    (0, params.as_entire_binding()),
                    (1, ops_buf.as_entire_binding()),
                    (2, storage.as_entire_binding()),
                    (3, luts_buf.as_entire_binding()),
                    (4, mask_buf.as_ref().unwrap_or(&no_mask).as_entire_binding()),
                ]
                .map(|(binding, resource)| wgpu::BindGroupEntry { binding, resource });
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.pipeline.get_bind_group_layout(0),
                    entries: &entries,
                });
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
    use aperio_core::mask::BrushStroke;
    use aperio_core::{ops::CurveChannel, Layer, Mask, Operation, SourceRef};

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
            Operation::Curve {
                channel: CurveChannel::Rgb,
                points: vec![[0.0, 0.05], [0.4, 0.5], [1.0, 0.95]],
            },
            Operation::Curve {
                channel: CurveChannel::Luma,
                points: vec![[0.0, 0.0], [0.6, 0.5], [1.0, 1.0]],
            },
            Operation::Curve {
                channel: CurveChannel::Blue,
                points: vec![[0.0, 0.1], [1.0, 0.9]],
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

    #[test]
    fn layers_and_masks_match_cpu_on_gpu() {
        let gpu = match Renderer::gpu() {
            Ok(gpu) => gpu,
            Err(e) if std::env::var_os("APERIO_REQUIRE_GPU").is_none() => {
                eprintln!("skipping: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let mut d = doc();
        let mut sky = Layer::new("Sky");
        sky.opacity = 0.8;
        sky.mask = Some(Mask::Linear {
            start: [0.0, 0.0],
            end: [0.0, 0.6],
        });
        sky.operations = vec![
            Operation::Exposure { ev: -0.8 },
            Operation::Curve {
                channel: CurveChannel::Blue,
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
        ];
        let mut face = Layer::new("Face");
        face.mask = Some(Mask::Radial {
            center: [0.5, 0.6],
            radius: [0.2, 0.3],
            angle_deg: 20.0,
            feather: 0.5,
            invert: false,
        });
        face.operations = vec![Operation::Exposure { ev: 0.5 }];
        let mut dodge = Layer::new("Dodge");
        dodge.mask = Some(Mask::Brush {
            strokes: vec![BrushStroke {
                points: vec![[0.2, 0.2], [0.8, 0.7]],
                size: 0.1,
                feather: 0.5,
                flow: 0.7,
                erase: false,
            }],
        });
        dodge.operations = vec![Operation::Contrast { amount: 0.4 }];
        d.layers.extend([sky, face, dodge]);

        let mut a = gradient(64);
        let mut b = a.clone();
        assert!(gpu.render(&d, &mut a).is_empty());
        Renderer::cpu().render(&d, &mut b);
        for (x, y) in a.pixels.iter().zip(&b.pixels) {
            for c in 0..4 {
                assert!((x[c] - y[c]).abs() < 1e-4, "{x:?} vs {y:?}");
            }
        }
    }

    #[test]
    fn plugin_masked_layers_are_skipped() {
        let mut d = doc();
        let mut ai = Layer::new("AI sky");
        ai.mask = Some(Mask::Plugin {
            plugin: "org.aperio.segment".into(),
            version: "0.1.0".into(),
            params: serde_json::Value::Null,
        });
        ai.operations = vec![Operation::Exposure { ev: -1.0 }];
        d.layers.push(ai);
        let mut img = gradient(4);
        let skipped = Renderer::cpu().render(&d, &mut img);
        assert_eq!(
            skipped,
            vec![Skipped {
                layer: 1,
                op: "plugin_mask"
            }]
        );
    }

    #[test]
    fn crop_runs_after_adjustments() {
        let mut d = doc();
        d.layers[0].operations = vec![
            Operation::Exposure { ev: 1.0 },
            Operation::Crop {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 0.25,
                angle_deg: 0.0,
            },
        ];
        let src = gradient(8);
        let mut img = src.clone();
        Renderer::cpu().render(&d, &mut img);
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(img.pixels[0][0], src.pixels[0][0] * 2.0);
    }
}
