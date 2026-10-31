//! Rasterize layer masks into per-pixel weights (0 = untouched, 1 = full
//! effect). Coordinates in the document are normalized to the uncropped,
//! oriented source; distances are measured in pixels so shapes keep their
//! proportions on non-square images.

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use aperio_core::mask::{BrushStroke, MaskMode, MaskPart};
use aperio_core::Mask;
use rayon::prelude::*;

use crate::color::{linear_to_srgb, luminance};

/// Weights for every pixel of a `width` x `height` image, row-major.
/// Returns `None` for masks that need more than the image size: plugin
/// masks, range masks (see [`range`]), and combined masks.
pub fn rasterize(mask: &Mask, width: u32, height: u32) -> Option<Vec<f32>> {
    let (w, h) = (width as f32, height as f32);
    let px = |p: [f32; 2]| [p[0] * w, p[1] * h];
    let mut out = vec![0.0f32; (width * height) as usize];
    let each = |out: &mut Vec<f32>, f: &(dyn Fn(f32, f32) -> f32 + Sync)| {
        out.par_chunks_mut(width.max(1) as usize)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, m) in row.iter_mut().enumerate() {
                    *m = f(x as f32 + 0.5, y as f32 + 0.5);
                }
            });
    };
    match mask {
        Mask::Linear { start, end } => {
            let (s, e) = (px(*start), px(*end));
            let d = [e[0] - s[0], e[1] - s[1]];
            let len2 = d[0] * d[0] + d[1] * d[1];
            each(&mut out, &|x, y| {
                if len2 == 0.0 {
                    return 1.0;
                }
                let t = ((x - s[0]) * d[0] + (y - s[1]) * d[1]) / len2;
                1.0 - smoothstep(0.0, 1.0, t)
            });
        }
        Mask::Radial {
            center,
            radius,
            angle_deg,
            feather,
            invert,
        } => {
            let c = px(*center);
            let (rx, ry) = ((radius[0] * w).max(1e-3), (radius[1] * h).max(1e-3));
            let (sin, cos) = angle_deg.to_radians().sin_cos();
            let inner = 1.0 - feather.clamp(0.0, 1.0);
            each(&mut out, &|x, y| {
                let (dx, dy) = (x - c[0], y - c[1]);
                // Rotate into the ellipse's frame, then normalize by its radii.
                let (u, v) = ((dx * cos + dy * sin) / rx, (-dx * sin + dy * cos) / ry);
                let d = (u * u + v * v).sqrt();
                let m = 1.0 - smoothstep(inner, 1.0, d);
                if *invert {
                    1.0 - m
                } else {
                    m
                }
            });
        }
        Mask::Brush { strokes } => return Some(brush(strokes, width, height)),
        Mask::Plugin { .. }
        | Mask::Luminance { .. }
        | Mask::Color { .. }
        | Mask::Combine { .. } => return None,
    }
    Some(out)
}

/// Weights for a luminance or color range mask, read from `pixels`.
/// `None` for any other kind of mask.
pub fn range(mask: &Mask, pixels: &[[f32; 4]]) -> Option<Vec<f32>> {
    let weight: Box<dyn Fn([f32; 3]) -> f32 + Sync> = match *mask {
        Mask::Luminance { low, high, feather } => {
            let f = feather.max(0.0);
            Box::new(move |rgb| {
                let e = linear_to_srgb(luminance(rgb).clamp(0.0, 1.0));
                smoothstep(low - f, low, e) * (1.0 - smoothstep(high, high + f, e))
            })
        }
        Mask::Color {
            hue,
            range,
            feather,
        } => {
            let (range, feather) = (range.max(0.0), feather.max(0.0));
            Box::new(move |rgb| {
                let c = rgb.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
                let (h, s) = hue_saturation(c);
                let d = (h - hue).rem_euclid(360.0);
                let d = d.min(360.0 - d);
                // Grays have no hue to match, so they fade out with chroma.
                (1.0 - smoothstep(range, range + feather, d)) * smoothstep(0.04, 0.2, s)
            })
        }
        _ => return None,
    };
    Some(
        pixels
            .par_iter()
            .map(|px| weight([px[0], px[1], px[2]]))
            .collect(),
    )
}

