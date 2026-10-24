// The AI tab (model downloads, AI selections, sky replacement, plugin
// effects) and the controls for plugin parameters, which are built from each
// plugin's own parameter list.

import { Channel, invoke } from "@tauri-apps/api/core";
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
  models: { name: string; license: string; source: string }[];
}

export const SEGMENT = "org.aperio.segment";
export const SKY = "org.aperio.sky";

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
          params()[spec.key] = path;
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
      syncs.push(paramControls(parent, info.mask.params, () => ((layer().mask as PluginMask).params ??= {}) as Params));
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
  const params = what === "background" ? { class: "subject", invert: true } : { class: what };
  const name = { sky: "Sky", subject: "Subject", background: "Background" }[what];
  pushLayer({ name, enabled: true, opacity: 1, mask: segmentMask(params), operations: [] });
}

export async function replaceSky() {
  const path = await pickImage("Choose a sky photo");
  if (!path) return;
  const version = plugins.get(SKY)?.version ?? "0.1.0";
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

/** Add a plugin effect (e.g. the example vignette) to the selected layer. */
export function addEffect(info: PluginInfo) {
  const params: Params = {};
  for (const p of info.operation?.params ?? []) if (p.kind !== "path") params[p.key] = p.default;
  editor.doc.layers[editor.selected].operations.push({ op: "plugin", plugin: info.id, version: info.version, params });
  editor.tab = "layers";
  commit();
}

// --- The AI tab -----------------------------------------------------------------------

interface ModelStatus {
  id: string;
  name: string;
  license: string;
  source: string;
  size_bytes: number;
  installed: boolean;
}

export function buildAi(root: HTMLElement) {
  const models = group(root, "AI models");
  const list = h("ul", { className: "models" });
  const download = h("button", { type: "button", className: "primary" });
  const progress = h("progress", { max: 1, value: 0, hidden: true });
  const note = h("p", { className: "hint muted" }, "Models run on this computer. They're downloaded once, only when you ask, and checked before use.");
  models.append(list, download, progress, note);

  let statuses: ModelStatus[] = [];
  let downloading = false;
  const mb = (b: number) => `${Math.round(b / 1e6)} MB`;
  const drawModels = () => {
    list.replaceChildren(
      ...statuses.map((m) =>
        h("li", {}, h("strong", {}, m.name), h("small", { className: "muted" }, `${m.license} · ${mb(m.size_bytes)} · ${m.installed ? "Downloaded" : "Not downloaded"}`)),
      ),
    );
    const missing = statuses.filter((m) => !m.installed);
    download.hidden = missing.length === 0;
    download.disabled = downloading;
    download.textContent = downloading ? "Downloading…" : `Download models (${mb(missing.reduce((s, m) => s + m.size_bytes, 0))})`;
  };
  const loadStatus = async () => {
    statuses = await invoke<ModelStatus[]>("models_status");
    drawModels();
  };
  download.addEventListener("click", async () => {
    downloading = true;
    progress.hidden = false;
    drawModels();
    const channel = new Channel<{ model: string; done: number; total: number }>();
    const missing = statuses.filter((m) => !m.installed);
    const total = missing.reduce((s, m) => s + m.size_bytes, 0);
    channel.onmessage = (p) => {
      const before = missing.slice(0, missing.findIndex((m) => m.id === p.model)).reduce((s, m) => s + m.size_bytes, 0);
      progress.value = (before + p.done) / total;
    };
    try {
      await invoke("download_models", { progress: channel });
    } catch (e) {
      alert(`Download failed: ${e}`);
    } finally {
      downloading = false;
      progress.hidden = true;
      await loadStatus();
      changed();
    }
  });
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
  select.append(row, h("p", { className: "hint muted" }, "Each adds a layer masked to that part of the photo; adjust it in the Adjust and Curves tabs. The first run on a photo takes a few seconds (subjects longer)."));

  const sky = group(root, "Sky replacement");
  const replace = h("button", { type: "button", textContent: "Replace sky…" });
  replace.addEventListener("click", () => void replaceSky());
  sky.append(replace, h("p", { className: "hint muted" }, "Puts a sky photo of your choice behind the AI sky mask. Position and exposure are in the layer's settings."));

  const effects = group(root, "Plugin effects");
  const effectRow = h("div", { className: "row" });
  effects.append(effectRow, h("p", { className: "hint muted" }, "Added to the selected layer."));

  return () => {
    const ops = [...plugins.values()].filter((p) => p.operation && p.id !== SKY);
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
