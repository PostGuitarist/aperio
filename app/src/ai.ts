// The AI tab (model downloads, AI selections, sky replacement, plugin
// effects) and the controls for plugin parameters, which are built from each
// plugin's own parameter list.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import type { Layer, Mask, Operation } from "./doc";
import { group, h, slider } from "./panels";
import { changed, commit, editor } from "./state";

// --- Plugin metadata (mirrors aperio-plugin's PluginInfo) -------------------------

export type ParamSpec = { key: string; label: string } & (
  | { kind: "number"; min: number; max: number; step: number; default: number }
  | { kind: "bool"; default: boolean }
  | { kind: "choice"; options: string[]; default: string }
  | { kind: "path" }
);

export interface PluginInfo {
  id: string;
  name: string;
  version: string;
  license: string;
  operation: { label: string; params: ParamSpec[] } | null;
  mask: { label: string; params: ParamSpec[] } | null;
  source: { label: string; params: ParamSpec[] } | null;
  models: { name: string; license: string; source: string }[];
}

export const SEGMENT = "org.aperio.segment";
export const SKY = "org.aperio.sky";
export const RELIGHT = "org.aperio.sky.relight";
export const DENOISE = "org.aperio.denoise";
const DENOISE_MODEL = "nafnet_sidd_width32";

export const plugins = new Map<string, PluginInfo>();

export async function loadPlugins() {
  for (const p of await invoke<PluginInfo[]>("list_plugins")) plugins.set(p.id, p);
}

type Params = Record<string, unknown>;

/** Controls for `specs`, reading and writing `params()` (looked up on every
 * change, since undo replaces the document). Returns a sync function. */
export function paramControls(parent: HTMLElement, specs: ParamSpec[], params: () => Params): () => void {
  const syncs: Array<() => void> = [];
  for (const spec of specs) {
    if (spec.kind === "number") {
      syncs.push(
        slider(parent, {
          label: spec.label,
          min: spec.min,
          max: spec.max,
          step: spec.step,
          neutral: spec.default,
          format: (v) => (spec.step >= 1 ? String(v) : v.toFixed(2)),
          get: () => Number(params()[spec.key] ?? spec.default),
          set: (v) => (params()[spec.key] = v),
        }),
      );
    } else if (spec.kind === "bool") {
      const box = h("input", { type: "checkbox" });
      box.addEventListener("change", () => {
        params()[spec.key] = box.checked;
        commit();
      });
      parent.append(h("label", { className: "check" }, box, spec.label));
      syncs.push(() => (box.checked = Boolean(params()[spec.key] ?? spec.default)));
    } else if (spec.kind === "choice") {
      const seg = h("div", { className: "segmented" });
      seg.dataset.param = spec.key;
      const buttons = spec.options.map((option) => {
        const b = h("button", { type: "button", textContent: option[0].toUpperCase() + option.slice(1) });
        b.addEventListener("click", () => {
          params()[spec.key] = option;
          commit();
        });
        seg.append(b);
        return [option, b] as const;
      });
      parent.append(seg);
      syncs.push(() => {
        const value = String(params()[spec.key] ?? spec.default);
        for (const [option, b] of buttons) b.classList.toggle("active", option === value);
      });
    } else {
      const name = h("span", { className: "muted file-name" });
      const choose = h("button", { type: "button", textContent: "Choose…" });
      choose.addEventListener("click", async () => {
        const path = await pickImage(spec.label);
        if (path) {
          const before = params()[spec.key];
          params()[spec.key] = path;
          if (typeof before === "string") followSky(before, path);
          commit();
        }
      });
      parent.append(h("div", { className: "path-row" }, h("span", {}, spec.label), choose), name);
      syncs.push(() => {
        const value = params()[spec.key];
        name.textContent = typeof value === "string" && value ? value.split(/[\\/]/).pop()! : "None chosen";
      });
    }
  }
  return () => syncs.forEach((s) => s());
}

async function pickImage(title: string) {
  const path = await open({ title, multiple: false, filters: [{ name: "Images", extensions: ["jpg", "jpeg", "png", "tif", "tiff"] }] });
  return typeof path === "string" ? path : null;
}

type PluginOp = Extract<Operation, { op: "plugin" }>;
type PluginMask = Extract<Mask, { kind: "plugin" }>;

