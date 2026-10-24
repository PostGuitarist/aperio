import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { loadPlugins } from "./ai";
import { drawHistogram } from "./histogram";
import { Frame, Overlay } from "./overlay";
import { addLayer, buildPanels, showTab } from "./panels";
import {
  type Opened,
  type Rendered,
  canRedo,
  canUndo,
  changed,
  editor,
  load,
  onBusy,
  onRenderError,
  onRendered,
  onUpdate,
  redo,
  refresh,
  undo,
} from "./state";

const $ = <T extends Element>(sel: string) => document.querySelector<T>(sel)!;
const stage = $<HTMLDivElement>("#stage");
const frameEl = $<HTMLDivElement>("#frame");
const preview = $<HTMLImageElement>("#preview");
const status = $<HTMLParagraphElement>("#status");
const title = $("#title");
const exportButton = $<HTMLButtonElement>("#export");
const undoButton = $<HTMLButtonElement>("#undo");
const redoButton = $<HTMLButtonElement>("#redo");
const beforeButton = $<HTMLButtonElement>("#before");
const timing = $("#timing");
const savedNote = $("#saved");
const notice = $<HTMLSpanElement>("#notice");
const histogram = $<HTMLCanvasElement>("#histogram");
const overlay = new Overlay($<SVGSVGElement>("#overlay"));

buildPanels($("#tabs"), $("#tab-body"));

// --- Preview layout -----------------------------------------------------------

/** The crop of the image currently on screen (not the one being requested). */
let shownCrop: Rendered["crop"];

/** Fit the preview into the stage and line the overlay up with it. */
function layout() {
  const o = editor.opened;
  if (!o || !preview.naturalWidth) return;
  const pad = 32;
  const [sw, sh] = [stage.clientWidth - pad, stage.clientHeight - pad];
  const [iw, ih] = Frame.outSize(o.preview_width, o.preview_height, shownCrop);
  const scale = Math.min(sw / iw, sh / ih);
  frameEl.style.width = `${iw * scale}px`;
  frameEl.style.height = `${ih * scale}px`;
  overlay.setFrame(new Frame(o.preview_width, o.preview_height, shownCrop, scale));
}

new ResizeObserver(layout).observe(stage);

onRendered((r) => {
  preview.onload = () => {
    shownCrop = r.crop;
    frameEl.hidden = false;
    status.hidden = true;
    layout();
    drawHistogram(histogram, preview);
  };
  preview.src = r.url;
  timing.textContent = `${editor.before ? "Before · " : ""}Preview ${r.ms} ms`;
  if (r.saved) savedNote.textContent = "Sidecar saved";
  void showSkipped();
});

interface SkippedStep {
  layer: number;
  op: string;
  reason: string;
}

/** Say in the status bar when part of the edit couldn't be rendered. */
async function showSkipped() {
  const skipped = await invoke<SkippedStep[]>("last_skipped");
  if (skipped.length === 0) {
    notice.textContent = "";
    return;
  }
  const missingModel = skipped.some((s) => s.reason.includes("isn't downloaded"));
  notice.textContent = missingModel
    ? "AI models aren't downloaded yet; get them in the AI tab"
    : `Not rendered: ${skipped[0].reason}${skipped.length > 1 ? ` (+${skipped.length - 1} more)` : ""}`;
  notice.title = skipped.map((s) => `Layer ${s.layer}: ${s.reason}`).join("\n");
}

// AI masks can take a while the first time; say so instead of looking stuck.
let busyTimer: number | undefined;
onBusy((busy) => {
  clearTimeout(busyTimer);
  if (busy) busyTimer = window.setTimeout(() => (timing.textContent = "Rendering…"), 600);
});

onRenderError((e) => {
  status.textContent = String(e);
  status.hidden = false;
});

onUpdate(() => {
  const ready = !!editor.opened;
  exportButton.disabled = beforeButton.disabled = !ready;
  undoButton.disabled = !canUndo();
  redoButton.disabled = !canRedo();
  beforeButton.classList.toggle("active", editor.before);
  for (const b of document.querySelectorAll<HTMLButtonElement>("#rail button")) {
    b.disabled = !ready;
    const tool = b.dataset.tool;
    const kind = editor.selected > 0 ? editor.doc.layers[editor.selected]?.mask?.kind : undefined;
    const active =
      (tool === "crop" && editor.tab === "crop") ||
      (tool === "ai" && editor.tab === "ai") ||
      (tool === "select" && editor.tab !== "crop" && editor.tab !== "ai" && !kind) ||
      (editor.tab !== "crop" && editor.tab !== "ai" && tool === kind);
    b.classList.toggle("active", active);
  }
  overlay.draw();
});

// --- Open and export -------------------------------------------------------------------

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
  frameEl.hidden = true;
  try {
    const opened = await invoke<Opened>("open_image", { path });
    title.textContent = `${opened.file_name} · ${opened.width}×${opened.height}`;
    savedNote.textContent = "";
    editor.tab = "adjust";
    load(opened);
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
    const skipped = await invoke<SkippedStep[]>("export_image", { path });
    if (skipped.length > 0) alert(`Exported, but some steps were left out:\n${skipped.map((s) => `• ${s.reason}`).join("\n")}`);
  } catch (e) {
    alert(`Export failed: ${e}`);
  } finally {
    exportButton.disabled = false;
    exportButton.textContent = "Export…";
  }
}

// --- Tools and shortcuts ----------------------------------------------------------------

function useTool(tool: string | undefined) {
  if (!editor.opened) return;
  if (tool === "select") {
    editor.selected = 0;
    showTab("adjust");
  } else if (tool === "crop") showTab("crop");
  else if (tool === "ai") showTab("ai");
  else if (tool === "brush" || tool === "linear" || tool === "radial") addLayer(tool);
}

function toggleBefore() {
  if (!editor.opened) return;
  editor.before = !editor.before;
  changed();
}

for (const b of document.querySelectorAll<HTMLButtonElement>("#rail button")) {
  b.addEventListener("click", () => useTool(b.dataset.tool));
}
$("#open").addEventListener("click", openPhoto);
exportButton.addEventListener("click", exportPhoto);
undoButton.addEventListener("click", undo);
redoButton.addEventListener("click", redo);
beforeButton.addEventListener("click", toggleBefore);

const KEYS: Record<string, string> = { v: "select", c: "crop", b: "brush", g: "linear", r: "radial", a: "ai" };

document.addEventListener("keydown", (e) => {
  const typing = e.target instanceof HTMLInputElement && e.target.type !== "range" && e.target.type !== "checkbox";
  if (typing) return;
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    if (e.shiftKey) redo();
    else undo();
  } else if (mod && e.key.toLowerCase() === "y") {
    e.preventDefault();
    redo();
  } else if (mod && e.key.toLowerCase() === "o") {
    e.preventDefault();
    void openPhoto();
  } else if (!mod && e.key === "\\") {
    toggleBefore();
  } else if (!mod && !e.altKey && KEYS[e.key]) {
    useTool(KEYS[e.key]);
  }
});

refresh();
void loadPlugins().then(refresh);
void invoke<string | null>("initial_path").then((path) => {
  if (path) void openPath(path);
});
