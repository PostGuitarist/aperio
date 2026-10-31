// The right-hand panel's tabs: Adjust, Curves, Crop, and Layers. Each is built
// once and re-synced from `editor` whenever something changes.

import {
  type Adjustments,
  type Channel,
  type Layer,
  type Mask,
  type Point,
  IDENTITY,
  NEUTRAL,
  bake,
  maskLabel,
  readAdjustments,
  readCrop,
  readCurve,
  writeAdjustments,
  writeCrop,
  writeCurve,
} from "./doc";
import { DENOISE, buildAi, denoiseControls, pluginLayerControls } from "./ai";
import { buildGrading, buildMixer, buildPresence } from "./color";
import { aspectFor, boxForAspect, currentBox, fitInside, setBox } from "./overlay";
import { invoke } from "@tauri-apps/api/core";
import { type Tab, changed, commit, editor, onUpdate, refresh } from "./state";

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props: Record<string, unknown> = {}, ...children: (Node | string)[]) {
  const e: HTMLElementTagNameMap[K] = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

const signed = (digits: number) => (v: number) => (v > 0 ? "+" : "") + v.toFixed(digits);

interface SliderOptions {
  label: string;
  min: number;
  max: number;
  step: number;
  neutral: number;
  format: (v: number) => string;
  get: () => number;
  set: (v: number) => void;
  /** Extra class on the track, e.g. a temperature gradient. */
  track?: string;
  /** A CSS background for the track, for gradients that change. */
  gradient?: () => string;
}

/** A labelled range input. Moving it previews; releasing it commits;
 * double-clicking resets it. Returns a function that re-reads its value. */
export function slider(parent: HTMLElement, o: SliderOptions): () => void {
  const value = h("output");
  const input = h("input", { type: "range", min: String(o.min), max: String(o.max), step: String(o.step) });
  if (o.track) input.classList.add(o.track);
  const show = () => (value.textContent = o.format(Number(input.value)));
  input.addEventListener("input", () => {
    o.set(Number(input.value));
    show();
    changed();
  });
  input.addEventListener("change", () => commit());
  input.addEventListener("dblclick", () => {
    o.set(o.neutral);
    commit();
  });
  parent.append(h("label", { className: "slider" }, h("span", {}, o.label), value, input));
  const sync = () => {
    if (o.gradient) input.style.background = o.gradient();
    input.disabled = !editor.opened;
    input.value = String(o.get());
    show();
  };
  sync();
  return sync;
}

export function group(parent: HTMLElement, title: string, onReset?: () => void) {
  const head = h("div", { className: "group-head" }, h("h3", {}, title));
  if (onReset) {
    const reset = h("button", { className: "link", type: "button", textContent: "Reset" });
    reset.addEventListener("click", onReset);
    head.append(reset);
  }
  const body = h("div", { className: "group" });
  parent.append(head, body);
  return body;
}

const SECTIONS_KEY = "aperio.sections";
const closed = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem(SECTIONS_KEY) ?? "[]") as string[];
    } catch {
      return [];
    }
  })(),
);

/** A collapsible part of the Edit tab. The header shows a dot while anything
 * in it differs from neutral, and a reset button. Returns its body and a
 * function that refreshes the dot. */
export function section(parent: HTMLElement, title: string, o: { isChanged: () => boolean; onReset: () => void }) {
  const chevron = h("span", { className: "chev" });
  const dot = h("span", { className: "changed-dot", title: "Changed" });
  const reset = h("button", { type: "button", className: "icon reset", title: `Reset ${title.toLowerCase()}`, textContent: "↺" });
  reset.addEventListener("click", (e) => {
    e.stopPropagation();
    o.onReset();
  });
  const head = h("button", { type: "button", className: "section-head" }, chevron, h("span", { className: "title" }, title), dot);
  const body = h("div", { className: "section-body" });
  const wrap = h("section", { className: "section" }, h("div", { className: "section-bar" }, head, reset), body);
  const show = () => {
    const open = !closed.has(title);
    wrap.classList.toggle("open", open);
    body.hidden = !open;
  };
  head.addEventListener("click", () => {
    if (closed.has(title)) closed.delete(title);
    else closed.add(title);
    try {
      localStorage.setItem(SECTIONS_KEY, JSON.stringify([...closed]));
    } catch {
      // Not remembered, then.
    }
    show();
  });
  show();
  parent.append(wrap);
  return {
    body,
    sync: () => {
      const on = !!editor.opened && o.isChanged();
      dot.hidden = !on;
      reset.disabled = !on;
    },
  };
}

