use serde::{Deserialize, Serialize};

/// One adjustment in a layer's operation list.
///
/// Built-in operations have typed parameters. Anything provided by a plugin
/// uses [`Operation::Plugin`], so documents stay readable even when the plugin
/// that wrote them is not installed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// Exposure shift in stops (EV).
    Exposure { ev: f32 },
    /// Contrast, from -1.0 to 1.0.
    Contrast { amount: f32 },
    /// White balance as temperature in kelvin and a green/magenta tint.
    WhiteBalance { temperature_k: f32, tint: f32 },
    /// Saturation and vibrance, each from -1.0 to 1.0.
    Color { saturation: f32, vibrance: f32 },
    /// Tone curve as control points in 0.0..=1.0, applied to the given channel.
    Curve {
        channel: CurveChannel,
        points: Vec<[f32; 2]>,
    },
    /// Crop rectangle in normalized image coordinates, plus rotation in degrees.
    Crop {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        angle_deg: f32,
    },
    /// An operation implemented by a plugin.
    Plugin {
        plugin: String,
        version: String,
        params: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveChannel {
    Rgb,
    Red,
    Green,
    Blue,
    Luma,
}
