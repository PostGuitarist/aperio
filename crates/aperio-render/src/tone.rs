//! Highlights, shadows, whites, and blacks, baked into a tone curve.
//!
//! Each control is a smooth, monotone bend of the sRGB-encoded tone scale.
//! Positive amounts use the bend directly; negative amounts use its inverse,
//! so -1 undoes +1 and the result can never fold over on itself. The four
//! bends are chained into one lookup table that runs as a luma curve, so
//! hue is kept and the GPU needs no new shader code.

use crate::curve::LUT_SIZE;

/// How far each control can move tones at full strength, in encoded units.
const BLACKS: f32 = 0.12;
const SHADOWS: f32 = 0.22;
const HIGHLIGHTS: f32 = 0.22;
const WHITES: f32 = 0.25;

/// A bump that is 0 at both ends and peaks at 1 when `x` is 1/4. Its slope
/// stays within about -2.37..=9.48, which bounds how strong a bend can be.
fn low_bump(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    256.0 / 27.0 * x * (1.0 - x).powi(3)
}

/// The bends, scaled by `amount` in 0..=1. Each is written in the direction
/// that stays increasing at full strength; highlights' bend darkens, so
/// raising highlights uses its inverse.
fn blacks(x: f32, amount: f32) -> f32 {
    x + amount * BLACKS * (1.0 - x.clamp(0.0, 1.0)).powi(3)
}

fn shadows(x: f32, amount: f32) -> f32 {
    x + amount * SHADOWS * low_bump(x)
}

fn highlights_down(x: f32, amount: f32) -> f32 {
    x - amount * HIGHLIGHTS * low_bump(1.0 - x)
}

fn whites(x: f32, amount: f32) -> f32 {
    x + amount * WHITES * x.clamp(0.0, 1.0).powi(3)
}

/// Apply bend `f` with `amount` in -1..=1: forward when positive, and its
/// inverse at the same strength when negative.
fn bend(f: fn(f32, f32) -> f32, x: f32, amount: f32) -> f32 {
    let amount = amount.clamp(-1.0, 1.0);
    if amount >= 0.0 {
        return f(x, amount);
    }
    let a = -amount;
    // Every bend is increasing on 0..=1, so bisection finds the inverse.
    // Targets below or above its range pin to the ends.
    if x <= f(0.0, a) {
        return 0.0;
    }
    if x >= f(1.0, a) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..32 {
        let mid = 0.5 * (lo + hi);
        if f(mid, a) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// True when the controls change nothing.
pub fn is_neutral(highlights: f32, shadows: f32, whites: f32, blacks: f32) -> bool {
    [highlights, shadows, whites, blacks]
        .iter()
        .all(|v| v.abs() < 1e-6)
}

/// Bake the four controls into a lookup table on sRGB-encoded values, for
/// use as a luma curve. Values can exceed 1 when whites are raised, which
/// pushes the brightest tones into clipping the way users expect.
pub fn bake(highlights_amt: f32, shadows_amt: f32, whites_amt: f32, blacks_amt: f32) -> Vec<f32> {
    (0..LUT_SIZE)
        .map(|i| {
            let x = i as f32 / (LUT_SIZE - 1) as f32;
            let x = bend(blacks, x, blacks_amt);
            let x = bend(shadows, x, shadows_amt);
            let x = bend(highlights_down, x, -highlights_amt);
            bend(whites, x, whites_amt)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::lookup;

    fn monotone(lut: &[f32]) -> bool {
        lut.windows(2).all(|w| w[1] >= w[0] - 1e-6)
    }

    #[test]
    fn neutral_is_identity() {
        let lut = bake(0.0, 0.0, 0.0, 0.0);
        for (i, v) in lut.iter().enumerate() {
            assert!((v - i as f32 / 255.0).abs() < 1e-6);
        }
    }

    #[test]
    fn every_extreme_stays_monotone() {
        for h in [-1.0, 0.0, 1.0] {
            for s in [-1.0, 0.0, 1.0] {
                for w in [-1.0, 0.0, 1.0] {
                    for b in [-1.0, 0.0, 1.0] {
                        assert!(monotone(&bake(h, s, w, b)), "{h} {s} {w} {b}");
                    }
                }
            }
        }
    }

    #[test]
    fn shadows_move_dark_tones_more_than_bright_ones() {
        let lut = bake(0.0, 1.0, 0.0, 0.0);
        let lift = |v: f32| lookup(&lut, v) - v;
        assert!(lift(0.3) > 0.15);
        assert!(lift(0.8) < 0.05);
        assert_eq!(lut[0], 0.0);
        assert!((lut[255] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn highlights_move_bright_tones_more_than_dark_ones() {
        let lut = bake(-1.0, 0.0, 0.0, 0.0);
        let drop = |v: f32| v - lookup(&lut, v);
        assert!(drop(0.7) > 0.1);
        assert!(drop(0.2) < 0.03);
    }

    #[test]
    fn whites_and_blacks_move_the_ends() {
        let lut = bake(0.0, 0.0, -1.0, 0.0);
        assert!(lut[255] < 0.9);
        assert_eq!(lut[0], 0.0);
        assert!(bake(0.0, 0.0, 1.0, 0.0)[255] > 1.2);

        let lifted = bake(0.0, 0.0, 0.0, 1.0);
        assert!(lifted[0] > 0.1);
        assert!((lifted[255] - 1.0).abs() < 1e-6);
        // Crushing blacks sends the darkest tones to zero.
        assert_eq!(bake(0.0, 0.0, 0.0, -1.0)[10], 0.0);
    }

    #[test]
    fn minus_one_undoes_plus_one() {
        let bends: [fn(f32, f32) -> f32; 4] = [blacks, shadows, highlights_down, whites];
        for f in bends {
            for i in 0..=20 {
                let x = i as f32 / 20.0;
                let back = bend(f, bend(f, x, 1.0), -1.0);
                assert!((back - x).abs() < 1e-4, "{x} -> {back}");
            }
        }
    }
}
