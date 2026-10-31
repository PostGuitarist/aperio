// Editor state, undo history, and the render loop. Panels change `editor.doc`
// and call `changed()`; the preview catches up with the latest change.

import { invoke } from "@tauri-apps/api/core";
import { type Channel, type CropOp, type EditDocument, maskLabel, readCrop } from "./doc";

export type Tab = "edit" | "crop" | "layers" | "ai" | "info";
export type Compare = "off" | "split" | "side";

export interface Opened {
  file_name: string;
  width: number;
  height: number;
  preview_width: number;
  preview_height: number;
  /** The folder the photo is in. */
  folder: string;
  doc: EditDocument;
}

export interface BrushSettings {
  size: number;
  feather: number;
  flow: number;
  erase: boolean;
}

export const editor = {
  opened: null as Opened | null,
  /** Full path of the open photo. */
  path: null as string | null,
  doc: null as unknown as EditDocument,
  selected: 0,
  /** Which part of the selected layer's combined mask is being edited. */
  part: 0,
  /** Set while the color range eyedropper waits for a click on the photo. */
  picking: null as ((u: number, v: number) => void) | null,
  tab: "edit" as Tab,
  channel: "rgb" as Channel,
  before: false,
  showMask: true,
  /** Width / height for the crop tool, or null for free. */
  aspect: null as number | null,
  brush: { size: 0.08, feather: 0.5, flow: 1, erase: false } as BrushSettings,
  /** 0 fits the photo to the window; otherwise a multiple of that size. */
  zoom: 0,
  compare: "off" as Compare,
  /** Where the split-view divider sits, 0..1 across the photo. */
  split: 0.5,
  clipping: false,
  guides: false,
};

/** Called after anything in `editor` changes, to redraw panels and overlay. */
const listeners: Array<() => void> = [];
export function onUpdate(f: () => void) {
  listeners.push(f);
}
export function refresh() {
  for (const f of listeners) f();
}

// --- History --------------------------------------------------------------

/** A document state and the change that produced it. */
export interface Step {
  json: string;
  label: string;
}

let saved = "";
let savedLabel = "Opened";
const undoStack: Step[] = [];
const redoStack: Step[] = [];

export function load(opened: Opened) {
  editor.opened = opened;
  editor.doc = opened.doc;
  editor.selected = 0;
  saved = JSON.stringify(opened.doc);
  savedLabel = "Opened";
  undoStack.length = redoStack.length = 0;
  refresh();
  requestRender(false);
}

/** Preview a change without recording it (e.g. while a slider moves). */
export function changed() {
  refresh();
  requestRender(false);
}

/** Record a finished change: add it to undo history and save the sidecar.
 * `label` names it in the History list, in place of a generated name. */
export function commit(label?: string) {
  const now = JSON.stringify(editor.doc);
  if (now !== saved) {
    undoStack.push({ json: saved, label: savedLabel });
    redoStack.length = 0;
    savedLabel = label ?? describeChange(JSON.parse(saved) as EditDocument, editor.doc);
    saved = now;
  }
  refresh();
  requestRender(true);
}

function restore(step: Step) {
  editor.doc = JSON.parse(step.json) as EditDocument;
  saved = step.json;
  savedLabel = step.label;
  editor.selected = Math.min(editor.selected, editor.doc.layers.length - 1);
  refresh();
  requestRender(true);
}

export function undo() {
  const prev = undoStack.pop();
  if (prev === undefined) return;
  redoStack.push({ json: saved, label: savedLabel });
  restore(prev);
}

export function redo() {
  const next = redoStack.pop();
  if (next === undefined) return;
  undoStack.push({ json: saved, label: savedLabel });
  restore(next);
}

export const canUndo = () => undoStack.length > 0;
export const canRedo = () => redoStack.length > 0;

/** Every step, oldest first, and which one is showing. */
export function history(): { labels: string[]; current: number } {
  const labels = [...undoStack.map((s) => s.label), savedLabel, ...[...redoStack].reverse().map((s) => s.label)];
  return { labels, current: undoStack.length };
}

/** Jump to step `i` of `history()`, keeping later steps for redo. */
export function goTo(i: number) {
  while (undoStack.length > i && canUndo()) undo();
  while (undoStack.length < i && canRedo()) redo();
}

const OP_NAMES: Record<string, string> = {
  exposure: "Exposure",
  contrast: "Contrast",
  tone: "Tone",
  white_balance: "White balance",
  color: "Color",
  presence: "Presence",
  hsl: "Color mixer",
  color_grade: "Color grading",
  curve: "Tone curve",
  crop: "Crop",
  plugin: "Effect",
};

