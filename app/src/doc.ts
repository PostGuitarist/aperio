// The edit document as the backend sends it, and helpers the panels share.
// Field names follow docs/sidecar-format.md.

export type Point = [number, number];
export type Channel = "rgb" | "red" | "green" | "blue" | "luma";

export type Operation =
  | { op: "exposure"; ev: number }
  | { op: "contrast"; amount: number }
  | { op: "tone"; highlights: number; shadows: number; whites: number; blacks: number }
  | { op: "white_balance"; temperature_k: number; tint: number }
  | { op: "color"; saturation: number; vibrance: number }
  | { op: "presence"; texture: number; clarity: number; dehaze: number }
  | { op: "hsl"; hue: number[]; saturation: number[]; luminance: number[] }
  | ({ op: "color_grade" } & Grade)
  | { op: "curve"; channel: Channel; points: Point[] }
  | { op: "crop"; x: number; y: number; width: number; height: number; angle_deg: number }
  | { op: "plugin"; plugin: string; version: string; params: unknown };

export interface Wheel {
  /** Degrees, 0 to 360. */
  hue: number;
  saturation: number;
  luminance: number;
}

export interface Grade {
  shadows: Wheel;
  midtones: Wheel;
  highlights: Wheel;
  global: Wheel;
  blending: number;
  balance: number;
}

export interface BrushStroke {
  points: Point[];
  size: number;
  feather: number;
  flow: number;
  erase: boolean;
}

export type Mask =
  | { kind: "brush"; strokes: BrushStroke[] }
  | { kind: "linear"; start: Point; end: Point }
  | { kind: "radial"; center: Point; radius: Point; angle_deg: number; feather: number; invert: boolean }
  /** Brightness range, in sRGB-encoded luminance 0..1. */
  | { kind: "luminance"; low: number; high: number; feather: number }
  /** Hue range, in degrees. */
  | { kind: "color"; hue: number; range: number; feather: number }
  | { kind: "combine"; parts: MaskPart[] }
  | { kind: "plugin"; plugin: string; version: string; params: unknown };

export type MaskMode = "add" | "subtract" | "intersect";

export interface MaskPart {
  mode: MaskMode;
  invert?: boolean;
  mask: Mask;
}

export type BlendMode = "normal" | "multiply" | "screen" | "overlay" | "soft_light" | "color" | "luminosity";

export const BLEND_MODES: Array<[BlendMode, string]> = [
  ["normal", "Normal"],
  ["multiply", "Multiply"],
  ["screen", "Screen"],
  ["overlay", "Overlay"],
  ["soft_light", "Soft light"],
  ["color", "Color"],
  ["luminosity", "Luminosity"],
];

export interface Layer {
  name: string;
  enabled: boolean;
  opacity: number;
  /** Left out when normal. */
  blend_mode?: BlendMode;
  mask?: Mask;
  operations: Operation[];
}

export interface EditDocument {
  format: string;
  version: number;
  source: { file_name: string; size_bytes: number; sha256?: string };
  layers: Layer[];
  /** Named copies of `layers` to come back to. Left out when empty. */
  snapshots?: Snapshot[];
}

export interface Snapshot {
  name: string;
  created_ms: number;
  layers: Layer[];
}

export type CropOp = Extract<Operation, { op: "crop" }>;
export type CurveOp = Extract<Operation, { op: "curve" }>;

// --- Basic adjustments ---------------------------------------------------

export const NEUTRAL_KELVIN = 6500;

export interface Adjustments {
  exposure: number;
  contrast: number;
  highlights: number;
  shadows: number;
  whites: number;
  blacks: number;
  temperature: number;
  tint: number;
  saturation: number;
  vibrance: number;
}

export const NEUTRAL: Adjustments = {
  exposure: 0,
  contrast: 0,
  highlights: 0,
  shadows: 0,
  whites: 0,
  blacks: 0,
  temperature: NEUTRAL_KELVIN,
  tint: 0,
  saturation: 0,
  vibrance: 0,
};

