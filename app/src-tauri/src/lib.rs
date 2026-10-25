mod library;
mod view;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aperio_core::{document::FORMAT_ID, sidecar_path, EditDocument, SourceRef};
use aperio_plugin::{Host, PluginInfo};
use aperio_plugin_segment::models;
use aperio_render::export::{export_with_metadata, ExportMetadata};
use aperio_render::metadata::{Metadata, MetadataPolicy};
use aperio_render::{
    browse, decode, jpeg_bytes, srgb8, ExportFormat, LinearImage, Renderer, Skipped,
};
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, Response};
use tauri::State;
use view::View;

/// Longest edge of the on-screen preview. Full resolution is used for export.
/// Matching the segment plugin's network source size lets exports reuse the
/// AI mask work done for the preview.
const PREVIEW_EDGE: u32 = aperio_plugin_segment::SOURCE_EDGE;
const PREVIEW_QUALITY: u8 = 85;

#[derive(Default)]
struct AppState {
    /// Shared with blocking render threads; created on first use.
    renderer: Arc<Mutex<Option<Renderer>>>,
    /// The bundled plugins, shared by the renderer and background work.
    plugins: Plugins,
    session: Mutex<Option<Session>>,
    /// Steps the last preview left out, for the UI to explain.
    skipped: Mutex<Vec<SkippedStep>>,
}

struct Plugins {
    host: Arc<Host>,
    segment: Arc<aperio_plugin_segment::Segment>,
    /// Bumped each time a photo is opened, so background work for the
    /// previous photo stops.
    photo: Arc<AtomicU64>,
}

impl Default for Plugins {
    fn default() -> Self {
        let segment = Arc::new(aperio_plugin_segment::Segment::new(models::default_dir()));
        let mut host = Host::new();
        host.register(segment.clone())
            .expect("bundled plugins register");
        host.register(aperio_plugin_sky::SkyReplace::new())
            .expect("bundled plugins register");
        host.register(aperio_plugin_vignette::Vignette)
            .expect("bundled plugins register");
        Self {
            host: Arc::new(host),
            segment,
            photo: Arc::new(AtomicU64::new(0)),
        }
    }
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
    /// The folder the photo is in, for the export screen's default.
    folder: PathBuf,
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
        folder: source.parent().map(Path::to_path_buf).unwrap_or_default(),
        doc: doc.clone(),
    };
    prepare_ai_masks(&state.plugins, &preview);
    *state.session.lock().unwrap() = Some(Session {
        source,
        full,
        preview,
        doc,
    });
    Ok(opened)
}

