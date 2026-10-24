mod sliders;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aperio_core::{sidecar_path, EditDocument, SourceRef};
use aperio_render::{decode, export, jpeg_bytes, ExportFormat, LinearImage, Renderer};
use serde::Serialize;
use sliders::Sliders;
use tauri::ipc::Response;
use tauri::State;

/// Longest edge of the on-screen preview. Full resolution is used for export.
const PREVIEW_EDGE: u32 = 2048;
const PREVIEW_QUALITY: u8 = 85;

#[derive(Default)]
struct AppState {
    renderer: Mutex<Option<Renderer>>,
    session: Mutex<Option<Session>>,
}

/// The photo being edited.
struct Session {
    source: PathBuf,
    full: LinearImage,
    preview: LinearImage,
    doc: EditDocument,
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    sidecar_format_version: u32,
}

#[derive(Serialize)]
struct Opened {
    file_name: String,
    width: u32,
    height: u32,
    sliders: Sliders,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        sidecar_format_version: aperio_core::document::FORMAT_VERSION,
    }
}

/// A photo passed on the command line (e.g. "Open with Aperio"), if any.
#[tauri::command]
fn initial_path() -> Option<PathBuf> {
    std::env::args_os().nth(1).map(PathBuf::from)
}

/// Decode a photo and load its sidecar, or start a new edit if there is none.
#[tauri::command]
async fn open_image(path: PathBuf, state: State<'_, AppState>) -> CmdResult<Opened> {
    let source = path.clone();
    let (full, doc) = tauri::async_runtime::spawn_blocking(move || load(&path))
        .await
        .map_err(err)??;
    let opened = Opened {
        file_name: doc.source.file_name.clone(),
        width: full.width,
        height: full.height,
        sliders: Sliders::from_doc(&doc),
    };
    let preview = full.downscale(PREVIEW_EDGE);
    *state.session.lock().unwrap() = Some(Session {
        source,
        full,
        preview,
        doc,
    });
    Ok(opened)
}

fn load(path: &Path) -> CmdResult<(LinearImage, EditDocument)> {
    let img = decode(path).map_err(err)?;
    let sidecar = sidecar_path(path);
    let doc = if sidecar.exists() {
        EditDocument::load(&sidecar).map_err(err)?
    } else {
        EditDocument::new(SourceRef {
            file_name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            size_bytes: std::fs::metadata(path).map_err(err)?.len(),
            sha256: None,
        })
    };
    Ok((img, doc))
}

/// Apply slider values and return the rendered preview as JPEG bytes.
/// With `save`, the sidecar is written too (on slider release, not every move).
#[tauri::command]
async fn render_preview(
    sliders: Sliders,
    save: bool,
    state: State<'_, AppState>,
) -> CmdResult<Response> {
    let (mut img, doc) = {
        let mut session = state.session.lock().unwrap();
        let session = session.as_mut().ok_or("no photo is open")?;
        sliders.apply_to(&mut session.doc);
        if save {
            session
                .doc
                .save(&sidecar_path(&session.source))
                .map_err(err)?;
        }
        (session.preview.clone(), session.doc.clone())
    };
    with_renderer(&state, |r| r.render(&doc, &mut img));
    jpeg_bytes(&img, PREVIEW_QUALITY)
        .map(Response::new)
        .map_err(err)
}

/// Render at full resolution and write to `path`; the format follows the extension.
#[tauri::command]
async fn export_image(path: PathBuf, state: State<'_, AppState>) -> CmdResult<()> {
    let format = ExportFormat::from_path(&path).map_err(err)?;
    let (mut img, doc) = {
        let session = state.session.lock().unwrap();
        let session = session.as_ref().ok_or("no photo is open")?;
        (session.full.clone(), session.doc.clone())
    };
    with_renderer(&state, |r| r.render(&doc, &mut img));
    tauri::async_runtime::spawn_blocking(move || export(&img, &path, format))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Run `f` with the shared renderer, creating it on first use (GPU setup
/// takes a moment, so it isn't done at launch).
fn with_renderer<T>(state: &AppState, f: impl FnOnce(&Renderer) -> T) -> T {
    let mut renderer = state.renderer.lock().unwrap();
    f(renderer.get_or_insert_with(Renderer::new))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            initial_path,
            open_image,
            render_preview,
            export_image
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aperio");
}
