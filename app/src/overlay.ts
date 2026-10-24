// On-canvas tools drawn over the preview: the crop rectangle, gradient and
// radial mask handles, and the brush. Shapes are kept in preview pixels of the
// uncropped photo (the document stores them normalized) and mapped to the
// screen through whatever crop the shown preview has.

import { type CropOp, type Mask, type Point, clamp01, readCrop, writeCrop } from "./doc";
import { changed, commit, editor, refresh } from "./state";

const SVG = "http://www.w3.org/2000/svg";
const HANDLE = 6;
const MIN_CROP = 16;

/** Maps between preview pixels of the uncropped photo and screen pixels. */
export class Frame {
  constructor(
    readonly W: number,
    readonly H: number,
    readonly crop: CropOp | undefined,
    /** Screen pixels per preview pixel. */
    readonly scale: number,
  ) {}

  /** Size of the shown image in preview pixels. */
  static outSize(W: number, H: number, crop: CropOp | undefined): [number, number] {
    if (!crop) return [W, H];
    return [Math.max(1, Math.round(crop.width * W)), Math.max(1, Math.round(crop.height * H))];
  }

  toView([x, y]: Point): Point {
    const k = this.scale;
    if (!this.crop) return [x * k, y * k];
    const [ow, oh] = Frame.outSize(this.W, this.H, this.crop);
    const c = this.cropCenter();
    const a = rad(this.crop.angle_deg);
    const [dx, dy] = [x - c[0], y - c[1]];
    const lx = dx * Math.cos(a) + dy * Math.sin(a);
    const ly = -dx * Math.sin(a) + dy * Math.cos(a);
    return [(lx + ow / 2) * k, (ly + oh / 2) * k];
  }

  toSource([vx, vy]: Point): Point {
    const k = this.scale;
    if (!this.crop) return [vx / k, vy / k];
    const [ow, oh] = Frame.outSize(this.W, this.H, this.crop);
    const c = this.cropCenter();
    const a = rad(this.crop.angle_deg);
    // Same mapping as `LinearImage::crop` in aperio-render.
    const [dx, dy] = [vx / k - ow / 2, vy / k - oh / 2];
    return [c[0] + dx * Math.cos(a) - dy * Math.sin(a), c[1] + dx * Math.sin(a) + dy * Math.cos(a)];
  }

  private cropCenter(): Point {
    const c = this.crop!;
    return [(c.x + c.width / 2) * this.W, (c.y + c.height / 2) * this.H];
  }
}

const rad = (deg: number) => (deg * Math.PI) / 180;
const rotate = ([x, y]: Point, a: number): Point => [x * Math.cos(a) - y * Math.sin(a), x * Math.sin(a) + y * Math.cos(a)];
const add = (a: Point, b: Point): Point => [a[0] + b[0], a[1] + b[1]];
const sub = (a: Point, b: Point): Point => [a[0] - b[0], a[1] - b[1]];
const len = ([x, y]: Point) => Math.hypot(x, y);

// --- Crop rectangle in preview pixels ------------------------------------------

export interface CropBox {
  c: Point;
  w: number;
  h: number;
  angle: number;
}

function toBox(crop: CropOp | undefined, W: number, H: number): CropBox {
  const op = crop ?? { x: 0, y: 0, width: 1, height: 1, angle_deg: 0 };
  return {
    c: [(op.x + op.width / 2) * W, (op.y + op.height / 2) * H],
    w: op.width * W,
    h: op.height * H,
    angle: op.angle_deg,
  };
}

function fromBox(b: CropBox, W: number, H: number): CropOp {
  const r = (v: number) => Math.round(v * 1e5) / 1e5;
  return {
    op: "crop",
    x: r((b.c[0] - b.w / 2) / W),
    y: r((b.c[1] - b.h / 2) / H),
    width: r(b.w / W),
    height: r(b.h / H),
    angle_deg: Math.round(b.angle * 100) / 100,
  };
}

function corners(b: CropBox): Point[] {
  const a = rad(b.angle);
  return [
    [-1, -1],
    [1, -1],
    [1, 1],
    [-1, 1],
  ].map(([sx, sy]) => add(b.c, rotate([(sx * b.w) / 2, (sy * b.h) / 2], a)));
}

