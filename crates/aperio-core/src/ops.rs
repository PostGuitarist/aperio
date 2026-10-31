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
    /// Tonal range adjustments, each from -1.0 to 1.0. Highlights and shadows
    /// move the bright and dark midtones; whites and blacks move the ends.
    Tone {
        #[serde(default)]
        highlights: f32,
        #[serde(default)]
        shadows: f32,
        #[serde(default)]
        whites: f32,
        #[serde(default)]
        blacks: f32,
    },
    /// Saturation and vibrance, each from -1.0 to 1.0.
    Color { saturation: f32, vibrance: f32 },
    /// Local contrast, each from -1.0 to 1.0. Texture works on fine detail,
    /// clarity on larger shapes in the midtones, and dehaze removes (or with
    /// negative values adds) atmospheric haze.
    Presence {
        #[serde(default)]
        texture: f32,
        #[serde(default)]
        clarity: f32,
        #[serde(default)]
        dehaze: f32,
    },
    /// Hue, saturation, and luminance per color band, each from -1.0 to 1.0.
    /// The eight bands, in order: red, orange, yellow, green, aqua, blue,
    /// purple, magenta.
    Hsl {
        #[serde(default)]
        hue: [f32; 8],
        #[serde(default)]
        saturation: [f32; 8],
        #[serde(default)]
        luminance: [f32; 8],
    },
    /// Color grading: tints for shadows, midtones, highlights, and the whole
    /// image, and how the three tonal ranges overlap.
    ColorGrade {
        #[serde(default)]
        shadows: GradeWheel,
        #[serde(default)]
        midtones: GradeWheel,
        #[serde(default)]
        highlights: GradeWheel,
        #[serde(default)]
        global: GradeWheel,
        /// How far the tonal ranges overlap, 0.0 to 1.0.
        #[serde(default = "half")]
        blending: f32,
        /// Moves the split between shadows and highlights, -1.0 to 1.0.
        /// Positive values give highlights more of the range.
        #[serde(default)]
        balance: f32,
    },
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

/// One color wheel of [`Operation::ColorGrade`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GradeWheel {
    /// Tint hue in degrees, 0.0 to 360.0 (0 is red, 120 green, 240 blue).
    #[serde(default)]
    pub hue: f32,
    /// Tint strength, 0.0 to 1.0.
    #[serde(default)]
    pub saturation: f32,
    /// Brightness shift for this range, -1.0 to 1.0.
    #[serde(default)]
    pub luminance: f32,
}

impl GradeWheel {
    pub fn is_neutral(&self) -> bool {
        self.saturation == 0.0 && self.luminance == 0.0
    }
}

fn half() -> f32 {
    0.5
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
