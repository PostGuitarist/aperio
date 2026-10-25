//! The Library screen's backend: list the photos in a folder, and make
//! thumbnails, cached on disk so a folder opens quickly the second time.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use aperio_core::sidecar_path;
use aperio_render::browse;
use image::RgbImage;
use serde::Serialize;

/// Extensions the library lists. Matches the editor's Open dialog.
const RAW: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "dng", "pef", "srw",
];
const RASTER: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];

/// Longest edge of a cached thumbnail.
pub const THUMB_EDGE: u32 = 512;
const THUMB_QUALITY: u8 = 82;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Photo {
    pub path: PathBuf,
    pub file_name: String,
    /// "raw", "jpeg", "png", "tiff" or "heif".
    pub kind: &'static str,
    pub size_bytes: u64,
    /// Last modified, in milliseconds since 1970.
    pub modified_ms: u64,
    /// Has an Aperio sidecar.
    pub edited: bool,
}

#[derive(Debug, Serialize)]
pub struct Folder {
    pub path: PathBuf,
    pub name: String,
    pub photos: Vec<Photo>,
}

fn kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if RAW.contains(&ext.as_str()) {
        Some("raw")
    } else if RASTER.contains(&ext.as_str()) {
        Some(match ext.as_str() {
            "jpg" | "jpeg" => "jpeg",
            "png" => "png",
            _ => "tiff",
        })
    } else if aperio_render::heif::EXTENSIONS.contains(&ext.as_str()) {
        Some("heif")
    } else {
        None
    }
}

/// The photos directly inside `dir` (not in subfolders), sorted by name.
/// Hidden files are skipped.
pub fn list(dir: &Path) -> std::io::Result<Folder> {
    let mut photos = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue;
        }
        let Some(kind) = kind(&path) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        photos.push(Photo {
            edited: sidecar_path(&path).is_file(),
            modified_ms: modified_ms(&meta),
            size_bytes: meta.len(),
            path,
            file_name,
            kind,
        });
    }
    photos.sort_by_key(|p| p.file_name.to_lowercase());
    Ok(Folder {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir.to_string_lossy().into_owned()),
        path: dir.to_path_buf(),
        photos,
    })
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Where thumbnails are cached.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("aperio")
        .join("thumbnails")
}

/// The cache file for `photo`. The name changes when the original is
/// modified or replaced, so a stale thumbnail is never shown.
pub fn cache_path(cache: &Path, photo: &Path) -> std::io::Result<PathBuf> {
    let meta = std::fs::metadata(photo)?;
    // FNV-1a: stable across builds, unlike the standard library's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let key = format!(
        "{}\0{}\0{}",
        photo.display(),
        meta.len(),
        modified_ms(&meta)
    );
    for b in key.bytes() {
        hash = (hash ^ b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    Ok(cache.join(format!("{hash:016x}.jpg")))
}

/// JPEG bytes for `photo`'s thumbnail, from the cache when possible.
pub fn thumbnail(cache: &Path, photo: &Path) -> Result<Vec<u8>, String> {
    let cached = cache_path(cache, photo).map_err(|e| e.to_string())?;
    if let Ok(bytes) = std::fs::read(&cached) {
        return Ok(bytes);
    }
    let img = browse::thumbnail(photo, THUMB_EDGE).map_err(|e| e.to_string())?;
    let bytes = encode(&img)?;
    store(&cached, &bytes);
    Ok(bytes)
}

/// Replace `photo`'s cached thumbnail with an edited rendering, so the
/// library shows the photo the way it was last edited.
pub fn store_edited(cache: &Path, photo: &Path, img: &RgbImage) {
    let small = image::DynamicImage::ImageRgb8(img.clone())
        .thumbnail(THUMB_EDGE, THUMB_EDGE)
        .into_rgb8();
    if let (Ok(path), Ok(bytes)) = (cache_path(cache, photo), encode(&small)) {
        store(&path, &bytes);
    }
}

fn encode(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, THUMB_QUALITY)
        .encode_image(img)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Best effort: a thumbnail that can't be cached is just made again later.
fn store(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Write then rename, so a reader never sees half a file.
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aperio-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_photos_with_their_kind_and_sidecar() {
        let dir = temp("list");
        for f in [
            "b.NEF",
            "a.jpg",
            "c.png",
            "notes.txt",
            ".hidden.jpg",
            "a.jpg.aperio.json",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::create_dir(dir.join("sub.jpg")).unwrap();
        let folder = list(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let got: Vec<_> = folder
            .photos
            .iter()
            .map(|p| (p.file_name.as_str(), p.kind, p.edited))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a.jpg", "jpeg", true),
                ("b.NEF", "raw", false),
                ("c.png", "png", false)
            ]
        );
    }

    #[test]
    fn thumbnails_are_cached_and_edited_ones_replace_them() {
        let dir = temp("thumb");
        let cache = dir.join("cache");
        let photo = dir.join("p.png");
        RgbImage::from_pixel(40, 20, image::Rgb([10, 200, 10]))
            .save(&photo)
            .unwrap();
        let first = thumbnail(&cache, &photo).unwrap();
        assert!(cache_path(&cache, &photo).unwrap().is_file());
        assert_eq!(thumbnail(&cache, &photo).unwrap(), first);

        store_edited(
            &cache,
            &photo,
            &RgbImage::from_pixel(40, 20, image::Rgb([200, 10, 10])),
        );
        let edited = image::load_from_memory(&thumbnail(&cache, &photo).unwrap()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(edited.to_rgb8().get_pixel(5, 5).0[0] > 150);
    }
}
