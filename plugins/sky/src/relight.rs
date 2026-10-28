//! `org.aperio.sky.relight`: makes the rest of the scene match a new sky.
//!
//! The sky replacement can only paint where the sky mask is, so this is a
//! separate operation for a companion layer masked to everything but the
//! sky (the AI sky mask, inverted). It reads the same sky photo and compares
//! it with the old sky (the part of the photo outside its mask), then:
//!
//! - shifts the scene's colour and brightness part of the way toward the
//!   new sky's light, more in bright areas, which are lit by the sky, than
//!   in shadows;
//! - tints water and other reflections (areas below the horizon that look
//!   like the old sky) with the new sky's colour, keeping their texture;
//! - adds a little of the new sky's haze around the horizon, so the
//!   skyline sits in the new air instead of looking cut out.
//!
//! It is colour matching driven by the masks: nothing is generated.
//!
//! Sidecar layer:
//!
//! ```json
//! { "name": "Match scene to sky",
//!   "mask": { "kind": "plugin", "plugin": "org.aperio.segment", "version": "0.1.0",
//!             "params": { "class": "sky", "invert": true } },
//!   "operations": [{ "op": "plugin", "plugin": "org.aperio.sky.relight", "version": "0.1.0",
//!                    "params": { "image": "/photos/skies/sunset.jpg", "amount": 0.5 } }] }
//! ```

use aperio_plugin::{ImageMut, ParamSpec, Plugin, PluginInfo, Provides, Result, API_VERSION};
use serde_json::Value;

use crate::{smoothstep, Sky, SkyCache};

pub const ID: &str = "org.aperio.sky.relight";

/// The furthest the scene's colour or brightness is moved, as a gain per
/// channel, so an extreme sky can't blow the scene out or turn it black.
const MAX_GAIN: f32 = 3.0;
/// How much of the brightness difference between the skies reaches the
/// scene at full amount. The sky lights the scene, but not alone.
const BRIGHTNESS_SHARE: f32 = 0.5;
/// How far from the horizon the haze reaches, as a share of the height
/// (where it has faded to about a third).
const HAZE_DEPTH: f32 = 0.05;
/// How much haze covers the horizon at full strength.
const HAZE_COVER: f32 = 0.3;

#[derive(Default)]
pub struct SkyRelight {
    cache: SkyCache,
}

impl SkyRelight {
    pub fn new() -> Self {
        Self::default()
    }
}

fn image_param() -> ParamSpec {
    ParamSpec::path("image", "Sky photo")
}
fn amount() -> ParamSpec {
    ParamSpec::number("amount", "Match lighting", 0.0, 1.0, 0.01, 0.5)
}
fn reflections() -> ParamSpec {
    ParamSpec::number("reflections", "Reflections", 0.0, 1.0, 0.01, 0.5)
}
fn haze() -> ParamSpec {
    ParamSpec::number("haze", "Horizon haze", 0.0, 1.0, 0.01, 0.3)
}

/// The light a sky gives: its average colour, and the colour near its
/// horizon (its lowest sky rows), which is what haze and reflections show.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Light {
    average: [f32; 3],
    low: [f32; 3],
}

impl Plugin for SkyRelight {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: ID.into(),
            name: "Sky relighting".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            license: "GPL-3.0-or-later".into(),
            operation: Some(Provides {
                label: "Match scene to sky".into(),
                params: vec![image_param(), amount(), reflections(), haze()],
            }),
            mask: None,
            source: None,
            models: vec![],
        }
    }

    fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()> {
        self.apply_masked(params, image, None)
    }

    fn apply_masked(
        &self,
        params: &Value,
        image: ImageMut<'_>,
        mask: Option<&[f32]>,
    ) -> Result<()> {
        let Some(path) = image_param().read_path(params)? else {
            return Ok(());
        };
        // The old sky is what the mask leaves out; without a mask there's
        // nothing to compare with.
        let Some(mask) = mask.filter(|m| m.len() == image.pixels.len()) else {
            return Ok(());
        };
        let Some(old) = old_light(image.pixels, mask, image.width as usize) else {
            return Ok(());
        };
        let sky = self.cache.load(&path)?;
        let new = new_light(&sky);
        relight(
            image,
            mask,
            old,
            new,
            Strength {
                amount: amount().read_number(params)? as f32,
                reflections: reflections().read_number(params)? as f32,
                haze: haze().read_number(params)? as f32,
            },
        );
        Ok(())
    }
}

