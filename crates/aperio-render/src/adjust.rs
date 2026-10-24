//! Per-pixel adjustments, compiled from document operations into a flat list
//! that the GPU shader and the CPU path both run.

use aperio_core::{EditDocument, Mask, Operation};

use crate::color::{blackbody_rgb, linear_to_srgb, luminance, srgb_to_linear};
use crate::curve::{self, LUT_SIZE};
use crate::geometry::CropRect;
use aperio_core::ops::CurveChannel;

/// Temperature the white balance slider treats as "no change".
pub const NEUTRAL_KELVIN: f32 = 6500.0;
/// Middle grey, the pivot for contrast.
const PIVOT: f32 = 0.18;

/// One compiled per-pixel step. Field meanings depend on the kind; this is
/// also the layout uploaded to the GPU (see `shader.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdjustOp {
    pub kind: u32,
    pub _pad: u32,
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub _pad2: [f32; 2],
}

// SAFETY: plain `repr(C)` struct of 4-byte scalars with no implicit padding.
unsafe impl bytemuck::Zeroable for AdjustOp {}
unsafe impl bytemuck::Pod for AdjustOp {}

pub const KIND_GAIN: u32 = 1;
pub const KIND_CONTRAST: u32 = 2;
pub const KIND_COLOR: u32 = 3;
pub const KIND_CURVE: u32 = 4;

const CHANNEL_RGB: f32 = 0.0;
const CHANNEL_LUMA: f32 = 4.0;

impl AdjustOp {
    fn new(kind: u32, a: f32, b: f32, c: f32) -> Self {
        Self {
            kind,
            _pad: 0,
            a,
            b,
            c,
            d: 0.0,
            _pad2: [0.0; 2],
        }
    }

    /// Multiply each channel. Used for exposure and white balance.
    pub fn gain(r: f32, g: f32, b: f32) -> Self {
        Self::new(KIND_GAIN, r, g, b)
    }

    pub fn contrast(amount: f32) -> Self {
        Self::new(KIND_CONTRAST, amount.clamp(-1.0, 1.0), 0.0, 0.0)
    }

    pub fn color(saturation: f32, vibrance: f32) -> Self {
        Self::new(
            KIND_COLOR,
            saturation.clamp(-1.0, 1.0),
            vibrance.clamp(-1.0, 1.0),
            0.0,
        )
    }

    /// A tone curve stored at `lut_index` in the program's lookup tables.
    fn curve(lut_index: usize, channel: CurveChannel) -> Self {
        let channel = match channel {
            CurveChannel::Rgb => CHANNEL_RGB,
            CurveChannel::Red => 1.0,
            CurveChannel::Green => 2.0,
            CurveChannel::Blue => 3.0,
            CurveChannel::Luma => CHANNEL_LUMA,
        };
        Self::new(KIND_CURVE, lut_index as f32, channel, 0.0)
    }

    /// The CPU reference for the shader's `apply` function.
    pub fn apply(&self, px: [f32; 3], luts: &[f32]) -> [f32; 3] {
        match self.kind {
            KIND_GAIN => [px[0] * self.a, px[1] * self.b, px[2] * self.c],
            KIND_CONTRAST => {
                // Power curve around middle grey: highlights and shadows move
                // apart (amount > 0) or together (amount < 0), grey stays put.
                let k = 1.0 + self.a;
                px.map(|v| PIVOT * (v.max(0.0) / PIVOT).powf(k))
            }
            KIND_COLOR => {
                let lum = luminance(px);
                let max = px[0].max(px[1]).max(px[2]);
                let min = px[0].min(px[1]).min(px[2]);
                let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
                // Vibrance boosts muted colors more than already-saturated ones.
                let s = (1.0 + self.a) * (1.0 + self.b * (1.0 - sat));
                px.map(|v| (lum + (v - lum) * s).max(0.0))
            }
            KIND_CURVE => {
                let start = self.a as usize * LUT_SIZE;
                let lut = &luts[start..start + LUT_SIZE];
                // Curves act on sRGB-encoded values, the way people expect
                // from other editors. Overbright values scale with the top.
                let tone = |v: f32| {
                    if v > 1.0 {
                        v * srgb_to_linear(lut[LUT_SIZE - 1])
                    } else {
                        srgb_to_linear(curve::lookup(lut, linear_to_srgb(v)))
                    }
                };
                match self.b {
                    CHANNEL_RGB => px.map(tone),
                    CHANNEL_LUMA => {
                        let lum = luminance(px);
                        let k = if lum > 0.0 { tone(lum) / lum } else { 1.0 };
                        px.map(|v| v * k)
                    }
                    c => {
                        let mut out = px;
                        let i = c as usize - 1;
                        out[i] = tone(px[i]);
                        out
                    }
                }
            }
            _ => px,
        }
    }
}

