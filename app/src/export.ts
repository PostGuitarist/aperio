// The Export screen: a dialog over the editor that renders the current edit
// at full size and writes a new file. Settings are remembered between
// exports; the folder defaults to an "exports" folder beside the photo.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { readCrop } from "./doc";
import { Frame } from "./overlay";
import { h } from "./panels";
import { editor } from "./state";

type Format = "jpeg" | "png" | "tiff";

interface Settings {
  format: Format;
  quality: number;
  resize: boolean;
  longEdge: number;
  name: string;
  /** A folder the user chose, or null for the default beside each photo. */
  folder: string | null;
}

interface SkippedStep {
  layer: number;
  op: string;
  reason: string;
}

interface Exported {
  path: string;
  width: number;
  height: number;
  ms: number;
  skipped: SkippedStep[];
}

const DEFAULTS: Settings = { format: "jpeg", quality: 90, resize: false, longEdge: 3840, name: "{name}_edit", folder: null };
const STORE = "aperio.export";

const FORMATS: [Format, string, string][] = [
  ["jpeg", "JPEG", "8-bit, smallest files. Best for sharing and the web."],
  ["png", "PNG", "8-bit, lossless."],
  ["tiff", "TIFF", "16-bit, lossless. Best for print or further editing."],
];

const ASPECTS: [string, number][] = [
  ["1:1", 1],
  ["5:4", 5 / 4],
  ["4:3", 4 / 3],
  ["3:2", 3 / 2],
  ["16:9", 16 / 9],
];

function loadSettings(): Settings {
  try {
    return { ...DEFAULTS, ...(JSON.parse(localStorage.getItem(STORE) ?? "{}") as Partial<Settings>) };
  } catch {
    return { ...DEFAULTS };
  }
}

function saveSettings(s: Settings) {
  try {
    localStorage.setItem(STORE, JSON.stringify(s));
  } catch {
    // Settings just won't be remembered.
  }
}

