//! Texture, clarity, and dehaze: adjustments that look at a pixel's
//! neighborhood, so they run on the CPU between a layer's per-pixel steps
//! (the same way plugin operations do) rather than in the shader.
//!
//! Every radius is a fraction of the image's long edge, so the preview and a
//! full-size export look the same.

use rayon::prelude::*;

use crate::color::{linear_to_srgb, luminance, srgb_to_linear};

/// The id compiled presence steps carry in [`crate::adjust::PluginStep`].
/// The renderer runs these itself; no plugin has this id.
pub const PRESENCE_STEP: &str = "org.aperio.builtin.presence";

/// Blur radii as a share of the long edge.
const TEXTURE_RADIUS: f32 = 0.0025;
const CLARITY_RADIUS: f32 = 0.02;
const HAZE_RADIUS: f32 = 0.015;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Presence {
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
}

impl Presence {
    pub fn is_neutral(&self) -> bool {
        self.texture == 0.0 && self.clarity == 0.0 && self.dehaze == 0.0
    }
}

/// Apply `p` to `pixels` (an image `width` × `height`) in place.
pub fn apply(p: Presence, width: u32, height: u32, pixels: &mut [[f32; 4]]) {
    let (w, h) = (width as usize, height as usize);
    if w < 3 || h < 3 || p.is_neutral() {
        return;
    }
    let long = w.max(h) as f32;
    let radius = |share: f32| ((share * long).round() as usize).max(1);

    if p.dehaze != 0.0 {
        dehaze(p.dehaze.clamp(-1.0, 1.0), w, h, radius(HAZE_RADIUS), pixels);
    }
    if p.texture == 0.0 && p.clarity == 0.0 {
        return;
    }

    // Both work on brightness in sRGB-encoded values, and scale each pixel's
    // RGB together so colors don't shift.
    let encoded: Vec<f32> = pixels
        .par_iter()
        .map(|px| linear_to_srgb(luminance([px[0], px[1], px[2]])))
        .collect();
    let fine = (p.texture != 0.0).then(|| blur(&encoded, w, h, radius(TEXTURE_RADIUS)));
    let coarse = (p.clarity != 0.0).then(|| blur(&encoded, w, h, radius(CLARITY_RADIUS)));
    pixels.par_iter_mut().enumerate().for_each(|(i, px)| {
        let e = encoded[i];
        let mut target = e;
        if let Some(f) = &fine {
            // At -1, fine detail is smoothed away entirely, and no further.
            let k = if p.texture > 0.0 { 1.2 } else { 1.0 };
            target += p.texture * k * (e - f[i]);
        }
        if let Some(c) = &coarse {
            // Strongest in the midtones, so skies and deep shadows don't
            // pick up halos or crushed blacks.
            let mid = 1.0 - (2.0 * e - 1.0).powi(2);
            target += p.clarity * 0.8 * mid * (e - c[i]);
        }
        let lum = srgb_to_linear(e);
        if lum <= 0.0 || e >= 1.0 {
            return;
        }
        let k = srgb_to_linear(target.clamp(0.0, 1.0)) / lum;
        for c in &mut px[..3] {
            *c = (*c * k).max(0.0);
        }
    });
}

/// A simplified dark-channel dehaze. Haze lifts the darkest channel of
/// everything behind it toward the color of the air, so the local minimum
/// across channels (blurred, so it follows shapes rather than pixels)
/// estimates how much haze each area has. Removing it pulls those areas back
/// down and restores their contrast; a negative amount adds haze instead.
fn dehaze(amount: f32, w: usize, h: usize, r: usize, pixels: &mut [[f32; 4]]) {
    let dark: Vec<f32> = pixels
        .par_iter()
        .map(|px| px[0].min(px[1]).min(px[2]).max(0.0))
        .collect();
    let dark = blur(&dark, w, h, r);
    // The air color: the average of the haziest 0.1% of pixels.
    let mut order: Vec<usize> = (0..dark.len()).collect();
    let top = (dark.len() / 1000).max(1);
    order.select_nth_unstable_by(dark.len() - top, |a, b| dark[*a].total_cmp(&dark[*b]));
    let mut air = [0.0f32; 3];
    for &i in &order[dark.len() - top..] {
        for c in 0..3 {
            air[c] += pixels[i][c] / top as f32;
        }
    }
    let air_level = air[0].max(air[1]).max(air[2]).max(1e-3);

    pixels.par_iter_mut().enumerate().for_each(|(i, px)| {
        let haze = (dark[i] / air_level).clamp(0.0, 1.0);
        if amount > 0.0 {
            // Transmission: how much of the scene shows through the haze.
            let t = (1.0 - 0.9 * amount * haze).max(0.1);
            for c in 0..3 {
                px[c] = ((px[c] - air[c] * (1.0 - t)) / t).max(0.0);
            }
        } else {
            let k = -amount * 0.6 * (1.0 - haze * 0.5);
            for c in 0..3 {
                px[c] += (air[c] - px[c]) * k;
            }
        }
    });
}

