//! Output sharpening: a light unsharp mask applied after the final resize,
//! to make up for the softness that resizing and printing add.

use serde::Deserialize;

use crate::color::{linear_to_srgb, luminance, srgb_to_linear};
use crate::LinearImage;

/// What the export is for, which sets the size of the detail to boost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SharpenFor {
    /// Fine detail, for viewing on a display at 100%.
    Screen,
    /// Coarser detail, which survives ink spreading on paper.
    Print,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    Low,
    Standard,
    High,
}

impl SharpenFor {
    /// Gaussian sigma in output pixels.
    fn sigma(self) -> f32 {
        match self {
            Self::Screen => 0.7,
            Self::Print => 1.4,
        }
    }
}

impl Strength {
    fn amount(self) -> f32 {
        match self {
            Self::Low => 0.35,
            Self::Standard => 0.7,
            Self::High => 1.1,
        }
    }
}

/// Differences below this (in sRGB-encoded units) are left alone, so flat
/// areas and noise don't get grainier.
const THRESHOLD: f32 = 0.01;

/// Sharpen `img` in place. Works on brightness only, in sRGB-encoded
/// values, so edges gain contrast without color fringes.
pub fn sharpen(img: &mut LinearImage, target: SharpenFor, strength: Strength) {
    let (w, h) = (img.width as usize, img.height as usize);
    if w < 3 || h < 3 {
        return;
    }
    let lum: Vec<f32> = img
        .pixels
        .iter()
        .map(|p| luminance([p[0], p[1], p[2]]))
        .collect();
    let encoded: Vec<f32> = lum.iter().map(|&l| linear_to_srgb(l)).collect();
    let blurred = gaussian(&encoded, w, h, target.sigma());
    let amount = strength.amount();
    for (i, px) in img.pixels.iter_mut().enumerate() {
        let detail = encoded[i] - blurred[i];
        // Fade in above the threshold instead of switching on, so there's
        // no visible step between sharpened and untouched areas.
        let detail = detail.signum() * (detail.abs() - THRESHOLD).max(0.0);
        if detail == 0.0 || lum[i] <= 0.0 {
            continue;
        }
        let target = srgb_to_linear((encoded[i] + amount * detail).clamp(0.0, 1.0));
        // Brightness past white keeps its own scale rather than clipping.
        let k = if lum[i] > 1.0 { 1.0 } else { target / lum[i] };
        for c in &mut px[..3] {
            *c = (*c * k).max(0.0);
        }
    }
}

/// Separable Gaussian blur with clamped edges. Rows and columns are split
/// across threads, since exports can be tens of megapixels.
fn gaussian(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil() as isize;
    let mut kernel: Vec<f32> = (-r..=r)
        .map(|x| (-(x * x) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    kernel.iter_mut().for_each(|k| *k /= sum);

    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let rows_per = h.div_ceil(threads).max(1);

    // Horizontal pass.
    let mut tmp = vec![0.0f32; w * h];
    std::thread::scope(|s| {
        for (chunk, out) in tmp.chunks_mut(rows_per * w).enumerate() {
            let kernel = &kernel;
            s.spawn(move || {
                for (dy, row) in out.chunks_mut(w).enumerate() {
                    let y = chunk * rows_per + dy;
                    let line = &src[y * w..(y + 1) * w];
                    for (x, o) in row.iter_mut().enumerate() {
                        *o = kernel
                            .iter()
                            .enumerate()
                            .map(|(k, kv)| {
                                let sx = (x as isize + k as isize - r).clamp(0, w as isize - 1);
                                kv * line[sx as usize]
                            })
                            .sum();
                    }
                }
            });
        }
    });

    // Vertical pass.
    let mut out = vec![0.0f32; w * h];
    std::thread::scope(|s| {
        for (chunk, part) in out.chunks_mut(rows_per * w).enumerate() {
            let (kernel, tmp) = (&kernel, &tmp);
            s.spawn(move || {
                for (dy, row) in part.chunks_mut(w).enumerate() {
                    let y = (chunk * rows_per + dy) as isize;
                    for (x, o) in row.iter_mut().enumerate() {
                        *o = kernel
                            .iter()
                            .enumerate()
                            .map(|(k, kv)| {
                                let sy = (y + k as isize - r).clamp(0, h as isize - 1);
                                kv * tmp[sy as usize * w + x]
                            })
                            .sum();
                    }
                }
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(w: u32, h: u32) -> LinearImage {
        let pixels = (0..w * h)
            .map(|i| {
                let v = if i % w < w / 2 { 0.05 } else { 0.5 };
                [v, v * 0.8, v * 0.6, 1.0]
            })
            .collect();
        LinearImage::new(w, h, pixels)
    }

    #[test]
    fn flat_images_are_untouched() {
        let mut img = LinearImage::new(16, 16, vec![[0.3, 0.2, 0.1, 1.0]; 256]);
        let before = img.clone();
        sharpen(&mut img, SharpenFor::Screen, Strength::High);
        assert_eq!(img, before);
    }

    #[test]
    fn edges_gain_contrast_and_keep_their_color() {
        let mut img = edge(32, 8);
        let before = img.clone();
        sharpen(&mut img, SharpenFor::Screen, Strength::Standard);
        let at = |im: &LinearImage, x: usize| im.pixels[4 * 32 + x];
        // Dark side of the edge gets darker, bright side brighter.
        assert!(at(&img, 15)[0] < at(&before, 15)[0]);
        assert!(at(&img, 16)[0] > at(&before, 16)[0]);
        // Far from the edge nothing moves.
        assert_eq!(at(&img, 2), at(&before, 2));
        // Channels scale together, so hue holds.
        let p = at(&img, 16);
        assert!((p[1] / p[0] - 0.8).abs() < 1e-4);
    }

    #[test]
    fn print_and_higher_strength_reach_further() {
        let run = |t, s| {
            let mut img = edge(32, 8);
            sharpen(&mut img, t, s);
            img.pixels[4 * 32 + 16][0]
        };
        assert!(run(SharpenFor::Screen, Strength::High) > run(SharpenFor::Screen, Strength::Low));
        let reach = |t| {
            let mut img = edge(32, 8);
            let before = img.clone();
            sharpen(&mut img, t, Strength::Standard);
            (0..32)
                .filter(|&x| img.pixels[4 * 32 + x] != before.pixels[4 * 32 + x])
                .count()
        };
        assert!(reach(SharpenFor::Print) > reach(SharpenFor::Screen));
    }
}
