// Mask controls in the Layers tab: the parts of a combined mask (each added
// to, subtracted from, or intersected with the ones before it), the
// selected part's own settings, and the color range eyedropper.

import { type Layer, type Mask, type MaskMode, type MaskPart, activeMask, hueName, maskLabel } from "./doc";
import { sampleAt } from "./histogram";
import { h, slider } from "./panels";
import { changed, commit, editor, refresh } from "./state";

export type MaskKind = "brush" | "linear" | "radial" | "luminance" | "color";

export const MASK_KINDS: Array<[MaskKind, string]> = [
  ["brush", "Brush"],
  ["linear", "Gradient"],
  ["radial", "Radial"],
  ["luminance", "Luminance"],
  ["color", "Color range"],
];

const MODES: Array<[MaskMode, string, string]> = [
  ["add", "+", "Add"],
  ["subtract", "−", "Subtract"],
  ["intersect", "∩", "Intersect"],
];

/** A new mask of `kind` with sensible starting values. */
export function newMask(kind: MaskKind): Mask {
  const o = editor.opened;
  const [w, h] = o ? [o.preview_width, o.preview_height] : [1, 1];
  const r = 0.25 * Math.min(w, h);
  switch (kind) {
    case "brush":
      return { kind: "brush", strokes: [] };
    case "linear":
      return { kind: "linear", start: [0.5, 0.15], end: [0.5, 0.5] };
    case "radial":
      return { kind: "radial", center: [0.5, 0.5], radius: [r / w, r / h], angle_deg: 0, feather: 0.5, invert: false };
    case "luminance":
      // The highlights.
      return { kind: "luminance", low: 0.6, high: 1, feather: 0.15 };
    case "color":
      // Sky blue.
      return { kind: "color", hue: 210, range: 20, feather: 25 };
  }
}

/** Add a part to the layer's mask, turning a single mask into a combined one. */
function addPart(layer: Layer, mode: MaskMode, kind: MaskKind) {
  const mask = layer.mask;
  const part: MaskPart = { mode, mask: newMask(kind) };
  if (mask?.kind === "combine") mask.parts.push(part);
  else layer.mask = { kind: "combine", parts: [...(mask ? [{ mode: "add" as MaskMode, mask }] : []), part] };
  editor.part = (layer.mask as Extract<Mask, { kind: "combine" }>).parts.length - 1;
  commit();
}

function removePart(layer: Layer, i: number) {
  const mask = layer.mask;
  if (mask?.kind !== "combine" || mask.parts.length < 2) return;
  mask.parts.splice(i, 1);
  // Back to a plain mask when one part is left and nothing about it needs
  // the combined form.
  const [only] = mask.parts;
  if (mask.parts.length === 1 && !only.invert) layer.mask = only.mask;
  editor.part = Math.max(0, Math.min(editor.part, mask.parts.length - 1));
  commit();
}

function shortLabel(mask: Mask) {
  return MASK_KINDS.find(([k]) => k === mask.kind)?.[1] ?? maskLabel(mask);
}

/** Hue in degrees of an 8-bit sRGB color, or null when it's too grey to
 * have one. */
function hueOf([r, g, b]: [number, number, number]): number | null {
  const mx = Math.max(r, g, b);
  const c = mx - Math.min(r, g, b);
  if (mx === 0 || c / mx < 0.1) return null;
  const k = mx === r ? ((g - b) / c + 6) % 6 : mx === g ? (b - r) / c + 2 : (r - g) / c + 4;
  return Math.round(k * 60) % 360;
}

/**
 * Controls for the selected layer's mask. `layer` reads the live layer, since
 * the document object is replaced on undo. Returns sync functions.
 */
