//! `org.aperio.denoise`: AI noise reduction.
//!
//! A source step: it runs once on the full-size photo, before any layer, and
//! the preview, masks, and export all start from its result. The network is
//! NAFNet (Chen et al., 2022) trained on SIDD, real smartphone photos at
//! many ISO settings; see [`MODEL`]. It runs over the photo in overlapping
//! tiles, so memory stays bounded at any photo size, and the tiles are
//! feathered together so no seams show. Its output is kept, so moving the
//! sliders afterwards only remixes it with the photo.
//!
//! Sidecar step, on the base layer:
//!
//! ```json
//! { "op": "plugin", "plugin": "org.aperio.denoise", "version": "0.1.0",
//!   "params": { "amount": 0.8, "detail": 0.2 } }
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aperio_ai::ort;
use aperio_ai::ModelFile;
use aperio_plugin::{
    ImageMut, ModelInfo, ParamSpec, Plugin, PluginError, PluginInfo, Progress, Provides, Result,
    API_VERSION,
};
use ort::session::Session;
use ort::value::Tensor;
use serde_json::Value;

pub const ID: &str = "org.aperio.denoise";

/// NAFNet width 32 trained on SIDD, MIT (code, weights, and dataset),
/// exported to ONNX by `tools/export-nafnet.py` and published as a release
/// asset of the Aperio repository.
pub const MODEL: ModelFile = ModelFile {
    id: "nafnet_sidd_width32",
    name: "Denoise (NAFNet, SIDD)",
    license: "MIT",
    source: "https://github.com/megvii-research/NAFNet",
    url: "https://github.com/PostGuitarist/aperio/releases/download/models-1/nafnet_sidd_width32.onnx",
    sha256: "2cdac0e97b13bf3527f64e94d6935a8c84d3c69cb4d0e6512d8df47902b5acf1",
    size_bytes: 117_073_538,
    optional: false,
    notice: None,
};

/// The models this plugin uses.
pub const MODELS: [ModelFile; 1] = [MODEL];

/// Pixels per tile edge given to the network, and how much neighbouring
/// tiles overlap. Larger tiles are a little faster but need more memory
/// (NAFNet's activations for a 512 px tile are a few hundred MB).
const TILE: usize = 512;
const OVERLAP: usize = 32;
/// The network halves the image four times, so its input sides must be
/// multiples of 16.
const MULTIPLE: usize = 16;

fn amount() -> ParamSpec {
    ParamSpec::number("amount", "Amount", 0.0, 1.0, 0.01, 0.8)
}

/// How much of the photo's fine brightness detail (and grain) to bring
/// back. Colour noise stays removed.
fn detail() -> ParamSpec {
    ParamSpec::number("detail", "Keep detail", 0.0, 1.0, 0.01, 0.2)
}

/// Network output for one photo: the photo's fingerprint and the denoised
/// pixels, linear RGB.
type Output = (u64, Arc<Vec<[f32; 3]>>);

pub struct Denoise {
    dir: PathBuf,
    /// The loaded network and where it runs. Loading takes a second or two.
    session: Mutex<Option<(Session, &'static str)>>,
    /// The last photo's network output, so a slider move only remixes it.
    last: Mutex<Option<Output>>,
}

impl Denoise {
    /// Use models from `dir` (see [`aperio_ai::default_dir`]).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            session: Mutex::new(None),
            last: Mutex::new(None),
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.dir
    }

