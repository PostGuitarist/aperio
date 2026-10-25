//! HEIC/HEIF photos, such as an iPhone's, decoded by the operating system.
//!
//! HEIC uses the HEVC video codec, which needs a patent license, so Aperio
//! doesn't ship a decoder of its own. Each platform uses the one it has:
//!
//! - macOS: ImageIO, built in.
//! - Windows: the Windows Imaging Component, with Microsoft's HEIF Image
//!   Extensions and HEVC Video Extensions from the Microsoft Store.
//! - Linux and others: libheif, loaded at run time if it's installed, with a
//!   decoder plugin such as libde265.
//!
//! Every path returns the photo upright, in linear light with sRGB primaries.
//! Photos in a wider color space (iPhones use Display P3) keep their colors,
//! with out-of-gamut values below 0 or above 1.

use std::path::Path;

use crate::{LinearImage, Result};

#[cfg(not(target_os = "macos"))]
mod color;
#[cfg(not(any(target_os = "macos", windows)))]
mod libheif;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(not(any(target_os = "macos", windows)))]
use libheif as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

/// Extensions treated as HEIF. `.hif` is what Canon and Sony cameras write.
pub const EXTENSIONS: &[&str] = &["heic", "heif", "hif"];

/// Whether `path` is a HEIC/HEIF photo by its extension.
pub fn is_heif(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    EXTENSIONS.contains(&ext.as_str())
}

/// Decode a HEIC/HEIF photo with the system's decoder.
pub fn decode(path: &Path) -> Result<LinearImage> {
    platform::decode(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    /// 64×48, left half Display P3 red, right half mid gray; lossless, with
    /// an `nclx` profile.
    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/display-p3-64x48.heic"
    );

    #[test]
    fn decodes_display_p3_into_linear_srgb() {
        let img = match decode(Path::new(FIXTURE)) {
            Ok(img) => img,
            // No system decoder here (libheif or Windows' HEIF extensions).
            Err(Error::Heif(m)) if m.contains("needs") => {
                eprintln!("skipped: {m}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        assert_eq!((img.width, img.height), (64, 48));
        let [r, g, b, a] = img.pixels[24 * 64 + 8];
        assert!((r - 1.225).abs() < 0.01, "{r}");
        assert!(g < -0.02 && b < -0.01, "{g} {b}");
        assert_eq!(a, 1.0);
        // sRGB 128 is about 0.216 in linear light, and gray stays gray.
        let gray = img.pixels[24 * 64 + 56];
        assert!(
            gray[..3].iter().all(|v| (v - 0.216).abs() < 0.01),
            "{gray:?}"
        );
    }

    #[test]
    fn recognizes_heif_extensions() {
        assert!(is_heif(Path::new("IMG_0001.HEIC")));
        assert!(is_heif(Path::new("a.hif")));
        assert!(!is_heif(Path::new("a.jpg")));
    }
}