const layer = () => editor.doc.layers[editor.selected];

function editingNote() {
  const note = h("p", { className: "editing muted" });
  return {
    note,
    sync: () => {
      const many = editor.opened && editor.doc.layers.length > 1;
      note.hidden = !many;
      if (many) note.textContent = `Editing layer: ${layer().name}`;
    },
  };
}

// --- Edit ---------------------------------------------------------------------

function buildEdit(root: HTMLElement) {
  const syncs: Array<() => void> = [];

  const adj = () => (editor.opened ? readAdjustments(layer()) : { ...NEUTRAL });
  const setter = (key: keyof Adjustments) => (v: number) => writeAdjustments(layer(), { ...adj(), [key]: v });
  const resetKeys = (keys: (keyof Adjustments)[]) => () => {
    const a = adj();
    for (const k of keys) a[k] = NEUTRAL[k];
    writeAdjustments(layer(), a);
    commit();
  };
  const add = (body: HTMLElement, key: keyof Adjustments, label: string, min: number, max: number, step: number, format: (v: number) => string, track?: string) =>
    syncs.push(slider(body, { label, min, max, step, neutral: NEUTRAL[key], format, get: () => adj()[key], set: setter(key), track }));

  const differs = (keys: (keyof Adjustments)[]) => () => keys.some((k) => adj()[k] !== NEUTRAL[k]);
  const LIGHT: (keyof Adjustments)[] = ["exposure", "contrast", "highlights", "shadows", "whites", "blacks"];
  const COLOR: (keyof Adjustments)[] = ["temperature", "tint", "vibrance", "saturation"];

  const light = section(root, "Light", { isChanged: differs(LIGHT), onReset: resetKeys(LIGHT) });
  syncs.push(light.sync);
  add(light.body, "exposure", "Exposure", -5, 5, 0.05, (v) => `${signed(2)(v)} EV`);
  add(light.body, "contrast", "Contrast", -1, 1, 0.01, signed(2));
  add(light.body, "highlights", "Highlights", -1, 1, 0.01, signed(2));
  add(light.body, "shadows", "Shadows", -1, 1, 0.01, signed(2));
  add(light.body, "whites", "Whites", -1, 1, 0.01, signed(2));
  add(light.body, "blacks", "Blacks", -1, 1, 0.01, signed(2));

  syncs.push(buildPresence(root, layer));

  const color = section(root, "Color", { isChanged: differs(COLOR), onReset: resetKeys(COLOR) });
  syncs.push(color.sync);
  add(color.body, "temperature", "Temperature", 2000, 12000, 50, (v) => `${v} K`, "temp");
  add(color.body, "tint", "Tint", -1, 1, 0.01, signed(2), "tint");
  add(color.body, "vibrance", "Vibrance", -1, 1, 0.01, signed(2));
  add(color.body, "saturation", "Saturation", -1, 1, 0.01, signed(2));

  syncs.push(buildMixer(root, layer), buildGrading(root, layer));

  const curve = section(root, "Tone curve", {
    isChanged: () => layer().operations.some((o) => o.op === "curve"),
    onReset: () => {
      for (const [ch] of CHANNELS) writeCurve(layer(), ch, IDENTITY);
      commit();
    },
  });
  syncs.push(curve.sync, buildCurves(curve.body));

  const hasDenoise = () => editor.doc.layers[0].operations.some((o) => o.op === "plugin" && o.plugin === DENOISE);
  const detail = section(root, "Detail", {
    isChanged: hasDenoise,
    onReset: () => {
      const base = editor.doc.layers[0];
      base.operations = base.operations.filter((o) => !(o.op === "plugin" && o.plugin === DENOISE));
      commit();
    },
  });
  syncs.push(detail.sync, denoiseControls(detail.body));

  return () => syncs.forEach((s) => s());
}

