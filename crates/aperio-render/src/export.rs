use std::path::Path;

use image::{codecs::jpeg::JpegEncoder, ImageBuffer, Rgb};

use crate::color::linear_to_srgb;
use crate::{Error, LinearImage, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// 8-bit sRGB JPEG with the given quality (1 to 100).
    Jpeg { quality: u8 },
    /// 8-bit sRGB PNG.
    Png,
    /// 16-bit sRGB TIFF.
    Tiff,
}

impl ExportFormat {
    /// Pick a format from a file extension, with default settings.
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "jpg" | "jpeg" => Ok(Self::Jpeg { quality: 92 }),
            "png" => Ok(Self::Png),
            "tif" | "tiff" => Ok(Self::Tiff),
            _ => Err(Error::UnsupportedFormat(ext)),
        }
    }
}

/// Encode a linear image as sRGB and write it. Alpha is dropped.
pub fn export(img: &LinearImage, path: &Path, format: ExportFormat) -> Result<()> {
    let encode = |px: &[f32; 4], scale: f32| px.map(|v| (linear_to_srgb(v) * scale).round());
    match format {
        ExportFormat::Jpeg { quality } => {
            let buf = to_rgb8(img, encode);
            let file = std::io::BufWriter::new(std::fs::File::create(path)?);
            JpegEncoder::new_with_quality(file, quality.clamp(1, 100)).encode_image(&buf)?;
        }
        ExportFormat::Png => to_rgb8(img, encode).save(path)?,
        ExportFormat::Tiff => {
            let data = img
                .pixels
                .iter()
                .flat_map(|px| {
                    let [r, g, b, _] = encode(px, 65535.0);
                    [r as u16, g as u16, b as u16]
                })
                .collect();
            ImageBuffer::<Rgb<u16>, Vec<u16>>::from_raw(img.width, img.height, data)
                .expect("buffer matches dimensions")
                .save(path)?;
        }
    }
    Ok(())
}

fn to_rgb8(
    img: &LinearImage,
    encode: impl Fn(&[f32; 4], f32) -> [f32; 4],
) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    let data = img
        .pixels
        .iter()
        .flat_map(|px| {
            let [r, g, b, _] = encode(px, 255.0);
            [r as u8, g as u8, b as u8]
        })
        .collect();
    ImageBuffer::from_raw(img.width, img.height, data).expect("buffer matches dimensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_every_format_and_round_trips_png() {
        let img = LinearImage::new(2, 1, vec![[0.5, 0.0, 2.0, 1.0], [0.0, 1.0, 0.0, 1.0]]);
        let dir = std::env::temp_dir();
        for ext in ["jpg", "png", "tiff"] {
            let path = dir.join(format!("aperio-export-{}.{ext}", std::process::id()));
            export(&img, &path, ExportFormat::from_path(&path).unwrap()).unwrap();
            let back = image::open(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert_eq!((back.width(), back.height()), (2, 1));
            if ext == "png" {
                // Linear 0.5 encodes to sRGB 188; values above 1 clip to 255.
                assert_eq!(back.to_rgb8().get_pixel(0, 0).0, [188, 0, 255]);
            }
        }
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert!(ExportFormat::from_path(Path::new("a.webp")).is_err());
    }
}
