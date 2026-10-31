// The Presence, Color mixer, and Color grading sections of the Edit tab.

import {
  type Grade,
  type Hsl,
  type Layer,
  type Presence,
  type Wheel,
  NEUTRAL_PRESENCE,
  isNeutralWheel,
  neutralGrade,
  neutralHsl,
  readGrade,
  readHsl,
  readPresence,
  writeGrade,
  writeHsl,
  writePresence,
} from "./doc";
import { h, section, slider } from "./panels";
import { changed, commit, editor } from "./state";

const percent = (v: number) => (v > 0 ? "+" : "") + Math.round(v);

// --- Presence -------------------------------------------------------------------

export function buildPresence(root: HTMLElement, layer: () => Layer): () => void {
  const get = () => (editor.opened ? readPresence(layer()) : { ...NEUTRAL_PRESENCE });
  const sec = section(root, "Presence", {
    isChanged: () => Object.values(get()).some((v) => v !== 0),
    onReset: () => {
      writePresence(layer(), { ...NEUTRAL_PRESENCE });
      commit();
    },
  });
  const syncs = [sec.sync];
  const rows: [keyof Presence, string][] = [
    ["texture", "Texture"],
    ["clarity", "Clarity"],
    ["dehaze", "Dehaze"],
  ];
  for (const [key, label] of rows) {
    syncs.push(
      slider(sec.body, {
        label,
        min: -100,
        max: 100,
        step: 1,
        neutral: 0,
        format: percent,
        get: () => Math.round(get()[key] * 100),
        set: (v) => writePresence(layer(), { ...get(), [key]: v / 100 }),
      }),
    );
  }
  return () => syncs.forEach((s) => s());
}

// --- Color mixer -------------------------------------------------------------------

const BANDS: [string, string, number][] = [
  ["Red", "#e5534b", 0],
  ["Orange", "#ef8a3c", 30],
  ["Yellow", "#e8cf4b", 60],
  ["Green", "#5cc46a", 120],
  ["Aqua", "#3fc1c9", 180],
  ["Blue", "#4a79e0", 240],
  ["Purple", "#9b5ce0", 270],
  ["Magenta", "#e05cb5", 300],
];

type MixMode = "band" | keyof Hsl;
const PROPS: [keyof Hsl, string][] = [
  ["hue", "Hue"],
  ["saturation", "Saturation"],
  ["luminance", "Luminance"],
];

/** The gradient a slider's track shows for one band and property. */
function bandGradient(band: number, prop: keyof Hsl) {
  const hue = BANDS[band][2];
  if (prop === "hue") return `linear-gradient(90deg, hsl(${hue - 30} 70% 55%), hsl(${hue} 70% 55%), hsl(${hue + 30} 70% 55%))`;
  if (prop === "saturation") return `linear-gradient(90deg, hsl(${hue} 0% 55%), hsl(${hue} 80% 55%))`;
  return `linear-gradient(90deg, hsl(${hue} 60% 18%), hsl(${hue} 70% 55%), hsl(${hue} 70% 85%))`;
}