// --- Curves -----------------------------------------------------------------------

const CHANNELS: [Channel, string][] = [
  ["rgb", "RGB"],
  ["red", "Red"],
  ["green", "Green"],
  ["blue", "Blue"],
  ["luma", "Luma"],
];
const CURVE_SIZE = 256;
const GRAB = 10;

function buildCurves(root: HTMLElement) {

  const tabs = h("div", { className: "segmented" });
  const buttons = CHANNELS.map(([ch, label]) => {
    const b = h("button", { type: "button", textContent: label });
    b.dataset.channel = ch;
    b.addEventListener("click", () => {
      editor.channel = ch;
      refresh();
    });
    tabs.append(b);
    return b;
  });
  root.append(tabs);

  const NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", `0 0 ${CURVE_SIZE} ${CURVE_SIZE}`);
  svg.classList.add("curve");
  root.append(h("div", { className: "curve-wrap" }, svg));
  root.append(h("p", { className: "hint muted" }, "Click to add a point, drag to move it, and double-click or drag it off the graph to remove it."));
  const reset = h("button", { type: "button", className: "link", textContent: "Reset this channel" });
  reset.addEventListener("click", () => {
    writeCurve(layer(), editor.channel, IDENTITY);
    commit();
  });
  root.append(reset);

  let points: Point[] = IDENTITY;
  const toPx = ([x, y]: Point): Point => [x * CURVE_SIZE, (1 - y) * CURVE_SIZE];
  const fromEvent = (e: MouseEvent): Point => {
    const r = svg.getBoundingClientRect();
    return [(e.clientX - r.left) / r.width, 1 - (e.clientY - r.top) / r.height];
  };
  const store = () => writeCurve(layer(), editor.channel, points);

  const draw = () => {
    svg.replaceChildren();
    const line = (d: string, cls: string) => {
      const p = document.createElementNS(NS, "path");
      p.setAttribute("d", d);
      p.setAttribute("class", cls);
      svg.append(p);
    };
    for (const t of [0.25, 0.5, 0.75]) {
      const v = t * CURVE_SIZE;
      line(`M${v},0V${CURVE_SIZE}M0,${v}H${CURVE_SIZE}`, "grid");
    }
    line(`M0,${CURVE_SIZE}L${CURVE_SIZE},0`, "grid");
    const ys = bake(points, 129);
    line("M" + ys.map((y, i) => toPx([i / 128, y]).join(",")).join("L"), `curve-line ${editor.channel}`);
    for (const p of points) {
      const c = document.createElementNS(NS, "circle");
      const [x, y] = toPx(p);
      c.setAttribute("cx", String(x));
      c.setAttribute("cy", String(y));
      c.setAttribute("r", "5");
      c.setAttribute("class", "point");
      svg.append(c);
    }
  };

  let dragging = -1;
  let active = false;
  svg.addEventListener("pointerdown", (e) => {
    if (!editor.opened) return;
    active = true;
    const [x, y] = fromEvent(e);
    const scale = svg.getBoundingClientRect().width / CURVE_SIZE;
    dragging = points.findIndex((p) => Math.hypot((p[0] - x) * CURVE_SIZE, (p[1] - y) * CURVE_SIZE) * scale < GRAB);
    if (dragging < 0) {
      points = [...points, [x, y] as Point].sort((a, b) => a[0] - b[0]);
      dragging = points.findIndex((p) => p[0] === x && p[1] === y);
      store();
      changed();
    }
    svg.setPointerCapture(e.pointerId);
  });
  svg.addEventListener("pointermove", (e) => {
    if (dragging < 0) return;
    const [x, y] = fromEvent(e);
    const interior = dragging > 0 && dragging < points.length - 1;
    // Dragging an inner point well off the graph removes it.
    if (interior && (y < -0.15 || y > 1.15)) {
      points = points.filter((_, i) => i !== dragging);
      dragging = -1;
      store();
      changed();
      return;
    }
    const lo = dragging > 0 ? points[dragging - 1][0] + 0.01 : 0;
    const hi = dragging < points.length - 1 ? points[dragging + 1][0] - 0.01 : 1;
    const r = (v: number) => Math.round(v * 1000) / 1000;
    points[dragging] = [r(Math.min(hi, Math.max(lo, x))), r(Math.min(1, Math.max(0, y)))];
    store();
    changed();
  });
  const end = () => {
    if (!active) return;
    active = false;
    dragging = -1;
    commit();
  };
  svg.addEventListener("pointerup", end);
  svg.addEventListener("dblclick", (e) => {
    const [x, y] = fromEvent(e);
    const i = points.findIndex((p) => Math.hypot(p[0] - x, p[1] - y) * CURVE_SIZE < GRAB);
    if (i > 0 && i < points.length - 1) {
      points = points.filter((_, j) => j !== i);
      store();
      commit();
    }
  });

  return () => {
    for (const b of buttons) b.classList.toggle("active", b.dataset.channel === editor.channel);
    if (dragging < 0) points = editor.opened ? readCurve(layer(), editor.channel) : IDENTITY;
    draw();
  };
}

