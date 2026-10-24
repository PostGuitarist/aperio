//! Per-pixel adjustments, compiled from document operations into a flat list
//! that the GPU shader and the CPU path both run.

use aperio_core::{EditDocument, Operation};

use crate::color::{blackbody_rgb, luminance};

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

    /// The CPU reference for the shader's `apply` function.
    pub fn apply(&self, px: [f32; 3]) -> [f32; 3] {
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

/// Compile a document into per-pixel steps.
///
/// This milestone renders only the base layer's global adjustments. Upper
/// layers, masks, curves, crop, and plugin operations are reported in the
/// returned `Skipped` list so callers can warn instead of silently dropping them.
pub fn compile(doc: &EditDocument) -> (Vec<AdjustOp>, Vec<Skipped>) {
    let mut ops = Vec::new();
    let mut skipped = Vec::new();
    for (index, layer) in doc.layers.iter().enumerate() {
        if !layer.enabled {
            continue;
        }
        for op in &layer.operations {
            if index > 0 || layer.mask.is_some() {
                skipped.push(Skipped {
                    layer: index,
                    op: op_name(op),
                });
                continue;
            }
            match *op {
                Operation::Exposure { ev } => {
                    let g = 2f32.powf(ev);
                    ops.push(AdjustOp::gain(g, g, g));
                }
                Operation::WhiteBalance {
                    temperature_k,
                    tint,
                } => ops.push(white_balance(temperature_k, tint)),
                Operation::Contrast { amount } => ops.push(AdjustOp::contrast(amount)),
                Operation::Color {
                    saturation,
                    vibrance,
                } => ops.push(AdjustOp::color(saturation, vibrance)),
                _ => skipped.push(Skipped {
                    layer: index,
                    op: op_name(op),
                }),
            }
        }
    }
    (ops, skipped)
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

/// Run compiled steps on one pixel.
pub fn apply_all(ops: &[AdjustOp], px: [f32; 4]) -> [f32; 4] {
    let rgb = ops.iter().fold([px[0], px[1], px[2]], |c, op| op.apply(c));
    [rgb[0], rgb[1], rgb[2], px[3]]
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
        let (ops, _) = compile(&doc(vec![Operation::Exposure { ev: 1.0 }]));
        assert_eq!(apply_all(&ops, [0.1, 0.2, 0.3, 1.0]), [0.2, 0.4, 0.6, 1.0]);
    }

    #[test]
    fn contrast_keeps_middle_grey() {
        let op = AdjustOp::contrast(0.5);
        let out = op.apply([PIVOT; 3]);
        assert!((out[0] - PIVOT).abs() < 1e-6);
        assert!(op.apply([0.5; 3])[0] > 0.5);
        assert!(op.apply([0.05; 3])[0] < 0.05);
    }

    #[test]
    fn zero_saturation_keeps_color_and_minus_one_is_grey() {
        let px = [0.4, 0.2, 0.1];
        let same = AdjustOp::color(0.0, 0.0).apply(px);
        assert!(same.iter().zip(px).all(|(a, b)| (a - b).abs() < 1e-6));
        let grey = AdjustOp::color(-1.0, 0.0).apply(px);
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
    fn reports_what_it_cannot_render_yet() {
        let mut d = doc(vec![Operation::Crop {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            angle_deg: 0.0,
        }]);
        let mut upper = Layer::new("Upper");
        upper.operations.push(Operation::Exposure { ev: 1.0 });
        d.layers.push(upper);
        let (ops, skipped) = compile(&d);
        assert!(ops.is_empty());
        assert_eq!(
            skipped,
            vec![
                Skipped {
                    layer: 0,
                    op: "crop"
                },
                Skipped {
                    layer: 1,
                    op: "exposure"
                }
            ]
        );
    }
}