/// Start the AI models on a new photo's preview in the background, so its
/// AI masks are usually ready by the time they're asked for. Only runs
/// models that are downloaded, and stops if another photo is opened.
fn prepare_ai_masks(plugins: &Plugins, preview: &LinearImage) {
    let photo = plugins.photo.fetch_add(1, Ordering::SeqCst) + 1;
    let (segment, current) = (plugins.segment.clone(), plugins.photo.clone());
    let preview = preview.clone();
    std::thread::spawn(move || {
        let source = aperio_plugin::Image {
            width: preview.width,
            height: preview.height,
            pixels: &preview.pixels,
        };
        segment.prepare(source, || current.load(Ordering::SeqCst) == photo);
    });
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
    let (mut img, source) = {
        let mut session = state.session.lock().unwrap();
        let session = session.as_mut().ok_or("no photo is open")?;
        session.doc = doc.clone();
        if save {
            session
                .doc
                .save(&sidecar_path(&session.source))
                .map_err(err)?;
        }
        (session.preview.clone(), session.source.clone())
    };
    // A saved edit seen the normal way also becomes the library thumbnail.
    let thumb = save && view.crop && view.mask_overlay.is_none() && !view.before;
    let (img, skipped) = with_renderer(&state, move |r| {
        let skipped = view::render(r, &doc, view, &mut img);
        (img, skipped)
    })
    .await?;
    if thumb {
        library::store_edited(&library::cache_dir(), &source, &srgb8(&img));
    }
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

/// The export screen's settings.
#[derive(Debug, Deserialize)]
struct ExportOptions {
    folder: PathBuf,
    /// File name without extension; `{name}` stands for the original's name.
    name: String,
    format: OutputFormat,
    /// JPEG quality, 1 to 100.
    #[serde(default = "default_quality")]
    quality: u8,
    /// Resize so the longer edge is this many pixels (never enlarges).
    #[serde(default)]
    long_edge: Option<u32>,
    /// How much of the original's metadata to copy.
    #[serde(default)]
    metadata: MetadataPolicy,
}

fn default_quality() -> u8 {
    90
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutputFormat {
    Jpeg,
    Png,
    Tiff,
}

impl ExportOptions {
    /// The file this would write, checked for names that would escape the
    /// chosen folder.
    fn target(&self, source: &Path) -> CmdResult<PathBuf> {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let name = self.name.trim().replace("{name}", &stem);
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return Err("choose a file name without slashes".into());
        }
        let ext = match self.format {
            OutputFormat::Jpeg => "jpg",
            OutputFormat::Png => "png",
            OutputFormat::Tiff => "tif",
        };
        Ok(self.folder.join(format!("{name}.{ext}")))
    }

    fn format(&self) -> ExportFormat {
        match self.format {
            OutputFormat::Jpeg => ExportFormat::Jpeg {
                quality: self.quality.clamp(1, 100),
            },
            OutputFormat::Png => ExportFormat::Png,
            OutputFormat::Tiff => ExportFormat::Tiff,
        }
    }
}

#[derive(Serialize)]
struct ExportTarget {
    path: PathBuf,
    exists: bool,
}

/// Where an export with `options` would go, and whether a file is already
/// there, so the export screen can show it and ask before replacing it.
#[tauri::command]
fn export_target(options: ExportOptions, state: State<'_, AppState>) -> CmdResult<ExportTarget> {
    let session = state.session.lock().unwrap();
    let session = session.as_ref().ok_or("no photo is open")?;
    let path = options.target(&session.source)?;
    let exists = path.exists();
    Ok(ExportTarget { path, exists })
}

#[derive(Serialize)]
struct Exported {
    path: PathBuf,
    width: u32,
    height: u32,
    ms: u64,
    skipped: Vec<SkippedStep>,
}

/// Render at full resolution, resize if asked, and write the file, creating
/// the folder if needed. Replaces an existing file, so the UI asks first.
#[tauri::command]
async fn export_image(options: ExportOptions, state: State<'_, AppState>) -> CmdResult<Exported> {
    let (mut img, doc, path, source) = {
        let session = state.session.lock().unwrap();
        let session = session.as_ref().ok_or("no photo is open")?;
        let path = options.target(&session.source)?;
        (
            session.full.clone(),
            session.doc.clone(),
            path,
            session.source.clone(),
        )
    };
    let policy = options.metadata;
    let format = options.format();
    let long_edge = options.long_edge;
    with_renderer(&state, move |r| {
        let started = std::time::Instant::now();
        let skipped = r.render(&doc, &mut img);
        let img = match long_edge {
            Some(edge) => img.resized(edge),
            None => img,
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        // A file whose metadata can't be read still exports, without it.
        let original = Metadata::read(&source).unwrap_or_default();
        let software = format!("Aperio {}", env!("CARGO_PKG_VERSION"));
        let metadata = ExportMetadata {
            source: &original,
            policy,
            software: &software,
        };
        export_with_metadata(&img, &path, format, Some(metadata)).map_err(err)?;
        Ok(Exported {
            width: img.width,
            height: img.height,
            ms: started.elapsed().as_millis() as u64,
            path,
            skipped: skipped.into_iter().map(Into::into).collect(),
        })
    })
    .await?
}

/// Whether rendering runs on the GPU, once the renderer has started.
#[tauri::command]
fn renderer_info(state: State<'_, AppState>) -> Option<bool> {
    state
        .renderer
        .try_lock()
        .ok()
        .and_then(|r| r.as_ref().map(Renderer::uses_gpu))
}

/// Run `f` with the shared renderer on a blocking thread (AI masks can take
/// many seconds), creating the renderer on first use: GPU setup takes a
/// moment, so it isn't done at launch.
async fn with_renderer<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce(&Renderer) -> T + Send + 'static,
) -> CmdResult<T> {
    let renderer = state.renderer.clone();
    let host = state.plugins.host.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut renderer = renderer.lock().unwrap();
        f(renderer.get_or_insert_with(|| Renderer::new().with_plugins(host)))
    })
    .await
    .map_err(err)
}

