// The Presets and Snapshots lists in the left panel. A preset is a set of
// adjustments applied to the selected layer: it sets what it includes and
// leaves everything else alone. Hovering a preset previews it. A snapshot is
// a named copy of the whole edit, kept in the sidecar.

import { invoke } from "@tauri-apps/api/core";
import { type Layer, type Operation, type Point, type Snapshot, clone } from "./doc";
import { h } from "./panels";
import { changed, commit, editor, onUpdate } from "./state";

export interface Preset {
  name: string;
  operations: Operation[];
}

/** Adjustments a preset can carry, in the order they're kept in a layer. */
const ORDER = ["white_balance", "exposure", "contrast", "tone", "color", "curve:rgb", "curve:red", "curve:green", "curve:blue", "curve:luma", "presence", "hsl", "color_grade"];

const keyOf = (op: Operation) => (op.op === "curve" ? `curve:${op.channel}` : op.op);

/** Set `ops` on `layer`, keeping its other operations. */
export function applyOperations(layer: Layer, ops: Operation[]) {
  const set = new Map<string, Operation>();
  for (const op of layer.operations) if (ORDER.includes(keyOf(op))) set.set(keyOf(op), op);
  for (const op of ops) if (ORDER.includes(keyOf(op))) set.set(keyOf(op), clone(op));
  const rest = layer.operations.filter((op) => !ORDER.includes(keyOf(op)));
  layer.operations = [...ORDER.filter((k) => set.has(k)).map((k) => set.get(k)!), ...rest];
}

/** The adjustments of `layer` a preset would save. */
function presetOperations(layer: Layer): Operation[] {
  return layer.operations.filter((op) => ORDER.includes(keyOf(op))).map(clone);
}

const wheel = (hue = 0, saturation = 0, luminance = 0) => ({ hue, saturation, luminance });
const grade = (g: { shadows?: ReturnType<typeof wheel>; midtones?: ReturnType<typeof wheel>; highlights?: ReturnType<typeof wheel>; global?: ReturnType<typeof wheel>; balance?: number }): Operation => ({
  op: "color_grade",
  shadows: g.shadows ?? wheel(),
  midtones: g.midtones ?? wheel(),
  highlights: g.highlights ?? wheel(),
  global: g.global ?? wheel(),
  blending: 0.5,
  balance: g.balance ?? 0,
});
const matte: Point[] = [
  [0, 0.07],
  [0.25, 0.26],
  [0.75, 0.77],
  [1, 0.95],
];
const mono = (contrast: number): Operation[] => [
  { op: "color", saturation: -1, vibrance: 0 },
  { op: "contrast", amount: contrast },
];

