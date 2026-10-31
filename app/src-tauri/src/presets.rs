//! User presets: named sets of adjustments saved as small JSON files in the
//! config folder, one file per preset, so they can be copied between
//! machines or shared.

use std::path::{Path, PathBuf};

use aperio_core::Operation;
use serde::{Deserialize, Serialize};

/// Value of the `format` field in every preset file.
const FORMAT_ID: &str = "aperio.preset";
const FORMAT_VERSION: u32 = 1;
const EXTENSION: &str = "aperio-preset.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// The adjustments a preset sets. Anything it leaves out stays as it is
    /// on the photo it's applied to.
    pub operations: Vec<Operation>,
}

#[derive(Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    #[serde(flatten)]
    preset: Preset,
}

/// Where presets are kept, e.g. `~/.config/aperio/presets` on Linux.
pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aperio")
        .join("presets")
}

/// Every readable preset in `dir`, sorted by name. Files that aren't
/// presets, or come from a newer build, are skipped.
pub fn list(dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut presets: Vec<Preset> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(EXTENSION))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|json| serde_json::from_str::<PresetFile>(&json).ok())
        .filter(|f| f.format == FORMAT_ID && f.version <= FORMAT_VERSION)
        .map(|f| f.preset)
        .collect();
    presets.sort_by_key(|p| p.name.to_lowercase());
    presets
}

/// Save `preset`, replacing any preset with the same name.
pub fn save(dir: &Path, preset: Preset) -> std::io::Result<()> {
    if preset.name.trim().is_empty() {
        return Err(std::io::Error::other("a preset needs a name"));
    }
    std::fs::create_dir_all(dir)?;
    let file = PresetFile {
        format: FORMAT_ID.into(),
        version: FORMAT_VERSION,
        preset,
    };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    std::fs::write(path(dir, &file.preset.name), json)
}

pub fn delete(dir: &Path, name: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path(dir, name)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// A file name for `name` that's safe on every platform. Names that differ
/// only in case share a file, as they would on macOS and Windows anyway.
fn path(dir: &Path, name: &str) -> PathBuf {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    dir.join(format!("{}.{EXTENSION}", slug.trim_matches('-')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(name: &str, ev: f32) -> Preset {
        Preset {
            name: name.into(),
            operations: vec![Operation::Exposure { ev }],
        }
    }

    #[test]
    fn saves_lists_replaces_and_deletes() {
        let dir = std::env::temp_dir().join(format!("aperio-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list(&dir).is_empty());
        save(&dir, preset("Warm / bright", 0.5)).unwrap();
        save(&dir, preset("Agfa", 0.1)).unwrap();
        save(&dir, preset("warm / bright", 0.7)).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a preset").unwrap();
        std::fs::write(dir.join("broken.aperio-preset.json"), "{").unwrap();
        let names: Vec<_> = list(&dir).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Agfa", "warm / bright"]);
        assert!(dir.join("warm---bright.aperio-preset.json").exists());
        delete(&dir, "Warm / bright").unwrap();
        delete(&dir, "missing").unwrap();
        assert_eq!(list(&dir), vec![preset("Agfa", 0.1)]);
        assert!(save(&dir, preset("  ", 0.0)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