/** Controls for a layer's plugin mask and plugin operations, for the Layers
 * tab. Returns a sync function. */
export function pluginLayerControls(parent: HTMLElement, layer: () => Layer): () => void {
  const syncs: Array<() => void> = [];
  const mask = layer().mask;
  if (mask?.kind === "plugin") {
    const info = plugins.get(mask.plugin);
    if (info?.mask) {
      const params = () => ((layer().mask as PluginMask).params ??= {}) as Params;
      syncs.push(paramControls(parent, info.mask.params, params));
      if (mask.plugin === SEGMENT) {
        // Best/Fast only applies to subjects.
        const detail = parent.querySelector<HTMLElement>('[data-param="detail"]');
        if (detail) syncs.push(() => (detail.hidden = params().class !== "subject"));
      }
      const show = h("input", { type: "checkbox" });
      show.addEventListener("change", () => {
        editor.showMask = show.checked;
        changed();
      });
      parent.append(h("label", { className: "check" }, show, "Show mask overlay"));
      syncs.push(() => (show.checked = editor.showMask));
    } else {
      parent.append(h("p", { className: "hint muted" }, `This mask comes from ${mask.plugin}, which isn't installed. It's kept in the sidecar but not rendered.`));
    }
  }
  layer().operations.forEach((op, index) => {
    if (op.op !== "plugin") return;
    const info = plugins.get(op.plugin);
    const body = group(parent, info?.operation?.label ?? op.plugin, () => {
      layer().operations.splice(index, 1);
      commit();
    });
    // The group's link removes the effect rather than resetting it.
    body.previousElementSibling?.querySelector("button")?.replaceChildren("Remove");
    if (!info?.operation) {
      body.append(h("p", { className: "hint muted" }, `${op.plugin} isn't installed, so this step is kept but not rendered.`));
      return;
    }
    syncs.push(paramControls(body, info.operation.params, () => ((layer().operations[index] as PluginOp).params ??= {}) as Params));
  });
  return () => syncs.forEach((s) => s());
}

// --- Layers the AI tab adds -------------------------------------------------------------

function segmentMask(params: Params): Mask {
  const version = plugins.get(SEGMENT)?.version ?? "0.1.0";
  return { kind: "plugin", plugin: SEGMENT, version, params };
}

function pushLayer(layer: Layer) {
  editor.doc.layers.push(layer);
  editor.selected = editor.doc.layers.length - 1;
  editor.showMask = true;
  editor.tab = "layers";
  commit();
}

export function addAiLayer(what: "sky" | "subject" | "background") {
  const params: Params = what === "background" ? { class: "subject", invert: true } : { class: what };
  // New subject layers use the fast model when it's downloaded; the layer's
  // Best/Fast switch changes it.
  if (params.class === "subject" && statuses.some((m) => m.id === FAST_SUBJECT && m.installed)) params.detail = "fast";
  const name = { sky: "Sky", subject: "Subject", background: "Background" }[what];
  pushLayer({ name, enabled: true, opacity: 1, mask: segmentMask(params), operations: [] });
}

export async function replaceSky() {
  const path = await pickImage("Choose a sky photo");
  if (!path) return;
  const version = plugins.get(SKY)?.version ?? "0.1.0";
  // The rest of the scene is relit to match on a layer of its own, masked
  // to everything but the sky, since the sky layer only shows in the sky.
  const relight = plugins.get(RELIGHT);
  if (relight) {
    const params: Params = { image: path };
    for (const p of relight.operation?.params ?? []) if (p.kind === "number") params[p.key] = p.default;
    editor.doc.layers.push({
      name: "Match scene to sky",
      enabled: true,
      opacity: 1,
      mask: segmentMask({ class: "sky", invert: true }),
      operations: [{ op: "plugin", plugin: RELIGHT, version: relight.version, params }],
    });
  }
  pushLayer({
    name: "New sky",
    enabled: true,
    opacity: 1,
    mask: segmentMask({ class: "sky" }),
    operations: [{ op: "plugin", plugin: SKY, version, params: { image: path } }],
  });
  // The new sky shows better without the red overlay.
  editor.showMask = false;
  changed();
}

/** When a sky layer's photo changes from `before` to `after`, relighting
 * steps that were matching `before` follow it. */
