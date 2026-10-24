mod view;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aperio_core::{document::FORMAT_ID, sidecar_path, EditDocument, SourceRef};
use aperio_plugin::{Host, PluginInfo};
use aperio_plugin_segment::models;
use aperio_render::{decode, export, jpeg_bytes, ExportFormat, LinearImage, Renderer, Skipped};
use serde::Serialize;
use tauri::ipc::{Channel, Response};
use tauri::State;
use view::View;

/// Longest edge of the on-screen preview. Full resolution is used for export.
const PREVIEW_EDGE: u32 = 2048;
const PREVIEW_QUALITY: u8 = 85;

#[derive(Default)]
struct AppState {
    /// Shared with blocking render threads; created on first use.
    renderer: Arc<Mutex<Option<Renderer>>>,
    session: Mutex<Option<Session>>,
    /// Steps the last preview left out, for the UI to explain.
    skipped: Mutex<Vec<SkippedStep>>,
}

#[derive(Debug, Clone, Serialize)]
struct SkippedStep {
    layer: usize,
    op: String,
    reason: String,
}

impl From<Skipped> for SkippedStep {
    fn from(s: Skipped) -> Self {
        Self {
            layer: s.layer,
            op: s.op,
            reason: s.reason,
        }
    }
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
    /// Size of the uncropped preview, which the editor's on-canvas tools
    /// measure against.
    preview_width: u32,
    preview_height: u32,
    doc: EditDocument,
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
    let preview = full.downscale(PREVIEW_EDGE);
    let opened = Opened {
        file_name: doc.source.file_name.clone(),
        width: full.width,
        height: full.height,
        preview_width: preview.width,
        preview_height: preview.height,
        doc: doc.clone(),
    };
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

/// Take the editor's current document and return the preview as JPEG bytes.
/// With `save`, the sidecar is written too (when an edit is committed, not on
/// every slider movement).
#[tauri::command]
async fn render_preview(
    doc: EditDocument,
    view: View,
    save: bool,
    state: State<'_, AppState>,
) -> CmdResult<Response> {
    if doc.format != FORMAT_ID || doc.layers.is_empty() {
        return Err("not an Aperio edit document".into());
    }
    let mut img = {
        let mut session = state.session.lock().unwrap();
        let session = session.as_mut().ok_or("no photo is open")?;
        session.doc = doc.clone();
        if save {
            session
                .doc
                .save(&sidecar_path(&session.source))
                .map_err(err)?;
        }
        session.preview.clone()
    };
    let (img, skipped) = with_renderer(&state, move |r| {
        let skipped = view::render(r, &doc, view, &mut img);
        (img, skipped)
    })
    .await?;
    *state.skipped.lock().unwrap() = skipped.into_iter().map(Into::into).collect();
    jpeg_bytes(&img, PREVIEW_QUALITY)
        .map(Response::new)
        .map_err(err)
}

/// Steps the last preview couldn't render (e.g. an AI model that isn't
/// downloaded), so the UI can say why.
#[tauri::command]
fn last_skipped(state: State<'_, AppState>) -> Vec<SkippedStep> {
    state.skipped.lock().unwrap().clone()
}

/// Render at full resolution and write to `path`; the format follows the
/// extension. Returns the steps that were left out.
#[tauri::command]
async fn export_image(path: PathBuf, state: State<'_, AppState>) -> CmdResult<Vec<SkippedStep>> {
    let format = ExportFormat::from_path(&path).map_err(err)?;
    let (mut img, doc) = {
        let session = state.session.lock().unwrap();
        let session = session.as_ref().ok_or("no photo is open")?;
        (session.full.clone(), session.doc.clone())
    };
    with_renderer(&state, move |r| {
        let skipped = r.render(&doc, &mut img);
        export(&img, &path, format).map_err(err)?;
        Ok(skipped.into_iter().map(Into::into).collect())
    })
    .await?
}

/// Run `f` with the shared renderer on a blocking thread (AI masks can take
/// many seconds), creating the renderer on first use: GPU setup takes a
/// moment, so it isn't done at launch.
async fn with_renderer<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Renderer) -> T + Send + 'static,
) -> CmdResult<T> {
    let renderer = state.renderer.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut renderer = renderer.lock().unwrap();
        f(renderer.get_or_insert_with(|| Renderer::new().with_plugins(Arc::new(plugins()))))
    })
    .await
    .map_err(err)
}

/// The plugins built into the app.
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

/// Installed plugins and what they provide, for the UI.
#[tauri::command]
fn list_plugins() -> Vec<PluginInfo> {
    plugins().plugins().cloned().collect()
}

#[derive(Serialize)]
struct ModelStatus {
    id: &'static str,
    name: &'static str,
    license: &'static str,
    source: &'static str,
    size_bytes: u64,
    installed: bool,
}

/// The AI models and whether each is downloaded.
#[tauri::command]
fn models_status() -> Vec<ModelStatus> {
    let dir = models::default_dir();
    models::ALL
        .iter()
        .map(|m| ModelStatus {
            id: m.id,
            name: m.name,
            license: m.license,
            source: m.source,
            size_bytes: m.size_bytes,
            installed: m.is_installed(&dir),
        })
        .collect()
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    model: &'static str,
    done: u64,
    total: u64,
}

/// Download every missing model, reporting progress on `progress`. Only
/// runs when the user asks for it.
#[tauri::command]
async fn download_models(progress: Channel<DownloadProgress>) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = models::default_dir();
        for m in models::ALL.iter().filter(|m| !m.is_installed(&dir)) {
            let mut last = 0;
            m.download(&dir, |done, total| {
                // Roughly every 1%, so the channel isn't flooded.
                if done - last >= total / 100 || done == total {
                    last = done;
                    let _ = progress.send(DownloadProgress {
                        model: m.id,
                        done,
                        total,
                    });
                }
            })?;
        }
        Ok(())
    })
    .await
    .map_err(err)?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            app_info,
            initial_path,
            list_plugins,
            open_image,
            render_preview,
            last_skipped,
            export_image,
            models_status,
            download_models
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aperio");
}
