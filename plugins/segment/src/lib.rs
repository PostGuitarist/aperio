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
//!
//! Subjects take `"detail": "best"` (BiRefNet, the default) or `"fast"`
//! (ISNet, an optional download).

pub mod models;
mod refine;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aperio_plugin::{
    Image, ModelInfo, ParamSpec, Plugin, PluginError, PluginInfo, Provides, Result, API_VERSION,
};
use models::Model;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use serde_json::Value;

pub const ID: &str = "org.aperio.segment";

/// Guided filter strength: lower keeps finer edges from the photo.
const REFINE_EPS: f32 = 1e-3;

pub struct Segment {
    dir: PathBuf,
    /// Loaded networks, by model id. Loading takes a second or two, so each
    /// is loaded on first use and kept.
    sessions: Mutex<HashMap<&'static str, (Session, &'static str)>>,
    /// Recent network outputs, so variants of one mask (a subject and its
    /// inverse, the background) and a mask computed ahead of time by
    /// [`Segment::prepare`] don't run the network again.
    outputs: Mutex<VecDeque<(u64, Arc<Vec<f32>>)>>,
}

/// How many network outputs to keep (each is at most 4 MB).
const OUTPUT_CACHE: usize = 4;

impl Segment {
    /// Use models from `dir` (see [`models::default_dir`]).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            sessions: Mutex::new(HashMap::new()),
            outputs: Mutex::new(VecDeque::new()),
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

    /// Run the installed models on `source` ahead of time (sky first, as it's
    /// quick), so AI masks for this photo are ready when they're asked for.
    /// Stops early if `keep_going` turns false, for example when another
    /// photo is opened.
    pub fn prepare(&self, source: Image<'_>, keep_going: impl Fn() -> bool) {
        for model in [models::SKY, models::SUBJECT] {
            if !keep_going() || !model.is_installed(&self.dir) {
                continue;
            }
            let _ = self.infer(model, source);
        }
    }

    /// Run `model` on `source` and return its probability map at the
    /// network's resolution.
    fn infer(&self, model: Model, source: Image<'_>) -> Result<Arc<Vec<f32>>> {
        let key = fingerprint(model.id, source);
        let cached = || {
            let outputs = self.outputs.lock().unwrap();
            outputs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, o)| o.clone())
        };
        if let Some(out) = cached() {
            return Ok(out);
        }
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
        // NCHW, normalized like the model's training data.
        let plane = (s * s) as usize;
        let mut input = vec![0.0f32; 3 * plane];
        for (i, px) in small.iter().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = (px[c] - model.mean[c]) / model.std[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, s as usize, s as usize], input))
            .map_err(|e| PluginError::Failed(e.to_string()))?;

        let mut sessions = self.sessions.lock().unwrap();
        // Another thread may have run this while we waited for the lock.
        if let Some(out) = cached() {
            return Ok(out);
        }
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
        let out: Arc<Vec<f32>> = Arc::new(
            data[..plane]
                .iter()
                .map(|&v| {
                    if model.logits {
                        1.0 / (1.0 + (-v).exp())
                    } else {
                        v.clamp(0.0, 1.0)
                    }
                })
                .collect(),
        );
        let mut outputs = self.outputs.lock().unwrap();
        if outputs.len() == OUTPUT_CACHE {
            outputs.pop_front();
        }
        outputs.push_back((key, out.clone()));
        Ok(out)
    }
}

