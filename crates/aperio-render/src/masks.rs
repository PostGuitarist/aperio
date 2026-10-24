//! Rasterize layer masks into per-pixel weights (0 = untouched, 1 = full
//! effect). Coordinates in the document are normalized to the uncropped,
//! oriented source; distances are measured in pixels so shapes keep their
//! proportions on non-square images.

use aperio_core::mask::BrushStroke;
use aperio_core::Mask;

/// Weights for every pixel of a `width` x `height` image, row-major.
/// Returns `None` for plugin masks, which this renderer can't generate yet.
pub fn rasterize(mask: &Mask, width: u32, height: u32) -> Option<Vec<f32>> {
    let (w, h) = (width as f32, height as f32);
    let px = |p: [f32; 2]| [p[0] * w, p[1] * h];
    let mut out = vec![0.0f32; (width * height) as usize];
    let each = |out: &mut Vec<f32>, f: &dyn Fn(f32, f32) -> f32| {
        for y in 0..height {
            for x in 0..width {
                out[(y * width + x) as usize] = f(x as f32 + 0.5, y as f32 + 0.5);
            }
        }
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
        Mask::Brush { strokes } => {
            let scale = w.max(h);
            for stroke in strokes {
                paint(&mut out, width, height, stroke, scale);
            }
        }
        Mask::Plugin { .. } => return None,
    }
    Some(out)
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

    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &pts {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let clamp_x = |v: f32| (v.max(0.0) as u32).min(width);
    let clamp_y = |v: f32| (v.max(0.0) as u32).min(height);
    for y in clamp_y(y0 - radius)..clamp_y(y1 + radius + 1.0) {
        for x in clamp_x(x0 - radius)..clamp_x(x1 + radius + 1.0) {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let d = polyline_distance(&pts, p) / radius;
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

fn polyline_distance(pts: &[[f32; 2]], p: [f32; 2]) -> f32 {
    if pts.len() == 1 {
        return ((p[0] - pts[0][0]).powi(2) + (p[1] - pts[0][1]).powi(2)).sqrt();
    }
    pts.windows(2)
        .map(|s| {
            let (a, b) = (s[0], s[1]);
            let ab = [b[0] - a[0], b[1] - a[1]];
            let len2 = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if len2 == 0.0 {
                0.0
            } else {
                (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        })
        .fold(f32::MAX, f32::min)
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
    fn plugin_masks_are_not_rasterized() {
        let m = Mask::Plugin {
            plugin: "x".into(),
            version: "1".into(),
            params: serde_json::Value::Null,
        };
        assert!(rasterize(&m, 2, 2).is_none());
    }
}
