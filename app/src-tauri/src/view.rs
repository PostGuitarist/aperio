//! What the editor shows on screen, as opposed to what gets exported: the
//! crop tool needs the uncropped photo, the mask tools tint the selected
//! layer's mask, and "before" hides every adjustment.

use aperio_core::{EditDocument, Operation};
use aperio_render::{compile, masks, LinearImage, Renderer};
use serde::Deserialize;

/// How the preview should be drawn.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct View {
    /// Apply the crop. Off while the crop tool is open.
    #[serde(default)]
    pub crop: bool,
    /// Tint this layer's mask red so it can be seen while editing it.
    #[serde(default)]
    pub mask_overlay: Option<usize>,
    /// Show the photo without adjustments (the crop still applies).
    #[serde(default)]
    pub before: bool,
}

/// Linear-light red used for the mask overlay, and how strongly it covers.
const OVERLAY_COLOR: [f32; 3] = [0.9, 0.02, 0.02];
const OVERLAY_STRENGTH: f32 = 0.5;

/// Render `doc` onto `img` the way `view` asks.
pub fn render(renderer: &Renderer, doc: &EditDocument, view: View, img: &mut LinearImage) {
    // Crop is applied here rather than by the renderer, so the overlay can be
    // drawn in uncropped coordinates first.
    let crop = compile(doc).crop;
    let mut doc = without_crop(doc);
    if view.before {
        for layer in &mut doc.layers {
            layer.enabled = false;
        }
    }
    renderer.render(&doc, img);
    if let Some(mask) = view
        .mask_overlay
        .and_then(|i| doc.layers.get(i))
        .and_then(|l| l.mask.as_ref())
    {
        if let Some(weights) = masks::rasterize(mask, img.width, img.height) {
            tint(img, &weights);
        }
    }
    if let (true, Some(rect)) = (view.crop, crop) {
        *img = img.crop(rect);
    }
}

fn without_crop(doc: &EditDocument) -> EditDocument {
    let mut doc = doc.clone();
    for layer in &mut doc.layers {
        layer
            .operations
            .retain(|op| !matches!(op, Operation::Crop { .. }));
    }
    doc
}

fn tint(img: &mut LinearImage, weights: &[f32]) {
    for (px, &w) in img.pixels.iter_mut().zip(weights) {
        let a = w.clamp(0.0, 1.0) * OVERLAY_STRENGTH;
        for c in 0..3 {
            px[c] += (OVERLAY_COLOR[c] - px[c]) * a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperio_core::{Layer, Mask, SourceRef};

    fn doc() -> EditDocument {
        let mut doc = EditDocument::new(SourceRef {
            file_name: "a.jpg".into(),
            size_bytes: 1,
            sha256: None,
        });
        doc.layers[0].operations = vec![
            Operation::Exposure { ev: 1.0 },
            Operation::Crop {
                x: 0.0,
                y: 0.0,
                width: 0.5,
                height: 1.0,
                angle_deg: 0.0,
            },
        ];
        doc
    }

    fn grey() -> LinearImage {
        LinearImage::new(4, 2, vec![[0.25, 0.25, 0.25, 1.0]; 8])
    }

    #[test]
    fn crop_can_be_turned_off_for_the_crop_tool() {
        let r = Renderer::cpu();
        let mut cropped = grey();
        render(
            &r,
            &doc(),
            View {
                crop: true,
                ..View::default()
            },
            &mut cropped,
        );
        assert_eq!((cropped.width, cropped.height), (2, 2));
        assert!((cropped.pixels[0][0] - 0.5).abs() < 1e-5);

        let mut full = grey();
        render(&r, &doc(), View::default(), &mut full);
        assert_eq!((full.width, full.height), (4, 2));
    }

    #[test]
    fn before_skips_adjustments_but_keeps_the_crop() {
        let r = Renderer::cpu();
        let mut img = grey();
        let view = View {
            crop: true,
            before: true,
            ..View::default()
        };
        render(&r, &doc(), view, &mut img);
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixels[0], [0.25, 0.25, 0.25, 1.0]);
    }

    #[test]
    fn overlay_tints_only_inside_the_mask() {
        let mut d = doc();
        d.layers[0].operations.clear();
        let mut layer = Layer::new("Left");
        layer.mask = Some(Mask::Linear {
            start: [0.0, 0.5],
            end: [0.5, 0.5],
        });
        d.layers.push(layer);
        let mut img = grey();
        let view = View {
            mask_overlay: Some(1),
            ..View::default()
        };
        render(&Renderer::cpu(), &d, view, &mut img);
        assert!(img.pixels[0][0] > 0.5 && img.pixels[0][1] < 0.25);
        assert_eq!(img.pixels[3], [0.25, 0.25, 0.25, 1.0]);
    }
}
