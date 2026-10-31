//! Library organizing: ratings, flags, and color labels (kept in each
//! photo's sidecar, so they travel with it), albums and smart collections
//! (kept in one file in the config folder), and syncing edits from one
//! photo to others.

use std::path::{Path, PathBuf};

use aperio_core::{sidecar_path, EditDocument, Flag, Label, Operation, SourceRef};
use serde::{Deserialize, Serialize};

/// The sidecar for `path`, or a new, empty edit if it has none.
pub fn load_or_new(path: &Path) -> aperio_core::Result<EditDocument> {
    let sidecar = sidecar_path(path);
    if sidecar.exists() {
        return EditDocument::load(&sidecar);
    }
    Ok(EditDocument::new(SourceRef {
        file_name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        size_bytes: std::fs::metadata(path)?.len(),
        sha256: None,
    }))
}

/// Save `doc` as `path`'s sidecar, or remove the sidecar when there's
/// nothing in it worth keeping.
fn store(path: &Path, doc: &EditDocument) -> aperio_core::Result<()> {
    let sidecar = sidecar_path(path);
    let empty = !doc.has_edits()
        && doc.rating == 0
        && doc.flag.is_none()
        && doc.label.is_none()
        && doc.snapshots.is_empty();
    if empty {
        match std::fs::remove_file(&sidecar) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(()),
        }
    }
    doc.save(&sidecar)
}

/// A change to some photos' marks. Fields left out stay as they are.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MarkChange {
    pub rating: Option<u8>,
    /// `"none"` clears the flag.
    pub flag: Option<Clearable<Flag>>,
    /// `"none"` clears the label.
    pub label: Option<Clearable<Label>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Clearable<T> {
    Set(T),
    Clear(None_),
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum None_ {
    None,
}

impl<T> Clearable<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Clearable::Set(v) => Some(v),
            Clearable::Clear(_) => None,
        }
    }
}

/// Apply `change` to each photo's sidecar. Stops at the first failure.
pub fn set_marks(paths: &[PathBuf], change: &MarkChange) -> aperio_core::Result<()> {
    for path in paths {
        let mut doc = load_or_new(path)?;
        if let Some(r) = change.rating {
            doc.rating = r.min(5);
        }
        if let Some(f) = change.flag {
            doc.flag = f.into_option();
        }
        if let Some(l) = change.label {
            doc.label = l.into_option();
        }
        store(path, &doc)?;
    }
    Ok(())
}

// --- Syncing edits --------------------------------------------------------------

/// Parts of an edit that can be synced to other photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncPart {
    /// Exposure, contrast, and the tone sliders.
    Light,
    /// White balance, saturation and vibrance, the color mixer, and grading.
    Color,
    /// Texture, clarity, and dehaze.
    Effects,
    Curves,
    /// Plugin steps on the base layer, such as denoise.
    Detail,
    Crop,
    /// Every layer above the base, with its masks.
    Layers,
}

impl SyncPart {
    fn covers(self, op: &Operation) -> bool {
        use Operation as O;
        match self {
            SyncPart::Light => {
                matches!(op, O::Exposure { .. } | O::Contrast { .. } | O::Tone { .. })
            }
            SyncPart::Color => matches!(
                op,
                O::WhiteBalance { .. } | O::Color { .. } | O::Hsl { .. } | O::ColorGrade { .. }
            ),
            SyncPart::Effects => matches!(op, O::Presence { .. }),
            SyncPart::Curves => matches!(op, O::Curve { .. }),
            SyncPart::Detail => matches!(op, O::Plugin { .. }),
            SyncPart::Crop => matches!(op, O::Crop { .. }),
            SyncPart::Layers => false,
        }
    }
}

/// Copy the chosen `parts` of `from`'s edit onto each photo in `to`. On the
/// base layer, the target's operations of those kinds are replaced by the
/// source's (or removed, if the source has none); everything else on the
/// target is kept. Returns the photos that changed, with their new edits.
pub fn sync(
    from: &Path,
    to: &[PathBuf],
    parts: &[SyncPart],
) -> aperio_core::Result<Vec<(PathBuf, EditDocument)>> {
    let source = load_or_new(from)?;
    let picked = |op: &Operation| parts.iter().any(|p| p.covers(op));
    let base_ops: Vec<Operation> = source
        .layers
        .first()
        .map(|l| l.operations.iter().filter(|o| picked(o)).cloned().collect())
        .unwrap_or_default();
    let mut changed = Vec::new();
    for path in to.iter().filter(|p| p.as_path() != from) {
        let mut doc = load_or_new(path)?;
        let before = doc.clone();
        if let Some(base) = doc.layers.first_mut() {
            base.operations.retain(|o| !picked(o));
            base.operations.extend(base_ops.iter().cloned());
            // The order the editor writes them in. The sort is stable, so
            // plugin steps keep their order among themselves.
            base.operations.sort_by_key(rank);
        }
        if parts.contains(&SyncPart::Layers) {
            doc.layers.truncate(1);
            doc.layers.extend(source.layers.iter().skip(1).cloned());
        }
        if doc != before {
            store(path, &doc)?;
            changed.push((path.clone(), doc));
        }
    }
    Ok(changed)
}

/// Where an operation goes in a layer: the order the editor keeps them in.
fn rank(op: &Operation) -> u8 {
    use aperio_core::ops::CurveChannel as C;
    use Operation as O;
    match op {
        O::WhiteBalance { .. } => 0,
        O::Exposure { .. } => 1,
        O::Contrast { .. } => 2,
        O::Tone { .. } => 3,
        O::Color { .. } => 4,
        O::Curve { channel, .. } => match channel {
            C::Rgb => 5,
            C::Red => 6,
            C::Green => 7,
            C::Blue => 8,
            C::Luma => 9,
        },
        O::Presence { .. } => 10,
        O::Hsl { .. } => 11,
        O::ColorGrade { .. } => 12,
        O::Crop { .. } => 13,
        O::Plugin { .. } => 14,
    }
}