const ADJUST_OPS = new Set(["exposure", "contrast", "tone", "white_balance", "color"]);

export function readAdjustments(layer: Layer): Adjustments {
  const a = { ...NEUTRAL };
  for (const op of layer.operations) {
    if (op.op === "exposure") a.exposure = op.ev;
    else if (op.op === "contrast") a.contrast = op.amount;
    else if (op.op === "tone") [a.highlights, a.shadows, a.whites, a.blacks] = [op.highlights ?? 0, op.shadows ?? 0, op.whites ?? 0, op.blacks ?? 0];
    else if (op.op === "white_balance") [a.temperature, a.tint] = [op.temperature_k, op.tint];
    else if (op.op === "color") [a.saturation, a.vibrance] = [op.saturation, op.vibrance];
  }
  return a;
}

/** Write the panel's values into `layer`, ahead of curves, crop, and plugin
 * operations. Neutral values are left out so untouched layers stay clean. */
export function writeAdjustments(layer: Layer, a: Adjustments) {
  const ops: Operation[] = [];
  if (a.temperature !== NEUTRAL.temperature || a.tint !== NEUTRAL.tint)
    ops.push({ op: "white_balance", temperature_k: a.temperature, tint: a.tint });
  if (a.exposure !== 0) ops.push({ op: "exposure", ev: a.exposure });
  if (a.contrast !== 0) ops.push({ op: "contrast", amount: a.contrast });
  if (a.highlights !== 0 || a.shadows !== 0 || a.whites !== 0 || a.blacks !== 0)
    ops.push({ op: "tone", highlights: a.highlights, shadows: a.shadows, whites: a.whites, blacks: a.blacks });
  if (a.saturation !== 0 || a.vibrance !== 0)
    ops.push({ op: "color", saturation: a.saturation, vibrance: a.vibrance });
  layer.operations = [...ops, ...layer.operations.filter((op) => !ADJUST_OPS.has(op.op))];
}

// --- Presence, color mixer, and color grading -----------------------------

export type Presence = { texture: number; clarity: number; dehaze: number };
export type Hsl = { hue: number[]; saturation: number[]; luminance: number[] };

export const NEUTRAL_PRESENCE: Presence = { texture: 0, clarity: 0, dehaze: 0 };
const zeros = () => new Array<number>(8).fill(0);
export const neutralHsl = (): Hsl => ({ hue: zeros(), saturation: zeros(), luminance: zeros() });
const neutralWheel = (): Wheel => ({ hue: 0, saturation: 0, luminance: 0 });
export const neutralGrade = (): Grade => ({ shadows: neutralWheel(), midtones: neutralWheel(), highlights: neutralWheel(), global: neutralWheel(), blending: 0.5, balance: 0 });

/** These come after the basic adjustments and curves, in this order. */
const LATER_OPS = ["presence", "hsl", "color_grade"];

type Later = Extract<Operation, { op: "presence" | "hsl" | "color_grade" }>;

function readLater<K extends Later["op"]>(layer: Layer, op: K): Extract<Later, { op: K }> | undefined {
  return layer.operations.find((o): o is Extract<Later, { op: K }> => o.op === op);
}

/** Put `value` in place of the layer's `op` operation, or remove it when
 * `value` is null. A new one goes after the basic adjustments and curves. */
function writeLater(layer: Layer, op: Later["op"], value: Later | null) {
  const at = layer.operations.findIndex((o) => o.op === op);
  if (at >= 0) {
    if (value) layer.operations[at] = value;
    else layer.operations.splice(at, 1);
    return;
  }
  if (!value) return;
  const rank = LATER_OPS.indexOf(op);
  const before = layer.operations.findIndex((o) => o.op === "crop" || o.op === "plugin" || LATER_OPS.indexOf(o.op) > rank);
  layer.operations.splice(before < 0 ? layer.operations.length : before, 0, value);
}

export function readPresence(layer: Layer): Presence {
  const o = readLater(layer, "presence");
  return o ? { texture: o.texture, clarity: o.clarity, dehaze: o.dehaze } : { ...NEUTRAL_PRESENCE };
}