/** Shrink `b` around its center (moving the center inside first if needed)
 * until every corner is on the photo, so no empty edges show. */
export function fitInside(b: CropBox, W: number, H: number): CropBox {
  const inside = (box: CropBox) =>
    corners(box).every(([x, y]) => x >= -0.01 && y >= -0.01 && x <= W + 0.01 && y <= H + 0.01);
  if (inside(b)) return b;
  const c: Point = [Math.min(W, Math.max(0, b.c[0])), Math.min(H, Math.max(0, b.c[1]))];
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 30; i++) {
    const s = (lo + hi) / 2;
    if (inside({ ...b, c, w: b.w * s, h: b.h * s })) lo = s;
    else hi = s;
  }
  return { ...b, c, w: Math.max(MIN_CROP, b.w * lo), h: Math.max(MIN_CROP, b.h * lo) };
}

/** Width / height in preview pixels for an aspect preset, turned to match
 * the photo's orientation. `ratio` is long side over short side. */
export function aspectFor(ratio: number | "original", W: number, H: number) {
  if (ratio === "original") return W / H;
  return W >= H ? ratio : 1 / ratio;
}

/** The largest rectangle of the given aspect at the current angle. */
export function boxForAspect(aspect: number, angle: number, W: number, H: number): CropBox {
  let w = W;
  let h = W / aspect;
  if (h > H) [w, h] = [H * aspect, H];
  return fitInside({ c: [W / 2, H / 2], w, h, angle }, W, H);
}

export function currentBox(): CropBox {
  const o = editor.opened!;
  return toBox(readCrop(editor.doc), o.preview_width, o.preview_height);
}

export function setBox(b: CropBox) {
  const o = editor.opened!;
  writeCrop(editor.doc, fromBox(b, o.preview_width, o.preview_height));
}

// --- Drawing ------------------------------------------------------------------

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>) {
  const e = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  return e;
}

const pointsAttr = (pts: Point[]) => pts.map((p) => p.join(",")).join(" ");

type Drag = (p: Point, e: PointerEvent) => void;

export class Overlay {
  private frame: Frame | null = null;
  private drag: { move: Drag; end?: () => void } | null = null;
  private cursor: Point | null = null;

  constructor(private svg: SVGSVGElement) {
    svg.addEventListener("pointerdown", (e) => this.down(e));
    svg.addEventListener("pointermove", (e) => this.move(e));
    svg.addEventListener("pointerup", (e) => this.up(e));
    svg.addEventListener("pointercancel", (e) => this.up(e));
    svg.addEventListener("pointerleave", () => {
      this.cursor = null;
      this.draw();
    });
  }

  setFrame(frame: Frame | null) {
    this.frame = frame;
    this.draw();
  }

  private local(e: PointerEvent): Point {
    const r = this.svg.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  }

  private layerMask(): Mask | undefined {
    if (editor.tab === "crop" || editor.before || editor.selected === 0) return undefined;
    return editor.doc.layers[editor.selected]?.mask;
  }

  draw() {
    const svg = this.svg;
    svg.replaceChildren();
    const f = this.frame;
    svg.classList.toggle("brush", false);
    // Mask guides run past the photo's edge and are clipped; crop handles
    // sit on the edge and need to show in full.
    svg.classList.toggle("cropping", editor.tab === "crop");
    if (!f || !editor.opened) return;
    if (editor.tab === "crop") return this.drawCrop(f);
    const mask = this.layerMask();
    if (mask?.kind === "linear") this.drawLinear(f, mask);
    else if (mask?.kind === "radial") this.drawRadial(f, mask);
    else if (mask?.kind === "brush") this.drawBrush(f);
  }

