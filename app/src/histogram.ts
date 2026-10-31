// What the preview looks like in numbers: the RGB + luminance histogram at
// the top of the panel, clipping warnings, and the pixel readout under the
// pointer. All of it reads the preview JPEG as shown.

const SAMPLE_EDGE = 256;
const BINS = 64;
/** Share of pixels at the ends before the clipping triangles light up. */
const CLIP_SHARE = 0.002;

const scratch = document.createElement("canvas");
/** The preview at full size, kept for the readout and clipping overlay. */
const full = document.createElement("canvas");
let fullData: ImageData | null = null;

export interface Clipped {
  shadows: boolean;
  highlights: boolean;
}

const luma = (r: number, g: number, b: number) => 0.2126 * r + 0.7152 * g + 0.0722 * b;

/** Draw the histogram of `img` and say whether either end is clipped. */
export function drawHistogram(canvas: HTMLCanvasElement, img: HTMLImageElement): Clipped {
  full.width = img.naturalWidth;
  full.height = img.naturalHeight;
  const fctx = full.getContext("2d", { willReadFrequently: true })!;
  fctx.drawImage(img, 0, 0);
  fullData = null;

  const k = Math.min(1, SAMPLE_EDGE / Math.max(img.naturalWidth, img.naturalHeight));
  const [w, h] = [Math.max(1, Math.round(img.naturalWidth * k)), Math.max(1, Math.round(img.naturalHeight * k))];
  scratch.width = w;
  scratch.height = h;
  const sctx = scratch.getContext("2d", { willReadFrequently: true })!;
  sctx.drawImage(img, 0, 0, w, h);
  const data = sctx.getImageData(0, 0, w, h).data;
  const counts = [0, 1, 2, 3].map(() => new Array<number>(BINS).fill(0));
  let low = 0;
  let high = 0;
  for (let i = 0; i < data.length; i += 4) {
    for (let c = 0; c < 3; c++) counts[c][(data[i + c] * BINS) >> 8]++;
    counts[3][(luma(data[i], data[i + 1], data[i + 2]) * BINS) >> 8]++;
    if (Math.max(data[i], data[i + 1], data[i + 2]) <= 2) low++;
    if (Math.max(data[i], data[i + 1], data[i + 2]) >= 253) high++;
  }
  // Scale to the tallest bin, ignoring the end bins so clipping doesn't
  // flatten everything else.
  const peak = Math.max(1, ...counts.slice(0, 3).flatMap((ch) => ch.slice(1, -1)));

  const ctx = canvas.getContext("2d")!;
  const { width: cw, height: ch } = canvas;
  ctx.clearRect(0, 0, cw, ch);
  ctx.strokeStyle = "rgba(255, 255, 255, 0.06)";
  ctx.lineWidth = 1;
  for (let q = 1; q < 4; q++) {
    ctx.beginPath();
    ctx.moveTo((q * cw) / 4, 0);
    ctx.lineTo((q * cw) / 4, ch);
    ctx.stroke();
  }
  const y = (n: number) => ch - Math.min(1, n / peak) * (ch - 4);
  ctx.globalCompositeOperation = "lighter";
  const colors = ["rgba(230, 70, 70, 0.55)", "rgba(80, 200, 110, 0.55)", "rgba(80, 130, 240, 0.55)"];
  counts.slice(0, 3).forEach((bins, c) => {
    ctx.fillStyle = colors[c];
    ctx.beginPath();
    ctx.moveTo(0, ch);
    bins.forEach((n, i) => ctx.lineTo((i / (BINS - 1)) * cw, y(n)));
    ctx.lineTo(cw, ch);
    ctx.closePath();
    ctx.fill();
  });
  ctx.globalCompositeOperation = "source-over";
  ctx.strokeStyle = "rgba(235, 232, 226, 0.8)";
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  counts[3].forEach((n, i) => ctx.lineTo((i / (BINS - 1)) * cw, y(n)));
  ctx.stroke();

  const total = data.length / 4;
  return { shadows: low / total > CLIP_SHARE, highlights: high / total > CLIP_SHARE };
}

function pixels(): ImageData | null {
  if (!fullData && full.width > 0) fullData = full.getContext("2d", { willReadFrequently: true })!.getImageData(0, 0, full.width, full.height);
  return fullData;
}

/** The 0..255 RGB of the preview at (u, v), each 0..1 across the image. */
export function sampleAt(u: number, v: number): [number, number, number] | null {
  const d = pixels();
  if (!d || u < 0 || v < 0 || u >= 1 || v >= 1) return null;
  const i = (Math.floor(v * d.height) * d.width + Math.floor(u * d.width)) * 4;
  return [d.data[i], d.data[i + 1], d.data[i + 2]];
}

/** Paint clipped highlights red and clipped shadows blue onto `canvas`, at
 * the preview's size. */
export function drawClipping(canvas: HTMLCanvasElement) {
  const d = pixels();
  if (!d) return;
  canvas.width = d.width;
  canvas.height = d.height;
  const ctx = canvas.getContext("2d")!;
  const out = ctx.createImageData(d.width, d.height);
  for (let i = 0; i < d.data.length; i += 4) {
    const max = Math.max(d.data[i], d.data[i + 1], d.data[i + 2]);
    const min = Math.min(d.data[i], d.data[i + 1], d.data[i + 2]);
    if (max >= 253) out.data.set([235, 60, 50, 255], i);
    else if (min <= 2 && max <= 6) out.data.set([60, 130, 255, 255], i);
  }
  ctx.putImageData(out, 0, 0);
}
