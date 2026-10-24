//! Tone curves, baked into lookup tables the shader and CPU path share.

/// Entries per curve lookup table.
pub const LUT_SIZE: usize = 256;

/// Bake control points (x, y in 0..=1) into a lookup table using a monotone
/// cubic (Fritsch–Carlson), so the curve never overshoots between points.
/// Points are sorted by x; with fewer than two, the curve is the identity.
pub fn bake(points: &[[f32; 2]]) -> Vec<f32> {
    let mut pts: Vec<[f32; 2]> = points
        .iter()
        .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
        .collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
    let lut_x = |i: usize| i as f32 / (LUT_SIZE - 1) as f32;
    if pts.len() < 2 {
        return (0..LUT_SIZE).map(lut_x).collect();
    }

    let n = pts.len();
    let delta: Vec<f32> = (0..n - 1)
        .map(|i| (pts[i + 1][1] - pts[i][1]) / (pts[i + 1][0] - pts[i][0]))
        .collect();
    let mut m = vec![0.0f32; n];
    m[0] = delta[0];
    m[n - 1] = delta[n - 2];
    for i in 1..n - 1 {
        m[i] = if delta[i - 1] * delta[i] <= 0.0 {
            0.0
        } else {
            (delta[i - 1] + delta[i]) / 2.0
        };
    }
    for i in 0..n - 1 {
        if delta[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / delta[i], m[i + 1] / delta[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * delta[i];
            m[i + 1] = t * b * delta[i];
        }
    }

    (0..LUT_SIZE)
        .map(|i| {
            let x = lut_x(i);
            if x <= pts[0][0] {
                return pts[0][1];
            }
            if x >= pts[n - 1][0] {
                return pts[n - 1][1];
            }
            let k = pts.windows(2).position(|w| x <= w[1][0]).unwrap();
            let (x0, x1) = (pts[k][0], pts[k + 1][0]);
            let h = x1 - x0;
            let t = (x - x0) / h;
            let (t2, t3) = (t * t, t * t * t);
            (2.0 * t3 - 3.0 * t2 + 1.0) * pts[k][1]
                + (t3 - 2.0 * t2 + t) * h * m[k]
                + (-2.0 * t3 + 3.0 * t2) * pts[k + 1][1]
                + (t3 - t2) * h * m[k + 1]
        })
        .map(|y| y.clamp(0.0, 1.0))
        .collect()
}

/// Look up `v` (0..=1) in a baked table with linear interpolation. Must match
/// `lut` in shader.wgsl.
pub fn lookup(lut: &[f32], v: f32) -> f32 {
    let p = v.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
    let i = (p.floor() as usize).min(LUT_SIZE - 2);
    let t = p - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_is_identity() {
        let lut = bake(&[[0.0, 0.0], [1.0, 1.0]]);
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            assert!((lookup(&lut, v) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn passes_through_points_and_stays_monotone() {
        let pts = [[0.0, 0.0], [0.25, 0.15], [0.75, 0.9], [1.0, 1.0]];
        let lut = bake(&pts);
        for p in pts {
            assert!((lookup(&lut, p[0]) - p[1]).abs() < 0.01);
        }
        assert!(lut.windows(2).all(|w| w[1] >= w[0] - 1e-6));
    }

    #[test]
    fn flat_ends_hold_outside_the_points() {
        let lut = bake(&[[0.2, 0.1], [0.8, 0.7]]);
        assert_eq!(lut[0], 0.1);
        assert_eq!(*lut.last().unwrap(), 0.7);
    }
}