/** A short name for what changed between two documents, for the history list. */
export function describeChange(before: EditDocument, after: EditDocument): string {
  if (after.layers.length > before.layers.length) return `Add layer: ${after.layers[after.layers.length - 1].name}`;
  if (after.layers.length < before.layers.length) {
    const gone = before.layers.find((l, i) => JSON.stringify(l) !== JSON.stringify(after.layers[i]));
    return `Delete layer${gone ? `: ${gone.name}` : ""}`;
  }
  for (let i = 0; i < after.layers.length; i++) {
    const [a, b] = [before.layers[i], after.layers[i]];
    if (JSON.stringify(a) === JSON.stringify(b)) continue;
    const where = i === 0 ? "" : `${b.name} · `;
    if (a.enabled !== b.enabled) return `${b.enabled ? "Show" : "Hide"} ${b.name}`;
    if (a.opacity !== b.opacity) return `${where}Opacity ${Math.round(b.opacity * 100)}%`;
    if ((a.blend_mode ?? "normal") !== (b.blend_mode ?? "normal")) return `${where}Blend ${(b.blend_mode ?? "normal").replace("_", " ")}`;
    if (JSON.stringify(a.mask) !== JSON.stringify(b.mask)) {
      const parts = (l: typeof a) => (l.mask?.kind === "combine" ? l.mask.parts.length : l.mask ? 1 : 0);
      if (parts(b) > parts(a) && b.mask?.kind === "combine") {
        const added = b.mask.parts[b.mask.parts.length - 1];
        return `${where}${{ add: "Add", subtract: "Subtract", intersect: "Intersect" }[added.mode]} ${maskLabel(added.mask).split(" ·")[0].toLowerCase()} mask`;
      }
      if (parts(b) < parts(a)) return `${where}Remove mask part`;
      return `${where}Edit mask`;
    }
    const names = new Set<string>();
    const ops = (l: typeof a) => new Map(l.operations.map((o) => [o.op === "curve" ? `curve:${o.channel}` : o.op === "plugin" ? `plugin:${o.plugin}` : o.op, JSON.stringify(o)]));
    const [oa, ob] = [ops(a), ops(b)];
    for (const k of new Set([...oa.keys(), ...ob.keys()])) if (oa.get(k) !== ob.get(k)) names.add(OP_NAMES[k.split(":")[0]] ?? k);
    if (names.size === 1) {
      const op = b.operations.find((o) => OP_NAMES[o.op] === [...names][0]);
      const sign = (v: number) => (v > 0 ? "+" : "") + v.toFixed(2);
      if (op?.op === "exposure") return `${where}Exposure ${sign(op.ev)} EV`;
      if (op?.op === "contrast") return `${where}Contrast ${sign(op.amount)}`;
      if (op?.op === "white_balance") return `${where}White balance ${op.temperature_k} K`;
    }
    if (names.size > 0) return where + [...names].slice(0, 2).join(", ") + (names.size > 2 ? "…" : "");
    return `${where}Edit`;
  }
  return "Edit";
}

// --- Rendering ------------------------------------------------------------

export interface Rendered {
  url: string;
  /** The crop applied to this image, if any, for mapping overlay coordinates. */
  crop: CropOp | undefined;
  ms: number;
  saved: boolean;
}

const renderedListeners: Array<(r: Rendered) => void> = [];
export function onRendered(f: (r: Rendered) => void) {
  renderedListeners.push(f);
}
const busyListeners: Array<(busy: boolean) => void> = [];
/** Told when rendering starts and when the queue is empty again. */
export function onBusy(f: (busy: boolean) => void) {
  busyListeners.push(f);
}
const errorListeners: Array<(e: unknown) => void> = [];
export function onRenderError(f: (e: unknown) => void) {
  errorListeners.push(f);
}

/** The view a render should use, given which tool is open. */
function currentView() {
  const masking = editor.showMask && editor.selected > 0 && editor.tab !== "crop";
  return {
    crop: editor.tab !== "crop",
    mask_overlay: masking && !editor.before ? editor.selected : null,
    before: editor.before,
  };
}

// Only one render in flight; while it runs, remember just the latest request.
let rendering = false;
let pending: { save: boolean } | null = null;
let lastUrl: string | null = null;

export function requestRender(save: boolean) {
  if (!editor.opened) return;
  if (rendering) {
    pending = { save: save || (pending?.save ?? false) };
    return;
  }
  void render(save);
}

async function render(save: boolean) {
  rendering = true;
  for (const f of busyListeners) f(true);
  const started = performance.now();
  const view = currentView();
  const crop = view.crop ? readCrop(editor.doc) : undefined;
  try {
    // Raw bytes arrive as an ArrayBuffer on most platforms, but as a plain
    // number array where the webview falls back to JSON IPC (seen on Linux).
    const bytes = await invoke<ArrayBuffer | number[]>("render_preview", { doc: editor.doc, view, save });
    const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
    const result = { url, crop: crop && { ...crop }, ms: Math.round(performance.now() - started), saved: save };
    for (const f of renderedListeners) f(result);
    if (lastUrl) URL.revokeObjectURL(lastUrl);
    lastUrl = url;
  } catch (e) {
    for (const f of errorListeners) f(e);
  } finally {
    rendering = false;
    if (pending) {
      const next = pending;
      pending = null;
      void render(next.save);
    } else {
      for (const f of busyListeners) f(false);
    }
  }
}

/** The photo with no adjustments, for the before/after views. Cached until
 * the photo or its crop changes. */
let beforeKey = "";
let beforeUrl: Promise<string> | null = null;
export function renderBefore(): Promise<string> {
  const crop = editor.tab !== "crop" ? readCrop(editor.doc) : undefined;
  const key = `${editor.opened?.file_name}:${JSON.stringify(crop)}`;
  if (beforeUrl && key === beforeKey) return beforeUrl;
  beforeKey = key;
  const view = { crop: editor.tab !== "crop", mask_overlay: null, before: true };
  beforeUrl = invoke<ArrayBuffer | number[]>("render_preview", { doc: editor.doc, view, save: false }).then((bytes) =>
    URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" })),
  );
  beforeUrl.catch(() => (beforeKey = ""));
  return beforeUrl;
}