export function buildMixer(root: HTMLElement, layer: () => Layer): () => void {
  const get = () => (editor.opened ? readHsl(layer()) : neutralHsl());
  const sec = section(root, "Color mixer", {
    isChanged: () => Object.values(get()).some((a) => a.some((v) => v !== 0)),
    onReset: () => {
      writeHsl(layer(), neutralHsl());
      commit();
    },
  });
  let mode: MixMode = "band";
  let band = 5;

  const modes = h("div", { className: "segmented" });
  const modeButtons = ([["band", "By color"], ...PROPS] as [MixMode, string][]).map(([m, label]) => {
    const b = h("button", { type: "button", textContent: label });
    b.addEventListener("click", () => {
      mode = m;
      draw();
    });
    modes.append(b);
    return [m, b] as const;
  });
  const dots = h("div", { className: "band-dots" });
  const dotButtons = BANDS.map(([name, color], i) => {
    const d = h("button", { type: "button", title: name, className: "band-dot" });
    d.style.background = color;
    d.addEventListener("click", () => {
      band = i;
      draw();
    });
    dots.append(d);
    return d;
  });
  const sliders = h("div", {});
  sec.body.append(modes, dots, sliders);

  let syncs: Array<() => void> = [];
  const add = (label: string, b: number, prop: keyof Hsl) =>
    syncs.push(
      slider(sliders, {
        label,
        min: -100,
        max: 100,
        step: 1,
        neutral: 0,
        format: percent,
        gradient: () => bandGradient(b, prop),
        get: () => Math.round(get()[prop][b] * 100),
        set: (v) => {
          const hsl = get();
          hsl[prop][b] = v / 100;
          writeHsl(layer(), hsl);
        },
      }),
    );
  const draw = () => {
    sliders.replaceChildren();
    syncs = [];
    dots.hidden = mode !== "band";
    if (mode === "band") for (const [prop, label] of PROPS) add(label, band, prop);
    else BANDS.forEach(([name], i) => add(name, i, mode as keyof Hsl));
    sync();
  };
  const sync = () => {
    sec.sync();
    for (const [m, b] of modeButtons) b.classList.toggle("active", m === mode);
    const hsl = get();
    dotButtons.forEach((d, i) => {
      d.classList.toggle("active", i === band);
      d.classList.toggle("changed", PROPS.some(([p]) => hsl[p][i] !== 0));
    });
    syncs.forEach((s) => s());
  };
  draw();
  return sync;
}

// --- Color grading -------------------------------------------------------------------

type Range = "shadows" | "midtones" | "highlights" | "global";
type GradeMode = "three" | Range;
const RANGES: [Range, string][] = [
  ["shadows", "Shadows"],
  ["midtones", "Midtones"],
  ["highlights", "Highlights"],
  ["global", "Global"],
];

// Hue runs counterclockwise from red at 3 o'clock; CSS conic gradients run
// clockwise, so the stops go backwards.
const WHEEL_BG =
  "radial-gradient(circle closest-side, #d9d6d0 0%, rgb(217 214 208 / 0) 100%), conic-gradient(from 90deg, " +
  [0, 330, 300, 270, 240, 210, 180, 150, 120, 90, 60, 30, 0].map((hue) => `hsl(${hue} 75% 55%)`).join(", ") +
  ")";

/** A color wheel: drag the dot to set hue (angle) and saturation (distance
 * from the center). Double-click resets. */
function wheel(parent: HTMLElement, size: number, get: () => Wheel, set: (w: Wheel) => void) {
  const dot = h("span", { className: "wheel-dot" });
  const disc = h("div", { className: "wheel" }, dot);
  disc.style.width = disc.style.height = `${size}px`;
  disc.style.background = WHEEL_BG;
  parent.append(disc);
  const fromEvent = (e: PointerEvent) => {
    const r = disc.getBoundingClientRect();
    const [dx, dy] = [e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2];
    const sat = Math.min(1, Math.hypot(dx, dy) / (r.width / 2));
    const hue = ((Math.atan2(-dy, dx) * 180) / Math.PI + 360) % 360;
    set({ ...get(), hue: Math.round(hue), saturation: Math.round(sat * 100) / 100 });
    changed();
  };
  let dragging = false;
  disc.addEventListener("pointerdown", (e) => {
    if (!editor.opened) return;
    dragging = true;
    disc.setPointerCapture(e.pointerId);
    fromEvent(e);
  });
  disc.addEventListener("pointermove", (e) => dragging && fromEvent(e));
  disc.addEventListener("pointerup", () => {
    if (!dragging) return;
    dragging = false;
    commit();
  });
  disc.addEventListener("dblclick", () => {
    set({ ...get(), hue: 0, saturation: 0 });
    commit();
  });
  return () => {
    const w = get();
    const a = (w.hue * Math.PI) / 180;
    const r = (w.saturation * size) / 2;
    dot.style.left = `${size / 2 + r * Math.cos(a)}px`;
    dot.style.top = `${size / 2 - r * Math.sin(a)}px`;
    dot.style.background = w.saturation > 0 ? `hsl(${w.hue} 75% 55%)` : "#d9d6d0";
    disc.classList.toggle("disabled", !editor.opened);
  };
}

