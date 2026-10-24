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
pub mod decode;
pub mod export;
mod gpu;

pub use adjust::{AdjustOp, Skipped};
pub use decode::decode;
pub use export::{export, ExportFormat};
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
