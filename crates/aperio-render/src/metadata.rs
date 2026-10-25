//! Camera metadata: read from the original (EXIF in JPEG, PNG and TIFF;
//! rawler's parsed tags for RAW files), summarized for the Library, and
//! written back into exports.

use std::io::{BufReader, Cursor};
use std::path::Path;

use exif::{Context, Field, In, Rational, SRational, Tag, Value};
use rawler::decoders::RawDecodeParams;
use rawler::rawsource::RawSource;
use serde::Deserialize;

use crate::decode::is_raster;
use crate::{Error, Result};

/// What an export keeps of the original's metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataPolicy {
    /// Camera, lens, exposure, dates, copyright and GPS location.
    All,
    /// Everything except GPS location.
    #[default]
    NoLocation,
    /// Nothing but the image itself.
    None,
}

/// Tags that describe the photo and stay true after editing. Anything else
/// (orientation, sizes, offsets, thumbnails, maker notes) would be wrong or
/// broken in a new file, so it is left out.
const KEEP: &[Tag] = &[
    Tag::Make,
    Tag::Model,
    Tag::Artist,
    Tag::Copyright,
    Tag::ImageDescription,
    Tag::DateTimeOriginal,
    Tag::DateTimeDigitized,
    Tag::OffsetTime,
    Tag::OffsetTimeOriginal,
    Tag::SubSecTimeOriginal,
    Tag::ExposureTime,
    Tag::FNumber,
    Tag::ExposureProgram,
    Tag::PhotographicSensitivity,
    Tag::ExposureBiasValue,
    Tag::MaxApertureValue,
    Tag::MeteringMode,
    Tag::LightSource,
    Tag::Flash,
    Tag::FocalLength,
    Tag::FocalLengthIn35mmFilm,
    Tag::LensMake,
    Tag::LensModel,
    Tag::LensSpecification,
    Tag::WhiteBalance,
    Tag::ExposureMode,
    Tag::SceneCaptureType,
];

/// The original's metadata, as EXIF fields.
#[derive(Debug, Clone, Default)]
pub struct Metadata {
    fields: Vec<Field>,
}

/// What the Library shows about a photo.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Summary {
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Shutter, aperture and ISO, e.g. "1/125 · f/8 · ISO 100".
    pub exposure: Option<String>,
    /// As the camera wrote it, e.g. "2026:10:12 07:41:03".
    pub captured: Option<String>,
}

