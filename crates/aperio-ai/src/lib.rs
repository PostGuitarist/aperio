//! What Aperio's AI plugins share: model files (where they come from, where
//! they're kept, how they're downloaded and checked) and loading them into
//! ONNX Runtime with the best execution provider this platform has.

pub mod models;
mod session;

pub use models::{default_dir, ModelFile};
pub use ort;
pub use session::load;