    /// Where the network runs ("CPU", "DirectML", "Core ML"), once loaded.
    pub fn runs_on(&self) -> Option<&'static str> {
        self.session.lock().unwrap().as_ref().map(|(_, on)| *on)
    }

    /// The network's output for `pixels`, from the last run if it was the
    /// same photo.
    fn denoised(
        &self,
        width: usize,
        height: usize,
        pixels: &[[f32; 4]],
        progress: Progress<'_>,
    ) -> Result<Arc<Vec<[f32; 3]>>> {
        let key = fingerprint(width, height, pixels);
        if let Some((k, out)) = self.last.lock().unwrap().as_ref() {
            if *k == key {
                return Ok(out.clone());
            }
        }
        if !MODEL.is_installed(&self.dir) {
            return Err(PluginError::ModelUnavailable(format!(
                "{} isn't downloaded yet (expected at {})",
                MODEL.name,
                MODEL.path(&self.dir).display()
            )));
        }
        let mut session = self.session.lock().unwrap();
        if session.is_none() {
            *session = Some(
                aperio_ai::load(&MODEL.path(&self.dir)).map_err(PluginError::ModelUnavailable)?,
            );
        }
        let (session, _) = session.as_mut().unwrap();
        let out = Arc::new(run_tiled(
            width,
            height,
            pixels,
            progress,
            |input, w, h| {
                let tensor = Tensor::from_array(([1usize, 3, h, w], input.to_vec()))
                    .map_err(|e| PluginError::Failed(e.to_string()))?;
                let outputs = session
                    .run(ort::inputs![tensor])
                    .map_err(|e| PluginError::Failed(format!("{} failed: {e}", MODEL.name)))?;
                let (_, data) = outputs[0]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| PluginError::Failed(e.to_string()))?;
                if data.len() != input.len() {
                    return Err(PluginError::Failed(format!(
                        "{} returned {} values for {}",
                        MODEL.name,
                        data.len(),
                        input.len()
                    )));
                }
                Ok(data.to_vec())
            },
        )?);
        *self.last.lock().unwrap() = Some((key, out.clone()));
        Ok(out)
    }
}

impl Plugin for Denoise {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: ID.into(),
            name: "AI denoise".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            license: "GPL-3.0-or-later".into(),
            operation: None,
            mask: None,
            source: Some(Provides {
                label: "Denoise".into(),
                params: vec![amount(), detail()],
            }),
            models: MODELS
                .iter()
                .map(|m| ModelInfo {
                    name: m.name.into(),
                    license: m.license.into(),
                    source: m.source.into(),
                })
                .collect(),
        }
    }

    fn source(&self, params: &Value, image: ImageMut<'_>, progress: Progress<'_>) -> Result<()> {
        let amount = amount().read_number(params)? as f32;
        let detail = detail().read_number(params)? as f32;
        if amount <= 0.0 {
            return Ok(());
        }
        let (w, h) = (image.width as usize, image.height as usize);
        let denoised = self.denoised(w, h, image.pixels, progress)?;
        mix(image.pixels, &denoised, amount, detail);
        progress.report(1.0)
    }
}

/// Blend the network output into the photo: `detail` brings back that
/// share of the brightness the network took out (but none of the colour
/// noise), and `amount` mixes the result with the original.
fn mix(pixels: &mut [[f32; 4]], denoised: &[[f32; 3]], amount: f32, detail: f32) {
    for (px, d) in pixels.iter_mut().zip(denoised) {
        let lost = luminance([px[0], px[1], px[2]]) - luminance(*d);
        for c in 0..3 {
            let kept = d[c] + detail * lost;
            px[c] += amount * (kept - px[c]);
        }
    }
}

/// Run `net` over the photo in overlapping tiles and feather them together.
/// `net` takes and returns one tile as NCHW sRGB in 0..1 (batch of one),
/// with sides that are multiples of [`MULTIPLE`]. Returns linear RGB.
///
/// The network only sees 0..1, so values above 1 (highlights from RAW
/// files) keep their brightness: what the network changes is added to the
/// photo, rather than the photo being replaced by its output.
fn run_tiled(
    width: usize,
    height: usize,
    pixels: &[[f32; 4]],
    progress: Progress<'_>,
    mut net: impl FnMut(&[f32], usize, usize) -> Result<Vec<f32>>,
) -> Result<Vec<[f32; 3]>> {
    let xs = tile_starts(width);
    let ys = tile_starts(height);
    let total = (xs.len() * ys.len()) as f32;
    let mut sum = vec![[0.0f32; 3]; width * height];
    let mut weight = vec![0.0f32; width * height];
    let mut done = 0.0;
    for &y0 in &ys {
        for &x0 in &xs {
            progress.report(done / total)?;
            let (tw, th) = (TILE.min(width - x0), TILE.min(height - y0));
            // Pad to the network's multiple by repeating the edge pixels.
            let (pw, ph) = (tw.next_multiple_of(MULTIPLE), th.next_multiple_of(MULTIPLE));
            let plane = pw * ph;
            let mut input = vec![0.0f32; 3 * plane];
            for y in 0..ph {
                for x in 0..pw {
                    let p = pixels[(y0 + y.min(th - 1)) * width + x0 + x.min(tw - 1)];
                    for c in 0..3 {
                        input[c * plane + y * pw + x] = to_srgb(p[c]);
                    }
                }
            }
            let output = net(&input, pw, ph)?;
            for y in 0..th {
                let wy = ramp(y, th, y0 > 0, y0 + th < height);
                for x in 0..tw {
                    let wx = ramp(x, tw, x0 > 0, x0 + tw < width);
                    let k = wx * wy;
                    let i = (y0 + y) * width + x0 + x;
                    let j = y * pw + x;
                    for c in 0..3 {
                        let before = input[c * plane + j];
                        let after = output[c * plane + j].clamp(0.0, 1.0);
                        let v = pixels[i][c] + to_linear(after) - to_linear(before);
                        sum[i][c] += k * v.max(0.0);
                    }
                    weight[i] += k;
                }
            }
            done += 1.0;
        }
    }
    Ok(sum
        .into_iter()
        .zip(weight)
        .map(|(s, w)| s.map(|v| v / w))
        .collect())
}