impl Metadata {
    /// Read `path`'s metadata. A file without any gives an empty set.
    pub fn read(path: &Path) -> Result<Self> {
        if is_raster(path) {
            let file = std::fs::File::open(path)?;
            return Ok(
                match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
                    Ok(exif) => Self {
                        fields: exif
                            .fields()
                            .filter(|f| f.ifd_num == In::PRIMARY)
                            .cloned()
                            .collect(),
                    },
                    Err(_) => Self::default(),
                },
            );
        }
        let source = RawSource::new(path).map_err(|e| Error::Raw(e.to_string()))?;
        let meta = rawler::get_decoder(&source)
            .and_then(|d| d.raw_metadata(&source, &RawDecodeParams::default()))
            .map_err(|e| Error::Raw(e.to_string()))?;
        Ok(Self::from_raw(&meta))
    }

    fn from_raw(meta: &rawler::decoders::RawMetadata) -> Self {
        let e = &meta.exif;
        let mut m = Self::default();
        let ascii = |s: &str| Value::Ascii(vec![s.trim().as_bytes().to_vec()]);
        let rational = |r: rawler::formats::tiff::Rational| {
            Value::Rational(vec![Rational {
                num: r.n,
                denom: r.d,
            }])
        };
        let mut put = |tag, value: Option<Value>| {
            if let Some(value) = value {
                m.fields.push(Field {
                    tag,
                    ifd_num: In::PRIMARY,
                    value,
                });
            }
        };
        let nonempty = |s: &str| (!s.trim().is_empty()).then(|| ascii(s));
        put(Tag::Make, nonempty(&meta.make));
        put(Tag::Model, nonempty(&meta.model));
        put(Tag::Artist, e.artist.as_deref().and_then(nonempty));
        put(Tag::Copyright, e.copyright.as_deref().and_then(nonempty));
        put(
            Tag::DateTimeOriginal,
            e.date_time_original.as_deref().and_then(nonempty),
        );
        put(
            Tag::DateTimeDigitized,
            e.create_date.as_deref().and_then(nonempty),
        );
        put(
            Tag::OffsetTimeOriginal,
            e.offset_time_original.as_deref().and_then(nonempty),
        );
        put(Tag::ExposureTime, e.exposure_time.map(rational));
        put(Tag::FNumber, e.fnumber.map(rational));
        put(
            Tag::ExposureProgram,
            e.exposure_program.map(|v| Value::Short(vec![v])),
        );
        put(
            Tag::PhotographicSensitivity,
            e.iso_speed_ratings.map(|v| Value::Short(vec![v])),
        );
        put(
            Tag::ExposureBiasValue,
            e.exposure_bias.map(|r| {
                Value::SRational(vec![SRational {
                    num: r.n,
                    denom: r.d,
                }])
            }),
        );
        put(Tag::MaxApertureValue, e.max_aperture_value.map(rational));
        put(
            Tag::MeteringMode,
            e.metering_mode.map(|v| Value::Short(vec![v])),
        );
        put(
            Tag::LightSource,
            e.light_source.map(|v| Value::Short(vec![v])),
        );
        put(Tag::Flash, e.flash.map(|v| Value::Short(vec![v])));
        put(Tag::FocalLength, e.focal_length.map(rational));
        let (lens_make, lens_model) = match &meta.lens {
            Some(l) => (Some(l.lens_make.clone()), Some(l.lens_model.clone())),
            None => (e.lens_make.clone(), e.lens_model.clone()),
        };
        put(Tag::LensMake, lens_make.as_deref().and_then(nonempty));
        put(Tag::LensModel, lens_model.as_deref().and_then(nonempty));
        put(
            Tag::WhiteBalance,
            e.white_balance.map(|v| Value::Short(vec![v])),
        );
        put(
            Tag::ExposureMode,
            e.exposure_mode.map(|v| Value::Short(vec![v])),
        );
        put(
            Tag::SceneCaptureType,
            e.scene_capture_type.map(|v| Value::Short(vec![v])),
        );
        if let Some(g) = &e.gps {
            let three = |v: [rawler::formats::tiff::Rational; 3]| {
                Value::Rational(
                    v.iter()
                        .map(|r| Rational {
                            num: r.n,
                            denom: r.d,
                        })
                        .collect(),
                )
            };
            put(
                Tag::GPSVersionID,
                g.gps_version_id.map(|v| Value::Byte(v.to_vec())),
            );
            put(
                Tag::GPSLatitudeRef,
                g.gps_latitude_ref.as_deref().and_then(nonempty),
            );
            put(Tag::GPSLatitude, g.gps_latitude.map(three));
            put(
                Tag::GPSLongitudeRef,
                g.gps_longitude_ref.as_deref().and_then(nonempty),
            );
            put(Tag::GPSLongitude, g.gps_longitude.map(three));
            put(
                Tag::GPSAltitudeRef,
                g.gps_altitude_ref.map(|v| Value::Byte(vec![v])),
            );
            put(Tag::GPSAltitude, g.gps_altitude.map(rational));
            put(Tag::GPSTimeStamp, g.gps_timestamp.map(three));
        }
        m
    }

    fn get(&self, tag: Tag) -> Option<&Value> {
        self.fields.iter().find(|f| f.tag == tag).map(|f| &f.value)
    }

    fn text(&self, tag: Tag) -> Option<String> {
        match self.get(tag)? {
            Value::Ascii(parts) => {
                let s = parts
                    .iter()
                    .map(|p| String::from_utf8_lossy(p).trim().to_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                (!s.is_empty()).then_some(s)
            }
            _ => None,
        }
    }

    fn ratio(&self, tag: Tag) -> Option<f64> {
        match self.get(tag)? {
            Value::Rational(v) => v.first().filter(|r| r.denom != 0).map(|r| r.to_f64()),
            _ => None,
        }
    }

    /// Whether the metadata includes a GPS location.
    pub fn has_location(&self) -> bool {
        self.fields.iter().any(|f| f.tag.context() == Context::Gps)
    }

    pub fn summary(&self) -> Summary {
        let make = self.text(Tag::Make);
        let model = self.text(Tag::Model);
        // Many cameras repeat the make in the model ("Canon" + "Canon EOS R5").
        let camera = match (make, model) {
            (Some(make), Some(model)) if model.to_lowercase().starts_with(&make.to_lowercase()) => {
                Some(model)
            }
            (Some(make), Some(model)) => Some(format!("{make} {model}")),
            (a, b) => a.or(b),
        };
        let lens = match (self.text(Tag::LensMake), self.text(Tag::LensModel)) {
            (Some(make), Some(model))
                if !model.to_lowercase().starts_with(&make.to_lowercase()) =>
            {
                Some(format!("{make} {model}"))
            }
            (_, model) => model,
        };
        let iso = self
            .get(Tag::PhotographicSensitivity)
            .and_then(|v| v.get_uint(0));
        let exposure = [
            self.ratio(Tag::ExposureTime).map(shutter),
            self.ratio(Tag::FNumber)
                .map(|f| format!("f/{}", trim_num(f))),
            iso.map(|iso| format!("ISO {iso}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        Summary {
            camera,
            lens,
            exposure: (!exposure.is_empty()).then(|| exposure.join(" · ")),
            captured: self
                .text(Tag::DateTimeOriginal)
                .or_else(|| self.text(Tag::DateTimeDigitized)),
        }
    }

    /// An EXIF block (TIFF structure, as JPEG's APP1 and PNG's eXIf chunk
    /// carry it) for an export of `width` x `height`, or `None` when the
    /// policy keeps nothing.
    pub fn exif_for_export(
        &self,
        policy: MetadataPolicy,
        software: &str,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        if policy == MetadataPolicy::None {
            return None;
        }
        let location = policy == MetadataPolicy::All;
        let mut fields: Vec<Field> = self
            .fields
            .iter()
            .filter(|f| KEEP.contains(&f.tag) || (location && f.tag.context() == Context::Gps))
            .cloned()
            .collect();
        let field = |tag, value| Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        };
        fields.push(field(
            Tag::Software,
            Value::Ascii(vec![software.as_bytes().to_vec()]),
        ));
        // Exports are always sRGB, already upright.
        fields.push(field(Tag::Orientation, Value::Short(vec![1])));
        fields.push(field(Tag::ColorSpace, Value::Short(vec![1])));
        fields.push(field(Tag::PixelXDimension, Value::Long(vec![width])));
        fields.push(field(Tag::PixelYDimension, Value::Long(vec![height])));
        let mut writer = exif::experimental::Writer::new();
        for f in &fields {
            writer.push_field(f);
        }
        let mut out = Cursor::new(Vec::new());
        writer.write(&mut out, true).ok()?;
        Some(out.into_inner())
    }

    /// Plain TIFF tags for exports where a full EXIF block can't be stored:
    /// (tag number, text) pairs for make, model, date, artist and copyright.
    pub fn tiff_text_tags(&self, policy: MetadataPolicy, software: &str) -> Vec<(u16, String)> {
        let mut tags = vec![(Tag::Software.number(), software.to_owned())];
        if policy == MetadataPolicy::None {
            return tags;
        }
        for tag in [
            Tag::Make,
            Tag::Model,
            Tag::Artist,
            Tag::Copyright,
            Tag::ImageDescription,
        ] {
            if let Some(v) = self.text(tag) {
                tags.push((tag.number(), v));
            }
        }
        if let Some(date) = self.text(Tag::DateTimeOriginal) {
            tags.push((Tag::DateTime.number(), date));
        }
        tags.sort();
        tags
    }
}

/// "1/125" for fast shutters, "0.5s" or "2s" for slow ones.
fn shutter(secs: f64) -> String {
    if secs > 0.0 && secs < 0.3 {
        format!("1/{}", (1.0 / secs).round())
    } else {
        format!("{}s", trim_num(secs))
    }
}

/// One decimal place at most, without a trailing ".0".
fn trim_num(v: f64) -> String {
    let s = format!("{v:.1}");
    s.strip_suffix(".0").map(str::to_owned).unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Metadata {
        let f = |tag, value| Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        };
        let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
        let r = |num, denom| Value::Rational(vec![Rational { num, denom }]);
        Metadata {
            fields: vec![
                f(Tag::Make, ascii("Canon")),
                f(Tag::Model, ascii("Canon EOS 550D")),
                f(Tag::LensModel, ascii("EF-S 55-250mm f/4-5.6 IS II")),
                f(Tag::ExposureTime, r(1, 500)),
                f(Tag::FNumber, r(56, 10)),
                f(Tag::PhotographicSensitivity, Value::Short(vec![100])),
                f(Tag::DateTimeOriginal, ascii("2014:09:17 17:08:37")),
                f(Tag::Orientation, Value::Short(vec![6])),
                f(Tag::GPSLatitudeRef, ascii("N")),
                f(
                    Tag::GPSLatitude,
                    Value::Rational(vec![
                        Rational { num: 46, denom: 1 },
                        Rational { num: 30, denom: 1 },
                        Rational { num: 0, denom: 1 },
                    ]),
                ),
            ],
        }
    }

    #[test]
    fn summarizes_camera_lens_and_exposure() {
        let s = sample().summary();
        assert_eq!(s.camera.as_deref(), Some("Canon EOS 550D"));
        assert_eq!(s.lens.as_deref(), Some("EF-S 55-250mm f/4-5.6 IS II"));
        assert_eq!(s.exposure.as_deref(), Some("1/500 · f/5.6 · ISO 100"));
        assert_eq!(s.captured.as_deref(), Some("2014:09:17 17:08:37"));
        assert_eq!(Metadata::default().summary(), Summary::default());
    }

    fn parse(blob: &[u8]) -> exif::Exif {
        exif::Reader::new().read_raw(blob.to_vec()).unwrap()
    }

    #[test]
    fn export_exif_drops_location_unless_asked_and_resets_orientation() {
        let m = sample();
        assert!(m.has_location());
        let without = parse(
            &m.exif_for_export(MetadataPolicy::NoLocation, "Aperio", 300, 200)
                .unwrap(),
        );
        assert!(without.get_field(Tag::GPSLatitude, In::PRIMARY).is_none());
        assert_eq!(
            without
                .get_field(Tag::Orientation, In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(1)
        );
        assert_eq!(
            without
                .get_field(Tag::PixelXDimension, In::PRIMARY)
                .unwrap()
                .value
                .get_uint(0),
            Some(300)
        );
        assert!(without.get_field(Tag::Model, In::PRIMARY).is_some());

        let with = parse(
            &m.exif_for_export(MetadataPolicy::All, "Aperio", 300, 200)
                .unwrap(),
        );
        assert!(with.get_field(Tag::GPSLatitude, In::PRIMARY).is_some());

        assert!(m
            .exif_for_export(MetadataPolicy::None, "Aperio", 300, 200)
            .is_none());
    }

    #[test]
    fn tiff_tags_carry_the_basics() {
        let tags = sample().tiff_text_tags(MetadataPolicy::NoLocation, "Aperio");
        assert!(tags.contains(&(Tag::Model.number(), "Canon EOS 550D".into())));
        assert!(tags.contains(&(Tag::DateTime.number(), "2014:09:17 17:08:37".into())));
        assert_eq!(
            sample().tiff_text_tags(MetadataPolicy::None, "Aperio"),
            vec![(Tag::Software.number(), "Aperio".into())]
        );
    }

    #[test]
    fn formats_exposure_values() {
        assert_eq!(shutter(1.0 / 125.0), "1/125");
        assert_eq!(shutter(0.5), "0.5s");
        assert_eq!(shutter(2.0), "2s");
        assert_eq!(trim_num(8.0), "8");
    }
}
