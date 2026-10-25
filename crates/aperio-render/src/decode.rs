use std::path::Path;

use rawler::imgop::develop::{Intermediate, ProcessingStep, RawDevelop};

use crate::color::srgb_to_linear;
use crate::heif;
use crate::{Error, LinearImage, Result};

const RASTER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];

/// Decode an original into linear RGBA.
///
/// JPEG, PNG, and TIFF go through the `image` crate and are linearized from
/// sRGB. HEIC/HEIF photos use the operating system's decoder (see
/// [`crate::heif`]). Anything else is tried as a camera RAW file with rawler, which
/// demosaics, applies the as-shot white balance and the camera color matrix,
/// and returns linear sRGB-primaries data.
pub fn decode(path: &Path) -> Result<LinearImage> {
    if is_raster(path) {
        decode_raster(path)
    } else if heif::is_heif(path) {
        heif::decode(path)
    } else {
        decode_raw(path)
    }
}

/// Whether `path` is a JPEG, PNG or TIFF by its extension. Anything that
/// isn't this or HEIF is treated as a camera RAW file.
pub fn is_raster(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    RASTER_EXTENSIONS.contains(&ext.as_str())
}

fn decode_raster(path: &Path) -> Result<LinearImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    let img = img.into_rgba32f();
    let (width, height) = img.dimensions();
    let pixels = img
        .pixels()
        .map(|p| {
            [
                srgb_to_linear(p[0]),
                srgb_to_linear(p[1]),
                srgb_to_linear(p[2]),
                p[3],
            ]
        })
        .collect();
    Ok(LinearImage::new(width, height, pixels))
}

fn decode_raw(path: &Path) -> Result<LinearImage> {
    let raw = rawler::decode_file(path).map_err(|e| Error::Raw(e.to_string()))?;
    // rawler's default pipeline minus the final sRGB gamma, so we stay linear.
    let develop = RawDevelop::new_with(&[
        ProcessingStep::Rescale,
        ProcessingStep::Demosaic,
        ProcessingStep::FujiRotate,
        ProcessingStep::CropActiveArea,
        ProcessingStep::WhiteBalance,
        ProcessingStep::Calibrate,
        ProcessingStep::CropDefault,
    ]);
    let developed = develop
        .develop_intermediate(&raw)
        .map_err(|e| Error::Raw(e.to_string()))?;
    let (width, height, pixels) = match developed {
        Intermediate::ThreeColor(px) => (
            px.width,
            px.height,
            px.data.iter().map(|&[r, g, b]| [r, g, b, 1.0]).collect(),
        ),
        Intermediate::Monochrome(px) => (
            px.width,
            px.height,
            px.data.iter().map(|&v| [v, v, v, 1.0]).collect(),
        ),
        Intermediate::FourColor(_) => {
            return Err(Error::Raw(
                "four-color output was not converted to RGB".into(),
            ))
        }
    };
    let (transpose, hflip, vflip) = raw.orientation.to_flips();
    Ok(LinearImage::new(width as u32, height as u32, pixels).orient(transpose, hflip, vflip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_linearizes_png() {
        let path = std::env::temp_dir().join(format!("aperio-decode-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 188, 0, 255]))
            .save(&path)
            .unwrap();
        let img = decode(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((img.width, img.height), (2, 1));
        let [r, g, b, a] = img.pixels[0];
        assert_eq!((r, b, a), (1.0, 0.0, 1.0));
        // sRGB 188 is about 0.5 in linear light.
        assert!((g - 0.5).abs() < 0.01, "{g}");
    }

    #[test]
    fn unknown_files_report_a_raw_error() {
        let path = std::env::temp_dir().join(format!("aperio-decode-{}.xyz", std::process::id()));
        std::fs::write(&path, b"not a photo").unwrap();
        let err = decode(&path).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(err, Error::Raw(_)));
    }
}