// --- Crop -----------------------------------------------------------------------------

const ASPECTS: [string, number | "original" | null][] = [
  ["Original", "original"],
  ["Free", null],
  ["1:1", 1],
  ["4:5", 5 / 4],
  ["3:2", 3 / 2],
  ["16:9", 16 / 9],
];

function buildCrop(root: HTMLElement) {
  const size = () => [editor.opened!.preview_width, editor.opened!.preview_height] as const;
  let chosen = "Free";
  const grid = h("div", { className: "aspect-grid" });
  const buttons = ASPECTS.map(([label, ratio]) => {
    const b = h("button", { type: "button", textContent: label });
    b.addEventListener("click", () => {
      if (!editor.opened) return;
      chosen = label;
      const [W, H] = size();
      editor.aspect = ratio === null ? null : aspectFor(ratio, W, H);
      if (editor.aspect) setBox(boxForAspect(editor.aspect, currentBox().angle, W, H));
      commit();
    });
    grid.append(b);
    return [label, b] as const;
  });
  const aspect = group(root, "Aspect ratio");
  aspect.append(grid);

  const straighten = group(root, "Straighten");
  // Shrink to fit from the box as it was when the drag started, so turning
  // the angle back regrows it.
  let base: ReturnType<typeof currentBox> | null = null;
  const syncAngle = slider(straighten, {
    label: "Angle",
    min: -45,
    max: 45,
    step: 0.1,
    neutral: 0,
    format: (v) => `${signed(1)(v)}°`,
    get: () => (editor.opened ? (readCrop(editor.doc)?.angle_deg ?? 0) : 0),
    set: (v) => {
      const [W, H] = size();
      base ??= currentBox();
      setBox(fitInside({ ...base, angle: v }, W, H));
    },
  });
  straighten.addEventListener("change", () => (base = null));

  const actions = h("div", { className: "row" });
  const reset = h("button", { type: "button", textContent: "Reset crop" });
  reset.addEventListener("click", () => {
    writeCrop(editor.doc, undefined);
    commit();
  });
  const done = h("button", { type: "button", className: "primary", textContent: "Done" });
  done.addEventListener("click", () => showTab("edit"));
  actions.append(reset, done);
  root.append(actions, h("p", { className: "hint muted" }, "Drag the corners or the middle of the frame on the photo. The crop is stored as parameters; the original is never changed."));

  return () => {
    syncAngle();
    for (const [label, b] of buttons) b.classList.toggle("active", label === chosen);
    reset.disabled = done.disabled = !editor.opened;
  };
}