/// The photos in a folder, for the Library screen.
#[tauri::command]
async fn list_folder(path: PathBuf) -> CmdResult<library::Folder> {
    tauri::async_runtime::spawn_blocking(move || library::list(&path))
        .await
        .map_err(err)?
        .map_err(err)
}

/// A photo's thumbnail as JPEG bytes, made on first request and cached.
#[tauri::command]
async fn thumbnail(path: PathBuf) -> CmdResult<Response> {
    tauri::async_runtime::spawn_blocking(move || library::thumbnail(&library::cache_dir(), &path))
        .await
        .map_err(err)?
        .map(Response::new)
}

/// Camera details for the Library inspector.
#[tauri::command]
async fn photo_details(path: PathBuf) -> CmdResult<browse::PhotoDetails> {
    tauri::async_runtime::spawn_blocking(move || browse::details(&path))
        .await
        .map_err(err)?
        .map_err(err)
}

/// Installed plugins and what they provide, for the UI.
#[tauri::command]
fn list_plugins(state: State<'_, AppState>) -> Vec<PluginInfo> {
    state.plugins.host.plugins().cloned().collect()
}

#[derive(Serialize)]
struct ModelStatus {
    id: &'static str,
    name: &'static str,
    license: &'static str,
    source: &'static str,
    size_bytes: u64,
    installed: bool,
    optional: bool,
    notice: Option<&'static str>,
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
            optional: m.optional,
            notice: m.notice,
        })
        .collect()
}

#[derive(Clone, Serialize)]
struct DownloadProgress {
    model: &'static str,
    done: u64,
    total: u64,
}

/// Download the missing models in `ids`, or every missing default (not
/// optional) model when `ids` is empty, reporting progress on `progress`.
/// Only runs when the user asks for it.
#[tauri::command]
async fn download_models(ids: Vec<String>, progress: Channel<DownloadProgress>) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = models::default_dir();
        let wanted = |m: &models::Model| {
            if ids.is_empty() {
                !m.optional
            } else {
                ids.iter().any(|id| id == m.id)
            }
        };
        for m in models::ALL
            .iter()
            .filter(|m| wanted(m) && !m.is_installed(&dir))
        {
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
            list_folder,
            thumbnail,
            photo_details,
            open_image,
            render_preview,
            last_skipped,
            export_target,
            export_image,
            renderer_info,
            models_status,
            download_models
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aperio");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(name: &str, format: OutputFormat) -> ExportOptions {
        ExportOptions {
            folder: PathBuf::from("/out"),
            name: name.into(),
            format,
            quality: 90,
            long_edge: None,
            metadata: MetadataPolicy::default(),
        }
    }

    #[test]
    fn export_target_fills_in_the_name_and_extension() {
        let source = Path::new("/photos/DSC_2041.NEF");
        assert_eq!(
            options("{name}_edit", OutputFormat::Jpeg).target(source),
            Ok(PathBuf::from("/out/DSC_2041_edit.jpg"))
        );
        assert_eq!(
            options("print", OutputFormat::Tiff).target(source),
            Ok(PathBuf::from("/out/print.tif"))
        );
    }

    #[test]
    fn export_target_rejects_names_that_leave_the_folder() {
        let source = Path::new("/photos/a.jpg");
        for bad in ["", "  ", "../a", "sub/a", "a\\b", ".."] {
            assert!(
                options(bad, OutputFormat::Png).target(source).is_err(),
                "{bad}"
            );
        }
    }
}