/// Hue in degrees and HSV saturation of an sRGB-encoded color.
fn hue_saturation(c: [f32; 3]) -> (f32, f32) {
    let mx = c[0].max(c[1]).max(c[2]);
    let chroma = mx - c[0].min(c[1]).min(c[2]);
    if chroma <= 0.0 {
        return (0.0, 0.0);
    }
    let h = if mx == c[0] {
        ((c[1] - c[2]) / chroma).rem_euclid(6.0)
    } else if mx == c[1] {
        (c[2] - c[0]) / chroma + 2.0
    } else {
        (c[0] - c[1]) / chroma + 4.0
    };
    (h * 60.0, chroma / mx)
}

/// Fold one part of a [`Mask::Combine`] into the weights so far. The first
/// part (when `out` is still `None`) starts the result whatever its mode.
pub fn combine(out: &mut Option<Vec<f32>>, part: &MaskPart, weights: &[f32]) {
    let w = |i: usize| {
        let v = weights[i].clamp(0.0, 1.0);
        if part.invert {
            1.0 - v
        } else {
            v
        }
    };
    match out {
        None => *out = Some((0..weights.len()).map(w).collect()),
        Some(m) => m.par_iter_mut().enumerate().for_each(|(i, m)| {
            let p = w(i);
            *m = match part.mode {
                MaskMode::Add => *m + p - *m * p,
                MaskMode::Subtract => *m * (1.0 - p),
                MaskMode::Intersect => *m * p,
            };
        }),
    }
}

fn brush(strokes: &[BrushStroke], width: u32, height: u32) -> Vec<f32> {
    let mut out = vec![0.0f32; (width * height) as usize];
    for stroke in strokes {
        paint(&mut out, width, height, stroke, brush_scale(width, height));
    }
    out
}

/// Brush sizes are a fraction of the image's longer edge.
fn brush_scale(width: u32, height: u32) -> f32 {
    width.max(height) as f32
}

/// Rasterized masks kept between renders: about 24 preview-sized masks, or
/// a couple at full size for export.
const CACHE_BYTES: usize = 256 << 20;

/// Recently rasterized masks, so a render only redraws the masks that
/// changed. While a brush stroke is being painted, only that stroke is
/// drawn, on top of the strokes before it.
#[derive(Default)]
pub struct MaskCache {
    entries: Mutex<VecDeque<(u64, Arc<Vec<f32>>)>>,
}

impl MaskCache {
    /// Like [`rasterize`], reusing earlier work where it can.
    pub fn rasterize(&self, mask: &Mask, width: u32, height: u32) -> Option<Arc<Vec<f32>>> {
        let key = match mask {
            Mask::Plugin { .. }
            | Mask::Luminance { .. }
            | Mask::Color { .. }
            | Mask::Combine { .. } => return None,
            Mask::Brush { strokes } => brush_key(strokes, width, height),
            m => shape_key(m, width, height),
        };
        if let Some(m) = self.find(key) {
            return Some(m);
        }
        let out = match mask {
            Mask::Brush { strokes } => match strokes.split_last() {
                Some((last, before)) => {
                    let base_key = brush_key(before, width, height);
                    let base = self.find(base_key).unwrap_or_else(|| {
                        let base = Arc::new(brush(before, width, height));
                        self.insert(base_key, base.clone());
                        base
                    });
                    let mut out = Vec::clone(&base);
                    paint(&mut out, width, height, last, brush_scale(width, height));
                    out
                }
                None => brush(&[], width, height),
            },
            m => rasterize(m, width, height)?,
        };
        let out = Arc::new(out);
        self.insert(key, out.clone());
        Some(out)
    }

