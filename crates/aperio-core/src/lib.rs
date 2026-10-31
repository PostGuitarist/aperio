//! Core data model for Aperio.
//!
//! An edit is never applied to the original file. Instead, every change is
//! recorded in an [`EditDocument`]: an ordered stack of layers, each holding a
//! list of operations and an optional mask. The document is saved next to the
//! original as a JSON sidecar (see `docs/sidecar-format.md`).

pub mod document;
pub mod mask;
pub mod ops;

pub use document::{sidecar_path, BlendMode, EditDocument, Layer, SourceRef, SIDECAR_EXTENSION};
pub use mask::Mask;
pub use ops::Operation;

/// Errors raised while reading or writing edit documents.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not an Aperio edit file (format was {0:?})")]
    WrongFormat(String),
    #[error("edit file version {found} is newer than this build supports ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("invalid edit file: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
