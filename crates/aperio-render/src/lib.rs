//! Turns an original photo plus its [`aperio_core::EditDocument`] into pixels.
//!
//! The pipeline is: [`decode`] the original into a [`LinearImage`], run the
//! document's adjustments on it with a [`Renderer`] (GPU, with a matching CPU
//! path for tests and machines without a usable GPU), then [`export`] it.
//!
//! All processing happens in linear light with sRGB (Rec. 709) primaries.
//! Gamma is applied only when encoding the output.

pub mod adjust;
pub mod color;
pub mod curve;
pub mod decode;
pub mod export;
pub mod geometry;
mod gpu;
pub mod masks;
pub mod tone;

pub use adjust::{compile, AdjustOp, LayerProgram, Program, Skipped};
pub use decode::decode;
pub use export::{export, jpeg_bytes, ExportFormat};
pub use geometry::CropRect;
pub use gpu::Renderer;

/// A decoded image in linear light, RGBA, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 4]>,
}

impl LinearImage {
    pub fn new(width: u32, height: u32, pixels: Vec<[f32; 4]>) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize);
        Self {
            width,
            height,
            pixels,
        }
    }

    /// A smaller copy whose longer edge is at most `max_edge`, averaging each
    /// block of source pixels. Returns a plain copy if it already fits.
    pub fn downscale(&self, max_edge: u32) -> Self {
        let factor = self.width.max(self.height).div_ceil(max_edge.max(1)).max(1);
        if factor == 1 {
            return self.clone();
        }
        let (w, h) = (self.width / factor, self.height / factor);
        let inv = 1.0 / (factor * factor) as f32;
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 4];
                for sy in y * factor..(y + 1) * factor {
                    let row = (sy * self.width) as usize;
                    for sx in x * factor..(x + 1) * factor {
                        let px = self.pixels[row + sx as usize];
                        for c in 0..4 {
                            sum[c] += px[c];
                        }
                    }
                }
                pixels.push(sum.map(|v| v * inv));
            }
        }
        Self::new(w, h, pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_averages_blocks_and_fits_the_edge() {
        let img = LinearImage::new(
            4,
            2,
            vec![
                [0.0; 4], [1.0; 4], [0.5; 4], [0.5; 4], [1.0; 4], [0.0; 4], [0.5; 4], [0.5; 4],
            ],
        );
        let small = img.downscale(2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.pixels, vec![[0.5; 4], [0.5; 4]]);
        assert_eq!(img.downscale(4), img);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported file type: {0}")]
    UnsupportedFormat(String),
    #[error("could not decode RAW file: {0}")]
    Raw(String),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("no usable GPU: {0}")]
    Gpu(String),
}

pub type Result<T> = std::result::Result<T, Error>;
