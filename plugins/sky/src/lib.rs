//! `org.aperio.sky`: replaces the sky with another photo.
//!
//! This is an operation, not a mask: it paints the chosen sky across the
//! whole frame, and the layer's mask (normally the AI sky mask from
//! `org.aperio.segment`) decides where it shows. Keeping the two apart means
//! the sky can also go behind a hand-painted or gradient mask.
//!
//! Sidecar layer:
//!
//! ```json
//! { "name": "New sky",
//!   "mask": { "kind": "plugin", "plugin": "org.aperio.segment", "version": "0.1.0",
//!             "params": { "class": "sky" } },
//!   "operations": [{ "op": "plugin", "plugin": "org.aperio.sky", "version": "0.1.0",
//!                    "params": { "image": "/photos/skies/sunset.jpg", "zoom": 1.2 } }] }
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use aperio_plugin::{
    ImageMut, ParamSpec, Plugin, PluginError, PluginInfo, Provides, Result, API_VERSION,
};
use serde_json::Value;

pub const ID: &str = "org.aperio.sky";

/// A decoded sky photo in linear light.
struct Sky {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
    /// How many rows from the top are sky, before any ground or trees.
    sky_rows: u32,
}

/// A decoded sky with the file and modification time it came from.
type CachedSky = (PathBuf, Option<SystemTime>, Arc<Sky>);

#[derive(Default)]
pub struct SkyReplace {
    /// The last sky photo, so moving a slider doesn't decode it again.
    cache: Mutex<Option<CachedSky>>,
}

impl SkyReplace {
    pub fn new() -> Self {
        Self::default()
    }

    fn load(&self, path: &str) -> Result<Arc<Sky>> {
        let path = PathBuf::from(path);
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock().unwrap();
        if let Some((p, m, sky)) = cache.as_ref() {
            if *p == path && *m == modified {
                return Ok(sky.clone());
            }
        }
        let img = image::open(&path)
            .map_err(|e| {
                PluginError::Failed(format!(
                    "couldn't open the sky photo {}: {e}",
                    path.display()
                ))
            })?
            .into_rgb32f();
        let (width, height) = img.dimensions();
        let pixels: Vec<[f32; 3]> = img
            .pixels()
            .map(|p| [to_linear(p[0]), to_linear(p[1]), to_linear(p[2])])
            .collect();
        let sky_rows = sky_rows(&pixels, width as usize, height as usize);
        let sky = Arc::new(Sky {
            width,
            height,
            pixels,
            sky_rows,
        });
        *cache = Some((path, modified, sky.clone()));
        Ok(sky)
    }
}

fn image_param() -> ParamSpec {
    ParamSpec::path("image", "Sky photo")
}
fn zoom() -> ParamSpec {
    ParamSpec::number("zoom", "Zoom", 1.0, 3.0, 0.01, 1.0)
}
fn shift_x() -> ParamSpec {
    ParamSpec::number("shift_x", "Horizontal position", -1.0, 1.0, 0.01, 0.0)
}
fn shift_y() -> ParamSpec {
    ParamSpec::number("shift_y", "Vertical position", -1.0, 1.0, 0.01, 0.0)
}
fn exposure() -> ParamSpec {
    ParamSpec::number("exposure", "Sky exposure", -3.0, 3.0, 0.05, 0.0)
}
fn flip() -> ParamSpec {
    ParamSpec::boolean("flip", "Flip horizontally", false)
}

