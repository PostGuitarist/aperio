//! Rasterize layer masks into per-pixel weights (0 = untouched, 1 = full
//! effect). Coordinates in the document are normalized to the uncropped,
//! oriented source; distances are measured in pixels so shapes keep their
//! proportions on non-square images.

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use aperio_core::mask::BrushStroke;
use aperio_core::Mask;
use rayon::prelude::*;

/// Weights for every pixel of a `width` x `height` image, row-major.
/// Returns `None` for plugin masks, which this renderer can't generate yet.
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
        Mask::Plugin { .. } => return None,
    }
    Some(out)
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
            Mask::Plugin { .. } => return None,
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
}
