//! Fast thumbnails and camera details for browsing a folder, without the
//! full decode that editing needs.

use std::path::Path;

use image::{DynamicImage, ImageDecoder, RgbImage};
use rawler::decoders::RawDecodeParams;
use rawler::formats::tiff::Rational;
use rawler::rawsource::RawSource;

use crate::decode::is_raster;
use crate::{Error, Result};

/// An sRGB thumbnail whose longer edge is at most `edge`, upright.
///
/// RAW files use the camera's embedded preview, so this shows the photo as
/// shot rather than as Aperio develops it. Raster files are decoded and
/// shrunk.
pub fn thumbnail(path: &Path, edge: u32) -> Result<RgbImage> {
    let img = if is_raster(path) {
        let mut decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut img = DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        img
    } else {
        let params = RawDecodeParams::default();
        let mut img = rawler::analyze::extract_preview_pixels(path, &params)
            .map_err(|e| Error::Raw(e.to_string()))?;
        if let Some(o) = raw_meta(path)
            .ok()
            .and_then(|m| m.exif.orientation)
            .and_then(|o| image::metadata::Orientation::from_exif(o as u8))
        {
            img.apply_orientation(o);
        }
        img
    };
    Ok(img.thumbnail(edge, edge).into_rgb8())
}

/// What a camera recorded about a photo. Fields are `None` when the file
/// doesn't say (this build reads camera details from RAW files only).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct PhotoDetails {
    /// Upright size in pixels, when known without decoding the photo.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Shutter, aperture and ISO, e.g. "1/125 · f/8 · ISO 100".
    pub exposure: Option<String>,
    /// As the camera wrote it, e.g. "2026:10:12 07:41:03".
    pub captured: Option<String>,
}

pub fn details(path: &Path) -> Result<PhotoDetails> {
    if is_raster(path) {
        let decoder = image::ImageReader::open(path)?
            .with_guessed_format()?
            .into_decoder()?;
        let (w, h) = decoder.dimensions();
        let turned = matches!(
            image::ImageReader::open(path)?
                .with_guessed_format()?
                .into_decoder()?
                .orientation()?,
            image::metadata::Orientation::Rotate90
                | image::metadata::Orientation::Rotate270
                | image::metadata::Orientation::Rotate90FlipH
                | image::metadata::Orientation::Rotate270FlipH
        );
        let (w, h) = if turned { (h, w) } else { (w, h) };
        return Ok(PhotoDetails {
            width: Some(w),
            height: Some(h),
            ..Default::default()
        });
    }
    let meta = raw_meta(path)?;
    let exif = &meta.exif;
    let camera =
        Some(format!("{} {}", meta.make, meta.model).trim().to_owned()).filter(|s| !s.is_empty());
    let lens = meta
        .lens
        .as_ref()
        .map(|l| {
            format!("{} {}", l.lens_make, l.lens_model)
                .trim()
                .to_owned()
        })
        .or_else(|| exif.lens_model.clone())
        .filter(|s| !s.is_empty());
    let exposure = [
        exif.exposure_time.map(shutter),
        exif.fnumber
            .filter(|f| f.d != 0)
            .map(|f| format!("f/{}", trim_num(f.n as f64 / f.d as f64))),
        exif.iso_speed_ratings
            .map(u32::from)
            .or(exif.iso_speed)
            .map(|iso| format!("ISO {iso}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    Ok(PhotoDetails {
        width: None,
        height: None,
        camera,
        lens,
        exposure: (!exposure.is_empty()).then(|| exposure.join(" · ")),
        captured: exif.date_time_original.clone().or(exif.create_date.clone()),
    })
}

fn raw_meta(path: &Path) -> Result<rawler::decoders::RawMetadata> {
    let source = RawSource::new(path).map_err(|e| Error::Raw(e.to_string()))?;
    let params = RawDecodeParams::default();
    rawler::get_decoder(&source)
        .and_then(|d| d.raw_metadata(&source, &params))
        .map_err(|e| Error::Raw(e.to_string()))
}

/// "1/125" for fast shutters, "0.5s" or "2s" for slow ones.
fn shutter(t: Rational) -> String {
    if t.d == 0 {
        return "?".into();
    }
    let secs = t.n as f64 / t.d as f64;
    if secs < 0.3 && secs > 0.0 {
        format!("1/{}", (1.0 / secs).round())
    } else {
        format!("{}s", trim_num(secs))
    }
}

/// One decimal place at most, without a trailing ".0".
fn trim_num(v: f64) -> String {
    let s = format!("{:.1}", v);
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_exposure_values() {
        assert_eq!(shutter(Rational { n: 1, d: 125 }), "1/125");
        assert_eq!(shutter(Rational { n: 10, d: 20 }), "0.5s");
        assert_eq!(shutter(Rational { n: 2, d: 1 }), "2s");
        assert_eq!(trim_num(8.0), "8");
        assert_eq!(trim_num(5.6), "5.6");
    }

    #[test]
    fn raster_thumbnails_fit_the_edge_and_report_their_size() {
        let path = std::env::temp_dir().join(format!("aperio-thumb-{}.png", std::process::id()));
        RgbImage::from_pixel(300, 200, image::Rgb([200, 100, 50]))
            .save(&path)
            .unwrap();
        let thumb = thumbnail(&path, 64).unwrap();
        let info = details(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(thumb.dimensions(), (64, 43));
        assert_eq!((info.width, info.height), (Some(300), Some(200)));
        assert_eq!(info.camera, None);
    }
}
