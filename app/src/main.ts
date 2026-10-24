import { invoke } from "@tauri-apps/api/core";

interface AppInfo {
  version: string;
  sidecar_format_version: number;
}

window.addEventListener("DOMContentLoaded", async () => {
  const status = document.querySelector<HTMLElement>("#status");
  if (!status) return;
  try {
    const info = await invoke<AppInfo>("app_info");
    status.textContent = `v${info.version} · sidecar format v${info.sidecar_format_version}`;
  } catch {
    // Running in a plain browser via `npm run dev`, without the Tauri backend.
    status.textContent = "Frontend preview (no backend)";
  }
});