    fn find(&self, key: u64) -> Option<Arc<Vec<f32>>> {
        let mut entries = self.entries.lock().unwrap();
        let i = entries.iter().position(|(k, _)| *k == key)?;
        // Most recently used last, so it's evicted last.
        let entry = entries.remove(i)?;
        let m = entry.1.clone();
        entries.push_back(entry);
        Some(m)
    }

    fn insert(&self, key: u64, mask: Arc<Vec<f32>>) {
        let bytes = |m: &Vec<f32>| m.len() * std::mem::size_of::<f32>();
        if bytes(&mask) > CACHE_BYTES {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        let mut total = bytes(&mask) + entries.iter().map(|(_, m)| bytes(m)).sum::<usize>();
        while total > CACHE_BYTES {
            match entries.pop_front() {
                Some((_, old)) => total -= bytes(&old),
                None => break,
            }
        }
        entries.push_back((key, mask));
    }
}

fn brush_key(strokes: &[BrushStroke], width: u32, height: u32) -> u64 {
    let mut h = DefaultHasher::new();
    ("brush", width, height, strokes.len()).hash(&mut h);
    for s in strokes {
        s.points.len().hash(&mut h);
        for v in s
            .points
            .iter()
            .flatten()
            .chain([&s.size, &s.feather, &s.flow])
        {
            v.to_bits().hash(&mut h);
        }
        s.erase.hash(&mut h);
    }
    h.finish()
}

fn shape_key(mask: &Mask, width: u32, height: u32) -> u64 {
    let mut h = DefaultHasher::new();
    let json = serde_json::to_string(mask).unwrap_or_default();
    ("shape", width, height, json).hash(&mut h);
    h.finish()
}

/// Apply one stroke: every pixel within reach gets the stroke's coverage
/// once, so overlapping dabs inside a stroke don't build up.
fn paint(out: &mut [f32], width: u32, height: u32, stroke: &BrushStroke, scale: f32) {
    if stroke.points.is_empty() {
        return;
    }
    let pts: Vec<[f32; 2]> = stroke
        .points
        .iter()
        .map(|p| [p[0] * width as f32, p[1] * height as f32])
        .collect();
    let radius = (stroke.size * scale / 2.0).max(0.5);
    let inner = 1.0 - stroke.feather.clamp(0.0, 1.0);
    let flow = stroke.flow.clamp(0.0, 1.0);

    // Pixels within reach of the points from `a` to `b`, clamped to the image.
    let reach = |a: [f32; 2], b: [f32; 2]| {
        let x = |v: f32| (v.max(0.0) as u32).min(width);
        let y = |v: f32| (v.max(0.0) as u32).min(height);
        (
            x(a[0].min(b[0]) - radius)..x(a[0].max(b[0]) + radius + 1.0),
            y(a[1].min(b[1]) - radius)..y(a[1].max(b[1]) + radius + 1.0),
        )
    };
    let (mut lo, mut hi) = (pts[0], pts[0]);
    for p in &pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let (xs, ys) = reach(lo, hi);
    if xs.is_empty() || ys.is_empty() {
        return;
    }
    // Each pixel's distance to the stroke, in radii. Every segment only
    // visits the pixels near it, so a long stroke costs about as much as its
    // painted area rather than its bounding box times its length.
    let stride = xs.len();
    let mut dist = vec![1.0f32; stride * ys.len()];
    let segments: Vec<_> = if pts.len() == 1 {
        vec![(pts[0], pts[0])]
    } else {
        pts.windows(2).map(|s| (s[0], s[1])).collect()
    };
    for (a, b) in segments {
        let (sx, sy) = reach(a, b);
        for y in sy {
            let row = (y - ys.start) as usize * stride;
            for x in sx.clone() {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let d = &mut dist[row + (x - xs.start) as usize];
                *d = d.min(segment_distance(a, b, p) / radius);
            }
        }
    }
    for y in ys.clone() {
        let row = (y - ys.start) as usize * stride;
        for x in xs.clone() {
            let d = dist[row + (x - xs.start) as usize];
            if d >= 1.0 {
                continue;
            }
            let a = flow * (1.0 - smoothstep(inner, 1.0, d));
            let m = &mut out[(y * width + x) as usize];
            *m = if stroke.erase {
                *m * (1.0 - a)
            } else {
                *m + a * (1.0 - *m)
            };
        }
    }
}

fn segment_distance(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    };
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: &[f32], w: u32, x: u32, y: u32) -> f32 {
        m[(y * w + x) as usize]
    }

    #[test]
    fn linear_goes_from_full_to_none() {
        let m = rasterize(
            &Mask::Linear {
                start: [0.0, 0.0],
                end: [0.0, 1.0],
            },
            4,
            100,
        )
        .unwrap();
        assert!(at(&m, 4, 0, 0) > 0.99);
        assert!((at(&m, 4, 0, 50) - 0.5).abs() < 0.05);
        assert!(at(&m, 4, 0, 99) < 0.01);
    }

    #[test]
    fn radial_is_full_inside_and_inverts() {
        let mask = |invert| Mask::Radial {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            angle_deg: 0.0,
            feather: 0.5,
            invert,
        };
        let m = rasterize(&mask(false), 100, 100).unwrap();
        assert_eq!(at(&m, 100, 50, 50), 1.0);
        assert_eq!(at(&m, 100, 0, 0), 0.0);
        let inv = rasterize(&mask(true), 100, 100).unwrap();
        assert_eq!(at(&inv, 100, 50, 50), 0.0);
    }

    #[test]
    fn brush_paints_then_erases() {
        let stroke = |erase| BrushStroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            size: 0.2,
            feather: 0.0,
            flow: 1.0,
            erase,
        };
        let paint = Mask::Brush {
            strokes: vec![stroke(false)],
        };
        let m = rasterize(&paint, 50, 50).unwrap();
        assert_eq!(at(&m, 50, 25, 25), 1.0);
        assert_eq!(at(&m, 50, 25, 2), 0.0);
        let erased = Mask::Brush {
            strokes: vec![stroke(false), stroke(true)],
        };
        let m = rasterize(&erased, 50, 50).unwrap();
        assert_eq!(at(&m, 50, 25, 25), 0.0);
    }

    #[test]
    fn long_strokes_match_the_distance_to_the_whole_line() {
        // A zigzag whose segments overlap: each pixel takes the nearest one.
        let stroke = BrushStroke {
            points: vec![[0.1, 0.2], [0.5, 0.8], [0.6, 0.3], [0.2, 0.5], [0.9, 0.9]],
            size: 0.15,
            feather: 0.6,
            flow: 0.8,
            erase: false,
        };
        let (w, h) = (60, 40);
        let m = rasterize(
            &Mask::Brush {
                strokes: vec![stroke.clone()],
            },
            w,
            h,
        )
        .unwrap();
        let pts: Vec<[f32; 2]> = stroke
            .points
            .iter()
            .map(|p| [p[0] * w as f32, p[1] * h as f32])
            .collect();
        let radius = stroke.size * w as f32 / 2.0;
        for y in 0..h {
            for x in 0..w {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let d = pts
                    .windows(2)
                    .map(|s| segment_distance(s[0], s[1], p))
                    .fold(f32::MAX, f32::min)
                    / radius;
                let want = if d >= 1.0 {
                    0.0
                } else {
                    0.8 * (1.0 - smoothstep(0.4, 1.0, d))
                };
                assert!((at(&m, w, x, y) - want).abs() < 1e-6, "{x},{y}");
            }
        }
    }

    #[test]
    fn cache_adds_the_stroke_being_painted_to_the_ones_before() {
        let cache = MaskCache::default();
        let mut strokes = vec![BrushStroke {
            points: vec![[0.2, 0.2], [0.8, 0.3]],
            size: 0.1,
            feather: 0.5,
            flow: 1.0,
            erase: false,
        }];
        let mut stroke = BrushStroke {
            points: vec![[0.5, 0.1]],
            size: 0.3,
            feather: 0.2,
            flow: 0.7,
            erase: true,
        };
        for p in [[0.5, 0.5], [0.4, 0.9], [0.1, 0.9]] {
            stroke.points.push(p);
            let mask = Mask::Brush {
                strokes: [strokes.clone(), vec![stroke.clone()]].concat(),
            };
            let cached = cache.rasterize(&mask, 40, 30).unwrap();
            assert_eq!(*cached, rasterize(&mask, 40, 30).unwrap());
            assert!(Arc::ptr_eq(
                &cached,
                &cache.rasterize(&mask, 40, 30).unwrap()
            ));
        }
        strokes.push(stroke);
        let gradient = Mask::Linear {
            start: [0.0, 0.0],
            end: [1.0, 1.0],
        };
        assert_eq!(
            *cache.rasterize(&gradient, 40, 30).unwrap(),
            rasterize(&gradient, 40, 30).unwrap()
        );
        // A different size is a different mask.
        assert_eq!(cache.rasterize(&gradient, 20, 30).unwrap().len(), 600);
    }

    #[test]
    fn plugin_masks_are_not_rasterized() {
        let m = Mask::Plugin {
            plugin: "x".into(),
            version: "1".into(),
            params: serde_json::Value::Null,
        };
        assert!(rasterize(&m, 2, 2).is_none());
    }

    #[test]
    fn luminance_range_selects_by_brightness() {
        let grey = |e: f32| {
            let v = crate::color::srgb_to_linear(e);
            [v, v, v, 1.0]
        };
        let m = Mask::Luminance {
            low: 0.4,
            high: 0.6,
            feather: 0.1,
        };
        let w = range(
            &m,
            &[grey(0.1), grey(0.35), grey(0.5), grey(0.65), grey(0.9)],
        )
        .unwrap();
        assert_eq!(w[0], 0.0);
        assert!(w[1] > 0.0 && w[1] < 1.0);
        assert!((w[2] - 1.0).abs() < 1e-4);
        assert!(w[3] > 0.0 && w[3] < 1.0);
        assert_eq!(w[4], 0.0);
    }

    #[test]
    fn color_range_selects_hues_and_skips_greys() {
        let m = Mask::Color {
            hue: 120.0,
            range: 20.0,
            feather: 20.0,
        };
        let px = [
            [0.05, 0.6, 0.05, 1.0], // green
            [0.05, 0.6, 0.2, 1.0],  // green-aqua, inside the feather
            [0.6, 0.05, 0.05, 1.0], // red
            [0.3, 0.3, 0.3, 1.0],   // grey
        ];
        let w = range(&m, &px).unwrap();
        assert!(w[0] > 0.99);
        assert!(w[1] > 0.0 && w[1] < w[0]);
        assert_eq!(w[2], 0.0);
        assert_eq!(w[3], 0.0);
        // The hue wraps around: 350 degrees is close to red.
        let red = Mask::Color {
            hue: 350.0,
            range: 20.0,
            feather: 10.0,
        };
        assert!(range(&red, &px).unwrap()[2] > 0.99);
    }

    #[test]
    fn combine_adds_subtracts_and_intersects() {
        let part = |mode, invert| MaskPart {
            mode,
            invert,
            mask: Mask::Linear {
                start: [0.0, 0.0],
                end: [1.0, 0.0],
            },
        };
        let a = [1.0, 0.5, 0.0];
        let b = [0.0, 0.5, 1.0];
        let run = |mode, invert| {
            let mut out = None;
            combine(&mut out, &part(MaskMode::Subtract, false), &a);
            combine(&mut out, &part(mode, invert), &b);
            out.unwrap()
        };
        assert_eq!(run(MaskMode::Add, false), vec![1.0, 0.75, 1.0]);
        assert_eq!(run(MaskMode::Subtract, false), vec![1.0, 0.25, 0.0]);
        assert_eq!(run(MaskMode::Intersect, false), vec![0.0, 0.25, 0.0]);
        // Inverting `b` gives [1, 0.5, 0].
        assert_eq!(run(MaskMode::Intersect, true), vec![1.0, 0.25, 0.0]);
    }
}