export function writePresence(layer: Layer, p: Presence) {
  const neutral = p.texture === 0 && p.clarity === 0 && p.dehaze === 0;
  writeLater(layer, "presence", neutral ? null : { op: "presence", ...p });
}

export function readHsl(layer: Layer): Hsl {
  const o = readLater(layer, "hsl");
  return o ? clone({ hue: o.hue, saturation: o.saturation, luminance: o.luminance }) : neutralHsl();
}

export function writeHsl(layer: Layer, v: Hsl) {
  const neutral = [...v.hue, ...v.saturation, ...v.luminance].every((x) => x === 0);
  writeLater(layer, "hsl", neutral ? null : { op: "hsl", ...clone(v) });
}

export function readGrade(layer: Layer): Grade {
  const o = readLater(layer, "color_grade");
  if (!o) return neutralGrade();
  const { op: _, ...g } = clone(o);
  return { ...neutralGrade(), ...g };
}

export const isNeutralWheel = (w: Wheel) => w.saturation === 0 && w.luminance === 0;

export function writeGrade(layer: Layer, g: Grade) {
  const neutral = [g.shadows, g.midtones, g.highlights, g.global].every(isNeutralWheel) && g.blending === 0.5 && g.balance === 0;
  writeLater(layer, "color_grade", neutral ? null : { op: "color_grade", ...clone(g) });
}

// --- Curves ---------------------------------------------------------------

export const IDENTITY: Point[] = [
  [0, 0],
  [1, 1],
];

export function readCurve(layer: Layer, channel: Channel): Point[] {
  const op = layer.operations.find((o): o is CurveOp => o.op === "curve" && o.channel === channel);
  return op ? op.points.map((p) => [p[0], p[1]] as Point) : IDENTITY.map((p) => [...p] as Point);
}

function isIdentity(points: Point[]) {
  return points.every((p) => Math.abs(p[0] - p[1]) < 1e-4);
}

/** Store a channel's curve, dropping it when it does nothing. Curves stay in
 * the order rgb, red, green, blue, luma after the basic adjustments. */
export function writeCurve(layer: Layer, channel: Channel, points: Point[]) {
  const order: Channel[] = ["rgb", "red", "green", "blue", "luma"];
  const curves = new Map<Channel, Point[]>();
  for (const op of layer.operations) if (op.op === "curve") curves.set(op.channel, op.points);
  if (isIdentity(points)) curves.delete(channel);
  else curves.set(channel, points);
  const rest: Operation[] = layer.operations.filter((op) => op.op !== "curve");
  const firstOther = rest.findIndex((op) => !ADJUST_OPS.has(op.op));
  const at = firstOther < 0 ? rest.length : firstOther;
  const curveOps: Operation[] = order
    .filter((c) => curves.has(c))
    .map((c) => ({ op: "curve", channel: c, points: curves.get(c)! }));
  rest.splice(at, 0, ...curveOps);
  layer.operations = rest;
}

/** Monotone cubic through `points`, sampled at `n` evenly spaced x values.
 * Mirrors `curve::bake` in aperio-render so the editor draws what renders. */