struct Strength {
    amount: f32,
    reflections: f32,
    haze: f32,
}

fn relight(image: ImageMut<'_>, mask: &[f32], old: Light, new: Light, s: Strength) {
    let (w, h) = (image.width as usize, image.height as usize);
    let horizon = sky_bottom(mask, w, h);
    let gain = light_gain(old.average, new.average);
    let reflect = light_gain(old.average, new.low);
    let old_size = norm(old.average) + 1e-3;
    for (i, px) in image.pixels.iter_mut().enumerate() {
        let c = [px[0], px[1], px[2]];
        let y = i / w;
        // Bright areas are lit by the sky; deep shadows mostly aren't.
        let lit = 0.4 + 0.6 * smoothstep(0.02, 0.5, luminance(c));
        let k = s.amount * lit;
        let mut out: [f32; 3] = std::array::from_fn(|ch| c[ch] * (1.0 + k * (gain[ch] - 1.0)));
        if let Some(horizon) = horizon {
            if y >= horizon {
                // Reflections: below the horizon and close in colour to the
                // old sky. Their brightness pattern (ripples) is kept.
                let like_sky = 1.0 - smoothstep(0.15, 0.45, distance(c, old.average) / old_size);
                let r = s.reflections * like_sky;
                for ch in 0..3 {
                    out[ch] += r * (c[ch] * reflect[ch] - out[ch]);
                }
            }
            // Haze in a soft band around the horizon, where the scene is
            // furthest away. It fades both ways, so trees that reach up into
            // the sky get no hard line across them.
            let depth = (y as f32 - horizon as f32) / (h as f32 * HAZE_DEPTH);
            let fog = s.haze * HAZE_COVER * (-depth * depth).exp();
            for (o, low) in out.iter_mut().zip(new.low) {
                *o += fog * (low - *o);
            }
        }
        for ch in 0..3 {
            px[ch] = out[ch].max(0.0);
        }
    }
}

/// Per-channel gain that moves light of colour `from` toward `to`: all of
/// the colour difference and [`BRIGHTNESS_SHARE`] of the brightness one
/// (in stops), clamped to [`MAX_GAIN`] either way.
fn light_gain(from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
    let (lf, lt) = (luminance(from).max(1e-4), luminance(to).max(1e-4));
    let brightness = (lt / lf).powf(BRIGHTNESS_SHARE);
    std::array::from_fn(|c| {
        let tint = (to[c] / lt).max(1e-4) / (from[c] / lf).max(1e-4);
        (tint * brightness).clamp(1.0 / MAX_GAIN, MAX_GAIN)
    })
}

/// The old sky's light: the photo where the mask (everything but the sky)
/// is (almost) off. Its low colour is from the sky's bottom tenth of rows.
fn old_light(pixels: &[[f32; 4]], mask: &[f32], width: usize) -> Option<Light> {
    let sky: Vec<(usize, [f32; 3])> = pixels
        .iter()
        .zip(mask)
        .enumerate()
        .filter(|(_, (_, &m))| m < 0.05)
        .map(|(i, (p, _))| (i / width, [p[0], p[1], p[2]]))
        .collect();
    let average = mean(sky.iter().map(|(_, c)| *c))?;
    let bottom = sky.iter().map(|(y, _)| *y).max()?;
    let top = sky.iter().map(|(y, _)| *y).min()?;
    let cut = bottom - (bottom - top) / 10;
    let low = mean(sky.iter().filter(|(y, _)| *y >= cut).map(|(_, c)| *c))?;
    Some(Light { average, low })
}

/// A sky photo's light, from its sky part (above any ground or trees).
fn new_light(sky: &Sky) -> Light {
    let (w, rows) = (sky.width as usize, sky.sky_rows.max(1) as usize);
    let average = mean(sky.pixels[..w * rows].iter().copied()).unwrap_or([0.5; 3]);
    let low_start = rows - (rows / 10).max(1);
    let low = mean(sky.pixels[w * low_start..w * rows].iter().copied()).unwrap_or(average);
    Light { average, low }
}

/// The row just below the sky: the lowest row where the sky (the mask's
/// off part) covers at least 2% of the width.
fn sky_bottom(mask: &[f32], width: usize, height: usize) -> Option<usize> {
    let needed = (width / 50).max(1);
    (0..height)
        .rev()
        .find(|&y| {
            mask[y * width..(y + 1) * width]
                .iter()
                .filter(|&&m| m < 0.5)
                .count()
                >= needed
        })
        .map(|y| y + 1)
}