/// A step the renderer left out, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub layer: usize,
    /// The operation name, or the plugin id for plugin steps.
    pub op: String,
    pub reason: String,
}

/// A document compiled for rendering.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    /// Per-pixel steps of every layer, back to back.
    pub ops: Vec<AdjustOp>,
    /// Enabled layers, bottom to top.
    pub layers: Vec<LayerProgram>,
    /// Curve lookup tables, `LUT_SIZE` entries each, indexed by curve steps.
    pub luts: Vec<f32>,
    /// Final crop and straighten, applied after every layer.
    pub crop: Option<CropRect>,
    /// Operations this version cannot render yet.
    pub skipped: Vec<Skipped>,
}

/// One layer: a slice of `Program::ops`, blended over the layers below
/// through its mask at its opacity.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerProgram {
    pub index: usize,
    pub op_start: usize,
    pub op_count: usize,
    pub opacity: f32,
    pub mask: Option<Mask>,
    /// Plugin operations in this layer, in order. Each runs after the first
    /// `at` of the layer's per-pixel steps.
    pub plugin_steps: Vec<PluginStep>,
}

/// A plugin operation and where it falls among its layer's per-pixel steps.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginStep {
    pub at: usize,
    pub plugin: String,
    pub version: String,
    pub params: serde_json::Value,
}

impl Program {
    /// Run one layer's per-pixel steps on a pixel (no blending).
    pub fn apply_layer(&self, layer: &LayerProgram, px: [f32; 4]) -> [f32; 4] {
        let ops = &self.ops[layer.op_start..layer.op_start + layer.op_count];
        let rgb = ops
            .iter()
            .fold([px[0], px[1], px[2]], |c, op| op.apply(c, &self.luts));
        [rgb[0], rgb[1], rgb[2], px[3]]
    }

    /// Run every layer on a pixel, as if all masks were fully on. Handy for
    /// tests; real rendering goes through `Renderer`.
    pub fn apply(&self, px: [f32; 4]) -> [f32; 4] {
        self.layers.iter().fold(px, |below, layer| {
            blend(below, self.apply_layer(layer, below), layer.opacity)
        })
    }
}

/// Mix `above` over `below` by `weight` (mask times opacity). Matches the
/// shader's blend.
pub fn blend(below: [f32; 4], above: [f32; 4], weight: f32) -> [f32; 4] {
    std::array::from_fn(|i| below[i] + (above[i] - below[i]) * weight)
}