export function bake(points: Point[], n = 256): number[] {
  const pts = points
    .map((p) => [clamp01(p[0]), clamp01(p[1])] as Point)
    .sort((a, b) => a[0] - b[0])
    .filter((p, i, all) => i === 0 || Math.abs(p[0] - all[i - 1][0]) >= 1e-6);
  const xs = Array.from({ length: n }, (_, i) => i / (n - 1));
  if (pts.length < 2) return xs;
  const k = pts.length;
  const delta = pts.slice(0, -1).map((p, i) => (pts[i + 1][1] - p[1]) / (pts[i + 1][0] - p[0]));
  const m = new Array<number>(k).fill(0);
  m[0] = delta[0];
  m[k - 1] = delta[k - 2];
  for (let i = 1; i < k - 1; i++) m[i] = delta[i - 1] * delta[i] <= 0 ? 0 : (delta[i - 1] + delta[i]) / 2;
  for (let i = 0; i < k - 1; i++) {
    if (delta[i] === 0) {
      m[i] = m[i + 1] = 0;
      continue;
    }
    const a = m[i] / delta[i];
    const b = m[i + 1] / delta[i];
    const s = a * a + b * b;
    if (s > 9) {
      const t = 3 / Math.sqrt(s);
      m[i] = t * a * delta[i];
      m[i + 1] = t * b * delta[i];
    }
  }
  return xs.map((x) => {
    if (x <= pts[0][0]) return pts[0][1];
    if (x >= pts[k - 1][0]) return pts[k - 1][1];
    let j = 0;
    while (x > pts[j + 1][0]) j++;
    const h = pts[j + 1][0] - pts[j][0];
    const t = (x - pts[j][0]) / h;
    const t2 = t * t;
    const t3 = t2 * t;
    const y =
      (2 * t3 - 3 * t2 + 1) * pts[j][1] +
      (t3 - 2 * t2 + t) * h * m[j] +
      (-2 * t3 + 3 * t2) * pts[j + 1][1] +
      (t3 - t2) * h * m[j + 1];
    return clamp01(y);
  });
}

// --- Crop -------------------------------------------------------------------

export function readCrop(doc: EditDocument): CropOp | undefined {
  const ops = doc.layers[0].operations.filter((o): o is CropOp => o.op === "crop");
  return ops[ops.length - 1];
}

/** Set (or with `undefined`, remove) the base layer's crop. It goes last,
 * since crop runs after every other operation anyway. */
export function writeCrop(doc: EditDocument, crop: CropOp | undefined) {
  const base = doc.layers[0];
  base.operations = base.operations.filter((o) => o.op !== "crop");
  const full = crop && crop.x === 0 && crop.y === 0 && crop.width === 1 && crop.height === 1 && crop.angle_deg === 0;
  if (crop && !full) base.operations.push(crop);
}

// --- Small helpers ------------------------------------------------------------

export function clamp01(v: number) {
  return Math.min(1, Math.max(0, v));
}

export function clone<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

const HUE_NAMES: Array<[number, string]> = [
  [15, "Red"],
  [45, "Orange"],
  [70, "Yellow"],
  [160, "Green"],
  [200, "Aqua"],
  [255, "Blue"],
  [290, "Purple"],
  [340, "Magenta"],
  [360, "Red"],
];

/** A rough name for a hue in degrees. */
export function hueName(hue: number) {
  const h = ((hue % 360) + 360) % 360;
  return HUE_NAMES.find(([end]) => h < end)?.[1] ?? "Red";
}

/** The mask that on-canvas tools edit: for combined masks, the selected
 * part. */
export function activeMask(mask: Mask | undefined, part: number): Mask | undefined {
  if (mask?.kind !== "combine") return mask;
  return mask.parts[Math.min(part, mask.parts.length - 1)]?.mask;
}

export function maskLabel(mask: Mask | undefined): string {
  switch (mask?.kind) {
    case undefined:
      return "Whole image";
    case "brush":
      return `Brush · ${mask.strokes.length} stroke${mask.strokes.length === 1 ? "" : "s"}`;
    case "linear":
      return "Linear gradient";
    case "radial":
      return mask.invert ? "Radial, inverted" : "Radial gradient";
    case "luminance":
      return `Luminance ${Math.round(mask.low * 100)}–${Math.round(mask.high * 100)}%`;
    case "color":
      return `Color range · ${hueName(mask.hue)}`;
    case "combine": {
      const n = mask.parts.length;
      return n === 1 ? maskLabel(mask.parts[0].mask) : `${n} masks combined`;
    }
    case "plugin": {
      if (mask.plugin !== "org.aperio.segment") return mask.plugin;
      const params = (mask.params ?? {}) as { class?: string; invert?: boolean };
      return `AI ${params.class ?? "sky"}${params.invert ? ", inverted" : ""}`;
    }
  }
}