fn mean(colors: impl Iterator<Item = [f32; 3]>) -> Option<[f32; 3]> {
    let (mut sum, mut n) = ([0.0f64; 3], 0usize);
    for c in colors {
        for ch in 0..3 {
            sum[ch] += c[ch] as f64;
        }
        n += 1;
    }
    (n > 0).then(|| sum.map(|v| (v / n as f64) as f32))
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn norm(c: [f32; 3]) -> f32 {
    c.iter().map(|v| v * v).sum::<f32>().sqrt()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    norm(std::array::from_fn(|i| a[i] - b[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 40x40 photo: blue-grey sky over the top half, a green field below
    /// with a pond (sky-coloured) in it. Mask = everything but the sky.
    fn scene() -> (Vec<[f32; 4]>, Vec<f32>) {
        let (w, h) = (40, 40);
        let mut px = Vec::new();
        let mut mask = Vec::new();
        for i in 0..w * h {
            let (x, y) = (i % w, i / w);
            if y < 20 {
                px.push([0.4, 0.45, 0.5, 1.0]);
                mask.push(0.0);
            } else if (28..34).contains(&y) && (10..30).contains(&x) {
                px.push([0.38, 0.43, 0.49, 1.0]);
                mask.push(1.0);
            } else {
                px.push([0.1, 0.3, 0.05, 1.0]);
                mask.push(1.0);
            }
        }
        (px, mask)
    }

    fn run(px: &mut [[f32; 4]], mask: &[f32], new: Light, s: Strength) {
        let old = old_light(px, mask, 40).unwrap();
        relight(
            ImageMut {
                width: 40,
                height: 40,
                pixels: px,
            },
            mask,
            old,
            new,
            s,
        );
    }

    const FIELD: usize = 38 * 40 + 2;
    const POND: usize = 30 * 40 + 20;

    #[test]
    fn the_same_sky_leaves_the_scene_alone() {
        let (mut px, mask) = scene();
        let before = px.clone();
        let old = old_light(&px, &mask, 40).unwrap();
        let s = Strength {
            amount: 1.0,
            reflections: 1.0,
            haze: 0.0,
        };
        run(&mut px, &mask, Light { ..old }, s);
        for (a, b) in px.iter().zip(&before) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-3, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn a_warm_sky_warms_the_scene_and_tints_the_pond() {
        let (mut px, mask) = scene();
        let before = px.clone();
        let sunset = Light {
            average: [0.8, 0.45, 0.25],
            low: [0.9, 0.4, 0.15],
        };
        let s = Strength {
            amount: 1.0,
            reflections: 1.0,
            haze: 0.0,
        };
        run(&mut px, &mask, sunset, s);
        let warmth = |p: [f32; 4]| p[0] / p[2];
        assert!(warmth(px[FIELD]) > warmth(before[FIELD]) * 1.5);
        // The pond takes on the new sky's colour more than the field does.
        let gain = |i: usize| warmth(px[i]) / warmth(before[i]);
        assert!(
            gain(POND) > gain(FIELD) * 1.2,
            "{} vs {}",
            gain(POND),
            gain(FIELD)
        );
        assert!(px[POND][0] > px[POND][2] * 2.0, "{:?}", px[POND]);
    }

    #[test]
    fn haze_sits_around_the_horizon() {
        let (mut px, mask) = scene();
        let before = px.clone();
        let old = old_light(&px, &mask, 40).unwrap();
        let s = Strength {
            amount: 0.0,
            reflections: 0.0,
            haze: 1.0,
        };
        let new = Light {
            average: old.average,
            low: [1.0, 1.0, 1.0],
        };
        run(&mut px, &mask, new, s);
        let at = |y: usize| y * 40 + 2;
        assert!(px[at(20)][1] > before[at(20)][1] + 0.1, "{:?}", px[at(20)]);
        assert_eq!(px[at(39)], before[at(39)]);
    }

    #[test]
    fn gains_are_bounded() {
        let g = light_gain([0.5, 0.5, 0.5], [100.0, 0.0, 0.0]);
        assert!(
            g.iter().all(|v| (1.0 / MAX_GAIN..=MAX_GAIN).contains(v)),
            "{g:?}"
        );
        assert_eq!(
            light_gain([0.2, 0.3, 0.4], [0.2, 0.3, 0.4]),
            [1.0, 1.0, 1.0]
        );
    }
}