/// Compile a document for rendering.
///
/// Every enabled layer's global adjustments and curves become per-pixel
/// steps, and the base layer's crop becomes the final geometry step. Plugin
/// operations are recorded per layer for the renderer to hand to plugins.
/// Crops on upper layers are reported in `skipped` so callers can warn
/// instead of silently dropping them.
pub fn compile(doc: &EditDocument) -> Program {
    let mut p = Program::default();
    for (index, layer) in doc.layers.iter().enumerate() {
        if !layer.enabled {
            continue;
        }
        let op_start = p.ops.len();
        let mut plugin_steps = Vec::new();
        for op in &layer.operations {
            match *op {
                Operation::Exposure { ev } => {
                    let g = 2f32.powf(ev);
                    p.ops.push(AdjustOp::gain(g, g, g));
                }
                Operation::WhiteBalance {
                    temperature_k,
                    tint,
                } => p.ops.push(white_balance(temperature_k, tint)),
                Operation::Contrast { amount } => p.ops.push(AdjustOp::contrast(amount)),
                Operation::Color {
                    saturation,
                    vibrance,
                } => p.ops.push(AdjustOp::color(saturation, vibrance)),
                Operation::Curve {
                    channel,
                    ref points,
                } => {
                    let lut = p.luts.len() / LUT_SIZE;
                    p.luts.extend(curve::bake(points));
                    p.ops.push(AdjustOp::curve(lut, channel));
                }
                // Crop is geometry, not a pixel step; the last one wins.
                Operation::Crop {
                    x,
                    y,
                    width,
                    height,
                    angle_deg,
                } if index == 0 => {
                    p.crop = Some(CropRect {
                        x,
                        y,
                        width,
                        height,
                        angle_deg,
                    })
                }
                Operation::Crop { .. } => p.skipped.push(Skipped {
                    layer: index,
                    op: "crop".into(),
                    reason: "crop only applies on the base layer".into(),
                }),
                Operation::Plugin {
                    ref plugin,
                    ref version,
                    ref params,
                } => plugin_steps.push(PluginStep {
                    at: p.ops.len() - op_start,
                    plugin: plugin.clone(),
                    version: version.clone(),
                    params: params.clone(),
                }),
            }
        }
        p.layers.push(LayerProgram {
            index,
            op_start,
            op_count: p.ops.len() - op_start,
            opacity: layer.opacity.clamp(0.0, 1.0),
            mask: layer.mask.clone(),
            plugin_steps,
        });
    }
    p
}