function joinPath(dir: string, name: string) {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

function baseName(path: string) {
  return path.split(/[\\/]/).pop() ?? path;
}

/** Output size after crop and the optional resize, matching the backend. */
function outputSize(s: Settings): [number, number] {
  const o = editor.opened!;
  const [w, h] = Frame.outSize(o.width, o.height, readCrop(editor.doc));
  const long = Math.max(w, h);
  if (!s.resize || s.longEdge <= 0 || s.longEdge >= long) return [w, h];
  const k = s.longEdge / long;
  return [Math.max(1, Math.round(w * k)), Math.max(1, Math.round(h * k))];
}

function cropLabel() {
  const crop = readCrop(editor.doc);
  if (!crop) return "None";
  const o = editor.opened!;
  const [w, h] = Frame.outSize(o.width, o.height, crop);
  const ratio = Math.max(w, h) / Math.min(w, h);
  const named = ASPECTS.find(([, r]) => Math.abs(r - ratio) / r < 0.005)?.[0];
  const shape = named ? (w >= h ? named : named.split(":").reverse().join(":")) : "Custom";
  const angle = Math.abs(crop.angle_deg) >= 0.05 ? `, ${crop.angle_deg > 0 ? "+" : ""}${crop.angle_deg.toFixed(1)}°` : "";
  return shape + angle;
}

function editsLabel() {
  const layers = editor.doc.layers.filter((l) => l.enabled);
  const ops = layers.reduce((n, l) => n + l.operations.filter((op) => op.op !== "crop").length, 0);
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  return `${plural(layers.length, "layer")}, ${plural(ops, "adjustment")}`;
}

const px = (w: number, h: number) => `${w} × ${h}`;

export interface ExportScreen {
  show(thumbnail: string | undefined): void;
}

/** Build the dialog once. `onDone` hears about each finished export. */
export function buildExport(onDone: (e: Exported) => void): ExportScreen {
  let s = loadSettings();
  let busy = false;

  const dialog = h("dialog", { className: "export" });
  dialog.setAttribute("aria-label", "Export");
  const summary = h("span", { className: "muted" });
  const head = h("header", {}, h("h2", {}, "Export"), summary);

  // Left: what is being exported.
  const thumb = h("img", { className: "thumb", alt: "" });
  const facts = h("dl", { className: "facts" });
  const left = h("div", { className: "export-info" }, thumb, facts, h("p", { className: "muted" }, "The original file is never changed. This writes a new image from the edit stack."));

  // Right: settings.
  const formatRow = h("div", { className: "segmented" });
  const formatButtons = FORMATS.map(([f, label]) => {
    const b = h("button", { type: "button", textContent: label });
    b.addEventListener("click", () => update({ format: f }));
    formatRow.append(b);
    return [f, b] as const;
  });
  const formatNote = h("p", { className: "hint muted" });

  const quality = h("input", { type: "range", min: "1", max: "100", step: "1", id: "export-quality" });
  const qualityValue = h("output");
  quality.addEventListener("input", () => update({ quality: Number(quality.value) }));
  const qualityRow = h("label", { className: "slider" }, h("span", {}, "Quality"), qualityValue, quality);

  const resize = h("select", { id: "export-resize" });
  resize.append(h("option", { value: "full", textContent: "Full size" }), h("option", { value: "long", textContent: "Long edge" }));
  resize.addEventListener("change", () => update({ resize: resize.value === "long" }));
  const longEdge = h("input", { type: "number", min: "16", max: "65535", step: "1", id: "export-long-edge" });
  longEdge.setAttribute("aria-label", "Long edge in pixels");
  longEdge.addEventListener("input", () => {
    const v = Math.round(Number(longEdge.value));
    if (v >= 16) update({ longEdge: v });
  });
  const resizeField = h("div", { className: "field-row" }, resize, longEdge, h("span", { className: "muted" }, "px"));

  const folder = h("output", { className: "path mono" });
  const choose = h("button", { type: "button", textContent: "Choose…" });
  choose.addEventListener("click", async () => {
    const picked = await open({ directory: true, multiple: false, defaultPath: currentFolder() });
    if (typeof picked === "string") update({ folder: picked });
  });
  const besidePhoto = h("button", { type: "button", className: "link", textContent: "Use a folder beside each photo" });
  besidePhoto.addEventListener("click", () => update({ folder: null }));

  const name = h("input", { type: "text", id: "export-name", spellcheck: false });
  name.addEventListener("input", () => update({ name: name.value }));
  const ext = h("span", { className: "muted mono" });
  const target = h("p", { className: "hint muted target" });

  const grid = h(
    "div",
    { className: "export-grid" },
    h("span", { className: "muted" }, "Color space"),
    h("span", {}, "sRGB"),
    h("label", { className: "muted", htmlFor: "export-resize" }, "Resize"),
    resizeField,
  );
  const right = h(
    "div",
    { className: "export-settings" },
    h("h3", {}, "Format"),
    formatRow,
    formatNote,
    qualityRow,
    grid,
    h("hr"),
    h("h3", {}, "Save to"),
    h("div", { className: "field-row" }, folder, choose),
    besidePhoto,
    h("label", { className: "muted name-label", htmlFor: "export-name" }, "File name"),
    h("div", { className: "field-row" }, name, ext),
    target,
  );

  const status = h("span", { className: "muted" });
  const cancel = h("button", { type: "button", textContent: "Cancel" });
  const go = h("button", { type: "button", className: "primary", textContent: "Export image" });
  cancel.addEventListener("click", () => dialog.close());
  go.addEventListener("click", () => void run());
  const foot = h("footer", {}, status, h("span", { className: "actions" }, cancel, go));

  dialog.append(head, h("div", { className: "export-body" }, left, right), foot);
  document.body.append(dialog);
  dialog.addEventListener("cancel", (e) => {
    if (busy) e.preventDefault();
  });

  function currentFolder() {
    return s.folder ?? joinPath(editor.opened!.folder, "exports");
  }

  function options() {
    return { folder: currentFolder(), name: s.name, format: s.format, quality: s.quality, long_edge: s.resize ? s.longEdge : null };
  }

  let exists = false;
  let targetPath = "";
  let targetCheck = 0;
  async function checkTarget() {
    const n = ++targetCheck;
    try {
      const t = await invoke<{ path: string; exists: boolean }>("export_target", { options: options() });
      if (n !== targetCheck) return;
      exists = t.exists;
      targetPath = t.path;
      target.textContent = t.exists ? `${baseName(t.path)} already exists and will be replaced.` : `Saves as ${baseName(t.path)}`;
      target.classList.toggle("warn", t.exists);
      go.disabled = busy;
    } catch (e) {
      if (n !== targetCheck) return;
      target.textContent = String(e);
      target.classList.add("warn");
      go.disabled = true;
    }
  }

  function update(change: Partial<Settings>) {
    s = { ...s, ...change };
    saveSettings(s);
    sync();
  }

  function sync() {
    const o = editor.opened!;
    summary.textContent = `${o.file_name} → 1 file`;
    for (const [f, b] of formatButtons) b.classList.toggle("active", f === s.format);
    formatNote.textContent = FORMATS.find(([f]) => f === s.format)![2];
    qualityRow.hidden = s.format !== "jpeg";
    quality.value = String(s.quality);
    qualityValue.textContent = String(s.quality);
    resize.value = s.resize ? "long" : "full";
    longEdge.hidden = !s.resize;
    longEdge.nextElementSibling!.toggleAttribute("hidden", !s.resize);
    if (document.activeElement !== longEdge) longEdge.value = String(s.longEdge);
    folder.textContent = currentFolder();
    folder.title = currentFolder();
    besidePhoto.hidden = s.folder === null;
    if (document.activeElement !== name) name.value = s.name;
    ext.textContent = { jpeg: ".jpg", png: ".png", tiff: ".tif" }[s.format];

    const [w, hgt] = outputSize(s);
    facts.replaceChildren(
      h("dt", {}, "Output"),
      h("dd", { className: "mono" }, px(w, hgt)),
      h("dt", {}, "Original"),
      h("dd", { className: "mono" }, px(o.width, o.height)),
      h("dt", {}, "Crop"),
      h("dd", {}, cropLabel()),
      h("dt", {}, "Edits"),
      h("dd", {}, editsLabel()),
    );
    void checkTarget();
  }

  async function run() {
    if (busy) return;
    if (exists && !confirm(`${baseName(targetPath)} already exists in this folder. Replace it?`)) return;
    busy = true;
    go.disabled = cancel.disabled = true;
    go.textContent = "Exporting…";
    status.textContent = "Rendering at full size…";
    try {
      const done = await invoke<Exported>("export_image", { options: options() });
      onDone(done);
      dialog.close();
    } catch (e) {
      status.textContent = `Export failed: ${e}`;
    } finally {
      busy = false;
      go.disabled = cancel.disabled = false;
      go.textContent = "Export image";
    }
  }

  return {
    show(thumbnail) {
      if (!editor.opened) return;
      if (thumbnail) thumb.src = thumbnail;
      thumb.hidden = !thumbnail;
      void invoke<boolean | null>("renderer_info").then((gpu) => {
        if (!busy) status.textContent = gpu === null ? "" : gpu ? "Renders on your GPU" : "Renders on the CPU";
      });
      sync();
      dialog.showModal();
    },
  };
}
