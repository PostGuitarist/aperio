import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { loadPlugins } from "./ai";
import { buildExport } from "./export";
import { buildFilmstrip } from "./filmstrip";
import { buildHistory } from "./history";
import { buildLibrary } from "./library";
import { drawClipping, drawHistogram, sampleAt } from "./histogram";
import { Frame, Overlay } from "./overlay";
import { type Command, buildPalette, keyLabel } from "./palette";
import { addLayer, buildPanels, h, photoDetails, showTab } from "./panels";
import {
  type Compare,
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
  renderBefore,
  undo,
} from "./state";

const $ = <T extends Element>(sel: string) => document.querySelector<T>(sel)!;
const stage = $<HTMLDivElement>("#stage");
const frames = $<HTMLDivElement>("#frames");
const frameEl = $<HTMLDivElement>("#frame");
const beforeFrame = $<HTMLDivElement>("#before-frame");
const preview = $<HTMLImageElement>("#preview");
const beforeSide = $<HTMLImageElement>("#before-side");
const beforeSplit = $<HTMLImageElement>("#before-split");
const divider = $<HTMLDivElement>("#divider");
const sideTag = $<HTMLSpanElement>("#side-tag");
const clipping = $<HTMLCanvasElement>("#clipping");
const guides = $<HTMLDivElement>("#guides");
const status = $<HTMLParagraphElement>("#status");
const title = $("#title");
const exportButton = $<HTMLButtonElement>("#export");
const undoButton = $<HTMLButtonElement>("#undo");
const redoButton = $<HTMLButtonElement>("#redo");
const timing = $("#timing");
const savedNote = $<HTMLSpanElement>("#saved");
const notice = $<HTMLSpanElement>("#notice");
const readout = $<HTMLSpanElement>("#readout");
const histogram = $<HTMLCanvasElement>("#histogram");
const clipLow = $<HTMLButtonElement>("#clip-low");
const clipHigh = $<HTMLButtonElement>("#clip-high");
const exif = $<HTMLDivElement>("#exif");
const overlay = new Overlay($<SVGSVGElement>("#overlay"));

buildPanels($("#tabs"), $("#tab-body"));
buildHistory($("#side-left"));
$("#command-key").textContent = keyLabel("Ctrl+K");

void invoke<boolean | null>("renderer_info").then((gpu) => {
  $("#gpu").textContent = gpu === null ? "" : gpu ? "● GPU" : "● CPU";
  $("#gpu").classList.toggle("gpu", !!gpu);
});

// --- Preview layout -----------------------------------------------------------

/** The crop of the image currently on screen (not the one being requested). */
let shownCrop: Rendered["crop"];

/** Fit the preview into the stage (or zoom it) and line the overlay up. */
function layout() {
  const o = editor.opened;
  if (!o || !preview.naturalWidth) return;
  const pad = 32;
  const side = editor.compare === "side" && editor.tab !== "crop";
  const gap = side ? 12 : 0;
  const [sw, sh] = [(stage.clientWidth - pad - gap) / (side ? 2 : 1), stage.clientHeight - pad];
  const [iw, ih] = Frame.outSize(o.preview_width, o.preview_height, shownCrop);
  const fit = Math.min(sw / iw, sh / ih);
  const scale = editor.zoom > 0 ? fit * editor.zoom : fit;
  for (const f of side ? [frameEl, beforeFrame] : [frameEl]) {
    f.style.width = `${iw * scale}px`;
    f.style.height = `${ih * scale}px`;
  }
  stage.classList.toggle("zoomed", editor.zoom > 0);
  overlay.setFrame(new Frame(o.preview_width, o.preview_height, shownCrop, scale));
}

new ResizeObserver(layout).observe(stage);

/** Show or hide the before/after views to match `editor.compare`. */
async function showCompare() {
  const mode = editor.opened && editor.tab !== "crop" && !editor.before ? editor.compare : "off";
  beforeFrame.hidden = mode !== "side" || frameEl.hidden;
  sideTag.hidden = mode !== "side";
  beforeSplit.hidden = divider.hidden = mode !== "split";
  frames.classList.toggle("side", mode === "side");
  placeDivider();
  layout();
  if (mode === "off") return;
  const url = await renderBefore().catch(() => "");
  if (url) beforeSide.src = beforeSplit.src = url;
}

function placeDivider() {
  const pct = `${editor.split * 100}%`;
  divider.style.left = pct;
  beforeSplit.style.clipPath = `inset(0 ${100 - editor.split * 100}% 0 0)`;
}