function followSky(before: string, after: string) {
  for (const layer of editor.doc.layers) {
    for (const op of layer.operations) {
      if (op.op === "plugin" && op.plugin === RELIGHT && (op.params as Params | undefined)?.image === before) {
        (op.params as Params).image = after;
      }
    }
  }
}

/** Add a plugin effect (e.g. the example vignette) to the selected layer. */
export function addEffect(info: PluginInfo) {
  const params: Params = {};
  for (const p of info.operation?.params ?? []) if (p.kind !== "path") params[p.key] = p.default;
  editor.doc.layers[editor.selected].operations.push({ op: "plugin", plugin: info.id, version: info.version, params });
  editor.tab = "layers";
  commit();
}

// --- Denoise (in the Adjust tab) -----------------------------------------------------

/** The Noise group: AI denoise on the base layer, which runs on the
 * full-size photo in the background. Returns a sync function. The group
 * appears once the plugin list has loaded, if the denoise plugin is
 * installed. */
export function denoiseControls(root: HTMLElement): () => void {
  const holder = h("div", {});
  root.append(holder);
  let sync: (() => void) | undefined;
  return () => {
    const info = plugins.get(DENOISE);
    if (!sync && info?.source) sync = buildDenoise(holder, info, info.source.params);
    sync?.();
  };
}

function buildDenoise(root: HTMLElement, info: PluginInfo, specs: ParamSpec[]): () => void {
  const base = () => editor.doc.layers[0];
  const index = () => (editor.opened ? base().operations.findIndex((op) => op.op === "plugin" && op.plugin === DENOISE) : -1);
  const body = group(root, "Noise");
  const on = h("input", { type: "checkbox" });
  on.addEventListener("change", () => {
    const i = index();
    if (on.checked && i < 0) {
      const params: Params = {};
      for (const p of specs) if (p.kind === "number") params[p.key] = p.default;
      // First, so it runs on the photo before anything else in the layer.
      base().operations.unshift({ op: "plugin", plugin: DENOISE, version: info.version, params });
    } else if (!on.checked && i >= 0) {
      base().operations.splice(i, 1);
    }
    commit();
  });
  body.append(h("label", { className: "check" }, on, "AI denoise"));
  const sliders = h("div", {});
  body.append(sliders);
  const scratch: Params = {};
  const syncSliders = paramControls(sliders, specs, () => {
    const i = index();
    return i < 0 ? scratch : (((base().operations[i] as PluginOp).params ??= {}) as Params);
  });
  const progress = h("progress", { max: 1, value: 0, hidden: true });
  const status = h("p", { className: "hint muted" });
  body.append(progress, status);

  let running: number | null = null;
  let error: string | null = null;
  const showStatus = () => {
    const missing = statuses.some((m) => m.id === DENOISE_MODEL && !m.installed);
    progress.hidden = running === null;
    progress.value = running ?? 0;
    if (index() < 0) status.textContent = "Removes noise from high-ISO photos. It runs once on the full-size photo, which can take a few minutes on a laptop.";
    else if (missing) status.textContent = "Download the denoise model in the AI tab first.";
    else if (running !== null) status.textContent = `Denoising the full-size photo… ${Math.round(running * 100)}%. You can keep editing.`;
    else if (error) status.textContent = `Denoise didn't run: ${error}`;
    else status.textContent = "Moving the sliders reuses the denoised photo, so it's quick.";
  };
  void listen<number>("clean-progress", (e) => {
    running = e.payload;
    showStatus();
  });
  void listen<{ error: string | null }>("clean-done", (e) => {
    running = null;
    error = e.payload.error;
    showStatus();
    // The preview now starts from the new result.
    changed();
  });
  return () => {
    const i = index();
    on.disabled = !editor.opened;
    on.checked = i >= 0;
    sliders.hidden = i < 0;
    syncSliders();
    showStatus();
  };
}

// --- The AI tab -----------------------------------------------------------------------

interface ModelStatus {
  id: string;
  name: string;
  license: string;
  source: string;
  size_bytes: number;
  installed: boolean;
  optional: boolean;
  notice: string | null;
}

const FAST_SUBJECT = "isnet_general_use";
let statuses: ModelStatus[] = [];

