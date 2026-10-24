//! The basic-adjustment panel's values and how they map onto the base layer.

use aperio_core::{EditDocument, Operation};
use aperio_render::adjust::NEUTRAL_KELVIN;
use serde::{Deserialize, Serialize};

/// Values of the sliders in the Adjustments panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sliders {
    pub exposure: f32,
    pub contrast: f32,
    pub temperature: f32,
    pub tint: f32,
    pub saturation: f32,
    pub vibrance: f32,
}

impl Default for Sliders {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            temperature: NEUTRAL_KELVIN,
            tint: 0.0,
            saturation: 0.0,
            vibrance: 0.0,
        }
    }
}

impl Sliders {
    /// Read the panel's values from the document's base layer.
    pub fn from_doc(doc: &EditDocument) -> Self {
        let mut s = Self::default();
        for op in doc.layers.first().map(|l| &l.operations[..]).unwrap_or(&[]) {
            match *op {
                Operation::Exposure { ev } => s.exposure = ev,
                Operation::Contrast { amount } => s.contrast = amount,
                Operation::WhiteBalance {
                    temperature_k,
                    tint,
                } => {
                    s.temperature = temperature_k;
                    s.tint = tint;
                }
                Operation::Color {
                    saturation,
                    vibrance,
                } => {
                    s.saturation = saturation;
                    s.vibrance = vibrance;
                }
                _ => {}
            }
        }
        s
    }

    /// Write the panel's values into the base layer. Operations the panel
    /// doesn't own (curves, crop, plugins) are kept after the panel's ones.
    /// Neutral values are left out so untouched photos get a clean sidecar.
    pub fn apply_to(&self, doc: &mut EditDocument) {
        let base = &mut doc.layers[0];
        let others: Vec<Operation> = base
            .operations
            .drain(..)
            .filter(|op| {
                !matches!(
                    op,
                    Operation::Exposure { .. }
                        | Operation::Contrast { .. }
                        | Operation::WhiteBalance { .. }
                        | Operation::Color { .. }
                )
            })
            .collect();
        let d = Self::default();
        if self.temperature != d.temperature || self.tint != d.tint {
            base.operations.push(Operation::WhiteBalance {
                temperature_k: self.temperature,
                tint: self.tint,
            });
        }
        if self.exposure != d.exposure {
            base.operations
                .push(Operation::Exposure { ev: self.exposure });
        }
        if self.contrast != d.contrast {
            base.operations.push(Operation::Contrast {
                amount: self.contrast,
            });
        }
        if self.saturation != d.saturation || self.vibrance != d.vibrance {
            base.operations.push(Operation::Color {
                saturation: self.saturation,
                vibrance: self.vibrance,
            });
        }
        base.operations.extend(others);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperio_core::{ops::CurveChannel, SourceRef};

    fn doc() -> EditDocument {
        EditDocument::new(SourceRef {
            file_name: "a.jpg".into(),
            size_bytes: 1,
            sha256: None,
        })
    }

    #[test]
    fn round_trips_through_the_document() {
        let s = Sliders {
            exposure: 0.5,
            contrast: -0.2,
            temperature: 5200.0,
            tint: 0.1,
            saturation: 0.3,
            vibrance: 0.0,
        };
        let mut d = doc();
        s.apply_to(&mut d);
        assert_eq!(Sliders::from_doc(&d), s);
    }

    #[test]
    fn neutral_sliders_write_nothing_and_keep_other_ops() {
        let curve = Operation::Curve {
            channel: CurveChannel::Rgb,
            points: vec![[0.0, 0.0], [1.0, 1.0]],
        };
        let mut d = doc();
        d.layers[0].operations = vec![Operation::Exposure { ev: 1.0 }, curve.clone()];
        Sliders::default().apply_to(&mut d);
        assert_eq!(d.layers[0].operations, vec![curve]);
    }
}