// --- Layers ----------------------------------------------------------------------

type NewKind = "brush" | "linear" | "radial";

const KIND_NAMES: Record<NewKind, string> = { brush: "Brush", linear: "Gradient", radial: "Radial" };

export function addLayer(kind: NewKind) {
  const o = editor.opened;
  if (!o) return;
  const r = 0.25 * Math.min(o.preview_width, o.preview_height);
  const mask: Mask =
    kind === "brush"
      ? { kind: "brush", strokes: [] }
      : kind === "linear"
        ? { kind: "linear", start: [0.5, 0.15], end: [0.5, 0.5] }
        : { kind: "radial", center: [0.5, 0.5], radius: [r / o.preview_width, r / o.preview_height], angle_deg: 0, feather: 0.5, invert: false };
  const n = editor.doc.layers.filter((l) => l.mask?.kind === kind).length + 1;
  const layer: Layer = { name: `${KIND_NAMES[kind]} ${n}`, enabled: true, opacity: 1, mask, operations: [] };
  editor.doc.layers.push(layer);
  editor.selected = editor.doc.layers.length - 1;
  editor.tab = "layers";
  commit();
}

function buildLayers(root: HTMLElement) {
  const head = h("div", { className: "group-head" }, h("h3", {}, "Layer stack"));
  const adds = h("div", { className: "row" });
  for (const kind of ["brush", "linear", "radial"] as NewKind[]) {
    const b = h("button", { type: "button", textContent: `+ ${KIND_NAMES[kind]}` });
    b.addEventListener("click", () => addLayer(kind));
    adds.append(b);
  }
  const list = h("ul", { className: "layers" });
  const props = h("div", { className: "layer-props" });
  root.append(head, adds, list, props);

  const drawList = () => {
    list.replaceChildren();
    if (!editor.opened) return;
    // Top of the stack first, like most editors.
    [...editor.doc.layers.keys()].reverse().forEach((i) => {
      const l = editor.doc.layers[i];
      const eye = h("button", { type: "button", className: "icon eye", title: l.enabled ? "Hide layer" : "Show layer", textContent: l.enabled ? "●" : "○" });
      eye.addEventListener("click", (e) => {
        e.stopPropagation();
        l.enabled = !l.enabled;
        commit();
      });
      const kind = h("span", { className: `kind ${l.mask?.kind ?? "base"}` });
      const text = h("div", { className: "text" }, h("strong", {}, l.name), h("small", { className: "muted" }, i === 0 ? "Whole image" : maskLabel(l.mask)));
      const row = h("li", { className: i === editor.selected ? "selected" : "" }, eye, kind, text);
      if (i > 0) {
        const del = h("button", { type: "button", className: "icon", title: "Delete layer", textContent: "×" });
        del.addEventListener("click", (e) => {
          e.stopPropagation();
          editor.doc.layers.splice(i, 1);
          editor.selected = Math.min(editor.selected, editor.doc.layers.length - 1);
          commit();
        });
        row.append(del);
      }
      row.addEventListener("click", () => {
        editor.selected = i;
        changed();
      });
      list.append(row);
    });
  };

  // Mask controls depend on the selected layer's kind, so they're rebuilt
  // only when that changes (not while a slider in them is moving).
  let propsKey = "";
  let syncProps: Array<() => void> = [];
  const buildProps = () => {
    props.replaceChildren();
    syncProps = [];
    if (!editor.opened) return;
    const l = layer();
    const body = group(props, l.name);
    syncProps.push(slider(body, { label: "Opacity", min: 0, max: 100, step: 1, neutral: 100, format: (v) => `${v}%`, get: () => Math.round(layer().opacity * 100), set: (v) => (layer().opacity = v / 100) }));
    const mask = l.mask;
    if (mask?.kind === "radial") {
      const radial = () => layer().mask as Extract<Mask, { kind: "radial" }>;
      syncProps.push(slider(body, { label: "Feather", min: 0, max: 100, step: 1, neutral: 50, format: (v) => `${v}%`, get: () => Math.round(radial().feather * 100), set: (v) => (radial().feather = v / 100) }));
      const invert = h("input", { type: "checkbox" });
      invert.addEventListener("change", () => {
        radial().invert = invert.checked;
        commit();
      });
      body.append(h("label", { className: "check" }, invert, "Invert (affect the outside)"));
      syncProps.push(() => (invert.checked = radial().invert));
      body.append(h("p", { className: "hint muted" }, "Drag the center to move, and the outer handles to resize and rotate."));
    } else if (mask?.kind === "linear") {
      body.append(h("p", { className: "hint muted" }, "Drag the solid handle (full effect) and the hollow one (no effect) on the photo; the small middle handle moves both."));
    } else if (mask?.kind === "brush") {
      const b = editor.brush;
      syncProps.push(slider(body, { label: "Brush size", min: 1, max: 50, step: 0.5, neutral: 8, format: (v) => `${v}%`, get: () => b.size * 100, set: (v) => (b.size = v / 100) }));
      syncProps.push(slider(body, { label: "Brush feather", min: 0, max: 100, step: 1, neutral: 50, format: (v) => `${v}%`, get: () => Math.round(b.feather * 100), set: (v) => (b.feather = v / 100) }));
      syncProps.push(slider(body, { label: "Flow", min: 5, max: 100, step: 1, neutral: 100, format: (v) => `${v}%`, get: () => Math.round(b.flow * 100), set: (v) => (b.flow = v / 100) }));
      const mode = h("div", { className: "segmented" });
      const paint = h("button", { type: "button", textContent: "Paint" });
      const erase = h("button", { type: "button", textContent: "Erase" });
      paint.addEventListener("click", () => ((b.erase = false), refresh()));
      erase.addEventListener("click", () => ((b.erase = true), refresh()));
      mode.append(paint, erase);
      const clear = h("button", { type: "button", className: "link", textContent: "Clear strokes" });
      clear.addEventListener("click", () => {
        (layer().mask as Extract<Mask, { kind: "brush" }>).strokes = [];
        commit();
      });
      body.append(mode, clear, h("p", { className: "hint muted" }, "Paint on the photo. Brush settings apply to new strokes."));
      syncProps.push(() => {
        paint.classList.toggle("active", !b.erase);
        erase.classList.toggle("active", b.erase);
      });
    }
    syncProps.push(pluginLayerControls(body, layer));
    if (mask && mask.kind !== "plugin") {
      const show = h("input", { type: "checkbox" });
      show.addEventListener("change", () => {
        editor.showMask = show.checked;
        changed();
      });
      body.append(h("label", { className: "check" }, show, "Show mask overlay"));
      syncProps.push(() => (show.checked = editor.showMask));
    }
    if (editor.selected > 0) body.append(h("p", { className: "hint muted" }, "The Edit tab changes this layer while it's selected."));
  };

  return () => {
    drawList();
    const pluginOps = editor.opened ? layer().operations.filter((o) => o.op === "plugin").length : 0;
    const key = editor.opened ? `${editor.selected}:${layer().mask?.kind ?? "none"}:${layer().name}:${editor.doc.layers.length}:${pluginOps}` : "";
    if (key !== propsKey) {
      propsKey = key;
      buildProps();
    }
    syncProps.forEach((s) => s());
  };
}

