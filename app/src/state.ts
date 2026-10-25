// Editor state, undo history, and the render loop. Panels change `editor.doc`
// and call `changed()`; the preview catches up with the latest change.

import { invoke } from "@tauri-apps/api/core";
import { type Channel, type CropOp, type EditDocument, readCrop } from "./doc";

export type Tab = "adjust" | "curves" | "crop" | "layers" | "ai";

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
  doc: null as unknown as EditDocument,
  selected: 0,
  tab: "adjust" as Tab,
  channel: "rgb" as Channel,
  before: false,
  showMask: true,
  /** Width / height for the crop tool, or null for free. */
  aspect: null as number | null,
  brush: { size: 0.08, feather: 0.5, flow: 1, erase: false } as BrushSettings,
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

let saved = "";
const undoStack: string[] = [];
const redoStack: string[] = [];

export function load(opened: Opened) {
  editor.opened = opened;
  editor.doc = opened.doc;
  editor.selected = 0;
  saved = JSON.stringify(opened.doc);
  undoStack.length = redoStack.length = 0;
  refresh();
  requestRender(false);
}

/** Preview a change without recording it (e.g. while a slider moves). */
export function changed() {
  refresh();
  requestRender(false);
}

/** Record a finished change: add it to undo history and save the sidecar. */
export function commit() {
  const now = JSON.stringify(editor.doc);
  if (now !== saved) {
    undoStack.push(saved);
    redoStack.length = 0;
    saved = now;
  }
  refresh();
  requestRender(true);
}

function restore(json: string) {
  editor.doc = JSON.parse(json) as EditDocument;
  saved = json;
  editor.selected = Math.min(editor.selected, editor.doc.layers.length - 1);
  refresh();
  requestRender(true);
}

export function undo() {
  const prev = undoStack.pop();
  if (prev === undefined) return;
  redoStack.push(saved);
  restore(prev);
}

export function redo() {
  const next = redoStack.pop();
  if (next === undefined) return;
  undoStack.push(saved);
  restore(next);
}

export const canUndo = () => undoStack.length > 0;
export const canRedo = () => redoStack.length > 0;

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