export function buildGrading(root: HTMLElement, layer: () => Layer): () => void {
  const get = (): Grade => (editor.opened ? readGrade(layer()) : neutralGrade());
  const sec = section(root, "Color grading", {
    isChanged: () => RANGES.some(([r]) => !isNeutralWheel(get()[r])),
    onReset: () => {
      writeGrade(layer(), neutralGrade());
      commit();
    },
  });
  let mode: GradeMode = "three";
  const modes = h("div", { className: "segmented" });
  const modeButtons = ([["three", "3-way"], ...RANGES.map(([r, l]) => [r, l.replace("Midtones", "Mids").replace("Highlights", "Highs")])] as [GradeMode, string][]).map(([m, label]) => {
    const b = h("button", { type: "button", textContent: label });
    b.addEventListener("click", () => {
      mode = m;
      draw();
    });
    modes.append(b);
    return [m, b] as const;
  });
  const area = h("div", {});
  const shared = h("div", {});
  sec.body.append(modes, area, shared);

  const setWheel = (r: Range) => (w: Wheel) => writeGrade(layer(), { ...get(), [r]: w });
  const lumSlider = (parent: HTMLElement, r: Range, label: string) =>
    slider(parent, {
      label,
      min: -100,
      max: 100,
      step: 1,
      neutral: 0,
      format: percent,
      get: () => Math.round(get()[r].luminance * 100),
      set: (v) => setWheel(r)({ ...get()[r], luminance: v / 100 }),
    });

  let syncs: Array<() => void> = [];
  const draw = () => {
    area.replaceChildren();
    syncs = [];
    if (mode === "three") {
      const grid = h("div", { className: "wheels" });
      area.append(grid);
      for (const [r, label] of RANGES.slice(0, 3)) {
        const cell = h("div", { className: "wheel-cell" });
        grid.append(cell);
        syncs.push(wheel(cell, 78, () => get()[r], setWheel(r)));
        const caption = h("div", { className: "wheel-caption" }, label);
        const values = h("div", { className: "wheel-values mono muted" });
        cell.append(caption, values);
        syncs.push(() => {
          const w = get()[r];
          values.textContent = w.saturation ? `H ${w.hue}° · S ${Math.round(w.saturation * 100)}` : "—";
        });
      }
      for (const [r, label] of RANGES.slice(0, 3)) syncs.push(lumSlider(area, r, `${label} luminance`));
    } else {
      const r = mode;
      const cell = h("div", { className: "wheel-single" });
      area.append(cell);
      syncs.push(wheel(cell, 150, () => get()[r], setWheel(r)));
      syncs.push(
        slider(area, {
          label: "Hue",
          min: 0,
          max: 359,
          step: 1,
          neutral: 0,
          format: (v) => `${v}°`,
          track: "hue",
          get: () => Math.round(get()[r].hue),
          set: (v) => setWheel(r)({ ...get()[r], hue: v }),
        }),
        slider(area, {
          label: "Saturation",
          min: 0,
          max: 100,
          step: 1,
          neutral: 0,
          format: (v) => String(v),
          gradient: () => `linear-gradient(90deg, #8a8d96, hsl(${get()[r].hue} 75% 55%))`,
          get: () => Math.round(get()[r].saturation * 100),
          set: (v) => setWheel(r)({ ...get()[r], saturation: v / 100 }),
        }),
        lumSlider(area, r, "Luminance"),
      );
    }
    sync();
  };
  syncs = [];
  const sharedSyncs = [
    slider(shared, {
      label: "Blending",
      min: 0,
      max: 100,
      step: 1,
      neutral: 50,
      format: (v) => String(v),
      get: () => Math.round(get().blending * 100),
      set: (v) => writeGrade(layer(), { ...get(), blending: v / 100 }),
    }),
    slider(shared, {
      label: "Balance",
      min: -100,
      max: 100,
      step: 1,
      neutral: 0,
      format: percent,
      get: () => Math.round(get().balance * 100),
      set: (v) => writeGrade(layer(), { ...get(), balance: v / 100 }),
    }),
  ];
  const sync = () => {
    sec.sync();
    for (const [m, b] of modeButtons) b.classList.toggle("active", m === mode);
    syncs.forEach((s) => s());
    sharedSyncs.forEach((s) => s());
  };
  draw();
  return sync;
}