/// Where tiles start along an edge of `len` pixels: every `TILE - OVERLAP`
/// pixels, with the last one moved back so it ends at the edge.
fn tile_starts(len: usize) -> Vec<usize> {
    if len <= TILE {
        return vec![0];
    }
    let step = TILE - OVERLAP;
    let mut starts: Vec<usize> = (0..len - TILE).step_by(step).collect();
    starts.push(len - TILE);
    starts
}

/// Feathering weight `i` pixels into a tile `len` long: rising over the
/// overlap at an edge shared with another tile (`before` / `after`), flat
/// elsewhere. Never zero, so every pixel has some weight.
fn ramp(i: usize, len: usize, before: bool, after: bool) -> f32 {
    let overlap = OVERLAP as f32;
    let mut w = 1.0f32;
    if before {
        w = w.min((i as f32 + 0.5) / overlap);
    }
    if after {
        w = w.min(((len - i) as f32 - 0.5) / overlap);
    }
    w.clamp(1e-3, 1.0)
}

/// Identifies a photo: its size and every pixel. FNV-1a over 32-bit words.
fn fingerprint(width: usize, height: usize, pixels: &[[f32; 4]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut add = |word: u32| h = (h ^ word as u64).wrapping_mul(0x0100_0000_01b3);
    add(width as u32);
    add(height as u32);
    for px in pixels {
        for c in &px[..3] {
            add(c.to_bits());
        }
    }
    h
}

fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn to_srgb(v: f32) -> f32 {
    let c = v.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
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

    /// A photo with structure in every direction and some highlights above 1.
    fn photo(w: usize, h: usize) -> Vec<[f32; 4]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                [
                    0.5 + 0.5 * (x * 0.05).sin(),
                    (y / h as f32).powi(2),
                    if ((x + y) as usize).is_multiple_of(97) {
                        2.5
                    } else {
                        0.2
                    },
                    1.0,
                ]
            })
            .collect()
    }

    #[test]
    fn tiles_cover_the_photo_with_overlap() {
        assert_eq!(tile_starts(100), vec![0]);
        assert_eq!(tile_starts(512), vec![0]);
        assert_eq!(tile_starts(513), vec![0, 1]);
        let starts = tile_starts(2000);
        assert_eq!(*starts.last().unwrap(), 2000 - TILE);
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] <= TILE - OVERLAP);
        }
    }

    #[test]
    fn a_network_that_changes_nothing_gives_the_photo_back() {
        // Sides that aren't multiples of 16 and need several tiles.
        let (w, h) = (1100, 530);
        let px = photo(w, h);
        let mut sizes = Vec::new();
        let out = run_tiled(w, h, &px, Progress::none(), |input, tw, th| {
            assert_eq!((tw % MULTIPLE, th % MULTIPLE), (0, 0));
            sizes.push((tw, th));
            Ok(input.to_vec())
        })
        .unwrap();
        assert_eq!(sizes.len(), 3 * 2);
        for (a, b) in out.iter().zip(&px) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-4, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn network_changes_are_added_and_tiles_blend_without_seams() {
        // A "network" that darkens every tile by the same amount in sRGB:
        // the result must be the same everywhere a flat photo is, including
        // where tiles overlap.
        let (w, h) = (700, 600);
        let px = vec![[0.5, 0.5, 0.5, 1.0]; w * h];
        let out = run_tiled(w, h, &px, Progress::none(), |input, _, _| {
            Ok(input.iter().map(|v| v - 0.1).collect())
        })
        .unwrap();
        let expected = to_linear(to_srgb(0.5) - 0.1);
        assert!(out
            .iter()
            .all(|p| p.iter().all(|v| (v - expected).abs() < 1e-5)));
    }

    #[test]
    fn stops_when_cancelled() {
        let px = photo(600, 600);
        let mut calls = 0;
        let stop_after_two = |done: f32| done < 0.5;
        let result = run_tiled(
            600,
            600,
            &px,
            Progress::new(&stop_after_two),
            |input, _, _| {
                calls += 1;
                Ok(input.to_vec())
            },
        );
        assert!(matches!(result, Err(PluginError::Cancelled)));
        assert_eq!(calls, 2);
    }

    #[test]
    fn mixing_keeps_brightness_detail_but_not_colour_noise() {
        let noisy = [0.6, 0.3, 0.5, 1.0];
        let clean = [0.4, 0.4, 0.4];
        let at = |amount, detail| {
            let mut px = [noisy];
            mix(&mut px, &[clean], amount, detail);
            px[0]
        };
        assert_eq!(at(0.0, 0.0), noisy);
        assert_eq!(at(1.0, 0.0), [0.4, 0.4, 0.4, 1.0]);
        // Full detail brings back the brightness the network removed, but
        // the result stays grey.
        let kept = at(1.0, 1.0);
        let lost = luminance([0.6, 0.3, 0.5]) - 0.4;
        assert!(
            kept[..3].iter().all(|v| (v - (0.4 + lost)).abs() < 1e-6),
            "{kept:?}"
        );
    }

    #[test]
    fn reports_a_missing_model() {
        let dir = std::env::temp_dir().join(format!("aperio-denoise-{}", std::process::id()));
        let plugin = Denoise::new(&dir);
        let mut px = photo(8, 8);
        let image = ImageMut {
            width: 8,
            height: 8,
            pixels: &mut px,
        };
        let err = plugin
            .source(&json!({}), image, Progress::none())
            .unwrap_err();
        assert!(matches!(err, PluginError::ModelUnavailable(_)), "{err}");
        // Amount 0 is a no-op and doesn't need the model.
        let image = ImageMut {
            width: 8,
            height: 8,
            pixels: &mut px,
        };
        plugin
            .source(&json!({"amount": 0}), image, Progress::none())
            .unwrap();
    }

    /// Runs the real model when it's downloaded (`aperio models download
    /// nafnet_sidd_width32`): noise added to a smooth photo is mostly
    /// removed.
    #[test]
    fn removes_noise_with_the_real_model() {
        let plugin = Denoise::new(aperio_ai::default_dir());
        if !MODEL.is_installed(plugin.models_dir()) {
            eprintln!("skipped: {} isn't downloaded", MODEL.name);
            return;
        }
        let (w, h) = (256, 256);
        let clean = photo(w, h)
            .into_iter()
            .map(|p| [p[0].min(1.0), p[1], 0.2, 1.0])
            .collect::<Vec<_>>();
        // Deterministic pseudo-random noise.
        let mut seed = 1u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
        };
        let mut px: Vec<[f32; 4]> = clean
            .iter()
            .map(|p| {
                [0, 1, 2, 3].map(|c| {
                    if c == 3 {
                        1.0
                    } else {
                        (p[c] + 0.1 * noise()).max(0.0)
                    }
                })
            })
            .collect();
        let error = |px: &[[f32; 4]]| -> f32 {
            px.iter()
                .zip(&clean)
                .map(|(a, b)| (0..3).map(|c| (a[c] - b[c]).powi(2)).sum::<f32>())
                .sum::<f32>()
                / px.len() as f32
        };
        let before = error(&px);
        let image = ImageMut {
            width: w as u32,
            height: h as u32,
            pixels: &mut px,
        };
        plugin
            .source(&json!({"amount": 1, "detail": 0}), image, Progress::none())
            .unwrap();
        let after = error(&px);
        eprintln!("ran on {:?}: error {before} -> {after}", plugin.runs_on());
        assert!(after < before / 3.0, "{before} -> {after}");
    }
}
