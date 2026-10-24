//! `aperio render <input> <output>`: apply an original's sidecar and export it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use aperio_core::{sidecar_path, EditDocument, SourceRef};
use aperio_plugin::Host;
use aperio_plugin_segment::models;
use aperio_render::{decode, export, ExportFormat, Renderer};

const USAGE: &str = "usage: aperio render <input> <output.jpg|png|tiff> [--sidecar <file>]
       aperio plugins
       aperio models [download]
       aperio models bench [photo]";

/// The plugins built into this binary.
fn plugins() -> Host {
    let mut host = Host::new();
    host.register(aperio_plugin_segment::Segment::new(models::default_dir()))
        .expect("bundled plugins register");
    host.register(aperio_plugin_sky::SkyReplace::new())
        .expect("bundled plugins register");
    host.register(aperio_plugin_vignette::Vignette)
        .expect("bundled plugins register");
    host
}

/// `aperio models`: show which AI models are downloaded; with `download`,
/// fetch the missing ones.
fn models_command(download: bool) -> Result<(), Box<dyn std::error::Error>> {
    let dir = models::default_dir();
    println!("models folder: {}", dir.display());
    for m in models::ALL {
        let installed = m.is_installed(&dir);
        println!(
            "  {:<40} {:>4} MB  {}  {}",
            m.name,
            m.size_bytes / 1_000_000,
            m.license,
            if installed {
                "installed"
            } else {
                "not downloaded"
            }
        );
        if download && !installed {
            let mut shown = 0;
            m.download(&dir, |done, total| {
                let pct = done * 100 / total.max(1);
                if pct >= shown + 10 {
                    shown = pct;
                    eprintln!("    {pct}%");
                }
            })?;
            println!("    downloaded and verified");
        }
    }
    if !download {
        println!("run `aperio models download` to fetch missing models");
    }
    Ok(())
}

/// `aperio models bench [photo]`: time the AI masks on a preview-sized copy
/// of `photo` (or a made-up sky over ground) and say where each model ran
/// (CPU or GPU). The first run of each includes loading the model; the
/// second shows the network's output being reused.
fn bench_command(photo: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    use aperio_plugin::{Image, Plugin};
    let img = match photo {
        Some(photo) => decode(std::path::Path::new(photo))?.downscale(2048),
        None => {
            let (width, height) = (1536u32, 2048u32);
            let pixels = (0..width * height)
                .map(|i| {
                    let y = (i / width) as f32 / height as f32;
                    if y < 0.6 {
                        [0.2 + 0.3 * y, 0.35 + 0.3 * y, 0.8, 1.0]
                    } else {
                        [0.05, 0.12, 0.03, 1.0]
                    }
                })
                .collect();
            aperio_render::LinearImage {
                width,
                height,
                pixels,
            }
        }
    };
    let segment = aperio_plugin_segment::Segment::new(models::default_dir());
    let source = Image {
        width: img.width,
        height: img.height,
        pixels: &img.pixels,
    };
    println!("{}x{} preview", img.width, img.height);
    for class in ["sky", "subject"] {
        let params = serde_json::json!({ "class": class });
        for run in ["first run", "cached"] {
            let started = std::time::Instant::now();
            segment.mask(&params, source)?;
            println!(
                "  {class:<8} {run:<10} {:>6} ms",
                started.elapsed().as_millis()
            );
        }
    }
    for (model, device) in segment.devices() {
        println!("  {model} ran on {device}");
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    match args.as_slice() {
        [cmd] if cmd == "models" => return models_command(false),
        [cmd, sub] if cmd == "models" && sub == "download" => return models_command(true),
        [cmd, sub] if cmd == "models" && sub == "bench" => return bench_command(None),
        [cmd, sub, photo] if cmd == "models" && sub == "bench" => {
            return bench_command(Some(photo))
        }
        _ => {}
    }
    if matches!(args.as_slice(), [cmd] if cmd == "plugins") {
        for p in plugins().plugins() {
            let provides: Vec<&str> = [p.operation.as_ref(), p.mask.as_ref()]
                .into_iter()
                .flatten()
                .map(|x| x.label.as_str())
                .collect();
            println!(
                "{} {}  {} ({})",
                p.id,
                p.version,
                p.name,
                provides.join(", ")
            );
        }
        return Ok(());
    }
    let (input, output, sidecar) = match args.as_slice() {
        [cmd, input, output] if cmd == "render" => (input, output, None),
        [cmd, input, output, flag, sidecar] if cmd == "render" && flag == "--sidecar" => {
            (input, output, Some(PathBuf::from(sidecar)))
        }
        _ => return Err(USAGE.into()),
    };
    let input = PathBuf::from(input);
    let output = PathBuf::from(output);
    let format = ExportFormat::from_path(&output)?;

    let sidecar = sidecar.unwrap_or_else(|| sidecar_path(&input));
    let doc = if sidecar.exists() {
        EditDocument::load(&sidecar)?
    } else {
        EditDocument::new(SourceRef {
            file_name: input
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            size_bytes: std::fs::metadata(&input)?.len(),
            sha256: None,
        })
    };

    let mut img = decode(&input)?;
    let renderer = Renderer::new().with_plugins(Arc::new(plugins()));
    for s in renderer.render(&doc, &mut img) {
        eprintln!(
            "warning: skipped `{}` on layer {}: {}",
            s.op, s.layer, s.reason
        );
    }
    export(&img, &output, format)?;
    eprintln!(
        "wrote {} ({}x{}, {})",
        output.display(),
        img.width,
        img.height,
        if renderer.uses_gpu() { "GPU" } else { "CPU" }
    );
    Ok(())
}