// --- Albums and smart collections -----------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Collections {
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub smart: Vec<SmartCollection>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Album {
    pub name: String,
    pub photos: Vec<PathBuf>,
}

/// Photos from the library's folders that match every rule set here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartCollection {
    pub name: String,
    #[serde(default)]
    pub min_rating: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<Flag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Part of the file name, ignoring case.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_contains: String,
}

#[derive(Serialize, Deserialize)]
struct CollectionsFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    collections: Collections,
}

const COLLECTIONS_FORMAT: &str = "aperio.collections";

pub fn collections_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aperio")
        .join("collections.json")
}

/// The saved albums and smart collections; none if the file is missing or
/// unreadable.
pub fn load_collections(path: &Path) -> Collections {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str::<CollectionsFile>(&json).ok())
        .filter(|f| f.format == COLLECTIONS_FORMAT)
        .map(|f| f.collections)
        .unwrap_or_default()
}

pub fn save_collections(path: &Path, collections: Collections) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = CollectionsFile {
        format: COLLECTIONS_FORMAT.into(),
        version: 1,
        collections,
    };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperio_core::Layer;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aperio-org-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn marks_are_set_cleared_and_empty_sidecars_removed() {
        let dir = temp("marks");
        let photo = dir.join("a.jpg");
        std::fs::write(&photo, b"x").unwrap();
        let change: MarkChange =
            serde_json::from_str(r#"{"rating": 4, "flag": "pick", "label": "red"}"#).unwrap();
        set_marks(std::slice::from_ref(&photo), &change).unwrap();
        let doc = load_or_new(&photo).unwrap();
        assert_eq!(
            (doc.rating, doc.flag, doc.label),
            (4, Some(Flag::Pick), Some(Label::Red))
        );
        // Only the flag changes.
        let change: MarkChange = serde_json::from_str(r#"{"flag": "none"}"#).unwrap();
        set_marks(std::slice::from_ref(&photo), &change).unwrap();
        let doc = load_or_new(&photo).unwrap();
        assert_eq!(
            (doc.rating, doc.flag, doc.label),
            (4, None, Some(Label::Red))
        );
        // With nothing left, the sidecar goes away.
        let change: MarkChange = serde_json::from_str(r#"{"rating": 0, "label": "none"}"#).unwrap();
        set_marks(std::slice::from_ref(&photo), &change).unwrap();
        assert!(!sidecar_path(&photo).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sync_copies_chosen_parts_and_keeps_the_rest() {
        let dir = temp("sync");
        let (a, b) = (dir.join("a.jpg"), dir.join("b.jpg"));
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"x").unwrap();
        let mut src = load_or_new(&a).unwrap();
        src.layers[0].operations = vec![
            Operation::Exposure { ev: 0.5 },
            Operation::Color {
                saturation: 0.2,
                vibrance: 0.0,
            },
            Operation::Crop {
                x: 0.1,
                y: 0.1,
                width: 0.8,
                height: 0.8,
                angle_deg: 0.0,
            },
        ];
        src.layers.push(Layer::new("Sky"));
        src.save(&sidecar_path(&a)).unwrap();
        let mut dst = load_or_new(&b).unwrap();
        dst.rating = 3;
        dst.layers[0].operations = vec![
            Operation::Exposure { ev: -1.0 },
            Operation::Contrast { amount: 0.3 },
            Operation::Color {
                saturation: -0.5,
                vibrance: 0.0,
            },
        ];
        dst.save(&sidecar_path(&b)).unwrap();

        let changed = sync(&a, &[a.clone(), b.clone()], &[SyncPart::Light]).unwrap();
        assert_eq!(changed.len(), 1);
        let got = load_or_new(&b).unwrap();
        // Light came over (contrast removed, since the source has none);
        // color, the rating, and the missing layers stayed as they were.
        assert_eq!(
            got.layers[0].operations,
            vec![
                Operation::Exposure { ev: 0.5 },
                Operation::Color {
                    saturation: -0.5,
                    vibrance: 0.0
                },
            ]
        );
        assert_eq!((got.rating, got.layers.len()), (3, 1));

        sync(
            &a,
            std::slice::from_ref(&b),
            &[SyncPart::Crop, SyncPart::Layers],
        )
        .unwrap();
        let got = load_or_new(&b).unwrap();
        assert!(matches!(
            got.layers[0].operations.last(),
            Some(Operation::Crop { .. })
        ));
        assert_eq!(got.layers.len(), 2);
        // Syncing the same thing again changes nothing.
        assert!(sync(&a, std::slice::from_ref(&b), &[SyncPart::Crop])
            .unwrap()
            .is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collections_round_trip_and_tolerate_a_missing_file() {
        let dir = temp("collections");
        let path = dir.join("collections.json");
        assert_eq!(load_collections(&path), Collections::default());
        let c = Collections {
            albums: vec![Album {
                name: "Portfolio".into(),
                photos: vec![dir.join("a.jpg")],
            }],
            smart: vec![SmartCollection {
                name: "Best".into(),
                min_rating: 4,
                flag: Some(Flag::Pick),
                label: None,
                edited: None,
                kind: None,
                name_contains: String::new(),
            }],
        };
        save_collections(&path, c.clone()).unwrap();
        assert_eq!(load_collections(&path), c);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
