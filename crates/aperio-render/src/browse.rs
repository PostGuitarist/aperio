//! Fast thumbnails and camera details for browsing a folder, without the
//! full decode that editing needs.

use std::path::Path;

use image::{DynamicImage, ImageDecoder, RgbImage};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;

use crate::decode::is_raster;
use crate::metadata::{Metadata, Summary};
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
        if let Some(o) =
            raw_orientation(path).and_then(|o| image::metadata::Orientation::from_exif(o as u8))
        {
            img.apply_orientation(o);
        }
        img
    };
    Ok(img.thumbnail(edge, edge).into_rgb8())
}

/// What a camera recorded about a photo. Fields are `None` when the file
/// doesn't say.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct PhotoDetails {
    /// Upright size in pixels, when known without decoding the photo.
    pub width: Option<u32>,
    pub height: Option<u32>,
    #[serde(flatten)]
    pub summary: Summary,
}

pub fn details(path: &Path) -> Result<PhotoDetails> {
    let summary = Metadata::read(path)?.summary();
    if !is_raster(path) {
        return Ok(PhotoDetails {
            width: None,
            height: None,
            summary,
        });
    }
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let (w, h) = decoder.dimensions();
    let turned = matches!(
        decoder.orientation()?,
        image::metadata::Orientation::Rotate90
            | image::metadata::Orientation::Rotate270
            | image::metadata::Orientation::Rotate90FlipH
            | image::metadata::Orientation::Rotate270FlipH
    );
    let (w, h) = if turned { (h, w) } else { (w, h) };
    Ok(PhotoDetails {
        width: Some(w),
        height: Some(h),
        summary,
    })
}

fn raw_orientation(path: &Path) -> Option<u16> {
    let source = RawSource::new(path).ok()?;
    let params = RawDecodeParams::default();
    rawler::get_decoder(&source)
        .and_then(|d| d.raw_metadata(&source, &params))
        .ok()?
        .exif
        .orientation
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(info.summary.camera, None);
    }
}
