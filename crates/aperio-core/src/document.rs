use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Mask, Operation, Result};

/// Value of the `format` field in every sidecar.
pub const FORMAT_ID: &str = "aperio.edit";
/// Sidecar schema version written by this build.
pub const FORMAT_VERSION: u32 = 1;
/// Appended to the source file name, e.g. `IMG_0001.CR3.aperio.json`.
pub const SIDECAR_EXTENSION: &str = "aperio.json";

/// The full, non-destructive edit of one source image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditDocument {
    pub format: String,
    pub version: u32,
    pub source: SourceRef,
    /// Bottom to top. The first layer is applied to the decoded source image.
    pub layers: Vec<Layer>,
    /// Named versions of this edit to come back to. Not rendered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub snapshots: Vec<Snapshot>,
    /// Star rating from the Library, 0 (none) to 5.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<Flag>,
    /// Color label from the Library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    Pick,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    Red,
    Yellow,
    Green,
    Blue,
    Purple,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// A saved copy of an edit's layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub name: String,
    /// When it was taken, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub created_ms: u64,
    pub layers: Vec<Layer>,
}

/// Identifies the original file this edit belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// File name only, so the sidecar still matches after a folder is moved.
    pub file_name: String,
    pub size_bytes: u64,
    /// Hex SHA-256 of the source, used to detect a replaced file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    /// How the layer's result combines with the layers below.
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend_mode: BlendMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    #[serde(default)]
    pub operations: Vec<Operation>,
}

/// Layer blend modes. Multiply, screen, overlay, and soft light work on
/// sRGB-encoded values, the way most editors do; color and luminosity swap
/// chroma and brightness between the layer and what's below it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    Color,
    Luminosity,
}

impl BlendMode {
    pub fn is_normal(&self) -> bool {
        *self == BlendMode::Normal
    }
}

fn default_true() -> bool {
    true
}

fn default_opacity() -> f32 {
    1.0
}

impl Layer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: true,
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            mask: None,
            operations: Vec::new(),
        }
    }
}

impl EditDocument {
    /// A new document with a single empty base layer.
    pub fn new(source: SourceRef) -> Self {
        Self {
            format: FORMAT_ID.to_owned(),
            version: FORMAT_VERSION,
            source,
            layers: vec![Layer::new("Base")],
            snapshots: Vec::new(),
            rating: 0,
            flag: None,
            label: None,
        }
    }

    /// True when the document changes how the photo looks: more than an
    /// empty base layer. Ratings, flags, labels, and snapshots don't count.
    pub fn has_edits(&self) -> bool {
        self.layers.len() > 1
            || self
                .layers
                .first()
                .is_some_and(|l| !l.operations.is_empty() || l.mask.is_some())
    }

    pub fn from_json(json: &str) -> Result<Self> {
        let doc: Self = serde_json::from_str(json)?;
        if doc.format != FORMAT_ID {
            return Err(Error::WrongFormat(doc.format));
        }
        if doc.version > FORMAT_VERSION {
            return Err(Error::UnsupportedVersion {
                found: doc.version,
                supported: FORMAT_VERSION,
            });
        }
        Ok(doc)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_json()?)?;
        Ok(())
    }
}

