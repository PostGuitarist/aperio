use std::sync::Arc;

use aperio_core::{EditDocument, Mask};
use aperio_plugin::{Host, Image, ImageMut, PluginError, Progress};
use rayon::prelude::*;

use crate::adjust::{blend, compile, LayerProgram, PluginStep, Program, Skipped};
use crate::masks::MaskCache;
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
    plugins: Option<Arc<Host>>,
    masks: MaskCache,
}

/// Returned when a caller cancels [`Renderer::run_source_steps`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// A layer ready to run, with its mask rasterized at the image's size.
type Prepared = (LayerProgram, Option<Arc<Vec<f32>>>);

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
            Ok(gpu) => Self {
                gpu: Some(gpu),
                plugins: None,
                masks: MaskCache::default(),
            },
            Err(e) => {
                log::warn!("rendering on the CPU: {e}");
                Self::cpu()
            }
        }
    }

    /// A renderer that never touches the GPU.
    pub fn cpu() -> Self {
        Self {
            gpu: None,
            plugins: None,
            masks: MaskCache::default(),
        }
    }

    /// Render plugin operations and masks with these plugins. Without a
    /// host, plugin steps are skipped and reported.
    pub fn with_plugins(mut self, host: Arc<Host>) -> Self {
        self.plugins = Some(host);
        self
    }

    pub fn plugins(&self) -> Option<&Host> {
        self.plugins.as_deref()
    }

    /// Set up the GPU or fail. Mostly useful for tests.
    pub fn gpu() -> Result<Self> {
        Ok(Self {
            gpu: Some(pollster::block_on(Gpu::new())?),
            plugins: None,
            masks: MaskCache::default(),
        })
    }

    pub fn uses_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Weights for a brush or gradient mask at this size, reusing recent
    /// work. `None` for plugin masks.
    pub fn drawn_mask(&self, mask: &Mask, width: u32, height: u32) -> Option<Arc<Vec<f32>>> {
        self.masks.rasterize(mask, width, height)
    }

    /// Run the document's source steps (such as denoising) on `img`, which
    /// should be the full-size photo: each enabled layer's source steps, in
    /// document order, ignoring layer masks and opacity. Run this before
    /// [`Renderer::render`], which leaves source steps out, and before
    /// making a preview-sized copy. Results are cached by the plugin host,
    /// so running it again with unchanged steps is quick.
    ///
    /// Returns the steps that couldn't run, or [`Cancelled`] if `progress`
    /// asked to stop (`img` is then unchanged).
    pub fn run_source_steps(
        &self,
        doc: &EditDocument,
        img: &mut LinearImage,
        progress: Progress<'_>,
    ) -> std::result::Result<Vec<Skipped>, Cancelled> {
        let mut skipped = Vec::new();
        let Some(host) = &self.plugins else {
            return Ok(skipped);
        };
        let mut result: Option<Arc<Vec<[f32; 4]>>> = None;
        for (layer, step) in self.source_steps(doc) {
            let input = Image {
                width: img.width,
                height: img.height,
                pixels: result.as_deref().map_or(&img.pixels, |p| p),
            };
            match host.source(&step.plugin, &step.version, &step.params, input, progress) {
                Ok(pixels) => result = Some(pixels),
                Err(PluginError::Cancelled) => return Err(Cancelled),
                Err(e) => skipped.push(Skipped {
                    layer,
                    op: step.plugin.clone(),
                    reason: e.to_string(),
                }),
            }
        }
        if let Some(pixels) = result {
            img.pixels.copy_from_slice(&pixels);
        }
        Ok(skipped)
    }

    /// The document's source steps that [`Renderer::run_source_steps`]
    /// runs, with their layer index, in order. Empty without plugins.
    pub fn source_steps(&self, doc: &EditDocument) -> Vec<(usize, PluginStep)> {
        let Some(host) = &self.plugins else {
            return Vec::new();
        };
        compile(doc)
            .layers
            .into_iter()
            .flat_map(|l| l.plugin_steps.into_iter().map(move |s| (l.index, s)))
            .filter(|(_, s)| host.is_source(&s.plugin))
            .collect()
    }

    /// Render `doc` onto `img`: each layer's adjustments blended over the
    /// layers below through its mask, then the crop, which can change the
    /// image size. Returns the steps that couldn't be rendered (a plugin
    /// that isn't installed, for example), which were left out. Source steps
    /// are left out without a report: see [`Renderer::run_source_steps`].
    pub fn render(&self, doc: &EditDocument, img: &mut LinearImage) -> Vec<Skipped> {
        let mut program = compile(doc);
        if let Some(host) = &self.plugins {
            for layer in &mut program.layers {
                layer.plugin_steps.retain(|s| !host.is_source(&s.plugin));
            }
        }
        let mut skipped = std::mem::take(&mut program.skipped);
        // Every mask is made before any layer runs, so plugin masks see the
        // photo without adjustments.
        let mut layers: Vec<Prepared> = Vec::new();
        for layer in std::mem::take(&mut program.layers) {
            if layer.opacity <= 0.0 || (layer.op_count == 0 && layer.plugin_steps.is_empty()) {
                continue;
            }
            let mask = match &layer.mask {
                None => None,
                Some(Mask::Plugin {
                    plugin,
                    version,
                    params,
                }) => match self.plugin_mask(plugin, version, params, img) {
                    Ok(m) => Some(m),
                    Err(reason) => {
                        skipped.push(Skipped {
                            layer: layer.index,
                            op: plugin.clone(),
                            reason: format!("mask left out: {reason}"),
                        });
                        continue;
                    }
                },
                Some(m) => self.drawn_mask(m, img.width, img.height),
            };
            layers.push((layer, mask));
        }

        // Runs of plain layers go to the GPU together; a layer with plugin
        // steps is run on its own copy and blended back on the CPU.
        let mut start = 0;
        while start < layers.len() {
            let end = layers[start..]
                .iter()
                .position(|(l, _)| !l.plugin_steps.is_empty())
                .map_or(layers.len(), |i| start + i);
            if end > start {
                self.run_layers(&program, &layers[start..end], &mut img.pixels);
            }
            if let Some((layer, mask)) = layers.get(end) {
                self.run_plugin_layer(&program, layer, mask.as_deref(), img, &mut skipped);
            }
            start = end + 1;
        }

        if let Some(rect) = program.crop {
            *img = img.crop(rect);
        }
        skipped
    }

    fn plugin_mask(
        &self,
        plugin: &str,
        version: &str,
        params: &serde_json::Value,
        source: &LinearImage,
    ) -> std::result::Result<Arc<Vec<f32>>, String> {
        let host = self.plugins.as_ref().ok_or("no plugins are loaded")?;
        let image = Image {
            width: source.width,
            height: source.height,
            pixels: &source.pixels,
        };
        host.mask(plugin, version, params, image)
            .map_err(|e| e.to_string())
    }

    /// Run a layer that has plugin operations: its per-pixel steps and plugin
    /// steps in order on a copy, then blend the copy back through the mask.
    fn run_plugin_layer(
        &self,
        program: &Program,
        layer: &LayerProgram,
        mask: Option<&Vec<f32>>,
        img: &mut LinearImage,
        skipped: &mut Vec<Skipped>,
    ) {
        let mut out = img.pixels.clone();
        let segment = |from: usize, to: usize| LayerProgram {
            op_start: layer.op_start + from,
            op_count: to - from,
            opacity: 1.0,
            mask: None,
            plugin_steps: Vec::new(),
            ..layer.clone()
        };
        let mut done = 0;
        for step in &layer.plugin_steps {
            if step.at > done {
                self.run_layers(program, &[(segment(done, step.at), None)], &mut out);
                done = step.at;
            }
            let result = match &self.plugins {
                None => Err("no plugins are loaded".to_string()),
                Some(host) => host
                    .apply(
                        &step.plugin,
                        &step.version,
                        &step.params,
                        ImageMut {
                            width: img.width,
                            height: img.height,
                            pixels: &mut out,
                        },
                        mask.map(|m| m.as_slice()),
                    )
                    .map_err(|e| e.to_string()),
            };
            if let Err(reason) = result {
                skipped.push(Skipped {
                    layer: layer.index,
                    op: step.plugin.clone(),
                    reason,
                });
            }
        }
        if layer.op_count > done {
            self.run_layers(program, &[(segment(done, layer.op_count), None)], &mut out);
        }
        img.pixels
            .par_iter_mut()
            .zip(out.par_iter())
            .enumerate()
            .for_each(|(i, (px, new))| {
                let w = layer.opacity * mask.map_or(1.0, |m| m[i]);
                if w > 0.0 {
                    *px = blend(*px, *new, w);
                }
            });
    }

    /// Run layers' per-pixel steps, each blended through its mask.
    fn run_layers(&self, program: &Program, layers: &[Prepared], pixels: &mut [[f32; 4]]) {
        if layers.iter().all(|(l, _)| l.op_count == 0) {
            return;
        }
        match &self.gpu {
            Some(gpu) => gpu.run(program, layers, pixels),
            None => {
                for (layer, mask) in layers {
                    pixels.par_iter_mut().enumerate().for_each(|(i, px)| {
                        let w = layer.opacity * mask.as_ref().map_or(1.0, |m| m[i]);
                        if w > 0.0 {
                            *px = blend(*px, program.apply_layer(layer, *px), w);
                        }
                    });
                }
            }
        }
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

    fn run(&self, program: &Program, layers: &[Prepared], pixels: &mut [[f32; 4]]) {
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
            Operation::Tone {
                highlights: -0.6,
                shadows: 0.5,
                whites: 0.3,
                blacks: -0.2,
            },
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

    /// Test plugin: its mask selects the left half of the image; its
    /// operation scales by `gain`.
    struct Halves;

    impl aperio_plugin::Plugin for Halves {
        fn info(&self) -> aperio_plugin::PluginInfo {
            aperio_plugin::PluginInfo {
                id: "org.example.halves".into(),
                name: "Halves".into(),
                version: "1.0.0".into(),
                api_version: aperio_plugin::API_VERSION,
                license: "MIT".into(),
                operation: None,
                mask: None,
                source: None,
                models: vec![],
            }
        }

        fn apply(
            &self,
            params: &serde_json::Value,
            image: ImageMut<'_>,
        ) -> aperio_plugin::Result<()> {
            let g = params["gain"].as_f64().unwrap_or(1.0) as f32;
            for px in image.pixels {
                for c in &mut px[..3] {
                    *c *= g;
                }
            }
            Ok(())
        }

        fn mask(
            &self,
            _: &serde_json::Value,
            source: Image<'_>,
        ) -> aperio_plugin::Result<Vec<f32>> {
            Ok((0..source.width * source.height)
                .map(|i| {
                    if i % source.width < source.width / 2 {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect())
        }
    }

    fn with_plugins(r: Renderer) -> Renderer {
        let mut host = Host::new();
        host.register(Halves).unwrap();
        host.register(aperio_plugin_vignette::Vignette).unwrap();
        r.with_plugins(Arc::new(host))
    }

    fn halves(params: serde_json::Value) -> Operation {
        Operation::Plugin {
            plugin: "org.example.halves".into(),
            version: "1.0.0".into(),
            params,
        }
    }

    fn flat(n: u32) -> LinearImage {
        LinearImage::new(n, n, vec![[0.25, 0.25, 0.25, 1.0]; (n * n) as usize])
    }

    /// Test source step: adds `add` to every channel, counting its runs,
    /// and cancels when progress asks it to.
    struct Lift(Arc<std::sync::atomic::AtomicUsize>);

    impl aperio_plugin::Plugin for Lift {
        fn info(&self) -> aperio_plugin::PluginInfo {
            aperio_plugin::PluginInfo {
                id: "org.example.lift".into(),
                name: "Lift".into(),
                version: "1.0.0".into(),
                api_version: aperio_plugin::API_VERSION,
                license: "MIT".into(),
                operation: None,
                mask: None,
                source: Some(aperio_plugin::Provides {
                    label: "Lift".into(),
                    params: vec![],
                }),
                models: vec![],
            }
        }

        fn source(
            &self,
            params: &serde_json::Value,
            image: ImageMut<'_>,
            progress: Progress<'_>,
        ) -> aperio_plugin::Result<()> {
            progress.report(0.5)?;
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let add = params["add"].as_f64().unwrap_or(0.0) as f32;
            for px in image.pixels {
                for c in &mut px[..3] {
                    *c += add;
                }
            }
            Ok(())
        }
    }

    fn lift(add: f32) -> Operation {
        Operation::Plugin {
            plugin: "org.example.lift".into(),
            version: "1.0.0".into(),
            params: serde_json::json!({ "add": add }),
        }
    }

    #[test]
    fn source_steps_run_once_before_layers_and_render_skips_them() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut host = Host::new();
        host.register(Lift(runs.clone())).unwrap();
        let r = Renderer::cpu().with_plugins(Arc::new(host));
        let mut d = doc();
        d.layers[0].operations = vec![Operation::Exposure { ev: 1.0 }, lift(0.25)];
        // A source step on an upper layer runs too, after the base one,
        // ignoring the layer's mask and opacity.
        let mut upper = Layer::new("Upper");
        upper.opacity = 0.5;
        upper.mask = Some(Mask::Plugin {
            plugin: "org.aperio.segment".into(),
            version: "0.1.0".into(),
            params: serde_json::Value::Null,
        });
        upper.operations = vec![lift(0.5)];
        d.layers.push(upper);

        let mut img = flat(4);
        assert_eq!(
            r.run_source_steps(&d, &mut img, Progress::none()),
            Ok(vec![])
        );
        assert_eq!(img.pixels[0], [1.0, 1.0, 1.0, 1.0]);
        // Rendering leaves the source steps out: only the exposure runs, and
        // the upper layer has nothing left to do, so its missing mask
        // doesn't matter.
        let skipped = r.render(&d, &mut img);
        assert_eq!(img.pixels[0], [2.0, 2.0, 2.0, 1.0]);
        assert_eq!(skipped, vec![]);

        // The same steps on the same photo come from the cache.
        let mut again = flat(4);
        r.run_source_steps(&d, &mut again, Progress::none())
            .unwrap();
        assert_eq!(again.pixels[0], [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2);

        // A disabled layer's steps don't run.
        d.layers[1].enabled = false;
        let mut img = flat(4);
        r.run_source_steps(&d, &mut img, Progress::none()).unwrap();
        assert_eq!(img.pixels[0], [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn source_steps_can_be_cancelled_and_report_missing_plugins() {
        let mut host = Host::new();
        host.register(Lift(Default::default())).unwrap();
        let r = Renderer::cpu().with_plugins(Arc::new(host));
        let mut d = doc();
        d.layers[0].operations = vec![lift(0.25)];
        let mut img = flat(2);
        let stop = |_: f32| false;
        assert_eq!(
            r.run_source_steps(&d, &mut img, Progress::new(&stop)),
            Err(Cancelled)
        );
        assert_eq!(img, flat(2));

        // Without the plugin, the step isn't known to be a source step: the
        // renderer reports it like any missing plugin operation.
        let mut img = flat(2);
        let r = Renderer::cpu().with_plugins(Arc::new(Host::new()));
        assert_eq!(
            r.run_source_steps(&d, &mut img, Progress::none()),
            Ok(vec![])
        );
        let skipped = r.render(&d, &mut img);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].reason.contains("not installed"), "{skipped:?}");
    }

    #[test]
    fn missing_plugins_are_reported_and_left_out() {
        let mut d = doc();
        d.layers[0].operations = vec![
            Operation::Exposure { ev: 1.0 },
            halves(serde_json::json!({"gain": 4})),
        ];
        let mut ai = Layer::new("AI sky");
        ai.mask = Some(Mask::Plugin {
            plugin: "org.aperio.segment".into(),
            version: "0.1.0".into(),
            params: serde_json::Value::Null,
        });
        ai.operations = vec![Operation::Exposure { ev: -1.0 }];
        d.layers.push(ai);
        let mut img = flat(4);
        let skipped = with_plugins(Renderer::cpu()).render(&d, &mut img);
        // The base layer's exposure and plugin ran; the AI layer didn't.
        assert_eq!(img.pixels[0], [2.0, 2.0, 2.0, 1.0]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(
            (skipped[0].layer, skipped[0].op.as_str()),
            (1, "org.aperio.segment")
        );
        assert!(skipped[0].reason.contains("not installed"), "{skipped:?}");

        let mut img = flat(4);
        let skipped = Renderer::cpu().render(&d, &mut img);
        assert_eq!(img.pixels[0], [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(skipped.len(), 2);
    }

    #[test]
    fn plugin_steps_run_in_order_through_the_layer_mask() {
        let mut d = doc();
        d.layers[0].operations.clear();
        let mut left = Layer::new("Left");
        left.mask = Some(Mask::Plugin {
            plugin: "org.example.halves".into(),
            version: "1.0.0".into(),
            params: serde_json::json!({}),
        });
        // Plugin steps sit between per-pixel ones: x2, x3, then a curve that
        // tops out at 0.5. In order, 0.25 * 2 * 3 = 1.5 is above the curve's
        // range, so it scales by the curve's end (0.5 sRGB); the other order
        // would give about 0.64.
        left.operations = vec![
            Operation::Exposure { ev: 1.0 },
            halves(serde_json::json!({"gain": 3})),
            Operation::Curve {
                channel: CurveChannel::Rgb,
                points: vec![[0.0, 0.0], [0.5, 0.5], [1.0, 0.5]],
            },
        ];
        d.layers.push(left);
        let mut img = flat(4);
        assert!(with_plugins(Renderer::cpu())
            .render(&d, &mut img)
            .is_empty());
        let expected = 1.5 * crate::color::srgb_to_linear(0.5);
        assert!(
            (img.pixels[0][0] - expected).abs() < 1e-4,
            "{:?}",
            img.pixels[0]
        );
        assert_eq!(img.pixels[3], [0.25, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn plugin_layers_match_cpu_on_gpu() {
        let gpu = match Renderer::gpu() {
            Ok(gpu) => with_plugins(gpu),
            Err(e) if std::env::var_os("APERIO_REQUIRE_GPU").is_none() => {
                eprintln!("skipping: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let mut d = doc();
        d.layers[0].operations.push(Operation::Plugin {
            plugin: aperio_plugin_vignette::ID.into(),
            version: "1.0.0".into(),
            params: serde_json::json!({"amount": -0.6}),
        });
        d.layers[0].operations.push(Operation::Exposure { ev: 0.3 });
        let mut a = gradient(64);
        let mut b = a.clone();
        assert!(gpu.render(&d, &mut a).is_empty());
        with_plugins(Renderer::cpu()).render(&d, &mut b);
        for (x, y) in a.pixels.iter().zip(&b.pixels) {
            for c in 0..4 {
                assert!((x[c] - y[c]).abs() < 1e-4, "{x:?} vs {y:?}");
            }
        }
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
