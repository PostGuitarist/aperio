//! `org.aperio.segment`: AI masks for the sky or the main subject.
//!
//! Runs open ONNX models locally with ONNX Runtime (see [`models`] for which
//! ones and their licenses). The network sees a small copy of the photo; its
//! mask is scaled back up and refined with a guided filter so edges follow
//! the photo. The plugin host caches the result, so this runs once per photo
//! and preview size.
//!
//! Sidecar mask:
//!
//! ```json
//! { "kind": "plugin", "plugin": "org.aperio.segment", "version": "0.1.0",
//!   "params": { "class": "sky", "invert": false } }
//! ```

pub mod models;
mod refine;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use aperio_plugin::{
    Image, ModelInfo, ParamSpec, Plugin, PluginError, PluginInfo, Provides, Result, API_VERSION,
};
use models::Model;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use serde_json::Value;

pub const ID: &str = "org.aperio.segment";

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];
/// Guided filter strength: lower keeps finer edges from the photo.
const REFINE_EPS: f32 = 1e-3;

pub struct Segment {
    dir: PathBuf,
    /// Loaded networks, by model id. Loading takes a second or two, so each
    /// is loaded on first use and kept.
    sessions: Mutex<HashMap<&'static str, (Session, &'static str)>>,
}

impl Segment {
    /// Use models from `dir` (see [`models::default_dir`]).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn models_dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Where each loaded model runs ("CPU", "DirectML", "Core ML"), by
    /// model id.
    pub fn devices(&self) -> Vec<(&'static str, &'static str)> {
        let sessions = self.sessions.lock().unwrap();
        let mut devices: Vec<_> = sessions.iter().map(|(id, (_, d))| (*id, *d)).collect();
        devices.sort();
        devices
    }

    /// Run `model` on `source` and return its probability map at the
    /// network's resolution.
    fn infer(&self, model: Model, source: Image<'_>) -> Result<Vec<f32>> {
        let path = model.path(&self.dir);
        if !model.is_installed(&self.dir) {
            return Err(PluginError::ModelUnavailable(format!(
                "{} isn't downloaded yet (expected at {})",
                model.name,
                path.display()
            )));
        }
        let s = model.input_size;
        let srgb: Vec<[f32; 3]> = source
            .pixels
            .iter()
            .map(|p| [to_srgb(p[0]), to_srgb(p[1]), to_srgb(p[2])])
            .collect();
        let small = refine::resize_area(&srgb, source.width, source.height, s, s);
        // NCHW, normalized with ImageNet statistics like both models' training.
        let plane = (s * s) as usize;
        let mut input = vec![0.0f32; 3 * plane];
        for (i, px) in small.iter().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = (px[c] - MEAN[c]) / STD[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, s as usize, s as usize], input))
            .map_err(|e| PluginError::Failed(e.to_string()))?;

        let mut sessions = self.sessions.lock().unwrap();
        if !sessions.contains_key(model.id) {
            sessions.insert(model.id, load(&path)?);
        }
        let (session, _) = sessions.get_mut(model.id).unwrap();
        let outputs = session
            .run(ort::inputs![tensor])
            .map_err(|e| PluginError::Failed(format!("{} failed: {e}", model.name)))?;
        let (_, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| PluginError::Failed(e.to_string()))?;
        if data.len() < plane {
            return Err(PluginError::Failed(format!(
                "{} returned too little data",
                model.name
            )));
        }
        Ok(data[..plane]
            .iter()
            .map(|&v| {
                if model.logits {
                    1.0 / (1.0 + (-v).exp())
                } else {
                    v.clamp(0.0, 1.0)
                }
            })
            .collect())
    }
}

/// Load a model, on the GPU if this platform's GPU execution provider can
/// take it, otherwise on the CPU. Returns the session and where it runs.
fn load(path: &std::path::Path) -> Result<(Session, &'static str)> {
    if let Some((name, provider)) = gpu_provider(path) {
        match build(path, Some(provider)) {
            Ok(session) => return Ok((session, name)),
            Err(e) => eprintln!("aperio: {name} unavailable, running AI masks on the CPU ({e})"),
        }
    }
    let session = build(path, None).map_err(|e| {
        PluginError::ModelUnavailable(format!("couldn't load {}: {e}", path.display()))
    })?;
    Ok((session, "CPU"))
}

fn build(
    path: &std::path::Path,
    gpu: Option<ort::ep::ExecutionProviderDispatch>,
) -> ort::Result<Session> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut builder = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(threads)?;
    if let Some(gpu) = gpu {
        // DirectML can't use ONNX Runtime's memory pattern optimization.
        builder = builder
            .with_memory_pattern(!cfg!(windows))?
            .with_execution_providers([gpu.error_on_failure()])?;
    }
    builder.commit_from_file(path)
}