export function buildAi(root: HTMLElement) {
  const models = group(root, "AI models");
  const list = h("ul", { className: "models" });
  const download = h("button", { type: "button", className: "primary" });
  const progress = h("progress", { max: 1, value: 0, hidden: true });
  const note = h("p", { className: "hint muted" }, "Models run on this computer. They're downloaded once, only when you ask, and checked before use.");
  const extras = h("div", {});
  models.append(list, download, progress, note, extras);

  let downloading = false;
  const mb = (b: number) => `${Math.round(b / 1e6)} MB`;
  const item = (m: ModelStatus) => h("li", {}, h("strong", {}, m.name), h("small", { className: "muted" }, `${m.license} · ${mb(m.size_bytes)} · ${m.installed ? "Downloaded" : "Not downloaded"}`));
  const drawModels = () => {
    list.replaceChildren(...statuses.filter((m) => !m.optional).map(item));
    const missing = statuses.filter((m) => !m.optional && !m.installed);
    download.hidden = missing.length === 0;
    download.disabled = downloading;
    download.textContent = downloading ? "Downloading…" : `Download models (${mb(missing.reduce((s, m) => s + m.size_bytes, 0))})`;
    // Optional models, each with what to know before choosing it.
    extras.replaceChildren(
      ...statuses
        .filter((m) => m.optional)
        .map((m) => {
          const get = h("button", { type: "button", textContent: `Download (${mb(m.size_bytes)})`, hidden: m.installed, disabled: downloading });
          get.addEventListener("click", () => void fetchModels([m]));
          return h("div", { className: "optional-model" }, h("ul", { className: "models" }, item(m)), m.notice ? h("p", { className: "hint muted" }, m.notice) : "", get);
        }),
    );
  };
  const loadStatus = async () => {
    statuses = await invoke<ModelStatus[]>("models_status");
    drawModels();
  };
  // Download `wanted` (the default models when it's all of them).
  const fetchModels = async (wanted: ModelStatus[], defaults = false) => {
    downloading = true;
    progress.hidden = false;
    drawModels();
    const channel = new Channel<{ model: string; done: number; total: number }>();
    const missing = wanted.filter((m) => !m.installed);
    const total = missing.reduce((s, m) => s + m.size_bytes, 0);
    channel.onmessage = (p) => {
      const before = missing.slice(0, missing.findIndex((m) => m.id === p.model)).reduce((s, m) => s + m.size_bytes, 0);
      progress.value = (before + p.done) / total;
    };
    try {
      await invoke("download_models", { ids: defaults ? [] : missing.map((m) => m.id), progress: channel });
    } catch (e) {
      alert(`Download failed: ${e}`);
    } finally {
      downloading = false;
      progress.hidden = true;
      await loadStatus();
      changed();
    }
  };
  download.addEventListener("click", () => void fetchModels(statuses.filter((m) => !m.optional), true));
  void loadStatus();

  const select = group(root, "Select with AI");
  const row = h("div", { className: "row" });
  for (const [what, label] of [
    ["sky", "+ Sky"],
    ["subject", "+ Subject"],
    ["background", "+ Background"],
  ] as const) {
    const b = h("button", { type: "button", textContent: label });
    b.addEventListener("click", () => addAiLayer(what));
    row.append(b);
  }
  select.append(row, h("p", { className: "hint muted" }, "Each adds a layer masked to that part of the photo; adjust it in the Adjust and Curves tabs. Aperio starts on these in the background when a photo opens, so they're usually ready by the time you add them."));

  const sky = group(root, "Sky replacement");
  const replace = h("button", { type: "button", textContent: "Replace sky…" });
  replace.addEventListener("click", () => void replaceSky());
  sky.append(replace, h("p", { className: "hint muted" }, "Puts a sky photo of your choice behind the AI sky mask. Position and exposure are in the layer's settings."));

  const effects = group(root, "Plugin effects");
  const effectRow = h("div", { className: "row" });
  effects.append(effectRow, h("p", { className: "hint muted" }, "Added to the selected layer."));

  return () => {
    const ops = [...plugins.values()].filter((p) => p.operation && p.id !== SKY && p.id !== RELIGHT);
    if (effectRow.childElementCount !== ops.length) {
      effectRow.replaceChildren(
        ...ops.map((p) => {
          const b = h("button", { type: "button", textContent: `+ ${p.operation!.label}` });
          b.addEventListener("click", () => addEffect(p));
          return b;
        }),
      );
    }
    for (const b of root.querySelectorAll<HTMLButtonElement>(".row button, button:not(.primary)")) b.disabled = !editor.opened;
    drawModels();
  };
}
