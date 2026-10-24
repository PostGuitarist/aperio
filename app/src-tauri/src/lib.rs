use serde::Serialize;

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    sidecar_format_version: u32,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        sidecar_format_version: aperio_core::document::FORMAT_VERSION,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("error while running Aperio");
}
