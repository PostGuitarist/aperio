// RGB histogram of the preview as shown, drawn behind the panel tabs.

const SAMPLE_EDGE = 256;
const BINS = 64;

const scratch = document.createElement("canvas");

export function drawHistogram(canvas: HTMLCanvasElement, img: HTMLImageElement) {
  const k = Math.min(1, SAMPLE_EDGE / Math.max(img.naturalWidth, img.naturalHeight));
  const [w, h] = [Math.max(1, Math.round(img.naturalWidth * k)), Math.max(1, Math.round(img.naturalHeight * k))];
  scratch.width = w;
  scratch.height = h;
  const sctx = scratch.getContext("2d", { willReadFrequently: true })!;
  sctx.drawImage(img, 0, 0, w, h);
  const data = sctx.getImageData(0, 0, w, h).data;
  const counts = [new Array<number>(BINS).fill(0), new Array<number>(BINS).fill(0), new Array<number>(BINS).fill(0)];
  for (let i = 0; i < data.length; i += 4) {
    for (let c = 0; c < 3; c++) counts[c][(data[i + c] * BINS) >> 8]++;
  }
  // Scale to the tallest bin, ignoring the end bins so clipping doesn't
  // flatten everything else.
  const peak = Math.max(1, ...counts.flatMap((ch) => ch.slice(1, -1)));

  const ctx = canvas.getContext("2d")!;
  const { width: cw, height: ch } = canvas;
  ctx.clearRect(0, 0, cw, ch);
  ctx.globalCompositeOperation = "lighter";
  const colors = ["rgba(230, 70, 70, 0.55)", "rgba(80, 200, 110, 0.55)", "rgba(80, 130, 240, 0.55)"];
  counts.forEach((bins, c) => {
    ctx.fillStyle = colors[c];
    ctx.beginPath();
    ctx.moveTo(0, ch);
    bins.forEach((n, i) => ctx.lineTo((i / (BINS - 1)) * cw, ch - Math.min(1, n / peak) * (ch - 4)));
    ctx.lineTo(cw, ch);
    ctx.closePath();
    ctx.fill();
  });
  ctx.globalCompositeOperation = "source-over";
}
