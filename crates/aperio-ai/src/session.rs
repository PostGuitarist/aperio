//! Running ONNX models with ONNX Runtime, on the GPU where this platform has
//! a provider that can take the model, otherwise on the CPU.

use std::path::Path;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

/// Load a model, on the GPU if one of this platform's GPU execution
/// providers can take it, otherwise on the CPU. Returns the session and
/// where it runs.
pub fn load(path: &Path) -> Result<(Session, &'static str), String> {
    let gpus = gpu_providers(path)
        .into_iter()
        .map(|(name, remember, provider)| (name, remember, Some(provider)));
    first_that_loads(path, gpus.chain([("CPU", None, None)]), build)
        .map_err(|e| format!("couldn't load {}: {e}", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
