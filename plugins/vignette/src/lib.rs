//! An example third-party plugin: darkens (or lightens) the edges of the
//! frame like a lens vignette.
//!
//! It uses nothing but the public `aperio-plugin` API, so it doubles as a
//! template: copy this crate, change the id, and register your plugin with
//! the host. See docs/plugins.md.
//!
//! Sidecar step:
//!
//! ```json
//! { "op": "plugin", "plugin": "org.aperio.example.vignette", "version": "1.0.0",
//!   "params": { "amount": -0.4, "midpoint": 0.5 } }
//! ```

use aperio_plugin::{ImageMut, ParamSpec, Plugin, PluginInfo, Provides, Result, API_VERSION};
use serde_json::Value;

pub const ID: &str = "org.aperio.example.vignette";

/// Exposure change at the corners for `amount` = ±1, in stops.
const MAX_STOPS: f32 = 2.0;

pub struct Vignette;

fn amount() -> ParamSpec {
    ParamSpec::number("amount", "Amount", -1.0, 1.0, 0.01, -0.4)
}

fn midpoint() -> ParamSpec {
    ParamSpec::number("midpoint", "Midpoint", 0.0, 1.0, 0.01, 0.5)
}

impl Plugin for Vignette {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: ID.into(),
            name: "Vignette (example)".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api_version: API_VERSION,
            license: "MIT OR Apache-2.0".into(),
            operation: Some(Provides {
                label: "Vignette".into(),
                params: vec![amount(), midpoint()],
            }),
            mask: None,
            source: None,
            models: vec![],
        }
    }

    fn apply(&self, params: &Value, image: ImageMut<'_>) -> Result<()> {
        let amount = amount().read_number(params)? as f32;
        let midpoint = midpoint().read_number(params)? as f32;
        let (w, h) = (image.width as f32, image.height as f32);
        // Distance from the center in pixels, so the falloff stays round on
        // non-square photos; 1.0 at the corners.
        let half_diagonal = (w * w + h * h).sqrt() / 2.0;
        for (i, px) in image.pixels.iter_mut().enumerate() {
            let x = (i as u32 % image.width) as f32 + 0.5 - w / 2.0;
            let y = (i as u32 / image.width) as f32 + 0.5 - h / 2.0;
            let d = (x * x + y * y).sqrt() / half_diagonal;
            let t = smoothstep(midpoint, 1.0, d);
            let gain = (amount * MAX_STOPS * t).exp2();
            for c in &mut px[..3] {
                *c *= gain;
            }
        }
        Ok(())
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn darkens_corners_and_leaves_the_center() {
        let mut px = vec![[0.5, 0.5, 0.5, 1.0]; 9 * 5];
        let img = ImageMut {
            width: 9,
            height: 5,
            pixels: &mut px,
        };
        Vignette.apply(&json!({"amount": -1.0}), img).unwrap();
        assert_eq!(px[2 * 9 + 4], [0.5, 0.5, 0.5, 1.0]);
        assert!(px[0][0] < 0.25 && px[0][0] > 0.125);
        assert_eq!(px[0][3], 1.0);
    }
}