impl Plugin for SkyReplace {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: ID.into(),
            name: "Sky replacement".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            license: "GPL-3.0-or-later".into(),
            operation: Some(Provides {
                label: "Replace sky".into(),
                params: vec![
                    image_param(),
                    zoom(),
                    shift_x(),
                    shift_y(),
                    exposure(),
                    flip(),
                ],
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
        // No sky chosen yet: leave the photo as it is.
        let Some(path) = image_param().read_path(params)? else {
            return Ok(());
        };
        let sky = self.load(&path)?;
        let zoom = zoom().read_number(params)? as f32;
        let (sx, sy) = (
            shift_x().read_number(params)? as f32,
            shift_y().read_number(params)? as f32,
        );
        let gain = (exposure().read_number(params)? as f32).exp2();
        let flip = flip().read_bool(params)?;

        // Fit the sky to the part of the frame it will show in: from the top
        // down to the horizon (the lowest row the mask mostly covers). Only
        // the sky part of the sky photo is used: the bottom of most sky
        // photos is ground or trees, which would show through wherever the
        // mask is partial (between branches, along the skyline).
        let (w, h) = (image.width as f32, image.height as f32);
        let region = horizon(mask, image.width as usize, image.height as usize)
            .map(|row| (row as f32 + h * HORIZON_MARGIN).min(h))
            .unwrap_or(h);
        let (kw, kh) = (sky.width as f32, sky.sky_rows as f32);
        // Cover the region (like CSS `object-fit: cover`), then zoom. The
        // shift moves the visible window across whatever spare sky is left.
        let scale = (kw / w).min(kh / region) / zoom; // sky pixels per frame pixel
        let (vw, vh) = (w * scale, region * scale);
        let x0 = (kw - vw) / 2.0 * (1.0 + sx);
        let y0 = (kh - vh) / 2.0 * (1.0 + sy);
        let old_sky = mask.and_then(|m| old_sky_color(image.pixels, m));
        for (i, px) in image.pixels.iter_mut().enumerate() {
            let fx = (i as u32 % image.width) as f32 + 0.5;
            let fy = (i as u32 / image.width) as f32 + 0.5;
            let fx = if flip { w - fx } else { fx };
            // Below the region, keep repeating the sky's lowest used row.
            let y = (y0 + fy * scale).min(kh - 0.5);
            let c = sample(&sky, x0 + fx * scale, y);
            let new = c.map(|v| v * gain);
            match (old_sky, mask) {
                (Some(old), Some(m)) => *px = edge_blend(*px, new, old, m[i]),
                _ => px[..3].copy_from_slice(&new),
            }
        }
        Ok(())
    }
}

/// The value to paint at a pixel the renderer will blend at mask weight `m`.
///
/// The AI mask is unsure along the skyline: haze, distant trees, and gaps
/// between leaves get weights well below 1 even where they look just like
/// the old sky, and show as a pale fringe against the new one. Pixels there
/// that are close in colour to the old sky are treated as more sky than the
/// mask says, by painting a value that lands on the stronger blend once the
/// renderer mixes it at `m`.
fn edge_blend(px: [f32; 4], new: [f32; 3], old: [f32; 3], m: f32) -> [f32; 4] {
    if m <= 0.02 || m >= 1.0 {
        return [new[0], new[1], new[2], px[3]];
    }
    let dist = (0..3).map(|c| (px[c] - old[c]).powi(2)).sum::<f32>().sqrt();
    let size = old.iter().map(|v| v * v).sum::<f32>().sqrt() + 1e-3;
    let like_sky = 1.0 - smoothstep(0.1, 0.35, dist / size);
    let r = m.max(like_sky) / m;
    let mut out = px;
    for c in 0..3 {
        out[c] = (px[c] + r * (new[c] - px[c])).max(0.0);
    }
    out
}

/// The average colour of the photo where the mask is (almost) fully on, or
/// `None` if there's no such area.
fn old_sky_color(pixels: &[[f32; 4]], mask: &[f32]) -> Option<[f32; 3]> {
    if mask.len() != pixels.len() {
        return None;
    }
    let (mut sum, mut n) = ([0.0f64; 3], 0usize);
    for (p, &m) in pixels.iter().zip(mask) {
        if m > 0.95 {
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
            n += 1;
        }
    }
    (n > 0).then(|| sum.map(|s| (s / n as f64) as f32))
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The number of rows from the top of a sky photo before the ground starts:
/// the first row whose darkest tenth is much darker than in the top rows,
/// as happens when a skyline of trees, hills, or buildings comes in. At
/// least a third of the photo is always used.
fn sky_rows(pixels: &[[f32; 3]], width: usize, height: usize) -> u32 {
    let dark_tenth = |y: usize| {
        let mut row: Vec<f32> = pixels[y * width..(y + 1) * width]
            .iter()
            .step_by(4)
            .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
            .collect();
        let k = row.len() / 10;
        *row.select_nth_unstable_by(k, f32::total_cmp).1
    };
    let top = (height / 10).max(1);
    let mut reference: Vec<f32> = (0..top).map(dark_tenth).collect();
    let mid = reference.len() / 2;
    let reference = *reference.select_nth_unstable_by(mid, f32::total_cmp).1;
    let start = height / 3;
    let ground = (start..height).find(|&y| dark_tenth(y) < reference * 0.15);
    // Stop a little above the skyline so its haze isn't used either.
    ground.map_or(height, |y| y.saturating_sub(height / 50).max(start)) as u32
}
/// How far below the horizon (as a share of the frame height) the sky is
/// fitted to, so the skyline itself is covered by real sky.
const HORIZON_MARGIN: f32 = 0.05;

/// The row below the lowest row in which the mask is over half on for at
/// least 2% of the pixels, or `None` with no mask (or an empty one).
fn horizon(mask: Option<&[f32]>, width: usize, height: usize) -> Option<usize> {
    let mask = mask.filter(|m| m.len() == width * height)?;
    let needed = (width / 50).max(1);
    (0..height)
        .rev()
        .find(|&y| {
            mask[y * width..(y + 1) * width]
                .iter()
                .filter(|&&m| m > 0.5)
                .count()
                >= needed
        })
        .map(|y| y + 1)
}

/// Bilinear sample at pixel coordinates (pixel centers at +0.5), clamped.
fn sample(sky: &Sky, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (sky.width as i64, sky.height as i64);
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: i64, y: i64| sky.pixels[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
    let (a, b, c, d) = (
        at(x0, y0),
        at(x0 + 1, y0),
        at(x0, y0 + 1),
        at(x0 + 1, y0 + 1),
    );
    std::array::from_fn(|i| {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        top + (bottom - top) * ty
    })
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 4x2 sky: left half red, right half blue.
    fn write_sky() -> PathBuf {
        let path = std::env::temp_dir().join(format!("aperio-sky-test-{}.png", std::process::id()));
        let img = image::RgbImage::from_fn(4, 2, |x, _| {
            if x < 2 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        });
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn paints_the_sky_across_the_frame() {
        let path = write_sky();
        let mut px = vec![[0.5, 0.5, 0.5, 0.7]; 8 * 4];
        let img = ImageMut {
            width: 8,
            height: 4,
            pixels: &mut px,
        };
        SkyReplace::new()
            .apply(&json!({"image": path.to_str().unwrap()}), img)
            .unwrap();
        assert_eq!(px[0], [1.0, 0.0, 0.0, 0.7]);
        assert_eq!(px[7], [0.0, 0.0, 1.0, 0.7]);

        let img = ImageMut {
            width: 8,
            height: 4,
            pixels: &mut px,
        };
        SkyReplace::new()
            .apply(
                &json!({"image": path.to_str().unwrap(), "flip": true, "exposure": -1}),
                img,
            )
            .unwrap();
        assert_eq!(px[0], [0.0, 0.0, 0.5, 0.7]);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn fits_the_sky_above_the_horizon() {
        // Sky photo: blue on top, dark ground in its bottom quarter.
        let path =
            std::env::temp_dir().join(format!("aperio-sky-horizon-{}.png", std::process::id()));
        image::RgbImage::from_fn(8, 8, |_, y| {
            if y < 6 {
                image::Rgb([0, 0, 255])
            } else {
                image::Rgb([10, 20, 10])
            }
        })
        .save(&path)
        .unwrap();
        // Frame: sky mask over the top 10 of 20 rows.
        let (w, h) = (20usize, 20usize);
        let mask: Vec<f32> = (0..w * h)
            .map(|i| if i / w < 10 { 1.0 } else { 0.0 })
            .collect();
        assert_eq!(horizon(Some(&mask), w, h), Some(10));
        let mut px = vec![[0.0, 0.0, 0.0, 1.0]; w * h];
        let img = ImageMut {
            width: w as u32,
            height: h as u32,
            pixels: &mut px,
        };
        SkyReplace::new()
            .apply_masked(&json!({"image": path.to_str().unwrap()}), img, Some(&mask))
            .unwrap();
        // No ground from the sky photo anywhere, even below the horizon.
        assert!(
            px.iter().all(|p| p[1] < 0.01 && p[2] > 0.99),
            "{:?}",
            px[w * 15]
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn edge_pixels_like_the_old_sky_become_new_sky() {
        let (old, new) = ([0.6, 0.6, 0.6], [0.3, 0.2, 0.5]);
        let blend = |px: [f32; 4], m: f32| {
            let p = edge_blend(px, new, old, m);
            std::array::from_fn::<f32, 3, _>(|c| m * p[c] + (1.0 - m) * px[c])
        };
        // Haze at a partial weight lands on the new sky...
        let haze = blend([0.62, 0.6, 0.58, 1.0], 0.7);
        assert!(
            haze.iter().zip(new).all(|(a, b)| (a - b).abs() < 1e-5),
            "{haze:?}"
        );
        // ...but a leaf is blended as the mask says.
        let leaf = blend([0.05, 0.2, 0.02, 1.0], 0.7);
        assert!((leaf[1] - (0.7 * 0.2 + 0.3 * 0.2)).abs() < 1e-5, "{leaf:?}");
    }

    #[test]
    fn does_nothing_until_a_sky_is_chosen_and_reports_bad_files() {
        let mut px = vec![[0.5, 0.5, 0.5, 1.0]; 4];
        let img = ImageMut {
            width: 2,
            height: 2,
            pixels: &mut px,
        };
        SkyReplace::new().apply(&json!({}), img).unwrap();
        assert_eq!(px[0], [0.5, 0.5, 0.5, 1.0]);
        let img = ImageMut {
            width: 2,
            height: 2,
            pixels: &mut px,
        };
        assert!(SkyReplace::new()
            .apply(&json!({"image": "/no/such/sky.jpg"}), img)
            .is_err());
    }
}