divider.addEventListener("pointerdown", (e) => {
  divider.setPointerCapture(e.pointerId);
  const move = (ev: PointerEvent) => {
    const r = frameEl.getBoundingClientRect();
    editor.split = Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width));
    placeDivider();
  };
  move(e);
  divider.addEventListener("pointermove", move);
  divider.addEventListener("pointerup", () => divider.removeEventListener("pointermove", move), { once: true });
});

onRendered((r) => {
  preview.onload = () => {
    shownCrop = r.crop;
    frameEl.hidden = false;
    status.hidden = true;
    layout();
    const clipped = drawHistogram(histogram, preview);
    clipLow.classList.toggle("on", clipped.shadows);
    clipHigh.classList.toggle("on", clipped.highlights);
    if (editor.clipping) drawClipping(clipping);
    void showCompare();
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

// The pixel under the pointer, from the preview as shown.
frameEl.addEventListener("pointermove", (e) => {
  const r = frameEl.getBoundingClientRect();
  const px = sampleAt((e.clientX - r.left) / r.width, (e.clientY - r.top) / r.height);
  if (!px) return void (readout.textContent = "");
  const l = Math.round(((0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]) / 255) * 100);
  readout.textContent = `R ${px[0]}  G ${px[1]}  B ${px[2]}  ·  L ${l}%`;
});
frameEl.addEventListener("pointerleave", () => (readout.textContent = ""));

// --- View bar: zoom, compare, clipping, guides ----------------------------------------------

const viewbar = $<HTMLDivElement>("#viewbar");

function segmented<T>(label: string, options: [T, string, string][], get: () => T, set: (v: T) => void) {
  const seg = h("div", { className: "segmented" });
  const buttons = options.map(([value, text, tip]) => {
    const b = h("button", { type: "button", title: tip });
    b.innerHTML = text;
    b.addEventListener("click", () => set(value));
    seg.append(b);
    return [value, b] as const;
  });
  viewbar.append(h("span", { className: "label" }, label), seg);
  onUpdate(() => {
    for (const [value, b] of buttons) {
      b.classList.toggle("active", value === get());
      b.disabled = !editor.opened;
    }
  });
}

function setZoom(z: number) {
  editor.zoom = z;
  refresh();
  layout();
  if (z > 0) stage.scrollTo({ left: (stage.scrollWidth - stage.clientWidth) / 2, top: (stage.scrollHeight - stage.clientHeight) / 2 });
}

function setCompare(c: Compare) {
  editor.compare = c;
  if (c !== "off" && editor.before) {
    editor.before = false;
    changed();
  } else refresh();
  void showCompare();
}

function setClipping(on: boolean) {
  editor.clipping = on;
  clipping.hidden = !on;
  if (on) drawClipping(clipping);
  refresh();
}

function setGuides(on: boolean) {
  editor.guides = on;
  guides.hidden = !on;
  refresh();
}

const SPLIT_ICON = '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M12 5v14"/></svg>';
const SIDE_ICON = '<svg viewBox="0 0 24 24"><rect x="2" y="6" width="9" height="12" rx="1.5"/><rect x="13" y="6" width="9" height="12" rx="1.5"/></svg>';
const GRID_ICON = '<svg viewBox="0 0 24 24"><rect x="4" y="4" width="16" height="16" rx="1"/><path d="M4 9.3h16M4 14.7h16M9.3 4v16M14.7 4v16"/></svg>';
segmented("Zoom", [[0, "Fit", "Fit (0)"], [2, "2×", "Zoom 2× (Z)"], [4, "4×", "Zoom 4×"]], () => editor.zoom, setZoom);
segmented<Compare>("Compare", [["off", "Off", "Show the edit"], ["split", SPLIT_ICON, "Split before/after (Y)"], ["side", SIDE_ICON, "Side by side (Shift+Y)"]], () => editor.compare, setCompare);
segmented("Clipping", [[true, '<span class="clip-icon"></span>', "Show clipped shadows and highlights (J)"]], () => editor.clipping, () => setClipping(!editor.clipping));
segmented("Guides", [[true, GRID_ICON, "Rule-of-thirds grid (Shift+G)"]], () => editor.guides, () => setGuides(!editor.guides));
viewbar.append(h("span", { className: "spacer" }));
const beforeButton = h("button", { type: "button", title: "Hold or press \\ to see the photo before edits", textContent: "Before" });
beforeButton.addEventListener("click", toggleBefore);
viewbar.append(beforeButton);
clipLow.addEventListener("click", () => setClipping(!editor.clipping));
clipHigh.addEventListener("click", () => setClipping(!editor.clipping));

// Dragging the zoomed photo pans it (the space bar or middle button works with any tool).
let panning: { x: number; y: number; sl: number; st: number } | null = null;
let spaceDown = false;
stage.addEventListener("pointerdown", (e) => {
  if (editor.zoom === 0 || !(spaceDown || e.button === 1 || e.target === stage)) return;
  e.preventDefault();
  e.stopPropagation();
  panning = { x: e.clientX, y: e.clientY, sl: stage.scrollLeft, st: stage.scrollTop };
  stage.setPointerCapture(e.pointerId);
}, true);
stage.addEventListener("pointermove", (e) => {
  if (!panning) return;
  stage.scrollLeft = panning.sl - (e.clientX - panning.x);
  stage.scrollTop = panning.st - (e.clientY - panning.y);
});
stage.addEventListener("pointerup", () => (panning = null));

onUpdate(() => {
  const ready = !!editor.opened;
  exportButton.disabled = beforeButton.disabled = !ready;
  undoButton.disabled = !canUndo();
  redoButton.disabled = !canRedo();
  beforeButton.classList.toggle("active", editor.before);
  for (const b of document.querySelectorAll<HTMLButtonElement>("#rail button[data-tool]")) {
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
  frameEl.hidden = beforeFrame.hidden = true;
  try {
    const opened = await invoke<Opened>("open_image", { path });
    openedPath = path;
    editor.path = path;
    title.replaceChildren(
      h("span", {}, opened.folder.split(/[\\/]/).pop() ?? ""),
      h("span", { className: "faint" }, " / "),
      h("strong", {}, opened.file_name),
      h("span", { className: "faint mono" }, `  ${opened.width}×${opened.height}`),
    );
    savedNote.textContent = "";
    exif.textContent = "";
    if (editor.tab === "crop") editor.tab = "edit";
    load(opened);
    void filmstrip.show(path, opened.folder);
    void photoDetails().then((d) => {
      if (!d || editor.path !== path) return;
      exif.replaceChildren(...[d.camera, d.lens, d.exposure].filter((v): v is string => !!v).map((v) => h("span", {}, v)));
    });
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

const filmstrip = buildFilmstrip($("#filmstrip"), (path) => void openPath(path));

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

// --- Tools, panels and commands ---------------------------------------------------------

function useTool(tool: string | undefined) {
  if (!editor.opened) return;
  if (tool === "select") {
    editor.selected = 0;
    showTab("edit");
  } else if (tool === "crop") showTab("crop");
  else if (tool === "ai") showTab("ai");
  else if (tool === "brush" || tool === "linear" || tool === "radial") addLayer(tool);
}

function toggleBefore() {
  if (!editor.opened) return;
  editor.before = !editor.before;
  changed();
  void showCompare();
}

function togglePanels() {
  const shell = $<HTMLElement>("#editor");
  shell.classList.toggle("panels-hidden");
  layout();
}

for (const b of document.querySelectorAll<HTMLButtonElement>("#rail button[data-tool]")) {
  b.addEventListener("click", () => useTool(b.dataset.tool));
}
$("#toggle-panels").addEventListener("click", togglePanels);
$("#open").addEventListener("click", openPhoto);
exportButton.addEventListener("click", () => exportPhoto());
undoButton.addEventListener("click", undo);
redoButton.addEventListener("click", redo);

const opened = () => !!editor.opened;
const step = (n: number) => () => {
  const next = filmstrip.neighbor(n);
  if (next) void openPath(next);
};

/** Every action, for the command palette, the shortcut sheet and the keys. */
const COMMANDS: Command[] = [
  { title: "Open photo…", group: "File", keys: "Ctrl+O", run: () => void openPhoto() },
  { title: "Export…", group: "File", keys: "Ctrl+E", run: () => exportPhoto(), enabled: opened },
  { title: "Library", group: "File", keys: "Ctrl+1", run: () => setMode("library") },
  { title: "Develop", group: "File", keys: "Ctrl+2", run: () => setMode("edit") },
  { title: "Next photo", group: "File", keys: "→", run: step(1), enabled: opened },
  { title: "Previous photo", group: "File", keys: "←", run: step(-1), enabled: opened },
  { title: "Undo", group: "Edit", keys: "Ctrl+Z", run: undo, enabled: canUndo },
  { title: "Redo", group: "Edit", keys: "Ctrl+Shift+Z", run: redo, enabled: canRedo },
  { title: "Edit tab", group: "Edit", keys: "V", run: () => useTool("select"), enabled: opened },
  { title: "Crop and straighten", group: "Edit", keys: "C", run: () => useTool("crop"), enabled: opened },
  { title: "New brush mask", group: "Masks", keys: "B", run: () => useTool("brush"), enabled: opened },
  { title: "New gradient mask", group: "Masks", keys: "G", run: () => useTool("linear"), enabled: opened },
  { title: "New radial mask", group: "Masks", keys: "R", run: () => useTool("radial"), enabled: opened },
  { title: "AI tools (sky, subject, background)", group: "Masks", keys: "A", run: () => useTool("ai"), enabled: opened },
  { title: "Show mask overlay", group: "Masks", keys: "O", run: () => ((editor.showMask = !editor.showMask), changed()), enabled: opened },
  { title: "Layers", group: "Panels", keys: "L", run: () => showTab("layers"), enabled: opened },
  { title: "Photo info", group: "Panels", keys: "I", run: () => showTab("info"), enabled: opened },
  { title: "Hide or show panels", group: "Panels", keys: "Tab", run: togglePanels },
  { title: "Hide or show filmstrip", group: "Panels", keys: "F", run: () => (filmstrip.toggle(), layout()) },
  { title: "Before / after", group: "View", keys: "\\", run: toggleBefore, enabled: opened },
  { title: "Split before/after", group: "View", keys: "Y", run: () => setCompare(editor.compare === "split" ? "off" : "split"), enabled: opened },
  { title: "Side-by-side before/after", group: "View", keys: "Shift+Y", run: () => setCompare(editor.compare === "side" ? "off" : "side"), enabled: opened },
  { title: "Show clipping", group: "View", keys: "J", run: () => setClipping(!editor.clipping), enabled: opened },
  { title: "Rule-of-thirds grid", group: "View", keys: "Shift+G", run: () => setGuides(!editor.guides), enabled: opened },
  { title: "Fit to window", group: "View", keys: "0", run: () => setZoom(0), enabled: opened },
  { title: "Zoom 2× / fit", group: "View", keys: "Z", run: () => setZoom(editor.zoom ? 0 : 2), enabled: opened },
  { title: "Zoom 4×", group: "View", run: () => setZoom(4), enabled: opened },
  { title: "Command palette", group: "Help", keys: "Ctrl+K", run: () => palette.open() },
  { title: "Keyboard shortcuts", group: "Help", keys: "?", run: () => palette.shortcuts() },
];

const palette = buildPalette(() => COMMANDS);
$("#command").addEventListener("click", () => palette.open());
$("#shortcuts").addEventListener("click", () => palette.shortcuts());

/** "Ctrl+Shift+Z" for this key event, matching how COMMANDS write keys. */
function chord(e: KeyboardEvent) {
  const key = e.key === "ArrowRight" ? "→" : e.key === "ArrowLeft" ? "←" : e.key.length === 1 ? e.key.toUpperCase() : e.key;
  // Shift is part of "?" itself.
  const shift = e.shiftKey && key !== "?" ? "Shift+" : "";
  return `${e.ctrlKey || e.metaKey ? "Ctrl+" : ""}${e.altKey ? "Alt+" : ""}${shift}${key}`;
}

document.addEventListener("keyup", (e) => {
  if (e.key === " ") spaceDown = false;
});

document.addEventListener("keydown", (e) => {
  const typing = (e.target instanceof HTMLInputElement && e.target.type !== "range" && e.target.type !== "checkbox") || e.target instanceof HTMLTextAreaElement;
  // Dialogs (export, palette) have their own keys (Escape closes them).
  if (typing || document.querySelector("dialog[open]")) return;
  const mod = e.ctrlKey || e.metaKey;
  if (mode === "library") {
    if (mod && (e.key === "1" || e.key === "2")) {
      e.preventDefault();
      setMode(e.key === "1" ? "library" : "edit");
    } else if (mod && e.key.toLowerCase() === "o") {
      e.preventDefault();
      void library.chooseFolder();
    } else if (mod && e.key.toLowerCase() === "k") {
      e.preventDefault();
      palette.open();
    } else if (!mod && !(e.target instanceof HTMLSelectElement || e.target instanceof HTMLInputElement)) library.key(e);
    return;
  }
  if (e.key === " " && editor.zoom > 0) {
    spaceDown = true;
    e.preventDefault();
    return;
  }
  // Arrow keys belong to a focused slider.
  if (e.target instanceof HTMLInputElement && e.target.type === "range" && e.key.startsWith("Arrow")) return;
  const pressed = chord(e) === "Ctrl+Y" ? "Ctrl+Shift+Z" : chord(e);
  const command = COMMANDS.find((c) => c.keys === pressed);
  if (!command) return;
  e.preventDefault();
  if (command.enabled?.() ?? true) command.run();
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