/// Gains that neutralize a light source of `kelvin`, relative to
/// [`NEUTRAL_KELVIN`]. Lower values cool the image, higher values warm it.
/// Positive tint adds magenta.
fn white_balance(kelvin: f32, tint: f32) -> AdjustOp {
    let target = blackbody_rgb(kelvin);
    let neutral = blackbody_rgb(NEUTRAL_KELVIN);
    let green = 1.0 - 0.3 * tint.clamp(-1.0, 1.0);
    AdjustOp::gain(neutral[0] / target[0], green, neutral[2] / target[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperio_core::{Layer, SourceRef};

    fn doc(ops: Vec<Operation>) -> EditDocument {
        let mut d = EditDocument::new(SourceRef {
            file_name: "t.jpg".into(),
            size_bytes: 0,
            sha256: None,
        });
        d.layers[0].operations = ops;
        d
    }

    #[test]
    fn exposure_doubles_per_stop() {
        let p = compile(&doc(vec![Operation::Exposure { ev: 1.0 }]));
        assert_eq!(p.apply([0.1, 0.2, 0.3, 1.0]), [0.2, 0.4, 0.6, 1.0]);
    }

    #[test]
    fn contrast_keeps_middle_grey() {
        let op = AdjustOp::contrast(0.5);
        let out = op.apply([PIVOT; 3], &[]);
        assert!((out[0] - PIVOT).abs() < 1e-6);
        assert!(op.apply([0.5; 3], &[])[0] > 0.5);
        assert!(op.apply([0.05; 3], &[])[0] < 0.05);
    }

    #[test]
    fn zero_saturation_keeps_color_and_minus_one_is_grey() {
        let px = [0.4, 0.2, 0.1];
        let same = AdjustOp::color(0.0, 0.0).apply(px, &[]);
        assert!(same.iter().zip(px).all(|(a, b)| (a - b).abs() < 1e-6));
        let grey = AdjustOp::color(-1.0, 0.0).apply(px, &[]);
        assert!((grey[0] - grey[2]).abs() < 1e-6);
    }

    #[test]
    fn neutral_white_balance_is_identity() {
        let op = white_balance(NEUTRAL_KELVIN, 0.0);
        assert!((op.a - 1.0).abs() < 1e-6 && op.b == 1.0 && (op.c - 1.0).abs() < 1e-6);
    }

    #[test]
    fn lower_temperature_cools_the_image() {
        let op = white_balance(4000.0, 0.0);
        assert!(op.c > 1.0 && op.a < 1.0);
    }

    #[test]
    fn identity_curve_changes_nothing_and_crop_is_collected() {
        let crop = CropRect {
            x: 0.1,
            y: 0.2,
            width: 0.5,
            height: 0.5,
            angle_deg: 3.0,
        };
        let p = compile(&doc(vec![
            Operation::Curve {
                channel: CurveChannel::Rgb,
                points: vec![[0.0, 0.0], [1.0, 1.0]],
            },
            Operation::Crop {
                x: crop.x,
                y: crop.y,
                width: crop.width,
                height: crop.height,
                angle_deg: crop.angle_deg,
            },
        ]));
        assert_eq!(p.crop, Some(crop));
        assert_eq!(p.luts.len(), LUT_SIZE);
        let out = p.apply([0.05, 0.2, 0.9, 1.0]);
        for (a, b) in out.iter().zip([0.05, 0.2, 0.9, 1.0]) {
            assert!((a - b).abs() < 1e-3, "{out:?}");
        }
    }

    #[test]
    fn curves_brighten_only_their_channel() {
        let lift = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        let px = [0.2, 0.2, 0.2, 1.0];
        let red = compile(&doc(vec![Operation::Curve {
            channel: CurveChannel::Red,
            points: lift.clone(),
        }]))
        .apply(px);
        assert!(red[0] > 0.2 && red[1] == 0.2 && red[2] == 0.2);
        let luma = compile(&doc(vec![Operation::Curve {
            channel: CurveChannel::Luma,
            points: lift,
        }]))
        .apply([0.3, 0.2, 0.1, 1.0]);
        // Luma curves brighten while keeping the color ratios.
        assert!(luma[0] > 0.3 && (luma[0] / luma[2] - 3.0).abs() < 1e-4);
    }

    #[test]
    fn records_plugin_steps_and_reports_upper_crops() {
        let mut d = doc(vec![
            Operation::Exposure { ev: 1.0 },
            Operation::Plugin {
                plugin: "x".into(),
                version: "1".into(),
                params: serde_json::Value::Null,
            },
            Operation::Contrast { amount: 0.1 },
        ]);
        let mut upper = Layer::new("Upper");
        upper.operations.push(Operation::Crop {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            angle_deg: 0.0,
        });
        d.layers.push(upper);
        let p = compile(&d);
        assert_eq!(p.ops.len(), 2);
        assert!(p.crop.is_none());
        assert_eq!(
            p.layers[0].plugin_steps,
            vec![PluginStep {
                at: 1,
                plugin: "x".into(),
                version: "1".into(),
                params: serde_json::Value::Null,
            }]
        );
        assert_eq!(
            p.skipped,
            vec![Skipped {
                layer: 1,
                op: "crop".into(),
                reason: "crop only applies on the base layer".into(),
            }]
        );
    }

    #[test]
    fn upper_layers_blend_at_their_opacity() {
        let mut d = doc(vec![Operation::Exposure { ev: 1.0 }]);
        let mut upper = Layer::new("Upper");
        upper.opacity = 0.5;
        upper.operations.push(Operation::Exposure { ev: 1.0 });
        d.layers.push(upper);
        let p = compile(&d);
        assert_eq!(p.layers.len(), 2);
        // 0.1 -> 0.2 (base), then halfway to 0.4 -> 0.3.
        let out = p.apply([0.1, 0.1, 0.1, 1.0]);
        assert!((out[0] - 0.3).abs() < 1e-6, "{out:?}");
    }
}
