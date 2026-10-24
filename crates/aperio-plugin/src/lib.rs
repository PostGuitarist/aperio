//! The plugin API for Aperio.
//!
//! A plugin adds operations (which change pixels) and/or masks (which choose
//! where a layer applies). AI features such as sky and subject segmentation
//! are plugins, written against this same API as third-party ones.
//!
//! Plugins are Rust crates built into the app: implement [`Plugin`] and
//! register it with a [`Host`]. Documents refer to a plugin by its id and
//! version and pass it free-form JSON parameters, so a sidecar that uses a
//! plugin still loads (and keeps the step) where the plugin isn't installed.
//!
//! This crate only depends on `serde_json`, so plugins don't pull in the
//! renderer. Images are linear-light RGBA with sRGB primaries, row-major.

mod host;
mod params;

pub use host::{Host, RegisterError};
pub use params::{ParamKind, ParamSpec};

use serde::Serialize;
use serde_json::Value;

/// Bumped when [`Plugin`] changes incompatibly. Plugins report the version
/// they were built for in [`PluginInfo::api_version`].
pub const API_VERSION: u32 = 1;

/// A read-only view of an image.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a [[f32; 4]],
}

/// A mutable view of an image. Operations change pixels in place and cannot
/// change the size.
#[derive(Debug)]
pub struct ImageMut<'a> {
    pub width: u32,
    pub height: u32,
    pub pixels: &'a mut [[f32; 4]],
}

impl ImageMut<'_> {
    pub fn as_image(&self) -> Image<'_> {
        Image {
            width: self.width,
            height: self.height,
            pixels: self.pixels,
        }
    }
}

/// What a plugin is and what it provides. Shown in the app and used to
/// check documents against installed plugins.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    /// Reverse-domain id, e.g. `org.aperio.segment`. Stored in sidecars.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Semantic version. Stored in sidecars next to the id.
    pub version: String,
    /// [`API_VERSION`] this plugin was built against.
    pub api_version: u32,
    /// SPDX license of the plugin's code.
    pub license: String,
    /// Present if the plugin provides an operation.
    pub operation: Option<Provides>,
    /// Present if the plugin provides a mask.
    pub mask: Option<Provides>,
    /// Model files the plugin needs, with their licenses (AI plugins).
    pub models: Vec<ModelInfo>,
}

/// An operation or mask a plugin provides, and the parameters it takes.
#[derive(Debug, Clone, Serialize)]
pub struct Provides {
    /// Label for the UI, e.g. "Vignette".
    pub label: String,
    pub params: Vec<ParamSpec>,
}

/// A model file an AI plugin uses. Every model must be listed in the
/// repository's LICENSES.md.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: String,
    /// SPDX license of the weights.
    pub license: String,
    /// Where the model comes from.
    pub source: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("this plugin doesn't provide that")]
    Unsupported,
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("model unavailable: {0}")]
    ModelUnavailable(String),
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, PluginError>;

/// Implemented by every plugin. Methods may be called from several threads.
pub trait Plugin: Send + Sync {
    fn info(&self) -> PluginInfo;

    /// Run the plugin's operation on `image`. The renderer blends the result
    /// through the layer's mask, so an operation can change every pixel.
    fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()> {
        let _ = (params, image);
        Err(PluginError::Unsupported)
    }

    /// Generate the plugin's mask: one weight per pixel of `source` (0 = not
    /// applied, 1 = fully applied), row-major. `source` is the decoded photo
    /// before any adjustments, so the mask doesn't shift as edits change.
    /// The host caches results, so this may be slow (e.g. a neural network).
    fn mask(&self, params: &Value, source: Image<'_>) -> Result<Vec<f32>> {
        let _ = (params, source);
        Err(PluginError::Unsupported)
    }
}

/// Whether a document written with plugin version `wanted` can be rendered
/// by `installed`: the same major version (or the same minor for 0.x), and
/// installed at least as new.
pub fn compatible(installed: &str, wanted: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let mut it = v.split(['.', '-', '+']).map(|p| p.parse::<u64>().ok());
        Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
    };
    let (Some(have), Some(want)) = (parse(installed), parse(wanted)) else {
        return installed == wanted;
    };
    let same_line = if have.0 == 0 {
        want.0 == 0 && have.1 == want.1
    } else {
        have.0 == want.0
    };
    same_line && have >= want
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compatibility() {
        assert!(compatible("1.2.0", "1.0.3"));
        assert!(!compatible("1.0.0", "1.2.0"));
        assert!(!compatible("2.0.0", "1.9.0"));
        assert!(compatible("0.3.1", "0.3.0"));
        assert!(!compatible("0.4.0", "0.3.0"));
        assert!(compatible("nightly", "nightly"));
    }
}