/// This platform's GPU execution provider and its name: DirectML on
/// Windows, Core ML on macOS. Nodes it can't run still go to the CPU.
/// Setting `APERIO_AI_CPU=1` skips the GPU, for comparing results or working
/// around a driver problem.
fn gpu_provider(
    model: &std::path::Path,
) -> Option<(&'static str, ort::ep::ExecutionProviderDispatch)> {
    if std::env::var_os("APERIO_AI_CPU").is_some_and(|v| v != "0") {
        return None;
    }
    let _ = model;
    #[cfg(windows)]
    return Some(("DirectML", ort::ep::DirectML::default().build()));
    #[cfg(target_os = "macos")]
    return Some((
        "Core ML",
        ort::ep::CoreML::default()
            .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
            .with_compute_units(ort::ep::coreml::ComputeUnits::All)
            // Core ML compiles the model on first use; keep that for next time.
            .with_model_cache_dir(model.with_extension("coreml").display())
            .build(),
    ));
    #[allow(unreachable_code)]
    None
}

fn class() -> ParamSpec {
    ParamSpec::choice("class", "Select", &["sky", "subject"], "sky")
}

fn invert() -> ParamSpec {
    ParamSpec::boolean("invert", "Invert", false)
}

impl Plugin for Segment {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: ID.into(),
            name: "AI sky and subject".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            license: "GPL-3.0-or-later".into(),
            operation: None,
            mask: Some(Provides {
                label: "AI select".into(),
                params: vec![class(), invert()],
            }),
            models: models::ALL
                .iter()
                .map(|m| ModelInfo {
                    name: m.name.into(),
                    license: m.license.into(),
                    source: m.source.into(),
                })
                .collect(),
        }
    }

    fn mask(&self, params: &Value, source: Image<'_>) -> Result<Vec<f32>> {
        let model = match class().read_choice(params)?.as_str() {
            "subject" => models::SUBJECT,
            _ => models::SKY,
        };
        let invert = invert().read_bool(params)?;
        let s = model.input_size;
        let coarse = self.infer(model, source)?;
        let (w, h) = (source.width, source.height);
        let up = refine::upscale(&coarse, s, s, w, h);
        let guide: Vec<f32> = source
            .pixels
            .iter()
            .map(|p| to_srgb(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]))
            .collect();
        // About one network pixel at full size, so the filter can move an
        // edge by as much as the upscale blurred it.
        let radius = (w.max(h) / s).max(2) as usize;
        let mut mask = refine::guided_filter(&guide, &up, w, h, radius, REFINE_EPS);
        if invert {
            for m in &mut mask {
                *m = 1.0 - *m;
            }
        }
        Ok(mask)
    }
}

fn to_srgb(v: f32) -> f32 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flat(w: u32, h: u32) -> Vec<[f32; 4]> {
        vec![[0.2, 0.3, 0.6, 1.0]; (w * h) as usize]
    }

    #[test]
    fn reports_missing_models() {
        let plugin = Segment::new(std::env::temp_dir().join("aperio-no-models-here"));
        let px = flat(4, 4);
        let img = Image {
            width: 4,
            height: 4,
            pixels: &px,
        };
        let err = plugin.mask(&json!({"class": "subject"}), img).unwrap_err();
        assert!(matches!(err, PluginError::ModelUnavailable(_)), "{err}");
        assert!(plugin.mask(&json!({"class": "cat"}), img).is_err());
    }

    /// Runs the real models when they're downloaded (set APERIO_MODELS_DIR
    /// or run `aperio models download`); skipped otherwise.
    #[test]
    fn real_models_produce_masks_when_installed() {
        let plugin = Segment::new(models::default_dir());
        if !models::ALL
            .iter()
            .all(|m| m.is_installed(plugin.models_dir()))
        {
            eprintln!("skipping: models not downloaded");
            return;
        }
        // Top half bright blue sky, bottom half dark green ground.
        let (w, h) = (64u32, 48u32);
        let px: Vec<[f32; 4]> = (0..w * h)
            .map(|i| {
                if i / w < h / 2 {
                    [0.35, 0.55, 0.9, 1.0]
                } else {
                    [0.03, 0.06, 0.02, 1.0]
                }
            })
            .collect();
        let img = Image {
            width: w,
            height: h,
            pixels: &px,
        };
        let sky = plugin.mask(&json!({"class": "sky"}), img).unwrap();
        assert_eq!(sky.len(), (w * h) as usize);
        let top = sky[(w * 4 + w / 2) as usize];
        let bottom = sky[(w * (h - 4) + w / 2) as usize];
        assert!(top > 0.5 && bottom < 0.5, "top {top} bottom {bottom}");
        let inverted = plugin
            .mask(&json!({"class": "sky", "invert": true}), img)
            .unwrap();
        assert!((inverted[0] - (1.0 - sky[0])).abs() < 1e-6);
    }
}