// --- Tabs -----------------------------------------------------------------------------

// --- Info -----------------------------------------------------------------------------

export interface Details {
  width: number | null;
  height: number | null;
  camera: string | null;
  lens: string | null;
  exposure: string | null;
  captured: string | null;
}

/** Camera details of the open photo, fetched once per photo. */
let detailsFor = "";
let details: Promise<Details | null> = Promise.resolve(null);
export function photoDetails(): Promise<Details | null> {
  const path = editor.path;
  if (!path) return Promise.resolve(null);
  if (path !== detailsFor) {
    detailsFor = path;
    details = invoke<Details>("photo_details", { path }).catch(() => null);
  }
  return details;
}

function buildInfo(root: HTMLElement) {
  const list = h("dl", { className: "facts info" });
  const edits = h("dl", { className: "facts info" });
  const head = (title: string) => h("div", { className: "group-head" }, h("h3", {}, title));
  root.append(head("Photo"), list, head("Edits"), edits);
  const fill = (dl: HTMLElement, rows: [string, string | null | undefined][]) =>
    dl.replaceChildren(...rows.filter(([, v]) => v).flatMap(([k, v]) => [h("dt", {}, k), h("dd", {}, v!)]));
  let shown = "";
  return () => {
    const o = editor.opened;
    if (!o) return fill(list, []);
    const layers = editor.doc.layers;
    const masks = layers.slice(1);
    fill(edits, [
      ["Layers", String(layers.length)],
      ["Masks", masks.length ? masks.map((l) => maskLabel(l.mask)).join(", ") : "None"],
      ["Sidecar", `${o.file_name}.aperio.json`],
    ]);
    if (shown === editor.path) return;
    shown = editor.path ?? "";
    fill(list, [
      ["File", o.file_name],
      ["Size", `${o.width} × ${o.height}`],
      ["Folder", o.folder],
    ]);
    void photoDetails().then((d) => {
      if (!d || shown !== editor.path) return;
      fill(list, [
        ["File", o.file_name],
        ["Size", `${o.width} × ${o.height}`],
        ["Camera", d.camera],
        ["Lens", d.lens],
        ["Exposure", d.exposure],
        ["Captured", d.captured],
        ["Folder", o.folder],
      ]);
    });
  };
}