/// Identifies a model run: the model and the image (its size plus a sample
/// of its pixels, enough to tell photos and preview sizes apart).
fn fingerprint(model: &str, source: Image<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (model, source.width, source.height).hash(&mut h);
    let step = (source.pixels.len() / 4096).max(1);
    for px in source.pixels.iter().step_by(step) {
        for c in px {
            c.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

/// Load a model, on the GPU if one of this platform's GPU execution
/// providers can take it, otherwise on the CPU. Returns the session and
/// where it runs.
fn load(path: &Path) -> Result<(Session, &'static str)> {
    let gpus = gpu_providers(path)
        .into_iter()
        .map(|(name, remember, provider)| (name, remember, Some(provider)));
    first_that_loads(path, gpus.chain([("CPU", None, None)]), build).map_err(|e| {
        PluginError::ModelUnavailable(format!("couldn't load {}: {e}", path.display()))
    })
}

/// Try `candidates` (name, tag to remember a failure by, provider) in order
/// and return the first session `build` makes. Compiling a model for Core ML
/// can take a minute before it fails (it can't convert every network), so
/// for a provider with a tag a failure is remembered in a file next to the
/// model and the provider is skipped from then on. Delete
/// `<model>.<tag>.unsupported` to try it again. Failures that are quick and
/// may not last, such as no GPU being found, aren't remembered.
fn first_that_loads<P, S>(
    path: &Path,
    candidates: impl IntoIterator<Item = (&'static str, Option<&'static str>, Option<P>)>,
    build: impl Fn(&Path, Option<P>) -> std::result::Result<S, ort::Error>,
) -> std::result::Result<(S, &'static str), ort::Error> {
    let mut last = None;
    for (name, tag, provider) in candidates {
        let marker = tag.map(|tag| path.with_extension(format!("{tag}.unsupported")));
        if marker.as_ref().is_some_and(|m| m.exists()) {
            continue;
        }
        let is_gpu = provider.is_some();
        match build(path, provider) {
            Ok(session) => return Ok((session, name)),
            Err(e) if is_gpu => {
                eprintln!("aperio: {name} can't run {}: {e}", path.display());
                if let Some(marker) = marker {
                    let _ = std::fs::write(marker, e.to_string());
                }
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| ort::Error::new("no way to run the model")))
}

type GpuProvider = (
    &'static str,
    Option<&'static str>,
    ort::ep::ExecutionProviderDispatch,
);

fn build(path: &Path, gpu: Option<ort::ep::ExecutionProviderDispatch>) -> ort::Result<Session> {
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

/// This platform's GPU execution providers, best first, with a display name
/// and the tag a failure is remembered by (see [`first_that_loads`]):
/// DirectML on Windows, Core ML on macOS. Nodes a provider can't run still
/// go to the CPU. Setting `APERIO_AI_CPU=1` skips the GPU, for comparing
/// results or working around a driver problem.
fn gpu_providers(model: &Path) -> Vec<GpuProvider> {
    if std::env::var_os("APERIO_AI_CPU").is_some_and(|v| v != "0") {
        return Vec::new();
    }
    let _ = model;
    #[cfg(windows)]
    return vec![("DirectML", None, ort::ep::DirectML::default().build())];
    #[cfg(target_os = "macos")]
    {
        use ort::ep::coreml::{ComputeUnits, ModelFormat};
        // Only Core ML's ML Program format: the older neural network format
        // takes BiRefNet but splits it with the CPU so much that it ran
        // several times slower than the CPU alone.
        let provider = ort::ep::CoreML::default()
            .with_model_format(ModelFormat::MLProgram)
            .with_compute_units(ComputeUnits::All)
            // Core ML compiles the model on first use; keep that for next time.
            .with_model_cache_dir(model.with_extension("coreml").display())
            .build();
        return vec![("Core ML", Some("coreml"), provider)];
    }
    #[allow(unreachable_code)]
    Vec::new()
}

fn class() -> ParamSpec {
    ParamSpec::choice("class", "Select", &["sky", "subject"], "sky")
}

/// Which subject model: BiRefNet ("best") or ISNet ("fast", an optional
/// download). Sky masks ignore it.
fn detail() -> ParamSpec {
    ParamSpec::choice("detail", "Subject detail", &["best", "fast"], "best")
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
                params: vec![class(), detail(), invert()],
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
        let model = match (
            class().read_choice(params)?.as_str(),
            detail().read_choice(params)?.as_str(),
        ) {
            // A sidecar asking for the fast model still renders where only
            // the best one is downloaded; not the other way round, so a
            // photo never silently gets the rougher mask.
            ("subject", "fast")
                if models::FAST_SUBJECT.is_installed(&self.dir)
                    || !models::SUBJECT.is_installed(&self.dir) =>
            {
                models::FAST_SUBJECT
            }
            ("subject", _) => models::SUBJECT,
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
    use std::fs::File;

    #[test]
    fn remembers_gpu_providers_that_fail() {
        let dir = std::env::temp_dir().join(format!("aperio-providers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join("m.onnx");
        let tries = std::cell::RefCell::new(Vec::new());
        let build = |_: &Path, p: Option<u8>| {
            tries.borrow_mut().push(p);
            match p {
                Some(1) => Err(ort::Error::new("can't convert")),
                _ => Ok(()),
            }
        };
        let candidates = || {
            [
                ("A", Some("a"), Some(1u8)),
                ("B", None, Some(1)),
                ("CPU", None, None),
            ]
        };
        let loaded = |c| first_that_loads(&model, c, build).unwrap().1;
        assert_eq!(loaded(candidates()), "CPU");
        assert!(dir.join("m.a.unsupported").exists());
        // A remembered failure isn't tried again; one without a tag is.
        assert_eq!(loaded(candidates()), "CPU");
        assert_eq!(*tries.borrow(), [Some(1), Some(1), None, Some(1), None]);
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    #[test]
    fn fast_subjects_fall_back_to_the_best_model() {
        let dir = std::env::temp_dir().join(format!("aperio-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plugin = Segment::new(&dir);
        let px = flat(4, 4);
        let img = Image {
            width: 4,
            height: 4,
            pixels: &px,
        };
        let fast = json!({"class": "subject", "detail": "fast"});
        let err = plugin.mask(&fast, img).unwrap_err().to_string();
        assert!(err.contains(models::FAST_SUBJECT.name), "{err}");
        // A stand-in "best" model of the right size: the fast mask now tries
        // it (and fails to load it) instead of asking for the fast one.
        let best = File::create(models::SUBJECT.path(&dir)).unwrap();
        best.set_len(models::SUBJECT.size_bytes).unwrap();
        let err = plugin.mask(&fast, img).unwrap_err().to_string();
        assert!(err.contains("birefnet_lite"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Runs the real models when they're downloaded (set APERIO_MODELS_DIR
    /// or run `aperio models download`); skipped otherwise.
    #[test]
    fn real_models_produce_masks_when_installed() {
        let plugin = Segment::new(models::default_dir());
        if !models::ALL
            .iter()
            .filter(|m| !m.optional)
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
        // The inverse reused the network's output; preparing adds the subject.
        assert_eq!(plugin.outputs.lock().unwrap().len(), 1);
        plugin.prepare(img, || true);
        assert_eq!(plugin.outputs.lock().unwrap().len(), 2);
        if models::FAST_SUBJECT.is_installed(plugin.models_dir()) {
            let fast = plugin
                .mask(&json!({"class": "subject", "detail": "fast"}), img)
                .unwrap();
            assert_eq!(fast.len(), (w * h) as usize);
            assert!(fast.iter().all(|m| (0.0..=1.0).contains(m)));
        }
    }
}
