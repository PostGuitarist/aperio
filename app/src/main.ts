import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { loadPlugins } from "./ai";
import { buildExport } from "./export";
import { buildLibrary } from "./library";
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
const savedNote = $<HTMLSpanElement>("#saved");
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
      { name: "Photos", extensions: ["cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "dng", "pef", "srw", "jpg", "jpeg", "png", "tif", "tiff", "heic", "heif", "hif"] },
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
    openedPath = path;
    title.textContent = `${opened.file_name} · ${opened.width}×${opened.height}`;
    savedNote.textContent = "";
    editor.tab = "adjust";
    load(opened);
  } catch (e) {
    status.textContent = `Could not open: ${e}`;
  }
}

const exportScreen = buildExport((done) => {
  const name = done.path.split(/[\\/]/).pop();
  savedNote.textContent = `Exported ${name} · ${done.width}×${done.height} · ${(done.ms / 1000).toFixed(1)} s`;
  savedNote.title = done.path;
  if (done.skipped.length > 0) {
    notice.textContent = `Exported without: ${done.skipped[0].reason}${done.skipped.length > 1 ? ` (+${done.skipped.length - 1} more)` : ""}`;
    notice.title = done.skipped.map((s) => `Layer ${s.layer}: ${s.reason}`).join("\n");
  }
});

function exportPhoto(thumbnail?: string) {
  if (editor.opened) exportScreen.show(thumbnail ?? (frameEl.hidden ? undefined : preview.src));
}

// --- Library and Edit screens ----------------------------------------------------------

type Mode = "library" | "edit";
let mode: Mode = "edit";
/** The full path of the photo in the editor. */
let openedPath: string | null = null;

const library = buildLibrary($("#library"), $<HTMLInputElement>("#search"), {
  edit: async (path) => {
    if (path !== openedPath) await openPath(path);
    setMode("edit");
  },
  quickExport: async (path, thumbnail) => {
    if (path !== openedPath) await openPath(path);
    setMode("edit");
    exportPhoto(thumbnail);
  },
});

function setMode(next: Mode) {
  const leaving = mode;
  mode = next;
  document.body.dataset.mode = next;
  $<HTMLElement>("#library").hidden = next !== "library";
  $<HTMLElement>("#editor").hidden = next !== "edit";
  for (const b of document.querySelectorAll<HTMLButtonElement>("#modes button")) {
    const on = b.dataset.mode === next;
    b.classList.toggle("active", on);
    b.setAttribute("aria-selected", String(on));
  }
  // Coming back from editing, pick up the new sidecar and thumbnail.
  if (next === "library") void library.show(leaving === "edit" ? (openedPath ?? undefined) : undefined);
  else layout();
}

for (const b of document.querySelectorAll<HTMLButtonElement>("#modes button")) {
  b.addEventListener("click", () => setMode(b.dataset.mode as Mode));
}
$("#open-folder").addEventListener("click", () => void library.chooseFolder());

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
exportButton.addEventListener("click", () => exportPhoto());
undoButton.addEventListener("click", undo);
redoButton.addEventListener("click", redo);
beforeButton.addEventListener("click", toggleBefore);

const KEYS: Record<string, string> = { v: "select", c: "crop", b: "brush", g: "linear", r: "radial", a: "ai" };

document.addEventListener("keydown", (e) => {
  const typing = e.target instanceof HTMLInputElement && e.target.type !== "range" && e.target.type !== "checkbox";
  // The export dialog has its own keys (Escape closes it).
  if (typing || document.querySelector("dialog[open]")) return;
  const mod = e.ctrlKey || e.metaKey;
  if (mod && (e.key === "1" || e.key === "2")) {
    e.preventDefault();
    setMode(e.key === "1" ? "library" : "edit");
    return;
  }
  if (mode === "library") {
    if (mod && e.key.toLowerCase() === "o") {
      e.preventDefault();
      void library.chooseFolder();
    } else if (!mod && !(e.target instanceof HTMLSelectElement || e.target instanceof HTMLInputElement)) library.key(e);
    return;
  }
  if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    if (e.shiftKey) redo();
    else undo();
  } else if (mod && e.key.toLowerCase() === "y") {
    e.preventDefault();
    redo();
  } else if (mod && e.key.toLowerCase() === "e") {
    e.preventDefault();
    exportPhoto();
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
// A photo passed on the command line opens in the editor; otherwise start
// in the library.
void invoke<string | null>("initial_path").then((path) => {
  if (path) {
    setMode("edit");
    void openPath(path);
  } else setMode("library");
});