  private handle(p: Point, onDrag: Drag, cls = "handle") {
    const h = el("circle", { cx: p[0], cy: p[1], r: HANDLE, class: cls });
    h.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      this.startDrag(e, onDrag);
    });
    this.svg.append(h);
  }

  private startDrag(e: PointerEvent, move: Drag, end?: () => void) {
    this.svg.setPointerCapture(e.pointerId);
    this.drag = { move, end };
  }

  private down(e: PointerEvent) {
    if (!this.frame) return;
    const mask = this.layerMask();
    if (mask?.kind === "brush") {
      e.preventDefault();
      this.startStroke(e, mask);
    }
  }

  private move(e: PointerEvent) {
    const p = this.local(e);
    this.cursor = p;
    if (this.drag) this.drag.move(p, e);
    else if (this.layerMask()?.kind === "brush") this.draw();
  }

  private up(e: PointerEvent) {
    if (!this.drag) return;
    if (this.svg.hasPointerCapture(e.pointerId)) this.svg.releasePointerCapture(e.pointerId);
    const end = this.drag.end;
    this.drag = null;
    if (end) end();
    else commit();
  }

  // --- Crop ---

  private drawCrop(f: Frame) {
    const { W, H } = f;
    const b = currentBox();
    const pts = corners(b).map((p) => f.toView(p));
    const [vw, vh] = [W * f.scale, H * f.scale];
    // Dim everything outside the crop with an even-odd path.
    const outer = `M0,0H${vw}V${vh}H0Z`;
    const inner = `M${pts.map((p) => p.join(",")).join("L")}Z`;
    this.svg.append(el("path", { d: outer + inner, class: "dim", "fill-rule": "evenodd" }));
    // Rule-of-thirds grid inside the rectangle.
    const at = (u: number, v: number) => f.toView(add(b.c, rotate([(u - 0.5) * b.w, (v - 0.5) * b.h], rad(b.angle))));
    for (const t of [1 / 3, 2 / 3]) {
      this.svg.append(el("polyline", { points: pointsAttr([at(t, 0), at(t, 1)]), class: "grid" }));
      this.svg.append(el("polyline", { points: pointsAttr([at(0, t), at(1, t)]), class: "grid" }));
    }
    const body = el("polygon", { points: pointsAttr(pts), class: "crop-box" });
    body.addEventListener("pointerdown", (e) => {
      const start = f.toSource(this.local(e));
      const from = currentBox();
      this.startDrag(e, (p) => {
        const d = sub(f.toSource(p), start);
        setBox({ ...from, c: add(from.c, d) });
        refresh();
      });
    });
    this.svg.append(body);
    const signs: Point[] = [
      [-1, -1],
      [1, -1],
      [1, 1],
      [-1, 1],
    ];
    pts.forEach((p, i) => {
      this.handle(p, (v) => this.resizeCrop(signs[i], f.toSource(v)), "handle corner");
    });
  }

  private resizeCrop([sx, sy]: Point, pointer: Point) {
    const b = currentBox();
    const a = rad(b.angle);
    const fixed = add(b.c, rotate([(-sx * b.w) / 2, (-sy * b.h) / 2], a));
    const d = rotate(sub(pointer, fixed), -a);
    let w = Math.max(MIN_CROP, d[0] * sx);
    let h = Math.max(MIN_CROP, d[1] * sy);
    if (editor.aspect) {
      if (w / h > editor.aspect) h = w / editor.aspect;
      else w = h * editor.aspect;
    }
    const c = add(fixed, rotate([(sx * w) / 2, (sy * h) / 2], a));
    setBox({ c, w, h, angle: b.angle });
    refresh();
  }

  // --- Linear gradient ---

  private drawLinear(f: Frame, mask: Extract<Mask, { kind: "linear" }>) {
    const { W, H } = f;
    const S: Point = [mask.start[0] * W, mask.start[1] * H];
    const E: Point = [mask.end[0] * W, mask.end[1] * H];
    const dir = sub(E, S);
    const l = len(dir) || 1;
    const n: Point = [(-dir[1] / l) * (W + H), (dir[0] / l) * (W + H)];
    const line = (p: Point, cls: string) =>
      this.svg.append(el("polyline", { points: pointsAttr([f.toView(sub(p, n)), f.toView(add(p, n))]), class: cls }));
    line(S, "guide");
    line(E, "guide dashed");
    this.svg.append(el("polyline", { points: pointsAttr([f.toView(S), f.toView(E)]), class: "guide thin" }));
    const set = (start: Point, end: Point) => {
      mask.start = norm(start, W, H);
      mask.end = norm(end, W, H);
      changed();
    };
    this.handle(f.toView(S), (p) => set(f.toSource(p), E));
    this.handle(f.toView(E), (p) => set(S, f.toSource(p)), "handle hollow");
    const mid: Point = [(S[0] + E[0]) / 2, (S[1] + E[1]) / 2];
    this.handle(f.toView(mid), (p) => {
      const d = sub(f.toSource(p), mid);
      set(add(S, d), add(E, d));
    }, "handle small");
  }

  // --- Radial gradient ---

  private drawRadial(f: Frame, mask: Extract<Mask, { kind: "radial" }>) {
    const { W, H } = f;
    const c: Point = [mask.center[0] * W, mask.center[1] * H];
    const [rx, ry] = [mask.radius[0] * W, mask.radius[1] * H];
    const a = rad(mask.angle_deg);
    const ellipse = (s: number, cls: string) => {
      const pts: Point[] = [];
      for (let i = 0; i <= 72; i++) {
        const t = (i / 72) * 2 * Math.PI;
        pts.push(f.toView(add(c, rotate([rx * s * Math.cos(t), ry * s * Math.sin(t)], a))));
      }
      this.svg.append(el("polyline", { points: pointsAttr(pts), class: cls }));
    };
    ellipse(1, "guide");
    const inner = 1 - clamp01(mask.feather);
    if (inner > 0.01) ellipse(inner, "guide dashed");
    this.handle(f.toView(c), (p) => {
      mask.center = norm(f.toSource(p), W, H);
      changed();
    });
    this.handle(f.toView(add(c, rotate([rx, 0], a))), (p) => {
      const v = sub(f.toSource(p), c);
      mask.radius = [Math.max(2, len(v)) / W, mask.radius[1]];
      mask.angle_deg = Math.round((Math.atan2(v[1], v[0]) * 180) / Math.PI * 10) / 10;
      changed();
    }, "handle hollow");
    this.handle(f.toView(add(c, rotate([0, ry], a))), (p) => {
      const v = rotate(sub(f.toSource(p), c), -a);
      mask.radius = [mask.radius[0], Math.max(2, Math.abs(v[1])) / H];
      changed();
    }, "handle hollow");
  }

  // --- Brush ---

  private brushRadius(f: Frame) {
    return (editor.brush.size * Math.max(f.W, f.H) * f.scale) / 2;
  }

  private drawBrush(f: Frame) {
    this.svg.classList.toggle("brush", true);
    if (!this.cursor) return;
    const r = this.brushRadius(f);
    this.svg.append(el("circle", { cx: this.cursor[0], cy: this.cursor[1], r, class: "guide" }));
    const inner = r * (1 - editor.brush.feather);
    if (inner > 1) this.svg.append(el("circle", { cx: this.cursor[0], cy: this.cursor[1], r: inner, class: "guide dashed" }));
  }

  private startStroke(e: PointerEvent, mask: Extract<Mask, { kind: "brush" }>) {
    const f = this.frame!;
    const { size, feather, flow, erase } = editor.brush;
    const stroke = { points: [norm(f.toSource(this.local(e)), f.W, f.H)], size, feather, flow, erase };
    mask.strokes.push(stroke);
    changed();
    // A new point every quarter of the brush radius keeps strokes small.
    const spacing = (size * Math.max(f.W, f.H)) / 8;
    this.startDrag(e, (p) => {
      const s = f.toSource(p);
      const last = stroke.points[stroke.points.length - 1];
      if (len(sub(s, [last[0] * f.W, last[1] * f.H])) < spacing) {
        this.draw();
        return;
      }
      stroke.points.push(norm(s, f.W, f.H));
      changed();
    });
  }
}

function norm([x, y]: Point, W: number, H: number): Point {
  const r = (v: number) => Math.round(v * 1e4) / 1e4;
  return [r(x / W), r(y / H)];
}