/// Three box blurs in a row, close to a Gaussian, at a cost that doesn't
/// grow with the radius. Edges are clamped.
pub fn blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0.0; src.len()];
    for _ in 0..3 {
        box_rows(&a, &mut b, w, r);
        transpose(&b, &mut a, w, h);
        box_rows(&a, &mut b, h, r);
        transpose(&b, &mut a, h, w);
    }
    a
}

fn box_rows(src: &[f32], dst: &mut [f32], w: usize, r: usize) {
    let inv = 1.0 / (2 * r + 1) as f32;
    dst.par_chunks_mut(w)
        .zip(src.par_chunks(w))
        .for_each(|(out, row)| {
            let at = |i: isize| row[i.clamp(0, w as isize - 1) as usize];
            let r = r as isize;
            let mut sum: f32 = (-r..=r).map(at).sum();
            for (x, o) in out.iter_mut().enumerate() {
                *o = sum * inv;
                let x = x as isize;
                sum += at(x + r + 1) - at(x - r);
            }
        });
}

fn transpose(src: &[f32], dst: &mut [f32], w: usize, h: usize) {
    dst.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, o) in col.iter_mut().enumerate() {
            *o = src[y * w + x];
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripes(w: u32, h: u32) -> Vec<[f32; 4]> {
        (0..w * h)
            .map(|i| {
                let v = if ((i % w) / 4).is_multiple_of(2) {
                    0.15
                } else {
                    0.25
                };
                [v, v, v, 1.0]
            })
            .collect()
    }

    #[test]
    fn blur_keeps_flat_areas_and_averages_detail() {
        let flat = vec![0.5; 40 * 30];
        assert!(blur(&flat, 40, 30, 3)
            .iter()
            .all(|v| (v - 0.5).abs() < 1e-5));
        let px = stripes(64, 8);
        let lum: Vec<f32> = px.iter().map(|p| p[0]).collect();
        let b = blur(&lum, 64, 8, 6);
        assert!(b[32 + 8 * 4] > 0.17 && b[32 + 8 * 4] < 0.23);
    }

    #[test]
    fn neutral_does_nothing() {
        let mut px = stripes(32, 32);
        let before = px.clone();
        apply(
            Presence {
                texture: 0.0,
                clarity: 0.0,
                dehaze: 0.0,
            },
            32,
            32,
            &mut px,
        );
        assert_eq!(px, before);
    }

    /// Standard deviation of the red channel.
    fn spread(px: &[[f32; 4]]) -> f32 {
        let n = px.len() as f32;
        let mean = px.iter().map(|p| p[0]).sum::<f32>() / n;
        (px.iter().map(|p| (p[0] - mean).powi(2)).sum::<f32>() / n).sqrt()
    }

    #[test]
    fn texture_and_clarity_add_local_contrast_and_negative_removes_it() {
        for (texture, clarity) in [(1.0, 0.0), (0.0, 1.0)] {
            let mut more = stripes(400, 40);
            apply(
                Presence {
                    texture,
                    clarity,
                    dehaze: 0.0,
                },
                400,
                40,
                &mut more,
            );
            let mut less = stripes(400, 40);
            apply(
                Presence {
                    texture: -texture,
                    clarity: -clarity,
                    dehaze: 0.0,
                },
                400,
                40,
                &mut less,
            );
            let base = spread(&stripes(400, 40));
            assert!(spread(&more) > base * 1.05, "{texture} {clarity}");
            assert!(spread(&less) < base * 0.97, "{texture} {clarity}");
        }
    }

    #[test]
    fn dehaze_darkens_hazy_areas_and_negative_lightens_them() {
        // A dark subject on the left, flat haze on the right.
        let make = || -> Vec<[f32; 4]> {
            (0..64 * 16)
                .map(|i| {
                    if i % 64 < 32 {
                        [0.05, 0.08, 0.1, 1.0]
                    } else {
                        [0.6, 0.65, 0.7, 1.0]
                    }
                })
                .collect()
        };
        let mut clear = make();
        apply(
            Presence {
                texture: 0.0,
                clarity: 0.0,
                dehaze: 0.8,
            },
            64,
            16,
            &mut clear,
        );
        let mut hazy = make();
        apply(
            Presence {
                texture: 0.0,
                clarity: 0.0,
                dehaze: -0.8,
            },
            64,
            16,
            &mut hazy,
        );
        // Somewhere in the hazy half but away from the blurred edge.
        let i = 8 * 64 + 60;
        let j = 8 * 64 + 2;
        assert!(clear[j][0] <= 0.05 + 1e-4);
        assert!(hazy[j][0] > 0.05);
        assert!(clear[i][0].is_finite() && hazy[i][0].is_finite());
    }
}
