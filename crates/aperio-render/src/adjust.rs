//! Per-pixel adjustments, compiled from document operations into a flat list
//! that the GPU shader and the CPU path both run.

use aperio_core::{EditDocument, Operation};

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

/// An operation the renderer does not handle yet and left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub layer: usize,
    pub op: &'static str,
}

/// A document compiled for rendering.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    /// Per-pixel steps, in order.
    pub ops: Vec<AdjustOp>,
    /// Curve lookup tables, `LUT_SIZE` entries each, indexed by curve steps.
    pub luts: Vec<f32>,
    /// Final crop and straighten, applied after the per-pixel steps.
    pub crop: Option<CropRect>,
    /// Operations this version cannot render yet.
    pub skipped: Vec<Skipped>,
}

impl Program {
    /// Run the per-pixel steps on one pixel.
    pub fn apply(&self, px: [f32; 4]) -> [f32; 4] {
        let rgb = self
            .ops
            .iter()
            .fold([px[0], px[1], px[2]], |c, op| op.apply(c, &self.luts));
        [rgb[0], rgb[1], rgb[2], px[3]]
    }
}

/// Compile a document for rendering.
///
/// The base layer's global adjustments, curves, and crop are rendered. Upper
/// layers, masks, and plugin operations are reported in `skipped` so callers
/// can warn instead of silently dropping them.
pub fn compile(doc: &EditDocument) -> Program {
    let mut p = Program::default();
    for (index, layer) in doc.layers.iter().enumerate() {
        if !layer.enabled {
            continue;
        }
        for op in &layer.operations {
            if index > 0 || layer.mask.is_some() {
                p.skipped.push(Skipped {
                    layer: index,
                    op: op_name(op),
                });
                continue;
            }
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
                    let index = p.luts.len() / LUT_SIZE;
                    p.luts.extend(curve::bake(points));
                    p.ops.push(AdjustOp::curve(index, channel));
                }
                // Crop is geometry, not a pixel step; the last one wins.
                Operation::Crop {
                    x,
                    y,
                    width,
                    height,
                    angle_deg,
                } => {
                    p.crop = Some(CropRect {
                        x,
                        y,
                        width,
                        height,
                        angle_deg,
                    })
                }
                Operation::Plugin { .. } => p.skipped.push(Skipped {
                    layer: index,
                    op: op_name(op),
                }),
            }
        }
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

fn op_name(op: &Operation) -> &'static str {
    match op {
        Operation::Exposure { .. } => "exposure",
        Operation::Contrast { .. } => "contrast",
        Operation::WhiteBalance { .. } => "white_balance",
        Operation::Color { .. } => "color",
        Operation::Curve { .. } => "curve",
        Operation::Crop { .. } => "crop",
        Operation::Plugin { .. } => "plugin",
    }
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
    fn reports_what_it_cannot_render_yet() {
        let mut d = doc(vec![Operation::Plugin {
            plugin: "x".into(),
            version: "1".into(),
            params: serde_json::Value::Null,
        }]);
        let mut upper = Layer::new("Upper");
        upper.operations.push(Operation::Exposure { ev: 1.0 });
        d.layers.push(upper);
        let p = compile(&d);
        assert!(p.ops.is_empty());
        assert_eq!(
            p.skipped,
            vec![
                Skipped {
                    layer: 0,
                    op: "plugin"
                },
                Skipped {
                    layer: 1,
                    op: "exposure"
                }
            ]
        );
    }
}