/** Presets that come with Aperio, by group. */
export const BUILT_IN: Array<[string, Preset[]]> = [
  [
    "Essentials",
    [
      {
        name: "Clean pop",
        operations: [
          { op: "contrast", amount: 0.15 },
          { op: "color", saturation: 0, vibrance: 0.25 },
          { op: "presence", texture: 0.1, clarity: 0.15, dehaze: 0 },
        ],
      },
      {
        name: "Bright and airy",
        operations: [
          { op: "exposure", ev: 0.3 },
          { op: "contrast", amount: -0.1 },
          { op: "tone", highlights: -0.3, shadows: 0.35, whites: 0.1, blacks: 0 },
          { op: "color", saturation: -0.05, vibrance: 0.1 },
        ],
      },
      {
        name: "Crisp landscape",
        operations: [
          { op: "tone", highlights: -0.4, shadows: 0.3, whites: 0, blacks: -0.05 },
          { op: "color", saturation: 0, vibrance: 0.3 },
          { op: "presence", texture: 0.2, clarity: 0.25, dehaze: 0.2 },
          { op: "hsl", hue: [0, 0, 0, 0, 0, 0, 0, 0], saturation: [0, 0, 0, 0.1, 0, 0.2, 0, 0], luminance: [0, 0, 0, 0, 0, -0.2, 0, 0] },
        ],
      },
    ],
  ],
  [
    "Mood",
    [
      {
        name: "Soft matte",
        operations: [
          { op: "contrast", amount: -0.1 },
          { op: "color", saturation: -0.15, vibrance: 0 },
          { op: "curve", channel: "rgb", points: matte },
        ],
      },
      {
        name: "Golden hour",
        operations: [
          { op: "white_balance", temperature_k: 8000, tint: 0.03 },
          { op: "color", saturation: 0, vibrance: 0.15 },
          grade({ shadows: wheel(25, 0.1), highlights: wheel(40, 0.25) }),
        ],
      },
      {
        name: "Teal and amber",
        operations: [
          { op: "contrast", amount: 0.1 },
          grade({ shadows: wheel(190, 0.3), highlights: wheel(35, 0.25), balance: 0.1 }),
        ],
      },
      {
        name: "Cold morning",
        operations: [
          { op: "exposure", ev: -0.2 },
          { op: "contrast", amount: 0.2 },
          { op: "white_balance", temperature_k: 5600, tint: 0 },
          { op: "tone", highlights: -0.3, shadows: 0, whites: 0, blacks: 0 },
          { op: "hsl", hue: [0, 0, 0, 0, 0, 0, 0, 0], saturation: [-0.2, -0.3, -0.3, -0.4, 0, 0, -0.2, -0.2], luminance: [0, 0, 0, 0, 0, 0, 0, 0] },
          grade({ shadows: wheel(215, 0.3, -0.1) }),
        ],
      },
    ],
  ],
  [
    "Black and white",
    [
      { name: "Hard mono", operations: [...mono(0.35), { op: "tone", highlights: -0.25, shadows: 0.1, whites: 0.05, blacks: -0.15 }, { op: "presence", texture: 0, clarity: 0.2, dehaze: 0 }] },
      { name: "Soft mono", operations: [...mono(-0.1), { op: "curve", channel: "rgb", points: matte }, grade({ global: wheel(35, 0.08) })] },
      { name: "Selenium", operations: [...mono(0.2), grade({ shadows: wheel(280, 0.12), highlights: wheel(40, 0.05) })] },
    ],
  ],
];

// --- Preview on hover ---------------------------------------------------------------

/** The selected layer as it was before a hover preview, to put back. */
let previewing: { index: number; json: string } | null = null;
let hoverTimer = 0;

function endPreview() {
  clearTimeout(hoverTimer);
  if (!previewing) return;
  editor.doc.layers[previewing.index] = JSON.parse(previewing.json) as Layer;
  previewing = null;
  changed();
}

function startPreview(p: Preset) {
  clearTimeout(hoverTimer);
  // A short wait, so sweeping the pointer down the list doesn't render
  // every preset on the way.
  hoverTimer = window.setTimeout(() => {
    if (!editor.opened) return;
    const index = editor.selected;
    if (!previewing) previewing = { index, json: JSON.stringify(editor.doc.layers[index]) };
    const layer = JSON.parse(previewing.json) as Layer;
    applyOperations(layer, p.operations);
    editor.doc.layers[previewing.index] = layer;
    changed();
  }, 120);
}

function apply(p: Preset) {
  endPreview();
  if (!editor.opened) return;
  applyOperations(editor.doc.layers[editor.selected], p.operations);
  commit(`Preset: ${p.name}`);
}

// --- Lists --------------------------------------------------------------------

