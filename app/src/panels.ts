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
import { buildAi, denoiseControls, pluginLayerControls } from "./ai";
import { aspectFor, boxForAspect, currentBox, fitInside, setBox } from "./overlay";
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

// --- Adjust ---------------------------------------------------------------------

function buildAdjust(root: HTMLElement) {
  const syncs: Array<() => void> = [];
  const editing = editingNote();
  root.append(editing.note);
  syncs.push(editing.sync);

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

  const light = group(root, "Light", resetKeys(["exposure", "contrast", "highlights", "shadows", "whites", "blacks"]));
  add(light, "exposure", "Exposure", -5, 5, 0.05, (v) => `${signed(2)(v)} EV`);
  add(light, "contrast", "Contrast", -1, 1, 0.01, signed(2));
  add(light, "highlights", "Highlights", -1, 1, 0.01, signed(2));
  add(light, "shadows", "Shadows", -1, 1, 0.01, signed(2));
  add(light, "whites", "Whites", -1, 1, 0.01, signed(2));
  add(light, "blacks", "Blacks", -1, 1, 0.01, signed(2));

  const color = group(root, "Color", resetKeys(["temperature", "tint", "vibrance", "saturation"]));
  add(color, "temperature", "Temperature", 2000, 12000, 50, (v) => `${v} K`, "temp");
  add(color, "tint", "Tint", -1, 1, 0.01, signed(2), "tint");
  add(color, "vibrance", "Vibrance", -1, 1, 0.01, signed(2));
  add(color, "saturation", "Saturation", -1, 1, 0.01, signed(2));

  syncs.push(denoiseControls(root));

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
  const editing = editingNote();
  root.append(editing.note);

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
  const reset = h("button", { type: "button", className: "link", textContent: "Reset curve" });
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
    editing.sync();
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
  done.addEventListener("click", () => showTab("adjust"));
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
    if (editor.selected > 0) body.append(h("p", { className: "hint muted" }, "The Adjust and Curves tabs change this layer while it's selected."));
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

const TABS: [Tab, string, (root: HTMLElement) => () => void][] = [
  ["adjust", "Adjust", buildAdjust],
  ["curves", "Curves", buildCurves],
  ["crop", "Crop", buildCrop],
  ["layers", "Layers", buildLayers],
  ["ai", "AI", buildAi],
];

export function showTab(tab: Tab) {
  editor.tab = tab;
  changed();
}

export function buildPanels(tabBar: HTMLElement, body: HTMLElement) {
  const pages = TABS.map(([tab, label, build]) => {
    const button = h("button", { type: "button", textContent: label });
    button.addEventListener("click", () => showTab(tab));
    tabBar.append(button);
    const page = h("section", { className: "page" });
    body.append(page);
    return { tab, button, page, sync: build(page) };
  });
  onUpdate(() => {
    for (const p of pages) {
      const active = p.tab === editor.tab;
      p.button.classList.toggle("active", active);
      p.button.disabled = !editor.opened;
      p.page.hidden = !active;
      if (active) p.sync();
    }
  });
}
