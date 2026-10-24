//! `aperio render <input> <output>`: apply an original's sidecar and export it.

use std::path::PathBuf;
use std::process::ExitCode;

use aperio_core::{sidecar_path, EditDocument, SourceRef};
use aperio_render::{decode, export, ExportFormat, Renderer};

const USAGE: &str = "usage: aperio render <input> <output.jpg|png|tiff> [--sidecar <file>]";

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
    let renderer = Renderer::new();
    for s in renderer.render(&doc, &mut img) {
        eprintln!(
            "warning: layer {} `{}` is not supported yet and was skipped",
            s.layer, s.op
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