export function maskControls(body: HTMLElement, layer: () => Layer): Array<() => void> {
  const syncs: Array<() => void> = [];
  const mask = layer().mask;
  if (!mask || mask.kind === "plugin") return syncs;

  // Parts list, for combined masks.
  if (mask.kind === "combine") {
    const list = h("ul", { className: "mask-parts" });
    mask.parts.forEach((part, i) => {
      const parts = () => (layer().mask as Extract<Mask, { kind: "combine" }>).parts;
      const mode = h("select", { title: "How this part combines with the ones above" }) as HTMLSelectElement;
      for (const [value, sign, label] of MODES) mode.append(h("option", { value, textContent: `${sign} ${label}` }));
      mode.value = part.mode;
      mode.disabled = i === 0;
      mode.addEventListener("click", (e) => e.stopPropagation());
      mode.addEventListener("change", () => {
        parts()[i].mode = mode.value as MaskMode;
        commit();
      });
      const invert = h("button", { type: "button", className: "chip", title: "Invert: use the opposite of this part", textContent: "◐" });
      invert.addEventListener("click", (e) => {
        e.stopPropagation();
        parts()[i].invert = !parts()[i].invert;
        commit();
      });
      const row = h("li", {}, h("span", { className: `kind ${part.mask.kind}` }), h("span", { className: "label", title: maskLabel(part.mask) }, i === 0 ? "Start with" : "", h("strong", {}, shortLabel(part.mask))), mode, invert);
      if (mask.parts.length > 1) {
        const del = h("button", { type: "button", className: "icon", title: "Remove this part", textContent: "×" });
        del.addEventListener("click", (e) => {
          e.stopPropagation();
          removePart(layer(), i);
        });
        row.append(del);
      }
      row.addEventListener("click", () => {
        editor.part = i;
        changed();
      });
      list.append(row);
      syncs.push(() => {
        row.classList.toggle("selected", i === Math.min(editor.part, parts().length - 1));
        invert.classList.toggle("active", !!parts()[i]?.invert);
        if (parts()[i]) mode.value = parts()[i].mode;
      });
    });
    body.append(h("h4", { className: "sub" }, "Mask parts"), list);
  }

  // Combine with another mask.
  const kind = h("select", { title: "Kind of mask to combine" }) as HTMLSelectElement;
  for (const [value, label] of MASK_KINDS) kind.append(h("option", { value, textContent: label }));
  kind.value = "luminance";
  const combine = h("div", { className: "mask-combine" }, kind);
  kind.title = "Kind of mask to add, subtract, or intersect";
  for (const [mode, sign, label] of MODES) {
    const b = h("button", { type: "button", title: `${label} a mask of the chosen kind`, textContent: `${sign} ${label}` });
    b.addEventListener("click", () => addPart(layer(), mode, kind.value as MaskKind));
    combine.append(b);
  }
  body.append(combine);

  // Settings of the part being edited.
  const edited = () => activeMask(layer().mask, editor.part)!;
  const active = activeMask(mask, editor.part);
  const pct = (v: number) => `${Math.round(v)}%`;
  const deg = (v: number) => `${Math.round(v)}°`;
  if (active?.kind === "radial") {
    const radial = () => edited() as Extract<Mask, { kind: "radial" }>;
    syncs.push(slider(body, { label: "Feather", min: 0, max: 100, step: 1, neutral: 50, format: pct, get: () => Math.round(radial().feather * 100), set: (v) => (radial().feather = v / 100) }));
    const invert = h("input", { type: "checkbox" });
    invert.addEventListener("change", () => {
      radial().invert = invert.checked;
      commit();
    });
    body.append(h("label", { className: "check" }, invert, "Invert (affect the outside)"));
    syncs.push(() => (invert.checked = radial().invert));
    body.append(h("p", { className: "hint muted" }, "Drag the center to move, and the outer handles to resize and rotate."));
  } else if (active?.kind === "linear") {
    body.append(h("p", { className: "hint muted" }, "Drag the solid handle (full effect) and the hollow one (no effect) on the photo; the small middle handle moves both."));
  } else if (active?.kind === "brush") {
    const b = editor.brush;
    syncs.push(slider(body, { label: "Brush size", min: 1, max: 50, step: 0.5, neutral: 8, format: pct, get: () => b.size * 100, set: (v) => (b.size = v / 100) }));
    syncs.push(slider(body, { label: "Brush feather", min: 0, max: 100, step: 1, neutral: 50, format: pct, get: () => Math.round(b.feather * 100), set: (v) => (b.feather = v / 100) }));
    syncs.push(slider(body, { label: "Flow", min: 5, max: 100, step: 1, neutral: 100, format: pct, get: () => Math.round(b.flow * 100), set: (v) => (b.flow = v / 100) }));
    const mode = h("div", { className: "segmented" });
    const paint = h("button", { type: "button", textContent: "Paint" });
    const erase = h("button", { type: "button", textContent: "Erase" });
    paint.addEventListener("click", () => ((b.erase = false), refresh()));
    erase.addEventListener("click", () => ((b.erase = true), refresh()));
    mode.append(paint, erase);
    const clear = h("button", { type: "button", className: "link", textContent: "Clear strokes" });
    clear.addEventListener("click", () => {
      (edited() as Extract<Mask, { kind: "brush" }>).strokes = [];
      commit();
    });
    body.append(mode, clear, h("p", { className: "hint muted" }, "Paint on the photo. Brush settings apply to new strokes."));
    syncs.push(() => {
      paint.classList.toggle("active", !b.erase);
      erase.classList.toggle("active", b.erase);
    });
  } else if (active?.kind === "luminance") {
    const lum = () => edited() as Extract<Mask, { kind: "luminance" }>;
    const ramp = () => {
      const { low, high } = lum();
      return `linear-gradient(90deg, #111 0%, #111 ${low * 100}%, var(--mask) ${low * 100}%, var(--mask) ${high * 100}%, #111 ${high * 100}%)`;
    };
    syncs.push(slider(body, { label: "Darkest", min: 0, max: 100, step: 1, neutral: 60, format: pct, gradient: ramp, get: () => lum().low * 100, set: (v) => (lum().low = Math.min(v / 100, lum().high)) }));
    syncs.push(slider(body, { label: "Brightest", min: 0, max: 100, step: 1, neutral: 100, format: pct, gradient: ramp, get: () => lum().high * 100, set: (v) => (lum().high = Math.max(v / 100, lum().low)) }));
    syncs.push(slider(body, { label: "Softness", min: 0, max: 50, step: 1, neutral: 15, format: pct, get: () => lum().feather * 100, set: (v) => (lum().feather = v / 100) }));
    body.append(h("p", { className: "hint muted" }, "Selects tones between the two brightness levels, read from the photo before any edits."));
  } else if (active?.kind === "color") {
    const col = () => edited() as Extract<Mask, { kind: "color" }>;
    const rainbow = "linear-gradient(90deg, hsl(0 70% 50%), hsl(60 70% 50%), hsl(120 70% 50%), hsl(180 70% 50%), hsl(240 70% 50%), hsl(300 70% 50%), hsl(360 70% 50%))";
    syncs.push(slider(body, { label: "Hue", min: 0, max: 360, step: 1, neutral: 210, format: (v) => `${deg(v)} ${hueName(v)}`, gradient: () => rainbow, get: () => col().hue, set: (v) => (col().hue = v) }));
    syncs.push(slider(body, { label: "Range", min: 0, max: 90, step: 1, neutral: 20, format: (v) => `±${deg(v)}`, get: () => col().range, set: (v) => (col().range = v) }));
    syncs.push(slider(body, { label: "Softness", min: 0, max: 90, step: 1, neutral: 25, format: deg, get: () => col().feather, set: (v) => (col().feather = v) }));
    const pick = h("button", { type: "button", className: "pick", textContent: "Pick a color from the photo" });
    pick.addEventListener("click", () => {
      if (editor.picking) {
        editor.picking = null;
        return changed();
      }
      // The red overlay would tint what's sampled, so hide it until the click.
      const shown = editor.showMask;
      editor.showMask = false;
      editor.picking = (u, v) => {
        editor.showMask = shown;
        const rgb = sampleAt(u, v);
        const hue = rgb && hueOf(rgb);
        if (hue !== null) {
          col().hue = hue;
          commit();
        } else changed();
      };
      changed();
    });
    body.append(pick);
    syncs.push(() => {
      pick.classList.toggle("active", !!editor.picking);
      pick.textContent = editor.picking ? "Click the photo… (or here to cancel)" : "Pick a color from the photo";
    });
  }
  const show = h("input", { type: "checkbox" });
  show.addEventListener("change", () => {
    editor.showMask = show.checked;
    changed();
  });
  body.append(h("label", { className: "check" }, show, "Show mask overlay"));
  syncs.push(() => (show.checked = editor.showMask));
  return syncs;
}