// --- Tabs -----------------------------------------------------------------------------

const ICONS: Record<string, string> = {
  edit: '<path d="M4 6h10M18 6h2M4 12h4M12 12h8M4 18h12"/><circle cx="16" cy="6" r="2"/><circle cx="10" cy="12" r="2"/><circle cx="18" cy="18" r="2"/>',
  layers: '<path d="M12 3l9 5-9 5-9-5z"/><path d="M3 13l9 5 9-5"/>',
  ai: '<path d="M12 3l2 5 5 2-5 2-2 5-2-5-5-2 5-2z"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8v.01"/>',
};

/** Tabs in the bar; Crop is opened from the tool rail instead. */
const TABS: [Tab, string | null, (root: HTMLElement) => () => void][] = [
  ["edit", "Edit", buildEdit],
  ["layers", "Layers", buildLayers],
  ["ai", "AI", buildAi],
  ["info", "Info", buildInfo],
  ["crop", null, buildCrop],
];

export function showTab(tab: Tab) {
  editor.tab = tab;
  changed();
}

export function buildPanels(tabBar: HTMLElement, body: HTMLElement) {
  const pages = TABS.map(([tab, label, build]) => {
    let button: HTMLButtonElement | null = null;
    if (label) {
      button = h("button", { type: "button", title: label });
      button.innerHTML = `<svg viewBox="0 0 24 24">${ICONS[tab]}</svg><span>${label}</span>`;
      button.addEventListener("click", () => showTab(tab));
      tabBar.append(button);
    }
    const page = h("section", { className: `page ${tab}` });
    if (tab === "crop") page.prepend(h("div", { className: "page-title" }, "Crop & straighten"));
    body.append(page);
    return { tab, button, page, sync: build(page) };
  });
  // Layer edits apply to the selected layer; say which, above every tab.
  const editing = editingNote();
  body.prepend(editing.note);
  onUpdate(() => {
    editing.sync();
    for (const p of pages) {
      const active = p.tab === editor.tab;
      p.button?.classList.toggle("active", active);
      if (p.button) p.button.disabled = !editor.opened;
      p.page.hidden = !active;
      if (active) p.sync();
    }
  });
}
