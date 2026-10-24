import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";

interface Sliders {
  exposure: number;
  contrast: number;
  temperature: number;
  tint: number;
  saturation: number;
  vibrance: number;
}

interface Opened {
  file_name: string;
  width: number;
  height: number;
  sliders: Sliders;
}

interface SliderSpec {
  key: keyof Sliders;
  label: string;
  min: number;
  max: number;
  step: number;
  neutral: number;
  format: (v: number) => string;
}

const signed = (v: number) => (v > 0 ? "+" : "") + v.toFixed(2);

const SPECS: SliderSpec[] = [
  { key: "exposure", label: "Exposure", min: -5, max: 5, step: 0.05, neutral: 0, format: (v) => `${signed(v)} EV` },
  { key: "contrast", label: "Contrast", min: -1, max: 1, step: 0.01, neutral: 0, format: signed },
  { key: "temperature", label: "Temperature", min: 2000, max: 12000, step: 50, neutral: 6500, format: (v) => `${v} K` },
  { key: "tint", label: "Tint", min: -1, max: 1, step: 0.01, neutral: 0, format: signed },
  { key: "saturation", label: "Saturation", min: -1, max: 1, step: 0.01, neutral: 0, format: signed },
  { key: "vibrance", label: "Vibrance", min: -1, max: 1, step: 0.01, neutral: 0, format: signed },
];

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const preview = $<HTMLImageElement>("#preview");
const status = $("#status");
const title = $("#title");
const form = $<HTMLFormElement>("#sliders");
const exportButton = $<HTMLButtonElement>("#export");
const resetButton = $<HTMLButtonElement>("#reset");

const inputs = new Map<keyof Sliders, HTMLInputElement>();

function buildSliders() {
  for (const spec of SPECS) {
    const row = document.createElement("label");
    row.className = "slider";
    const name = document.createElement("span");
    name.textContent = spec.label;
    const value = document.createElement("output");
    const input = document.createElement("input");
    Object.assign(input, { type: "range", min: spec.min, max: spec.max, step: spec.step, value: spec.neutral, disabled: true });
    const show = () => (value.textContent = spec.format(Number(input.value)));
    input.addEventListener("input", () => {
      show();
      requestRender(false);
    });
    // Save the sidecar when the user lets go, not on every movement.
    input.addEventListener("change", () => requestRender(true));
    // Double-click a slider to reset it.
    input.addEventListener("dblclick", () => {
      input.value = String(spec.neutral);
      show();
      requestRender(true);
    });
    show();
    row.append(name, value, input);
    form.append(row);
    inputs.set(spec.key, input);
  }
}

function current(): Sliders {
  const s = {} as Sliders;
  for (const [key, input] of inputs) s[key] = Number(input.value);
  return s;
}

function setSliders(s: Sliders) {
  for (const [key, input] of inputs) {
    input.value = String(s[key]);
    input.dispatchEvent(new Event("input"));
  }
}

// Only one render in flight; while it runs, remember just the latest request.
let rendering = false;
let pending: { save: boolean } | null = null;
let previewUrl: string | null = null;

function requestRender(save: boolean) {
  if (rendering) {
    pending = { save: save || (pending?.save ?? false) };
    return;
  }
  void render(save);
}

async function render(save: boolean) {
  rendering = true;
  try {
    // Raw bytes arrive as an ArrayBuffer on most platforms, but as a plain
    // number array where the webview falls back to JSON IPC (seen on Linux).
    const bytes = await invoke<ArrayBuffer | number[]>("render_preview", { sliders: current(), save });
    const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
    preview.src = url;
    preview.hidden = false;
    if (previewUrl) URL.revokeObjectURL(previewUrl);
    previewUrl = url;
  } catch (e) {
    status.textContent = String(e);
  } finally {
    rendering = false;
    if (pending) {
      const next = pending;
      pending = null;
      void render(next.save);
    }
  }
}

async function openPhoto() {
  const path = await open({
    multiple: false,
    filters: [
      { name: "Photos", extensions: ["cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "dng", "pef", "srw", "jpg", "jpeg", "png", "tif", "tiff"] },
      { name: "All files", extensions: ["*"] },
    ],
  });
  if (typeof path === "string") await openPath(path);
}

async function openPath(path: string) {
  status.textContent = "Opening…";
  status.hidden = false;
  try {
    const opened = await invoke<Opened>("open_image", { path });
    title.textContent = `${opened.file_name} · ${opened.width}×${opened.height}`;
    status.hidden = true;
    for (const input of inputs.values()) input.disabled = false;
    exportButton.disabled = false;
    resetButton.disabled = false;
    setSliders(opened.sliders);
    requestRender(false);
  } catch (e) {
    status.textContent = `Could not open: ${e}`;
  }
}

async function exportPhoto() {
  const path = await save({
    filters: [
      { name: "JPEG", extensions: ["jpg"] },
      { name: "PNG", extensions: ["png"] },
      { name: "TIFF (16-bit)", extensions: ["tif"] },
    ],
  });
  if (!path) return;
  exportButton.disabled = true;
  exportButton.textContent = "Exporting…";
  try {
    await invoke("export_image", { path });
  } catch (e) {
    alert(`Export failed: ${e}`);
  } finally {
    exportButton.disabled = false;
    exportButton.textContent = "Export…";
  }
}

buildSliders();
void invoke<string | null>("initial_path").then((path) => {
  if (path) void openPath(path);
});
$("#open").addEventListener("click", openPhoto);
exportButton.addEventListener("click", exportPhoto);
resetButton.addEventListener("click", () => {
  for (const spec of SPECS) inputs.get(spec.key)!.value = String(spec.neutral);
  setSliders(current());
  requestRender(true);
});
