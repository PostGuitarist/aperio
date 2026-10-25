use std::path::Path;

use image::codecs::{jpeg::JpegEncoder, png::PngEncoder};
use image::{ExtendedColorType, ImageBuffer, ImageEncoder, Rgb};

use crate::color::linear_to_srgb;
use crate::metadata::{Metadata, MetadataPolicy};
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
    export_with_metadata(img, path, format, None)
}

/// Metadata to store in an export.
pub struct ExportMetadata<'a> {
    pub source: &'a Metadata,
    pub policy: MetadataPolicy,
    /// Written as the Software tag, e.g. "Aperio 0.1.0".
    pub software: &'a str,
}

/// Like [`export`], also storing metadata. JPEG and PNG get a full EXIF
/// block; TIFF gets the plain text tags (camera, date, copyright).
pub fn export_with_metadata(
    img: &LinearImage,
    path: &Path,
    format: ExportFormat,
    metadata: Option<ExportMetadata<'_>>,
) -> Result<()> {
    let encode = |px: &[f32; 4], scale: f32| px.map(|v| (linear_to_srgb(v) * scale).round());
    let exif = metadata.as_ref().and_then(|m| {
        m.source
            .exif_for_export(m.policy, m.software, img.width, img.height)
    });
    let file = || -> Result<_> { Ok(std::io::BufWriter::new(std::fs::File::create(path)?)) };
    match format {
        ExportFormat::Jpeg { quality } => {
            let buf = to_rgb8(img, encode);
            let mut encoder = JpegEncoder::new_with_quality(file()?, quality.clamp(1, 100));
            if let Some(exif) = exif {
                encoder
                    .set_exif_metadata(exif)
                    .map_err(image::ImageError::Unsupported)?;
            }
            encoder.encode_image(&buf)?;
        }
        ExportFormat::Png => {
            let buf = to_rgb8(img, encode);
            let mut encoder = PngEncoder::new(file()?);
            if let Some(exif) = exif {
                encoder
                    .set_exif_metadata(exif)
                    .map_err(image::ImageError::Unsupported)?;
            }
            encoder.write_image(&buf, img.width, img.height, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff => {
            let data: Vec<u16> = img
                .pixels
                .iter()
                .flat_map(|px| {
                    let [r, g, b, _] = encode(px, 65535.0);
                    [r as u16, g as u16, b as u16]
                })
                .collect();
            let tags = metadata
                .map(|m| m.source.tiff_text_tags(m.policy, m.software))
                .unwrap_or_default();
            write_tiff(file()?, img.width, img.height, &data, &tags)?;
        }
    }
    Ok(())
}

fn write_tiff(
    out: impl std::io::Write + std::io::Seek,
    width: u32,
    height: u32,
    data: &[u16],
    tags: &[(u16, String)],
) -> Result<()> {
    use tiff::encoder::{colortype::RGB16, Compression, TiffEncoder};
    let tiff = |e: tiff::TiffError| Error::Io(std::io::Error::other(e.to_string()));
    let mut encoder = TiffEncoder::new(out)
        .map_err(tiff)?
        .with_compression(Compression::Deflate(Default::default()));
    let mut image = encoder.new_image::<RGB16>(width, height).map_err(tiff)?;
    for (tag, text) in tags {
        image
            .encoder()
            .write_tag(tiff::tags::Tag::from_u16_exhaustive(*tag), text.as_str())
            .map_err(tiff)?;
    }
    image.write_data(data).map_err(tiff)
}

/// The image as 8-bit sRGB, alpha dropped.
pub fn srgb8(img: &LinearImage) -> ImageBuffer<Rgb<u8>, Vec<u8>> {
    to_rgb8(img, |px, scale| {
        px.map(|v| (linear_to_srgb(v) * scale).round())
    })
}

/// Encode as an in-memory sRGB JPEG, for previews.
pub fn jpeg_bytes(img: &LinearImage, quality: u8) -> Result<Vec<u8>> {
    let buf = srgb8(img);
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).encode_image(&buf)?;
    Ok(out)
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
    fn exports_carry_camera_metadata() {
        // A JPEG with a camera model and GPS, as the "original".
        let dir = std::env::temp_dir().join(format!("aperio-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("original.jpg");
        let fields = [
            exif::Field {
                tag: exif::Tag::Model,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"Test Cam".to_vec()]),
            },
            exif::Field {
                tag: exif::Tag::GPSLatitudeRef,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"N".to_vec()]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        fields.iter().for_each(|f| writer.push_field(f));
        let mut blob = std::io::Cursor::new(Vec::new());
        writer.write(&mut blob, true).unwrap();
        let img = LinearImage::new(4, 3, vec![[0.2, 0.4, 0.6, 1.0]; 12]);
        let mut enc = JpegEncoder::new(std::fs::File::create(&original).unwrap());
        enc.set_exif_metadata(blob.into_inner()).unwrap();
        enc.encode_image(&to_rgb8(&img, |p, s| p.map(|v| linear_to_srgb(v) * s)))
            .unwrap();

        let source = Metadata::read(&original).unwrap();
        assert!(source.has_location());
        let read_back = |path: &Path| {
            exif::Reader::new()
                .read_from_container(&mut std::io::BufReader::new(
                    std::fs::File::open(path).unwrap(),
                ))
                .unwrap()
        };
        for ext in ["jpg", "png", "tif"] {
            let out = dir.join(format!("out.{ext}"));
            let meta = ExportMetadata {
                source: &source,
                policy: MetadataPolicy::NoLocation,
                software: "Aperio test",
            };
            export_with_metadata(
                &img,
                &out,
                ExportFormat::from_path(&out).unwrap(),
                Some(meta),
            )
            .unwrap();
            let exif = read_back(&out);
            let model = exif.get_field(exif::Tag::Model, exif::In::PRIMARY).unwrap();
            assert_eq!(model.display_value().to_string(), "\"Test Cam\"", "{ext}");
            assert!(
                exif.get_field(exif::Tag::GPSLatitudeRef, exif::In::PRIMARY)
                    .is_none(),
                "{ext}"
            );
            assert!(
                exif.get_field(exif::Tag::Software, exif::In::PRIMARY)
                    .is_some(),
                "{ext}"
            );
            assert_eq!(image::open(&out).unwrap().width(), 4);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jpeg_bytes_decode_back() {
        let img = LinearImage::new(3, 2, vec![[0.2, 0.4, 0.6, 1.0]; 6]);
        let bytes = jpeg_bytes(&img, 90).unwrap();
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!((back.width(), back.height()), (3, 2));
    }

    #[test]
    fn rejects_unknown_extensions() {
        assert!(ExportFormat::from_path(Path::new("a.webp")).is_err());
    }
}