export function buildPresets(root: HTMLElement) {
  const add = h("button", { type: "button", className: "icon", title: "Save the selected layer's adjustments as a preset", textContent: "+" });
  const form = h("form", { className: "inline-name", hidden: true });
  const name = h("input", { type: "text", placeholder: "Preset name", maxLength: 60 });
  form.append(name, h("button", { type: "submit", textContent: "Save" }));
  const list = h("div", { className: "preset-groups" });
  root.append(h("div", { className: "group-head" }, h("h3", {}, "Presets"), add), form, list);

  let mine: Preset[] = [];
  const row = (p: Preset, onDelete?: () => void) => {
    const li = h("li", { title: onDelete ? p.name : `${p.name}. Hover to preview, click to apply.` }, h("span", {}, p.name));
    li.addEventListener("mouseenter", () => startPreview(p));
    li.addEventListener("mouseleave", endPreview);
    li.addEventListener("click", () => apply(p));
    if (onDelete) {
      const del = h("button", { type: "button", className: "icon", title: "Delete preset", textContent: "×" });
      del.addEventListener("click", (e) => {
        e.stopPropagation();
        onDelete();
      });
      li.append(del);
    }
    return li;
  };
  const group = (title: string, items: HTMLElement[]) => {
    const d = h("details", { className: "preset-group" }, h("summary", {}, title), h("ul", { className: "preset-list" }, ...items));
    d.open = openGroups.has(title);
    d.addEventListener("toggle", () => {
      if (d.open) openGroups.add(title);
      else openGroups.delete(title);
      try {
        localStorage.setItem(GROUPS_KEY, JSON.stringify([...openGroups]));
      } catch {
        // Not remembered, then.
      }
    });
    return d;
  };
  const draw = () => {
    const groups = BUILT_IN.map(([title, presets]) => group(title, presets.map((p) => row(p))));
    if (mine.length)
      groups.unshift(
        group(
          "Your presets",
          mine.map((p) =>
            row(p, async () => {
              await invoke("delete_preset", { name: p.name });
              await load();
            }),
          ),
        ),
      );
    list.replaceChildren(...groups);
  };
  const load = async () => {
    try {
      mine = await invoke<Preset[]>("list_presets");
    } catch {
      mine = [];
    }
    draw();
  };

  add.addEventListener("click", () => {
    form.hidden = !form.hidden;
    if (!form.hidden) {
      name.value = "";
      name.focus();
    }
  });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const n = name.value.trim();
    if (!n || !editor.opened) return;
    const operations = presetOperations(editor.doc.layers[editor.selected]);
    await invoke("save_preset", { preset: { name: n, operations } });
    form.hidden = true;
    openGroups.add("Your presets");
    await load();
  });
  name.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") form.hidden = true;
  });

  onUpdate(() => {
    add.disabled = !editor.opened;
  });
  void load();
}

const GROUPS_KEY = "aperio.presetGroups";
const openGroups = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem(GROUPS_KEY) ?? '["Essentials"]') as string[];
    } catch {
      return ["Essentials"];
    }
  })(),
);

export function buildSnapshots(root: HTMLElement) {
  const add = h("button", { type: "button", className: "icon", title: "Take a snapshot of this edit", textContent: "+" });
  const form = h("form", { className: "inline-name", hidden: true });
  const name = h("input", { type: "text", maxLength: 60 });
  form.append(name, h("button", { type: "submit", textContent: "Take" }));
  const list = h("ul", { className: "snapshot-list" });
  const empty = h("p", { className: "hint muted" }, "Save versions of this edit to compare and come back to.");
  root.append(h("div", { className: "group-head" }, h("h3", {}, "Snapshots"), add), form, list, empty);

  const snapshots = () => (editor.doc.snapshots ??= []);
  add.addEventListener("click", () => {
    form.hidden = !form.hidden;
    if (!form.hidden) {
      name.value = `Snapshot ${snapshots().length + 1}`;
      name.select();
    }
  });
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    const n = name.value.trim();
    if (!n || !editor.opened) return;
    const snap: Snapshot = { name: n, created_ms: Date.now(), layers: clone(editor.doc.layers) };
    snapshots().push(snap);
    form.hidden = true;
    commit(`Snapshot: ${n}`);
  });
  name.addEventListener("keydown", (e) => {
    e.stopPropagation();
    if (e.key === "Escape") form.hidden = true;
  });

  let last = "";
  onUpdate(() => {
    add.disabled = !editor.opened;
    const snaps = editor.opened ? (editor.doc.snapshots ?? []) : [];
    const current = editor.opened ? JSON.stringify(editor.doc.layers) : "";
    const key = JSON.stringify(snaps.map((s) => [s.name, s.created_ms])) + "#" + snaps.findIndex((s) => JSON.stringify(s.layers) === current);
    empty.hidden = snaps.length > 0 || !editor.opened;
    if (key === last) return;
    last = key;
    list.replaceChildren(
      ...snaps.map((s, i) => {
        const time = new Date(s.created_ms).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
        const del = h("button", { type: "button", className: "icon", title: "Delete snapshot", textContent: "×" });
        del.addEventListener("click", (e) => {
          e.stopPropagation();
          snapshots().splice(i, 1);
          commit(`Delete snapshot: ${s.name}`);
        });
        const li = h("li", { className: JSON.stringify(s.layers) === current ? "current" : "", title: "Click to go back to this version" }, h("span", { className: "name" }, s.name), h("small", { className: "muted" }, time), del);
        li.addEventListener("click", () => {
          editor.doc.layers = clone(s.layers);
          editor.selected = Math.min(editor.selected, editor.doc.layers.length - 1);
          commit(`Restore snapshot: ${s.name}`);
        });
        return li;
      }),
    );
  });
}