/// Where the sidecar for `source` lives: next to it, with the full original
/// name kept so `a.CR3` and `a.JPG` get separate sidecars.
pub fn sidecar_path(source: &Path) -> PathBuf {
    let mut name = source.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(SIDECAR_EXTENSION);
    source.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::CurveChannel;

    fn sample() -> EditDocument {
        let mut doc = EditDocument::new(SourceRef {
            file_name: "IMG_0001.CR3".into(),
            size_bytes: 25_000_000,
            sha256: None,
        });
        doc.layers[0].operations = vec![
            Operation::Exposure { ev: 0.5 },
            Operation::Curve {
                channel: CurveChannel::Rgb,
                points: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
            },
        ];
        let mut sky = Layer::new("Sky");
        sky.mask = Some(Mask::Plugin {
            plugin: "org.aperio.segment".into(),
            version: "0.1.0".into(),
            params: serde_json::json!({ "class": "sky" }),
        });
        sky.operations.push(Operation::Exposure { ev: -0.7 });
        doc.layers.push(sky);
        doc
    }

    #[test]
    fn round_trips_through_json() {
        let doc = sample();
        let parsed = EditDocument::from_json(&doc.to_json().unwrap()).unwrap();
        assert_eq!(doc, parsed);
    }

    #[test]
    fn combined_masks_and_blend_modes_round_trip() {
        use crate::mask::{MaskMode, MaskPart};
        let mut doc = sample();
        let mut layer = Layer::new("Shadows in the sky");
        layer.blend_mode = BlendMode::SoftLight;
        layer.mask = Some(Mask::Combine {
            parts: vec![
                MaskPart {
                    mode: MaskMode::Add,
                    invert: false,
                    mask: Mask::Linear {
                        start: [0.5, 0.0],
                        end: [0.5, 0.5],
                    },
                },
                MaskPart {
                    mode: MaskMode::Intersect,
                    invert: true,
                    mask: Mask::Luminance {
                        low: 0.6,
                        high: 1.0,
                        feather: 0.1,
                    },
                },
            ],
        });
        doc.layers.push(layer);
        let json = doc.to_json().unwrap();
        assert!(json.contains("\"blend_mode\": \"soft_light\""));
        assert!(json.contains("\"mode\": \"intersect\""));
        assert_eq!(EditDocument::from_json(&json).unwrap(), doc);
        // Normal blending isn't written, so older builds still read the file.
        assert!(!sample().to_json().unwrap().contains("blend_mode"));
    }

    #[test]
    fn snapshots_round_trip_and_are_left_out_when_empty() {
        let mut doc = sample();
        assert!(!doc.to_json().unwrap().contains("snapshots"));
        doc.snapshots.push(Snapshot {
            name: "Warm".into(),
            created_ms: 1_790_000_000_000,
            layers: doc.layers.clone(),
        });
        let json = doc.to_json().unwrap();
        assert_eq!(EditDocument::from_json(&json).unwrap(), doc);
    }

    #[test]
    fn library_marks_round_trip_and_do_not_count_as_edits() {
        let mut doc = EditDocument::new(SourceRef {
            file_name: "a.jpg".into(),
            size_bytes: 1,
            sha256: None,
        });
        assert!(!doc.to_json().unwrap().contains("rating"));
        doc.rating = 4;
        doc.flag = Some(Flag::Pick);
        doc.label = Some(Label::Green);
        let json = doc.to_json().unwrap();
        assert!(json.contains("\"flag\": \"pick\""));
        assert_eq!(EditDocument::from_json(&json).unwrap(), doc);
        assert!(!doc.has_edits());
        assert!(sample().has_edits());
    }

    #[test]
    fn fills_layer_defaults() {
        let json = r#"{
            "format": "aperio.edit", "version": 1,
            "source": { "file_name": "a.jpg", "size_bytes": 1 },
            "layers": [{ "name": "Base" }]
        }"#;
        let doc = EditDocument::from_json(json).unwrap();
        assert_eq!(doc.layers[0], Layer::new("Base"));
    }

    #[test]
    fn rejects_other_formats_and_newer_versions() {
        let mut doc = sample();
        doc.format = "something.else".into();
        assert!(matches!(
            EditDocument::from_json(&doc.to_json().unwrap()),
            Err(Error::WrongFormat(_))
        ));

        let mut doc = sample();
        doc.version = FORMAT_VERSION + 1;
        assert!(matches!(
            EditDocument::from_json(&doc.to_json().unwrap()),
            Err(Error::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn sidecar_keeps_original_extension() {
        assert_eq!(
            sidecar_path(Path::new("/photos/a.CR3")),
            PathBuf::from("/photos/a.CR3.aperio.json")
        );
    }

    #[test]
    fn save_and_load() {
        let path = std::env::temp_dir().join(format!("aperio-test-{}.json", std::process::id()));
        let doc = sample();
        doc.save(&path).unwrap();
        let loaded = EditDocument::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc, loaded);
    }
}
