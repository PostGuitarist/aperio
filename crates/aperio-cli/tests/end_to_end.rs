//! End-to-end smoke tests: run the `aperio` binary on a photo with a full
//! edit (adjustments, a gradient mask, an AI sky mask with sky replacement,
//! and a crop) and check the exported files, the way a release build would
//! be used.
//!
//! The AI test needs the sky model. It's skipped when the model isn't
//! downloaded, unless `APERIO_REQUIRE_MODELS` is set, as it is in CI.

use std::path::{Path, PathBuf};
use std::process::Command;

use aperio_plugin_segment::models;
use image::{Rgb, RgbImage};

/// A fresh, empty folder for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aperio-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `aperio render <input> <output>`; returns stderr. Fails the test on a
/// non-zero exit or any skipped step.
fn render(input: &Path, output: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_aperio"))
        .arg("render")
        .arg(input)
        .arg(output)
        .output()
        .expect("aperio runs");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "aperio render failed:\n{stderr}");
    assert!(!stderr.contains("skipped"), "a step was skipped:\n{stderr}");
    stderr
}

/// Mean sRGB color of a region given in fractions of the image.
fn mean(img: &RgbImage, x0: f32, y0: f32, x1: f32, y1: f32) -> [f32; 3] {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (mut sum, mut n) = ([0f32; 3], 0f32);
    for y in (y0 * h) as u32..(y1 * h) as u32 {
        for x in (x0 * w) as u32..(x1 * w) as u32 {
            let p = img.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += p[c] as f32;
            }
            n += 1.0;
        }
    }
    sum.map(|v| v / n)
}

/// A landscape: blue sky fading to pale at the horizon, over green hills
/// with a bumpy skyline and some texture.
fn landscape(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
        let skyline = 0.55 + 0.05 * (fx * 9.0).sin() + 0.03 * (fx * 23.0).cos();
        if fy < skyline {
            let t = fy / skyline;
            Rgb([
                (70.0 + 120.0 * t) as u8,
                (130.0 + 90.0 * t) as u8,
                (220.0 + 25.0 * t) as u8,
            ])
        } else {
            let grain = ((x * 7 + y * 13) % 17) as f32;
            Rgb([
                (35.0 + grain) as u8,
                (95.0 + 2.0 * grain) as u8,
                (30.0 + grain) as u8,
            ])
        }
    })
}

#[test]
fn edits_masks_replaces_the_sky_and_exports() {
    let dir = &models::default_dir();
    if !models::SKY.is_installed(dir) {
        assert!(
            std::env::var_os("APERIO_REQUIRE_MODELS").is_none(),
            "the sky model isn't downloaded in {}",
            dir.display()
        );
        eprintln!("skipped: run `aperio models download skyseg` first");
        return;
    }

    let work = scratch("sky");
    let photo = work.join("hills.jpg");
    landscape(1200, 900).save(&photo).unwrap();
    // A warm sunset sky to put in.
    let sky = work.join("sunset.png");
    RgbImage::from_fn(800, 500, |_, y| {
        let t = y as f32 / 500.0;
        Rgb([
            (230.0 + 20.0 * t) as u8,
            (110.0 + 60.0 * t) as u8,
            (60.0 + 30.0 * t) as u8,
        ])
    })
    .save(&sky)
    .unwrap();

    let sidecar = serde_json::json!({
        "format": "aperio.edit",
        "version": 1,
        "source": { "file_name": "hills.jpg", "size_bytes": std::fs::metadata(&photo).unwrap().len() },
        "layers": [
            {
                "name": "Base",
                "operations": [
                    { "op": "exposure", "ev": 0.2 },
                    { "op": "contrast", "amount": 0.1 },
                    { "op": "tone", "shadows": 0.2, "highlights": -0.1 },
                    { "op": "white_balance", "temperature_k": 6000, "tint": 0.0 },
                    { "op": "color", "saturation": 0.0, "vibrance": 0.2 },
                    { "op": "curve", "channel": "luma", "points": [[0, 0], [0.5, 0.55], [1, 1]] },
                    { "op": "crop", "x": 0.05, "y": 0.0, "width": 0.9, "height": 1.0, "angle_deg": 0.0 }
                ]
            },
            {
                "name": "Darker ground",
                "mask": { "kind": "linear", "start": [0.5, 1.0], "end": [0.5, 0.6] },
                "operations": [{ "op": "exposure", "ev": -0.3 }]
            },
            {
                "name": "Sky",
                "mask": { "kind": "plugin", "plugin": "org.aperio.segment", "version": "0.1.0",
                          "params": { "class": "sky" } },
                "operations": [{ "op": "plugin", "plugin": "org.aperio.sky", "version": "0.1.0",
                                 "params": { "image": sky.to_string_lossy() } }]
            }
        ]
    });
    std::fs::write(
        work.join("hills.jpg.aperio.json"),
        serde_json::to_string_pretty(&sidecar).unwrap(),
    )
    .unwrap();

    for ext in ["jpg", "png", "tiff"] {
        let out = work.join(format!("hills_edit.{ext}"));
        render(&photo, &out);
        let img = image::open(&out).unwrap().into_rgb8();
        // The crop keeps 90% of the width.
        assert_eq!(img.dimensions(), (1080, 900), "{ext}");
        // The sky is now the warm sunset: more red than blue.
        let top = mean(&img, 0.1, 0.05, 0.9, 0.3);
        assert!(
            top[0] > top[2] + 40.0,
            "{ext}: sky wasn't replaced: {top:?}"
        );
        // The ground is still green.
        let ground = mean(&img, 0.1, 0.8, 0.9, 0.95);
        assert!(
            ground[1] > ground[0] + 20.0 && ground[1] > ground[2] + 20.0,
            "{ext}: ground changed: {ground:?}"
        );
    }
    std::fs::remove_dir_all(&work).unwrap();
}

#[test]
fn opens_heic_and_exports_tiff() {
    let work = scratch("heic");
    let photo = work.join("p3.heic");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../aperio-render/tests/data/display-p3-64x48.heic"
        ),
        &photo,
    )
    .unwrap();
    std::fs::write(
        work.join("p3.heic.aperio.json"),
        r#"{ "format": "aperio.edit", "version": 1,
             "source": { "file_name": "p3.heic", "size_bytes": 0 },
             "layers": [{ "name": "Base", "operations": [{ "op": "exposure", "ev": 1.0 }] }] }"#,
    )
    .unwrap();
    let out = work.join("p3.tiff");
    let result = Command::new(env!("CARGO_BIN_EXE_aperio"))
        .arg("render")
        .arg(&photo)
        .arg(&out)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    // Without a system HEIC decoder (libheif on Linux, Microsoft's
    // extensions on Windows) there's nothing to test.
    if !result.status.success() && stderr.contains("needs") {
        eprintln!("skipped: {stderr}");
        return;
    }
    assert!(result.status.success(), "{stderr}");
    let img = image::open(&out).unwrap().into_rgb16();
    assert_eq!(img.dimensions(), (64, 48));
    // Mid gray, one stop brighter.
    let gray = img.get_pixel(56, 24);
    assert!(
        gray[0] > 40_000 && gray[0].abs_diff(gray[2]) < 200,
        "{gray:?}"
    );
    std::fs::remove_dir_all(&work).unwrap();
}
