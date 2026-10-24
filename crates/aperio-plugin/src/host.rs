//! Holds the installed plugins, routes document steps to them, and caches
//! generated masks.

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::{compatible, Image, ImageMut, Plugin, PluginError, PluginInfo, Result, API_VERSION};

/// Masks kept in memory. A mask is one f32 per pixel, so 16 preview-sized
/// masks are about 256 MiB at most; full-size ones are evicted first in use.
const MASK_CACHE_ENTRIES: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("plugin {id} was built for plugin API {found}, this build has {API_VERSION}")]
    ApiVersion { id: String, found: u32 },
    #[error("a plugin with id {0} is already installed")]
    Duplicate(String),
}

/// The installed plugins.
#[derive(Default)]
pub struct Host {
    plugins: Vec<(PluginInfo, Box<dyn Plugin>)>,
    masks: Mutex<VecDeque<(u64, Arc<Vec<f32>>)>>,
}

impl Host {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        plugin: impl Plugin + 'static,
    ) -> std::result::Result<(), RegisterError> {
        let info = plugin.info();
        if info.api_version != API_VERSION {
            return Err(RegisterError::ApiVersion {
                id: info.id,
                found: info.api_version,
            });
        }
        if self.plugins.iter().any(|(i, _)| i.id == info.id) {
            return Err(RegisterError::Duplicate(info.id));
        }
        self.plugins.push((info, Box::new(plugin)));
        Ok(())
    }

    /// Everything installed, in registration order.
    pub fn plugins(&self) -> impl Iterator<Item = &PluginInfo> {
        self.plugins.iter().map(|(info, _)| info)
    }

    /// The installed plugin that can render steps written by `id` at
    /// `version`, or why there isn't one.
    pub fn find(&self, id: &str, version: &str) -> Result<&dyn Plugin> {
        let (info, plugin) = self
            .plugins
            .iter()
            .find(|(info, _)| info.id == id)
            .ok_or_else(|| PluginError::Failed(format!("{id} is not installed")))?;
        if !compatible(&info.version, version) {
            return Err(PluginError::Failed(format!(
                "{id} {version} is needed, {} is installed",
                info.version
            )));
        }
        Ok(plugin.as_ref())
    }

    /// Run a plugin operation on `image`, given the layer's mask if it has one.
    pub fn apply(
        &self,
        id: &str,
        version: &str,
        params: &Value,
        image: ImageMut<'_>,
        mask: Option<&[f32]>,
    ) -> Result<()> {
        self.find(id, version)?.apply_masked(params, image, mask)
    }

    /// Generate (or reuse) a plugin mask for `source`.
    pub fn mask(
        &self,
        id: &str,
        version: &str,
        params: &Value,
        source: Image<'_>,
    ) -> Result<Arc<Vec<f32>>> {
        let plugin = self.find(id, version)?;
        let key = cache_key(id, version, params, source);
        if let Some((_, m)) = self.masks.lock().unwrap().iter().find(|(k, _)| *k == key) {
            return Ok(m.clone());
        }
        let mask = plugin.mask(params, source)?;
        let expected = source.width as usize * source.height as usize;
        if mask.len() != expected {
            return Err(PluginError::Failed(format!(
                "{id} returned a mask of {} pixels for a {}x{} image",
                mask.len(),
                source.width,
                source.height
            )));
        }
        let mask = Arc::new(mask);
        let mut cache = self.masks.lock().unwrap();
        if cache.len() == MASK_CACHE_ENTRIES {
            cache.pop_front();
        }
        cache.push_back((key, mask.clone()));
        Ok(mask)
    }
}

/// Identifies a mask request: the plugin, its parameters, and the image
/// (its size plus a sample of its pixels, which is enough to tell photos and
/// preview sizes apart without hashing every pixel).
fn cache_key(id: &str, version: &str, params: &Value, source: Image<'_>) -> u64 {
    let mut h = DefaultHasher::new();
    (id, version, params.to_string(), source.width, source.height).hash(&mut h);
    let step = (source.pixels.len() / 4096).max(1);
    for px in source.pixels.iter().step_by(step) {
        for c in px {
            c.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Provides;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Mask = the red channel; operation = multiply by `gain`.
    struct Test {
        calls: Arc<AtomicUsize>,
    }

    impl Plugin for Test {
        fn info(&self) -> PluginInfo {
            PluginInfo {
                id: "org.example.test".into(),
                name: "Test".into(),
                version: "1.1.0".into(),
                api_version: API_VERSION,
                license: "MIT".into(),
                operation: Some(Provides {
                    label: "Gain".into(),
                    params: vec![],
                }),
                mask: None,
                models: vec![],
            }
        }

        fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()> {
            let g = params["gain"].as_f64().unwrap_or(1.0) as f32;
            for px in image.pixels {
                for c in &mut px[..3] {
                    *c *= g;
                }
            }
            Ok(())
        }

        fn mask(&self, _: &Value, source: Image<'_>) -> Result<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(source.pixels.iter().map(|p| p[0]).collect())
        }
    }

    fn host() -> (Host, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut host = Host::new();
        host.register(Test {
            calls: calls.clone(),
        })
        .unwrap();
        (host, calls)
    }

    #[test]
    fn routes_operations_by_id_and_version() {
        let (host, _) = host();
        let mut px = vec![[0.5, 0.5, 0.5, 1.0]];
        let img = ImageMut {
            width: 1,
            height: 1,
            pixels: &mut px,
        };
        host.apply("org.example.test", "1.0.0", &json!({"gain": 2}), img, None)
            .unwrap();
        assert_eq!(px[0], [1.0, 1.0, 1.0, 1.0]);

        let img = ImageMut {
            width: 1,
            height: 1,
            pixels: &mut px,
        };
        let err = host
            .apply("org.example.test", "2.0.0", &json!({}), img, None)
            .unwrap_err();
        assert!(err.to_string().contains("1.1.0 is installed"));
        assert!(host.find("org.example.missing", "1.0.0").is_err());
    }

    #[test]
    fn masks_are_cached_per_image_and_params() {
        let (host, calls) = host();
        let a = vec![[0.25, 0.0, 0.0, 1.0]; 4];
        let b = vec![[0.75, 0.0, 0.0, 1.0]; 4];
        let img = |p| Image {
            width: 2,
            height: 2,
            pixels: p,
        };
        let id = "org.example.test";
        let m = host.mask(id, "1.0.0", &json!({}), img(&a)).unwrap();
        assert_eq!(*m, vec![0.25; 4]);
        host.mask(id, "1.0.0", &json!({}), img(&a)).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        host.mask(id, "1.0.0", &json!({"x": 1}), img(&a)).unwrap();
        host.mask(id, "1.0.0", &json!({}), img(&b)).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn rejects_duplicates() {
        let (mut host, calls) = host();
        assert!(matches!(
            host.register(Test { calls }),
            Err(RegisterError::Duplicate(_))
        ));
    }
}
